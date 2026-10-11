-- Fix found by supabase/tests/rls_test.sql check #19:
-- a user with 'locations.all' passed the location check for ANY location id, including another
-- tenant's, and was only stopped by a foreign key. Ownership is now verified explicitly.

create or replace function can_access_location(p_tenant uuid, p_location uuid) returns boolean
language sql stable security definer set search_path = public as $$
  select is_member(p_tenant)
    and exists (select 1 from stock_locations where id = p_location and tenant_id = p_tenant)
    and (has_permission(p_tenant, 'locations.all')
         or exists (select 1 from user_location_access
                    where tenant_id = p_tenant and user_id = auth.uid() and location_id = p_location)) $$;

create or replace function post_stock_movement(
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
  if not exists (select 1 from products where id = p_product and tenant_id = p_tenant) then
    raise exception 'forbidden product' using errcode = '42501'; end if;
  if p_allow_negative and not has_permission(p_tenant, 'inventory.allow_negative') then
    raise exception 'forbidden negative stock' using errcode = '42501'; end if;
  if p_origin not in ('mobile','cloud') then raise exception 'bad origin'; end if;
  if p_qty = 0 or p_bucket not in ('on_hand','damaged','in_transit') then raise exception 'invalid movement'; end if;

  insert into inventory_balances (tenant_id, location_id, product_id)
    values (p_tenant, p_location, p_product) on conflict do nothing;
  select case p_bucket when 'on_hand' then on_hand when 'damaged' then damaged else in_transit end into cur
    from inventory_balances where location_id = p_location and product_id = p_product for update;

  insert into stock_movements (tenant_id, location_id, product_id, bucket, qty_delta, movement_type,
      source_type, source_id, idempotency_key, allow_negative, origin_type, device_id, actor_id)
    values (p_tenant, p_location, p_product, p_bucket, p_qty, p_movement_type,
      p_source_type, p_source_id, p_key, p_allow_negative, p_origin, p_device, auth.uid())
    on conflict (tenant_id, idempotency_key) do nothing returning id into inserted;
  if inserted is null then return 'duplicate'; end if;

  if not p_allow_negative and cur + p_qty < 0 then
    raise exception 'insufficient stock' using errcode = 'P0001';
  end if;

  update inventory_balances set
      on_hand    = on_hand    + case when p_bucket = 'on_hand'    then p_qty else 0 end,
      damaged    = damaged    + case when p_bucket = 'damaged'    then p_qty else 0 end,
      in_transit = in_transit + case when p_bucket = 'in_transit' then p_qty else 0 end,
      updated_at = now()
    where location_id = p_location and product_id = p_product;
  return 'posted';
end $$;
