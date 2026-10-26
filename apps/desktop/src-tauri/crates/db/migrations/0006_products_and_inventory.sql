-- Migration 0006: product details (company, brand, model, Urdu name, scheme) and the business tables for stock.
-- The stock LEDGER itself (stock_movements, inventory_balances and their triggers) is from migration 0001; this adds
-- the records that explain why stock moved: operations (restock/adjust), transfers and counts.

-- ---------- companies and brands ----------
CREATE TABLE companies (
  id         TEXT PRIMARY KEY,
  tenant_id  TEXT NOT NULL REFERENCES tenants(id),
  name       TEXT NOT NULL,
  is_active  INTEGER NOT NULL DEFAULT 1 CHECK (is_active IN (0,1)),
  revision   INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  UNIQUE (tenant_id, id)
);
CREATE UNIQUE INDEX companies_name_unique ON companies(tenant_id, lower(name));

CREATE TABLE brands (
  id         TEXT PRIMARY KEY,
  tenant_id  TEXT NOT NULL REFERENCES tenants(id),
  name       TEXT NOT NULL,
  company_id TEXT,
  is_active  INTEGER NOT NULL DEFAULT 1 CHECK (is_active IN (0,1)),
  revision   INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  UNIQUE (tenant_id, id),
  FOREIGN KEY (tenant_id, company_id) REFERENCES companies(tenant_id, id)
);
CREATE UNIQUE INDEX brands_name_unique ON brands(tenant_id, lower(name));

-- ---------- product details ----------
-- SQLite cannot add a composite foreign key to an existing table, so company_id/brand_id use single-column
-- references plus triggers that insist the company/brand belongs to the same shop.
ALTER TABLE products ADD COLUMN name_ur TEXT;
ALTER TABLE products ADD COLUMN model TEXT;
ALTER TABLE products ADD COLUMN company_id TEXT REFERENCES companies(id);
ALTER TABLE products ADD COLUMN brand_id TEXT REFERENCES brands(id);
-- A scheme is "buy N, get M free": both numbers or neither. (A CHECK passes on NULL, hence the explicit IS NOT NULL.)
ALTER TABLE products ADD COLUMN scheme_buy INTEGER CHECK (scheme_buy IS NULL OR scheme_buy >= 1);
ALTER TABLE products ADD COLUMN scheme_free INTEGER
  CHECK ((scheme_buy IS NULL AND scheme_free IS NULL) OR (scheme_buy IS NOT NULL AND scheme_free IS NOT NULL AND scheme_free >= 1));

CREATE TRIGGER products_company_same_tenant_ins BEFORE INSERT ON products
WHEN NEW.company_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM companies WHERE id = NEW.company_id AND tenant_id = NEW.tenant_id)
BEGIN SELECT RAISE(ABORT,'company belongs to another shop'); END;
CREATE TRIGGER products_company_same_tenant_upd BEFORE UPDATE OF company_id, tenant_id ON products
WHEN NEW.company_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM companies WHERE id = NEW.company_id AND tenant_id = NEW.tenant_id)
BEGIN SELECT RAISE(ABORT,'company belongs to another shop'); END;
CREATE TRIGGER products_brand_same_tenant_ins BEFORE INSERT ON products
WHEN NEW.brand_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM brands WHERE id = NEW.brand_id AND tenant_id = NEW.tenant_id)
BEGIN SELECT RAISE(ABORT,'brand belongs to another shop'); END;
CREATE TRIGGER products_brand_same_tenant_upd BEFORE UPDATE OF brand_id, tenant_id ON products
WHEN NEW.brand_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM brands WHERE id = NEW.brand_id AND tenant_id = NEW.tenant_id)
BEGIN SELECT RAISE(ABORT,'brand belongs to another shop'); END;

-- ---------- stock operations: restock and adjustments (one product per row) ----------
-- kind: receive (+qty), correction (+/- qty), damage (qty moves on_hand -> damaged), write_off (qty leaves damaged).
CREATE TABLE stock_operations (
  id              TEXT PRIMARY KEY,
  tenant_id       TEXT NOT NULL REFERENCES tenants(id),
  kind            TEXT NOT NULL CHECK (kind IN ('receive','correction','damage','write_off')),
  location_id     TEXT NOT NULL,
  product_id      TEXT NOT NULL,
  qty             INTEGER NOT NULL CHECK (qty <> 0),
  note            TEXT,
  reference       TEXT,
  actor_id        TEXT,
  device_id       TEXT,
  idempotency_key TEXT NOT NULL,
  created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  UNIQUE (tenant_id, id),
  UNIQUE (tenant_id, idempotency_key),
  FOREIGN KEY (tenant_id, location_id) REFERENCES stock_locations(tenant_id, id),
  FOREIGN KEY (tenant_id, product_id)  REFERENCES products(tenant_id, id)
);
CREATE INDEX stock_operations_by_product ON stock_operations(tenant_id, product_id, created_at);
CREATE TRIGGER stock_operations_no_update BEFORE UPDATE ON stock_operations BEGIN SELECT RAISE(ABORT,'stock_operations is append-only'); END;
CREATE TRIGGER stock_operations_no_delete BEFORE DELETE ON stock_operations BEGIN SELECT RAISE(ABORT,'stock_operations is append-only'); END;

