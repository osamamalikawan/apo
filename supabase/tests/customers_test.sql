-- Customers and catalog (migration 0005) test. Paste into the Supabase SQL Editor.
-- Everything it creates is rolled back: it ends by raising "ALL n CHECKS PASSED (test data rolled back)".
do $$
declare
  ta uuid := gen_random_uuid(); tb uuid := gen_random_uuid();
  uv uuid := gen_random_uuid(); un uuid := gen_random_uuid(); ub uuid := gen_random_uuid();
  rv uuid := gen_random_uuid(); rn uuid := gen_random_uuid(); rb uuid := gen_random_uuid();
  cat uuid := gen_random_uuid();
  n bigint; passed int := 0;
begin
  -- ===== setup as the SQL editor's admin role =====
  insert into auth.users(id, aud, role, email) values
    (uv,'authenticated','authenticated','viewer-'||uv||'@test.local'),
    (un,'authenticated','authenticated','nobody-'||un||'@test.local'),
    (ub,'authenticated','authenticated','other-'||ub||'@test.local');
  insert into tenants(id,name) values (ta,'Shop A'),(tb,'Shop B');
  insert into tenant_members(tenant_id,user_id,full_name) values (ta,uv,'Viewer'),(ta,un,'Nobody'),(tb,ub,'Other');
  insert into roles(id,tenant_id,name) values (rv,ta,'Viewer'),(rn,ta,'Cashier'),(rb,tb,'Viewer');
  insert into role_permissions(tenant_id,role_id,permission) values
    (ta,rv,'customers.view'),(ta,rn,'sales.create'),(tb,rb,'customers.view');
  insert into user_roles(tenant_id,user_id,role_id) values (ta,uv,rv),(ta,un,rn),(tb,ub,rb);
  insert into customers(tenant_id,code,name) values
    (ta,'C00001','Ali Traders'),(ta,'C00002','Bilal Motors'),(tb,'C00001','Other Shop Customer');

  -- ===== defaults =====
  select count(*) into n from customers where tenant_id = ta and is_active and revision = 1;
  if n <> 2 then raise exception 'FAILED #1: defaults (is_active, revision) wrong, % rows', n; end if; passed := passed + 1;
  insert into product_categories(id,tenant_id,name) values (cat,ta,'Brakes');
  select count(*) into n from product_categories where id = cat and revision = 1;
  if n <> 1 then raise exception 'FAILED #2: category revision should default to 1'; end if; passed := passed + 1;

  -- ===== uniqueness =====
  begin
    insert into customers(tenant_id,code,name) values (ta,'C00001','Duplicate code');
    raise exception 'FAILED #3: duplicate customer code accepted in one shop';
  exception when unique_violation then passed := passed + 1; end;
  -- the same code in another shop is fine (already inserted above for tb)
  select count(*) into n from customers where code = 'C00001';
  if n <> 2 then raise exception 'FAILED #4: each shop should be able to use C00001, found %', n; end if; passed := passed + 1;

  insert into products(tenant_id,sku,name,barcode) values (ta,'SKU-1','Part one','8901234567890');
  begin
    insert into products(tenant_id,sku,name,barcode) values (ta,'SKU-2','Part two','8901234567890');
    raise exception 'FAILED #5: duplicate barcode accepted in one shop';
  exception when unique_violation then passed := passed + 1; end;
  insert into products(tenant_id,sku,name,barcode) values (ta,'SKU-3','No barcode one',null),(ta,'SKU-4','No barcode two',null);
  passed := passed + 1;  -- #6: any number of products without a barcode
  insert into products(tenant_id,sku,name,barcode) values (tb,'SKU-1','Other shop part','8901234567890');
  passed := passed + 1;  -- #7: the same barcode in another shop is fine

  -- ===== deletes are blocked even for the admin role =====
  begin
    delete from customers where tenant_id = ta;
    raise exception 'FAILED #8: customers could be deleted';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlerrm not like '%deactivate instead%' then raise exception 'FAILED #8: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;

  -- ===== a member WITH customers.view sees only their own shop =====
  perform set_config('request.jwt.claims', json_build_object('sub',uv,'role','authenticated')::text, true);
  execute 'set local role authenticated';
  select count(*) into n from customers;
  if n <> 2 then raise exception 'FAILED #9: viewer sees % customers, expected 2', n; end if; passed := passed + 1;
  select count(*) into n from customers where tenant_id = tb;
  if n <> 0 then raise exception 'FAILED #10: viewer can see another shop''s customers'; end if; passed := passed + 1;

  -- ===== clients cannot write =====
  begin
    insert into customers(tenant_id,code,name) values (ta,'C00099','Sneaky');
    raise exception 'FAILED #11: viewer inserted a customer';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlstate <> '42501' then raise exception 'FAILED #11: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;
  begin
    update customers set name = 'Hacked';
    raise exception 'FAILED #12: viewer updated a customer';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlstate <> '42501' then raise exception 'FAILED #12: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;
  begin
    delete from customers;
    raise exception 'FAILED #13: viewer deleted customers';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlstate <> '42501' then raise exception 'FAILED #13: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;

  -- ===== a member WITHOUT customers.view sees nothing =====
  perform set_config('request.jwt.claims', json_build_object('sub',un,'role','authenticated')::text, true);
  select count(*) into n from customers;
  if n <> 0 then raise exception 'FAILED #14: member without customers.view sees % customers', n; end if; passed := passed + 1;

  -- ===== another shop's viewer sees only their own =====
  perform set_config('request.jwt.claims', json_build_object('sub',ub,'role','authenticated')::text, true);
  select count(*) into n from customers;
  if n <> 1 then raise exception 'FAILED #15: other shop sees % customers, expected 1', n; end if; passed := passed + 1;
  select count(*) into n from customers where name = 'Ali Traders';
  if n <> 0 then raise exception 'FAILED #16: other shop can see Shop A customers'; end if; passed := passed + 1;

  -- ===== signed-out callers have no access at all =====
  perform set_config('request.jwt.claims', '', true);
  execute 'set local role anon';
  begin
    perform 1 from customers limit 1;
    raise exception 'FAILED #17: anon can read customers';
  exception when others then
    if sqlerrm like 'FAILED #%' then raise; end if;
    if sqlstate <> '42501' then raise exception 'FAILED #17: wrong error % %', sqlstate, sqlerrm; end if;
    passed := passed + 1;
  end;

  execute 'reset role';
  raise exception 'ALL % CHECKS PASSED (test data rolled back)', passed;
end $$;
