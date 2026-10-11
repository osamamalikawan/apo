-- Two-tenant security test. Run in the Supabase SQL Editor AFTER migrations 0001 and 0002.
-- It builds test data, switches between simulated users, checks each rule, then ROLLS EVERYTHING BACK.
-- Expected result is an "error" reading: ALL 24 CHECKS PASSED (test data rolled back)
-- Any message starting with FAILED names the check that broke.
do $$
declare
  ta uuid := gen_random_uuid(); tb uuid := gen_random_uuid();
  ua uuid := gen_random_uuid(); ub uuid := gen_random_uuid(); uc uuid := gen_random_uuid();
  ra uuid := gen_random_uuid(); rb uuid := gen_random_uuid(); rc uuid := gen_random_uuid();
  loc_main uuid := gen_random_uuid(); loc_g1 uuid := gen_random_uuid(); loc_b uuid := gen_random_uuid();
  pa uuid := gen_random_uuid(); pb uuid := gen_random_uuid();
  n bigint; r text; passed int := 0;
begin
  -- ===== setup (as the SQL editor's admin role) =====
  insert into auth.users(id, aud, role, email) values
    (ua,'authenticated','authenticated','cashier-'||ua||'@test.local'),
    (ub,'authenticated','authenticated','manager-'||ub||'@test.local'),
    (uc,'authenticated','authenticated','admin-'||uc||'@test.local');
  insert into tenants(id,name) values (ta,'Test Tenant A'),(tb,'Test Tenant B');
  insert into tenant_members(tenant_id,user_id,full_name) values (ta,ua,'Cashier A'),(ta,ub,'Manager A'),(tb,uc,'Admin B');
  insert into roles(id,tenant_id,name) values (ra,ta,'Cashier'),(rb,ta,'Manager'),(rc,tb,'Admin');
  insert into role_permissions(tenant_id,role_id,permission) values
    (ta,ra,'sales.create'),(ta,ra,'inventory.view'),
    (ta,rb,'inventory.*'),(ta,rb,'locations.all'),
    (tb,rc,'*');
  insert into user_roles(tenant_id,user_id,role_id) values (ta,ua,ra),(ta,ub,rb),(tb,uc,rc);
  insert into stock_locations(id,tenant_id,name,code) values
    (loc_main,ta,'Main Shop','MAIN'),(loc_g1,ta,'Godown 1','G1'),(loc_b,tb,'Main B','MAINB');
  insert into user_location_access(tenant_id,user_id,location_id) values (ta,ua,loc_main);
  insert into products(id,tenant_id,sku,name) values (pa,ta,'SKU-A','Product A'),(pb,tb,'SKU-B','Product B');

  -- ===== Manager of tenant A =====
  perform set_config('request.jwt.claims', json_build_object('sub',ub,'role','authenticated')::text, true);
  execute 'set local role authenticated';
  r := post_stock_movement(ta, loc_main, pa, 'on_hand', 100, 'adjustment', 'test', 'recv1', 'k-recv1');
  if r <> 'posted' then raise exception 'FAILED #1: manager receive returned %', r; end if; passed := passed + 1;
  r := post_stock_movement(ta, loc_main, pa, 'on_hand', 100, 'adjustment', 'test', 'recv1', 'k-recv1');
  if r <> 'duplicate' then raise exception 'FAILED #2: retry returned %', r; end if; passed := passed + 1;
  select count(*) into n from stock_movements where tenant_id = ta;
  if n <> 1 then raise exception 'FAILED #3: expected 1 movement, found %', n; end if; passed := passed + 1;

  -- ===== Cashier of tenant A (main shop only) =====
  perform set_config('request.jwt.claims', json_build_object('sub',ua,'role','authenticated')::text, true);
  select count(*) into n from stock_locations;
  if n <> 1 then raise exception 'FAILED #4: cashier sees % locations, expected 1', n; end if; passed := passed + 1;
  select count(*) into n from tenants;
  if n <> 1 then raise exception 'FAILED #5: cashier sees % tenants, expected 1', n; end if; passed := passed + 1;
  r := post_stock_movement(ta, loc_main, pa, 'on_hand', -30, 'sale', 'test', 'sale1', 'k-sale1');
  if r <> 'posted' then raise exception 'FAILED #6: sale returned %', r; end if; passed := passed + 1;
  select on_hand into n from inventory_balances where location_id = loc_main and product_id = pa;
  if n <> 70 then raise exception 'FAILED #7: balance % expected 70', n; end if; passed := passed + 1;

  begin  -- #8 other location
    perform post_stock_movement(ta, loc_g1, pa, 'on_hand', -1, 'sale', 'test', 'sale2', 'k-sale2');
    raise exception 'FAILED #8: cashier sold from Godown 1';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlstate <> '42501' then raise exception 'FAILED #8: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;

  begin  -- #9 overdraw
    perform post_stock_movement(ta, loc_main, pa, 'on_hand', -100, 'sale', 'test', 'sale3', 'k-sale3');
    raise exception 'FAILED #9: overdraw allowed';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlerrm <> 'insufficient stock' then raise exception 'FAILED #9: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;
  select on_hand into n from inventory_balances where location_id = loc_main and product_id = pa;
  if n <> 70 then raise exception 'FAILED #10: balance changed to % after rejected overdraw', n; end if; passed := passed + 1;

  begin  -- #11 cashier cannot adjust
    perform post_stock_movement(ta, loc_main, pa, 'on_hand', 5, 'adjustment', 'test', 'adj1', 'k-adj1');
    raise exception 'FAILED #11: cashier adjusted stock';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlstate <> '42501' then raise exception 'FAILED #11: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;

  begin  -- #12 negative stock needs permission
    perform post_stock_movement(ta, loc_main, pa, 'on_hand', -500, 'sale', 'test', 'neg1', 'k-neg1', true);
    raise exception 'FAILED #12: cashier used negative stock';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlstate <> '42501' then raise exception 'FAILED #12: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;

  begin  -- #13 direct insert blocked
    insert into stock_movements(tenant_id,location_id,product_id,qty_delta,movement_type,source_type,source_id,idempotency_key)
      values (ta, loc_main, pa, 1000, 'x', 'x', 'x', 'k-direct');
    raise exception 'FAILED #13: direct insert into ledger worked';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlstate <> '42501' then raise exception 'FAILED #13: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;

  begin  -- #14 direct balance edit blocked
    update inventory_balances set on_hand = 999999 where location_id = loc_main;
    raise exception 'FAILED #14: direct balance update worked';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlstate <> '42501' then raise exception 'FAILED #14: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;

  begin  -- #15 direct ledger delete blocked
    delete from stock_movements;
    raise exception 'FAILED #15: direct ledger delete worked';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlstate <> '42501' then raise exception 'FAILED #15: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;

  select count(*) into n from products where tenant_id = tb;
  if n <> 0 then raise exception 'FAILED #16: cashier can see % products of another tenant', n; end if; passed := passed + 1;

  -- ===== Admin of tenant B (a different business) =====
  perform set_config('request.jwt.claims', json_build_object('sub',uc,'role','authenticated')::text, true);
  select count(*) into n from stock_movements;
  if n <> 0 then raise exception 'FAILED #17: tenant B sees % of tenant A movements', n; end if; passed := passed + 1;

  begin  -- #18 post into tenant A
    perform post_stock_movement(ta, loc_main, pa, 'on_hand', 50, 'adjustment', 'test', 'x', 'k-cross1');
    raise exception 'FAILED #18: cross-tenant post worked';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlstate <> '42501' then raise exception 'FAILED #18: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;

  begin  -- #19 own tenant id but another tenant's location
    perform post_stock_movement(tb, loc_main, pa, 'on_hand', 50, 'adjustment', 'test', 'x', 'k-cross2');
    raise exception 'FAILED #19: used tenant A location under tenant B';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlstate <> '42501' then raise exception 'FAILED #19: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;

  begin  -- #24 another tenant's product under my own tenant
    perform post_stock_movement(tb, loc_b, pa, 'on_hand', 5, 'adjustment', 'test', 'x', 'k-cross3');
    raise exception 'FAILED #24: used tenant A product under tenant B';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlstate <> '42501' then raise exception 'FAILED #24: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;

  select count(*) into n from products;
  if n <> 1 then raise exception 'FAILED #20: tenant B sees % products, expected 1', n; end if; passed := passed + 1;
  r := post_stock_movement(tb, loc_b, pb, 'on_hand', 5, 'adjustment', 'test', 'own', 'k-own');
  if r <> 'posted' then raise exception 'FAILED #21: tenant B own post returned %', r; end if; passed := passed + 1;

  -- ===== Signed-out caller =====
  perform set_config('request.jwt.claims', '', true);
  execute 'set local role anon';
  begin
    perform 1 from stock_movements limit 1;
    raise exception 'FAILED #22: anon can read the ledger';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlstate <> '42501' then raise exception 'FAILED #22: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;

  -- ===== Back to admin: ledger integrity =====
  execute 'reset role';
  begin
    update stock_movements set qty_delta = 1;
    raise exception 'FAILED #23: ledger row was editable';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlerrm not like '%append-only%' then raise exception 'FAILED #23: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;

  raise exception 'ALL % CHECKS PASSED (test data rolled back)', passed;
end $$;
