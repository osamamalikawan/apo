-- Cloud schema, migration 0001. NOT YET EXECUTED against a Postgres instance (see STATUS.md).
-- Mirrors the shop SQLite schema for: tenancy, identity, locations, products, stock ledger.
-- Security model: RLS on every tenant table; stock tables are READ-ONLY to clients and are
-- written only through SECURITY DEFINER RPCs that check membership, permission, location scope,
-- tenant status and idempotency. The mobile app and the Worker both go through these.

create table tenants (
  id uuid primary key default gen_random_uuid(),
  name text not null,
  currency text not null default 'PKR',
  timezone text not null default 'Asia/Karachi',
  status text not null default 'active' check (status in ('active','suspended','terminated')),
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create table tenant_members (
  tenant_id uuid not null references tenants(id),
  user_id uuid not null references auth.users(id),
  full_name text not null,
  is_active boolean not null default true,
  primary key (tenant_id, user_id)
);

create table roles (
  id uuid primary key default gen_random_uuid(),
  tenant_id uuid not null references tenants(id),
  name text not null,
  is_template boolean not null default false,
  unique (tenant_id, id), unique (tenant_id, name)
);
create table role_permissions (
  tenant_id uuid not null, role_id uuid not null,
  permission text not null check (permission = '*' or permission ~ '^[a-z_]+\.([a-z_]+|\*)$'),
  primary key (role_id, permission),
  foreign key (tenant_id, role_id) references roles(tenant_id, id) on delete cascade
);
create table user_roles (
  tenant_id uuid not null, user_id uuid not null, role_id uuid not null,
  primary key (user_id, role_id),
  foreign key (tenant_id, user_id) references tenant_members(tenant_id, user_id),
  foreign key (tenant_id, role_id) references roles(tenant_id, id)
);

create table stock_locations (
  id uuid primary key default gen_random_uuid(),
  tenant_id uuid not null references tenants(id),
  name text not null, code text not null,
  location_type text not null default 'storage',
  description text, parent_location_id uuid, responsible_user_id uuid,
  is_active boolean not null default true,
  revision integer not null default 1,
  created_at timestamptz not null default now(), updated_at timestamptz not null default now(),
  unique (tenant_id, id), unique (tenant_id, code),
  foreign key (tenant_id, parent_location_id) references stock_locations(tenant_id, id)
);
create table user_location_access (
  tenant_id uuid not null, user_id uuid not null, location_id uuid not null,
  primary key (user_id, location_id),
  foreign key (tenant_id, user_id) references tenant_members(tenant_id, user_id),
  foreign key (tenant_id, location_id) references stock_locations(tenant_id, id)
);

create table product_categories (
  id uuid primary key default gen_random_uuid(), tenant_id uuid not null references tenants(id),
  name text not null, unique (tenant_id, id), unique (tenant_id, name)
);
create table products (
  id uuid primary key default gen_random_uuid(),
  tenant_id uuid not null references tenants(id),
  sku text not null, name text not null, barcode text, category_id uuid,
  unit text not null default 'pcs', is_active boolean not null default true,
  revision integer not null default 1,
  created_at timestamptz not null default now(), updated_at timestamptz not null default now(),
  unique (tenant_id, id), unique (tenant_id, sku),
  foreign key (tenant_id, category_id) references product_categories(tenant_id, id)
);

create table inventory_balances (
  tenant_id uuid not null, location_id uuid not null, product_id uuid not null,
  on_hand bigint not null default 0, damaged bigint not null default 0, in_transit bigint not null default 0,
  reorder_level bigint, updated_at timestamptz not null default now(),
  primary key (location_id, product_id),
  foreign key (tenant_id, location_id) references stock_locations(tenant_id, id),
  foreign key (tenant_id, product_id) references products(tenant_id, id)
);

create table stock_movements (
  id uuid primary key default gen_random_uuid(),
  tenant_id uuid not null, location_id uuid not null, product_id uuid not null,
  bucket text not null default 'on_hand' check (bucket in ('on_hand','damaged','in_transit')),
  qty_delta bigint not null check (qty_delta <> 0),
  movement_type text not null, source_type text not null, source_id text not null,
  idempotency_key text not null,
  allow_negative boolean not null default false,
  origin_type text not null default 'server' check (origin_type in ('server','mobile','cloud')),
  device_id uuid, actor_id uuid,
  occurred_at timestamptz not null default now(),
  unique (tenant_id, idempotency_key),
  foreign key (tenant_id, location_id) references stock_locations(tenant_id, id),
  foreign key (tenant_id, product_id) references products(tenant_id, id)
);
create index on stock_movements (tenant_id, product_id, occurred_at);
create index on stock_movements (tenant_id, location_id, occurred_at);

create function forbid_change() returns trigger language plpgsql as $$
begin raise exception '% is append-only', tg_table_name; end $$;
create trigger stock_movements_immutable before update or delete on stock_movements
  for each row execute function forbid_change();

create table audit_logs (
  id uuid primary key default gen_random_uuid(), tenant_id uuid not null references tenants(id),
  actor_id uuid, device_id uuid, action text not null, entity text not null, entity_id text,
  before_json jsonb, after_json jsonb, at timestamptz not null default now()
);
create trigger audit_logs_immutable before update or delete on audit_logs
  for each row execute function forbid_change();

-- ---------- authorization helpers (SECURITY DEFINER so RLS can call them safely) ----------
create function is_member(p_tenant uuid) returns boolean
language sql stable security definer set search_path = public as $$
  select exists (select 1 from tenant_members
                 where tenant_id = p_tenant and user_id = auth.uid() and is_active) $$;

create function has_permission(p_tenant uuid, p_perm text) returns boolean
language sql stable security definer set search_path = public as $$
  select is_member(p_tenant) and exists (
    select 1 from user_roles ur join role_permissions rp on rp.role_id = ur.role_id
    where ur.tenant_id = p_tenant and ur.user_id = auth.uid()
      and (rp.permission = '*' or rp.permission = p_perm
           or rp.permission = split_part(p_perm, '.', 1) || '.*')) $$;

-- A user with no explicit rows in user_location_access gets NO location; admins use a role with '*'
-- plus the 'locations.all' permission.
create function can_access_location(p_tenant uuid, p_location uuid) returns boolean
language sql stable security definer set search_path = public as $$
  select is_member(p_tenant) and (
    has_permission(p_tenant, 'locations.all')
    or exists (select 1 from user_location_access
               where tenant_id = p_tenant and user_id = auth.uid() and location_id = p_location)) $$;

-- ---------- RLS ----------
do $$ declare t text; begin
  foreach t in array array['tenants','tenant_members','roles','role_permissions','user_roles',
    'stock_locations','user_location_access','product_categories','products',
    'inventory_balances','stock_movements','audit_logs'] loop
    execute format('alter table %I enable row level security', t);
    execute format('alter table %I force row level security', t);
  end loop; end $$;

create policy tenants_read on tenants for select using (is_member(id));
create policy members_read on tenant_members for select using (is_member(tenant_id));
create policy roles_read on roles for select using (is_member(tenant_id));
create policy role_perms_read on role_permissions for select using (is_member(tenant_id));
create policy user_roles_read on user_roles for select using (is_member(tenant_id));
create policy products_read on products for select using (is_member(tenant_id));
create policy categories_read on product_categories for select using (is_member(tenant_id));
create policy locations_read on stock_locations for select using (can_access_location(tenant_id, id));
create policy ula_read on user_location_access for select using (is_member(tenant_id));
create policy balances_read on inventory_balances for select using (can_access_location(tenant_id, location_id));
create policy movements_read on stock_movements for select using (can_access_location(tenant_id, location_id));
create policy audit_read on audit_logs for select using (has_permission(tenant_id, 'audit.view'));

-- Clients get read access only. All writes arrive through RPCs (or the service role in the Worker).
revoke insert, update, delete on all tables in schema public from anon, authenticated;
grant select on all tables in schema public to authenticated;

-- ---------- RPC: post one stock movement, idempotently ----------
create function post_stock_movement(
  p_tenant uuid, p_location uuid, p_product uuid, p_bucket text, p_qty bigint,
  p_movement_type text, p_source_type text, p_source_id text, p_key text,
  p_allow_negative boolean default false, p_device uuid default null, p_origin text default 'mobile'
) returns text
language plpgsql security definer set search_path = public as $$
declare cur bigint; inserted uuid; perm text;
begin
  if auth.uid() is null then raise exception 'not authenticated' using errcode = '28000'; end if;
  if not is_member(p_tenant) then raise exception 'forbidden' using errcode = '42501'; end if;
  if (select status from tenants where id = p_tenant) <> 'active' then
    raise exception 'tenant not active' using errcode = '42501'; end if;
  perm := case p_movement_type when 'sale' then 'sales.create' when 'purchase' then 'purchasing.receive'
                               when 'return' then 'sales.return' else 'inventory.adjust' end;
  if not has_permission(p_tenant, perm) then raise exception 'forbidden' using errcode = '42501'; end if;
  if not can_access_location(p_tenant, p_location) then raise exception 'forbidden location' using errcode = '42501'; end if;
  if p_allow_negative and not has_permission(p_tenant, 'inventory.allow_negative') then
    raise exception 'forbidden negative stock' using errcode = '42501'; end if;
  if p_origin not in ('mobile','cloud') then raise exception 'bad origin'; end if;
  if p_qty = 0 or p_bucket not in ('on_hand','damaged','in_transit') then raise exception 'invalid movement'; end if;

  -- serialize on the balance row; create it if absent
  insert into inventory_balances (tenant_id, location_id, product_id)
    values (p_tenant, p_location, p_product) on conflict do nothing;
  select case p_bucket when 'on_hand' then on_hand when 'damaged' then damaged else in_transit end into cur
    from inventory_balances where location_id = p_location and product_id = p_product for update;

  -- idempotency first: a retry must report 'duplicate' even if stock has since changed
  insert into stock_movements (tenant_id, location_id, product_id, bucket, qty_delta, movement_type,
      source_type, source_id, idempotency_key, allow_negative, origin_type, device_id, actor_id)
    values (p_tenant, p_location, p_product, p_bucket, p_qty, p_movement_type,
      p_source_type, p_source_id, p_key, p_allow_negative, p_origin, p_device, auth.uid())
    on conflict (tenant_id, idempotency_key) do nothing returning id into inserted;
  if inserted is null then return 'duplicate'; end if;

  if not p_allow_negative and cur + p_qty < 0 then
    raise exception 'insufficient stock' using errcode = 'P0001'; -- rolls back the insert above
  end if;

  update inventory_balances set
      on_hand    = on_hand    + case when p_bucket = 'on_hand'    then p_qty else 0 end,
      damaged    = damaged    + case when p_bucket = 'damaged'    then p_qty else 0 end,
      in_transit = in_transit + case when p_bucket = 'in_transit' then p_qty else 0 end,
      updated_at = now()
    where location_id = p_location and product_id = p_product;
  return 'posted';
end $$;

revoke all on function post_stock_movement from public, anon;
grant execute on function post_stock_movement to authenticated;
