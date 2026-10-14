-- Migration 0005 (Postgres): customers, revisions on categories, one product per barcode.
-- Mirrors apps/desktop/src-tauri/crates/db/migrations/0005_customers_and_catalog.sql (SQLite).
-- Apply once, in the Supabase SQL Editor, after 0001-0004. Not re-runnable.
-- Clients get read access only; writes arrive from the Worker (service role), like the other master data.

alter table product_categories add column revision integer not null default 1;

-- A barcode identifies exactly one product within a shop (NULL barcodes are unrestricted).
create unique index products_barcode_unique on products (tenant_id, barcode) where barcode is not null;

create table customers (
  id uuid primary key default gen_random_uuid(),
  tenant_id uuid not null references tenants(id),
  code text not null,
  name text not null,
  phone text,
  address text,
  notes text,
  is_active boolean not null default true,
  revision integer not null default 1,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  unique (tenant_id, id),
  unique (tenant_id, code)
);

-- Customers are deactivated, never deleted: sales and the accounts ledger will refer to them.
create function customers_no_delete() returns trigger language plpgsql as $$
begin raise exception 'customers cannot be deleted; deactivate instead' using errcode = '23000'; end $$;
create trigger customers_no_delete before delete on customers
  for each row execute function customers_no_delete();

alter table customers enable row level security;
alter table customers force row level security;
create policy customers_read on customers for select using (has_permission(tenant_id, 'customers.view'));

-- Supabase grants new tables to anon/authenticated by default: take it all back, then allow reads.
revoke all on customers from anon, authenticated;
grant select on customers to authenticated;
