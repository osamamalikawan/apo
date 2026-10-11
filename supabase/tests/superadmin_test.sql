-- Super Admin / subscription security test. Run in the Supabase SQL Editor AFTER migrations 0001-0004.
-- Builds test data, switches between simulated users, checks each rule, then ROLLS EVERYTHING BACK.
-- Expected result is an "error" reading: ALL 20 CHECKS PASSED (test data rolled back)
-- Any message starting with FAILED names the check that broke.
do $$
declare
  ta uuid := gen_random_uuid();
  up uuid := gen_random_uuid(); uc uuid := gen_random_uuid(); ub uuid := gen_random_uuid();
  rc uuid := gen_random_uuid(); rb uuid := gen_random_uuid();
  loc uuid := gen_random_uuid(); pa uuid := gen_random_uuid(); plan uuid := gen_random_uuid();
  n bigint; r text; rev bigint; j jsonb; passed int := 0;
begin
  -- ===== setup as the SQL editor's admin role =====
  insert into auth.users(id, aud, role, email) values
    (up,'authenticated','authenticated','platform-'||up||'@test.local'),
    (uc,'authenticated','authenticated','cashier-'||uc||'@test.local'),
    (ub,'authenticated','authenticated','manager-'||ub||'@test.local');
  insert into platform_admins(user_id) values (up);   -- up is a platform admin and NOT a member of the tenant
  insert into tenants(id,name) values (ta,'Subscription Test Tenant');
  insert into tenant_members(tenant_id,user_id,full_name) values (ta,uc,'Cashier'),(ta,ub,'Manager');
  insert into roles(id,tenant_id,name) values (rc,ta,'Cashier'),(rb,ta,'Manager');
  insert into role_permissions(tenant_id,role_id,permission) values
    (ta,rc,'sales.create'),(ta,rb,'inventory.*'),(ta,rb,'locations.all');
  insert into user_roles(tenant_id,user_id,role_id) values (ta,uc,rc),(ta,ub,rb);
  insert into stock_locations(id,tenant_id,name,code) values (loc,ta,'Main','MAIN');
  insert into user_location_access(tenant_id,user_id,location_id) values (ta,uc,loc);
  insert into products(id,tenant_id,sku,name) values (pa,ta,'SKU','Product');
  insert into subscription_plans(id,name,max_devices,max_users,max_locations,modules)
    values (plan,'Test plan '||plan,3,10,4,array['sales','inventory']);
  insert into tenant_subscriptions(tenant_id,plan_id,current_period_end) values (ta,plan,now() + interval '30 days');

  -- ===== manager stocks the shop =====
  perform set_config('request.jwt.claims', json_build_object('sub',ub,'role','authenticated')::text, true);
  execute 'set local role authenticated';
  r := post_stock_movement(ta, loc, pa, 'on_hand', 100, 'adjustment', 'test', 's0', 'k-s0');
  if r <> 'posted' then raise exception 'FAILED #1: initial stock returned %', r; end if; passed := passed + 1;

  -- ===== cashier cannot touch subscriptions =====
  perform set_config('request.jwt.claims', json_build_object('sub',uc,'role','authenticated')::text, true);
  begin
    perform superadmin_set_subscription(ta, 'suspended');
    raise exception 'FAILED #2: cashier suspended a tenant';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlstate <> '42501' then raise exception 'FAILED #2: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;
  begin
    perform 1 from platform_admins limit 1;
    raise exception 'FAILED #3: cashier can read platform_admins';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlstate <> '42501' then raise exception 'FAILED #3: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;
  select count(*) into n from tenant_subscriptions;
  if n <> 1 then raise exception 'FAILED #4: cashier sees % subscriptions, expected 1', n; end if; passed := passed + 1;
  begin
    update tenant_subscriptions set status = 'active', current_period_end = now() + interval '99 years';
    raise exception 'FAILED #5: cashier edited the subscription directly';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlstate <> '42501' then raise exception 'FAILED #5: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;
  r := post_stock_movement(ta, loc, pa, 'on_hand', -10, 'sale', 'test', 'sale1', 'k-sale1');
  if r <> 'posted' then raise exception 'FAILED #6: sale while active returned %', r; end if; passed := passed + 1;

  -- ===== platform admin suspends the tenant =====
  perform set_config('request.jwt.claims', json_build_object('sub',up,'role','authenticated')::text, true);
  rev := superadmin_set_subscription(ta, 'suspended', null, 'unpaid invoice');
  if rev <> 2 then raise exception 'FAILED #7: revision after suspend is %, expected 2', rev; end if; passed := passed + 1;
  select count(*) into n from platform_audit_logs where tenant_id = ta;
  if n <> 1 then raise exception 'FAILED #8: platform admin sees % audit rows, expected 1', n; end if; passed := passed + 1;

  -- ===== cashier: writes stop, reads continue =====
  perform set_config('request.jwt.claims', json_build_object('sub',uc,'role','authenticated')::text, true);
  begin
    perform post_stock_movement(ta, loc, pa, 'on_hand', -1, 'sale', 'test', 'sale2', 'k-sale2');
    raise exception 'FAILED #9: sale accepted while suspended';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlstate <> '42501' or sqlerrm <> 'tenant not active' then
      raise exception 'FAILED #9: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;
  select count(*) into n from inventory_balances;
  if n <> 1 then raise exception 'FAILED #10: suspended tenant cannot read its balances (% rows)', n; end if; passed := passed + 1;
  select count(*) into n from platform_audit_logs;
  if n <> 0 then raise exception 'FAILED #11: cashier can read platform audit logs (% rows)', n; end if; passed := passed + 1;
  begin
    perform tenant_entitlement_payload(ta);
    raise exception 'FAILED #12: cashier can build entitlement payloads';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlstate <> '42501' then raise exception 'FAILED #12: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;

  -- ===== signed-out caller =====
  perform set_config('request.jwt.claims', '', true);
  execute 'set local role anon';
  begin
    perform superadmin_set_subscription(ta, 'active');
    raise exception 'FAILED #13: anon called the Super Admin function';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlstate <> '42501' then raise exception 'FAILED #13: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;

  -- ===== admin role: payload content =====
  execute 'reset role';
  j := tenant_entitlement_payload(ta);
  if j->>'status' <> 'suspended' or (j->>'revision')::bigint <> 2 then
    raise exception 'FAILED #14: payload says % rev %', j->>'status', j->>'revision'; end if; passed := passed + 1;
  if (j->'limits'->>'max_devices')::int <> 3 or j->'modules' <> '["sales","inventory"]'::jsonb then
    raise exception 'FAILED #15: payload limits/modules wrong: %', j; end if; passed := passed + 1;
  if not (j->>'issued_at' ~ '^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$'
      and j->>'full_until' ~ '^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$'
      and j->>'grace_until' ~ '^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$'
      and j->>'grace_until' > j->>'full_until') then
    raise exception 'FAILED #16: payload dates malformed or out of order: %', j; end if; passed := passed + 1;

  -- ===== platform admin: bad input, then reinstatement =====
  perform set_config('request.jwt.claims', json_build_object('sub',up,'role','authenticated')::text, true);
  execute 'set local role authenticated';
  begin
    perform superadmin_set_subscription(ta, 'bogus');
    raise exception 'FAILED #17: invalid status accepted';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlstate <> '22023' then raise exception 'FAILED #17: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;
  begin
    perform superadmin_set_subscription(gen_random_uuid(), 'suspended');
    raise exception 'FAILED #18: unknown tenant accepted';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlstate <> 'P0002' then raise exception 'FAILED #18: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;
  rev := superadmin_set_subscription(ta, 'active', now() + interval '60 days', 'paid');
  if rev <> 3 then raise exception 'FAILED #19: revision after reinstate is %', rev; end if;
  perform set_config('request.jwt.claims', json_build_object('sub',uc,'role','authenticated')::text, true);
  r := post_stock_movement(ta, loc, pa, 'on_hand', -5, 'sale', 'test', 'sale3', 'k-sale3');
  if r <> 'posted' then raise exception 'FAILED #19: sale after reinstate returned %', r; end if; passed := passed + 1;

  -- ===== audit trail is permanent =====
  execute 'reset role';
  begin
    delete from platform_audit_logs;
    raise exception 'FAILED #20: platform audit rows could be deleted';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlerrm not like '%append-only%' then raise exception 'FAILED #20: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;

  raise exception 'ALL % CHECKS PASSED (test data rolled back)', passed;
end $$;
