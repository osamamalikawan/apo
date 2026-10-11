-- Subscriptions and Super Admin control. NOT YET EXECUTED (run supabase/tests/superadmin_test.sql after).
--
-- Control flow for revocation:
--   Super Admin portal -> Worker -> superadmin_set_subscription(...)   (platform admins only)
--     * bumps tenant_subscriptions.revision, flips tenants.status so cloud RPCs refuse writes at once
--     * writes an immutable platform audit row
--   Shop servers learn of it at their next sync: the Worker calls tenant_entitlement_payload(...)
--   (service_role only), signs the JSON with the cloud-only Ed25519 key, and returns it to the server.
-- Nothing is ever deleted when a subscription lapses: suspended tenants keep read access.

create table subscription_plans (
  id uuid primary key default gen_random_uuid(),
  name text not null unique,
  max_devices integer not null check (max_devices >= 0),
  max_users integer not null check (max_users >= 0),
  max_locations integer not null check (max_locations >= 0),
  modules text[] not null default '{}',
  monthly_price_minor bigint check (monthly_price_minor >= 0),
  annual_price_minor bigint check (annual_price_minor >= 0),
  currency text not null default 'PKR',
  is_active boolean not null default true
);

create table tenant_subscriptions (
  tenant_id uuid primary key references tenants(id),
  plan_id uuid not null references subscription_plans(id),
  status text not null default 'active' check (status in ('active','suspended','cancelled')),
  current_period_end timestamptz not null,
  grace_days integer not null default 7 check (grace_days between 0 and 90),
  offline_check_days integer not null default 7 check (offline_check_days between 1 and 60),
  revision bigint not null default 1,
  status_reason text,
  updated_at timestamptz not null default now(),
  updated_by uuid
);

create table platform_admins (
  user_id uuid primary key references auth.users(id),
  created_at timestamptz not null default now()
);

create table platform_audit_logs (
  id uuid primary key default gen_random_uuid(),
  actor_id uuid,
  tenant_id uuid,
  action text not null,
  detail jsonb,
  at timestamptz not null default now()
);
create trigger platform_audit_immutable before update or delete on platform_audit_logs
  for each row execute function forbid_change();

create function is_platform_admin() returns boolean
language sql stable security definer set search_path = public as $$
  select exists (select 1 from platform_admins where user_id = auth.uid()) $$;

alter table subscription_plans enable row level security;      alter table subscription_plans force row level security;
alter table tenant_subscriptions enable row level security;    alter table tenant_subscriptions force row level security;
alter table platform_admins enable row level security;         alter table platform_admins force row level security;
alter table platform_audit_logs enable row level security;     alter table platform_audit_logs force row level security;

create policy plans_read on subscription_plans for select using (auth.uid() is not null);
create policy subs_read on tenant_subscriptions for select using (is_member(tenant_id) or is_platform_admin());
create policy platform_audit_read on platform_audit_logs for select using (is_platform_admin());
-- platform_admins: no policy and no grant, so no client can read or change it.

revoke all on subscription_plans, tenant_subscriptions, platform_admins, platform_audit_logs from anon, authenticated;
grant select on subscription_plans, tenant_subscriptions, platform_audit_logs to authenticated;

-- Suspend, cancel or reinstate a tenant. Returns the new subscription revision.
create function superadmin_set_subscription(
  p_tenant uuid, p_status text, p_period_end timestamptz default null, p_reason text default null
) returns bigint
language plpgsql security definer set search_path = public as $$
declare v_old text; v_rev bigint;
begin
  if auth.uid() is null or not is_platform_admin() then
    raise exception 'forbidden' using errcode = '42501'; end if;
  if p_status not in ('active','suspended','cancelled') then
    raise exception 'invalid status' using errcode = '22023'; end if;

  select status into v_old from tenant_subscriptions where tenant_id = p_tenant for update;
  if not found then raise exception 'no subscription for tenant' using errcode = 'P0002'; end if;

  update tenant_subscriptions set
      status = p_status,
      current_period_end = coalesce(p_period_end, current_period_end),
      status_reason = p_reason,
      revision = revision + 1,
      updated_at = now(),
      updated_by = auth.uid()
    where tenant_id = p_tenant
    returning revision into v_rev;

  -- cloud RPCs (post_stock_movement and later ones) check tenants.status, so writes stop immediately
  update tenants set status = case p_status when 'active' then 'active' else 'suspended' end,
                     updated_at = now()
    where id = p_tenant;

  insert into platform_audit_logs(actor_id, tenant_id, action, detail)
    values (auth.uid(), p_tenant, 'subscription.set_status',
            jsonb_build_object('from', v_old, 'to', p_status, 'reason', p_reason, 'revision', v_rev,
                               'period_end', p_period_end));
  return v_rev;
end $$;

-- The unsigned payload the Worker signs and hands to a shop server. service_role only.
create function tenant_entitlement_payload(p_tenant uuid) returns jsonb
language sql stable security definer set search_path = public as $$
  with x as (
    select s.tenant_id, s.revision, s.status, s.grace_days,
           p.max_devices, p.max_users, p.max_locations, p.modules,
           least(s.current_period_end, now() + make_interval(days => s.offline_check_days)) as full_until
    from tenant_subscriptions s join subscription_plans p on p.id = s.plan_id
    where s.tenant_id = p_tenant)
  select jsonb_build_object(
    'tenant_id', tenant_id,
    'revision', revision,
    'status', status,
    'issued_at',   to_char(now() at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'),
    'full_until',  to_char(full_until at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'),
    'grace_until', to_char((full_until + make_interval(days => grace_days)) at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'),
    'limits', jsonb_build_object('max_devices', max_devices, 'max_users', max_users, 'max_locations', max_locations),
    'modules', to_jsonb(modules))
  from x $$;

revoke all on function tenant_entitlement_payload(uuid) from public, anon, authenticated;
grant execute on function tenant_entitlement_payload(uuid) to service_role;
revoke all on function superadmin_set_subscription(uuid, text, timestamptz, text) from public, anon;
grant execute on function superadmin_set_subscription(uuid, text, timestamptz, text) to authenticated;
revoke all on function is_platform_admin() from public, anon;
grant execute on function is_platform_admin() to authenticated;
