-- Migration 0005: customers, revisions on categories, one product per barcode.
ALTER TABLE product_categories ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;

-- A barcode identifies exactly one product within a shop (NULL barcodes are unrestricted).
CREATE UNIQUE INDEX products_barcode_unique ON products(tenant_id, barcode) WHERE barcode IS NOT NULL;

CREATE TABLE customers (
  id         TEXT PRIMARY KEY,
  tenant_id  TEXT NOT NULL REFERENCES tenants(id),
  code       TEXT NOT NULL,
  name       TEXT NOT NULL,
  phone      TEXT,
  address    TEXT,
  notes      TEXT,
  is_active  INTEGER NOT NULL DEFAULT 1 CHECK (is_active IN (0,1)),
  revision   INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  UNIQUE (tenant_id, id),
  UNIQUE (tenant_id, code)
);
-- Customers are deactivated, never deleted: sales and the accounts ledger will refer to them.
CREATE TRIGGER customers_no_delete BEFORE DELETE ON customers
BEGIN SELECT RAISE(ABORT,'customers cannot be deleted; deactivate instead'); END;

INSERT INTO schema_migrations (version, name) VALUES (5, 'customers_and_catalog');