-- ---------- transfers: dispatch from one location, receive at another ----------
CREATE TABLE stock_transfers (
  id               TEXT PRIMARY KEY,
  tenant_id        TEXT NOT NULL REFERENCES tenants(id),
  number           TEXT NOT NULL,
  from_location_id TEXT NOT NULL,
  to_location_id   TEXT NOT NULL,
  status           TEXT NOT NULL DEFAULT 'in_transit' CHECK (status IN ('in_transit','received','cancelled')),
  note             TEXT,
  created_by       TEXT,
  created_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  received_by      TEXT,
  received_at      TEXT,
  cancelled_by     TEXT,
  cancelled_at     TEXT,
  idempotency_key  TEXT NOT NULL,
  revision         INTEGER NOT NULL DEFAULT 1,
  UNIQUE (tenant_id, id),
  UNIQUE (tenant_id, number),
  UNIQUE (tenant_id, idempotency_key),
  CHECK (from_location_id <> to_location_id),
  FOREIGN KEY (tenant_id, from_location_id) REFERENCES stock_locations(tenant_id, id),
  FOREIGN KEY (tenant_id, to_location_id)   REFERENCES stock_locations(tenant_id, id)
);
CREATE TRIGGER stock_transfers_no_delete BEFORE DELETE ON stock_transfers BEGIN SELECT RAISE(ABORT,'transfers cannot be deleted'); END;
CREATE TABLE stock_transfer_items (
  tenant_id   TEXT NOT NULL,
  transfer_id TEXT NOT NULL,
  product_id  TEXT NOT NULL,
  qty         INTEGER NOT NULL CHECK (qty > 0),
  PRIMARY KEY (transfer_id, product_id),
  FOREIGN KEY (tenant_id, transfer_id) REFERENCES stock_transfers(tenant_id, id),
  FOREIGN KEY (tenant_id, product_id)  REFERENCES products(tenant_id, id)
);
CREATE TRIGGER stock_transfer_items_no_update BEFORE UPDATE ON stock_transfer_items BEGIN SELECT RAISE(ABORT,'transfer lines cannot change'); END;
CREATE TRIGGER stock_transfer_items_no_delete BEFORE DELETE ON stock_transfer_items BEGIN SELECT RAISE(ABORT,'transfer lines cannot change'); END;

-- ---------- stock counts: what was counted against what the system said, posted at once ----------
CREATE TABLE stock_counts (
  id              TEXT PRIMARY KEY,
  tenant_id       TEXT NOT NULL REFERENCES tenants(id),
  number          TEXT NOT NULL,
  location_id     TEXT NOT NULL,
  note            TEXT,
  created_by      TEXT,
  created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  idempotency_key TEXT NOT NULL,
  UNIQUE (tenant_id, id),
  UNIQUE (tenant_id, number),
  UNIQUE (tenant_id, idempotency_key),
  FOREIGN KEY (tenant_id, location_id) REFERENCES stock_locations(tenant_id, id)
);
CREATE TABLE stock_count_lines (
  tenant_id  TEXT NOT NULL,
  count_id   TEXT NOT NULL,
  product_id TEXT NOT NULL,
  expected   INTEGER NOT NULL,
  counted    INTEGER NOT NULL CHECK (counted >= 0),
  PRIMARY KEY (count_id, product_id),
  FOREIGN KEY (tenant_id, count_id)   REFERENCES stock_counts(tenant_id, id),
  FOREIGN KEY (tenant_id, product_id) REFERENCES products(tenant_id, id)
);
CREATE TRIGGER stock_counts_no_update BEFORE UPDATE ON stock_counts BEGIN SELECT RAISE(ABORT,'stock counts are append-only'); END;
CREATE TRIGGER stock_counts_no_delete BEFORE DELETE ON stock_counts BEGIN SELECT RAISE(ABORT,'stock counts are append-only'); END;
CREATE TRIGGER stock_count_lines_no_update BEFORE UPDATE ON stock_count_lines BEGIN SELECT RAISE(ABORT,'stock counts are append-only'); END;
CREATE TRIGGER stock_count_lines_no_delete BEFORE DELETE ON stock_count_lines BEGIN SELECT RAISE(ABORT,'stock counts are append-only'); END;

INSERT INTO schema_migrations (version, name) VALUES (6, 'products_and_inventory');
