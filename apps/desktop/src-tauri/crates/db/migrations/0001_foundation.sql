-- Shop Server database, migration 0001: tenancy, identity, devices, locations, stock ledger, sync.
-- Conventions: ids are UUID text; timestamps are ISO-8601 UTC text; quantities are integers
-- (the app layer decides the unit scale); tenant ownership is enforced with composite foreign keys.
PRAGMA foreign_keys = ON;

CREATE TABLE schema_migrations (
  version     INTEGER PRIMARY KEY,
  name        TEXT NOT NULL,
  applied_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE TABLE tenants (
  id          TEXT PRIMARY KEY,
  name        TEXT NOT NULL,
  currency    TEXT NOT NULL DEFAULT 'PKR',
  timezone    TEXT NOT NULL DEFAULT 'Asia/Karachi',
  status      TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','suspended','terminated')),
  created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

-- The server instance for this shop. Exactly one row per database.
CREATE TABLE shop_nodes (
  id            TEXT PRIMARY KEY,
  tenant_id     TEXT NOT NULL REFERENCES tenants(id),
  name          TEXT NOT NULL,
  cert_fingerprint TEXT,
  created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE UNIQUE INDEX one_shop_node ON shop_nodes((1));

CREATE TABLE users (
  id            TEXT PRIMARY KEY,
  tenant_id     TEXT NOT NULL REFERENCES tenants(id),
  username      TEXT NOT NULL,
  full_name     TEXT NOT NULL,
  password_hash TEXT,            -- Argon2id PHC string; never a plaintext password
  pin_hash      TEXT,
  is_active     INTEGER NOT NULL DEFAULT 1 CHECK (is_active IN (0,1)),
  revision      INTEGER NOT NULL DEFAULT 1,
  created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  UNIQUE (tenant_id, id),
  UNIQUE (tenant_id, username)
);

CREATE TABLE roles (
  id          TEXT PRIMARY KEY,
  tenant_id   TEXT NOT NULL REFERENCES tenants(id),
  name        TEXT NOT NULL,
  is_template INTEGER NOT NULL DEFAULT 0,
  UNIQUE (tenant_id, id),
  UNIQUE (tenant_id, name)
);
CREATE TABLE role_permissions (
  tenant_id   TEXT NOT NULL,
  role_id     TEXT NOT NULL,
  permission  TEXT NOT NULL CHECK (permission = '*' OR permission GLOB '[a-z_]*.[a-z_*]*'),
  PRIMARY KEY (role_id, permission),
  FOREIGN KEY (tenant_id, role_id) REFERENCES roles(tenant_id, id) ON DELETE CASCADE
);
CREATE TABLE user_roles (
  tenant_id TEXT NOT NULL,
  user_id   TEXT NOT NULL,
  role_id   TEXT NOT NULL,
  PRIMARY KEY (user_id, role_id),
  FOREIGN KEY (tenant_id, user_id) REFERENCES users(tenant_id, id),
  FOREIGN KEY (tenant_id, role_id) REFERENCES roles(tenant_id, id)
);

-- Devices: clients paired to this server. The credential is stored only as a hash.
CREATE TABLE devices (
  id               TEXT PRIMARY KEY,
  tenant_id        TEXT NOT NULL REFERENCES tenants(id),
  name             TEXT NOT NULL,
  kind             TEXT NOT NULL CHECK (kind IN ('server','desktop_client','mobile')),
  credential_hash  TEXT NOT NULL,
  app_version      TEXT,
  last_seen_at     TEXT,
  revoked_at       TEXT,
  created_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  UNIQUE (tenant_id, id)
);
CREATE TABLE device_pairings (
  id          TEXT PRIMARY KEY,
  tenant_id   TEXT NOT NULL REFERENCES tenants(id),
  code_hash   TEXT NOT NULL,
  created_by  TEXT NOT NULL,
  expires_at  TEXT NOT NULL,
  used_at     TEXT,
  device_id   TEXT REFERENCES devices(id),
  FOREIGN KEY (tenant_id, created_by) REFERENCES users(tenant_id, id)
);

CREATE TABLE audit_logs (
  id          TEXT PRIMARY KEY,
  tenant_id   TEXT NOT NULL REFERENCES tenants(id),
  actor_id    TEXT,
  device_id   TEXT,
  action      TEXT NOT NULL,
  entity      TEXT NOT NULL,
  entity_id   TEXT,
  before_json TEXT,
  after_json  TEXT,
  at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX audit_by_entity ON audit_logs(tenant_id, entity, entity_id);
CREATE TRIGGER audit_logs_no_update BEFORE UPDATE ON audit_logs BEGIN SELECT RAISE(ABORT,'audit_logs is append-only'); END;
CREATE TRIGGER audit_logs_no_delete BEFORE DELETE ON audit_logs BEGIN SELECT RAISE(ABORT,'audit_logs is append-only'); END;

-- Stock locations (never hardcoded; tenant-managed).
CREATE TABLE stock_locations (
  id                 TEXT PRIMARY KEY,
  tenant_id          TEXT NOT NULL REFERENCES tenants(id),
  name               TEXT NOT NULL,
  code               TEXT NOT NULL,
  location_type      TEXT NOT NULL DEFAULT 'storage',
  description        TEXT,
  parent_location_id TEXT,
  responsible_user_id TEXT,
  is_active          INTEGER NOT NULL DEFAULT 1 CHECK (is_active IN (0,1)),
  revision           INTEGER NOT NULL DEFAULT 1,
  created_at         TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at         TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  UNIQUE (tenant_id, id),
  UNIQUE (tenant_id, code),
  FOREIGN KEY (tenant_id, parent_location_id) REFERENCES stock_locations(tenant_id, id),
  FOREIGN KEY (tenant_id, responsible_user_id) REFERENCES users(tenant_id, id)
);
-- Locations are deactivated, never deleted: their history must remain.
CREATE TRIGGER stock_locations_no_delete BEFORE DELETE ON stock_locations
BEGIN SELECT RAISE(ABORT,'stock locations cannot be deleted; deactivate instead'); END;

CREATE TABLE user_location_access (
  tenant_id   TEXT NOT NULL,
  user_id     TEXT NOT NULL,
  location_id TEXT NOT NULL,
  PRIMARY KEY (user_id, location_id),
  FOREIGN KEY (tenant_id, user_id) REFERENCES users(tenant_id, id),
  FOREIGN KEY (tenant_id, location_id) REFERENCES stock_locations(tenant_id, id)
);

CREATE TABLE product_categories (
  id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL REFERENCES tenants(id), name TEXT NOT NULL,
  UNIQUE (tenant_id, id), UNIQUE (tenant_id, name)
);
CREATE TABLE products (
  id          TEXT PRIMARY KEY,
  tenant_id   TEXT NOT NULL REFERENCES tenants(id),
  sku         TEXT NOT NULL,
  name        TEXT NOT NULL,
  barcode     TEXT,
  category_id TEXT,
  unit        TEXT NOT NULL DEFAULT 'pcs',
  is_active   INTEGER NOT NULL DEFAULT 1 CHECK (is_active IN (0,1)),
  revision    INTEGER NOT NULL DEFAULT 1,
  created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  UNIQUE (tenant_id, id),
  UNIQUE (tenant_id, sku),
  FOREIGN KEY (tenant_id, category_id) REFERENCES product_categories(tenant_id, id)
);

-- Maintained balance (derived). The ledger below is the source of truth.
CREATE TABLE inventory_balances (
  tenant_id   TEXT NOT NULL,
  location_id TEXT NOT NULL,
  product_id  TEXT NOT NULL,
  on_hand     INTEGER NOT NULL DEFAULT 0,
  damaged     INTEGER NOT NULL DEFAULT 0,
  in_transit  INTEGER NOT NULL DEFAULT 0,
  reorder_level INTEGER,
  updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  PRIMARY KEY (location_id, product_id),
  FOREIGN KEY (tenant_id, location_id) REFERENCES stock_locations(tenant_id, id),
  FOREIGN KEY (tenant_id, product_id)  REFERENCES products(tenant_id, id)
);

-- Append-only stock ledger.
CREATE TABLE stock_movements (
  id              TEXT PRIMARY KEY,
  tenant_id       TEXT NOT NULL,
  location_id     TEXT NOT NULL,
  product_id      TEXT NOT NULL,
  bucket          TEXT NOT NULL DEFAULT 'on_hand' CHECK (bucket IN ('on_hand','damaged','in_transit')),
  qty_delta       INTEGER NOT NULL CHECK (qty_delta <> 0),
  movement_type   TEXT NOT NULL,   -- purchase, sale, return, adjustment, transfer_out, transfer_in, count_adjustment, ...
  source_type     TEXT NOT NULL,
  source_id       TEXT NOT NULL,
  idempotency_key TEXT NOT NULL,
  allow_negative  INTEGER NOT NULL DEFAULT 0 CHECK (allow_negative IN (0,1)),
  origin_type     TEXT NOT NULL DEFAULT 'server' CHECK (origin_type IN ('server','mobile','cloud')),
  device_id       TEXT,
  actor_id        TEXT,
  occurred_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  UNIQUE (tenant_id, idempotency_key),
  FOREIGN KEY (tenant_id, location_id) REFERENCES stock_locations(tenant_id, id),
  FOREIGN KEY (tenant_id, product_id)  REFERENCES products(tenant_id, id)
);
CREATE INDEX movements_by_product ON stock_movements(tenant_id, product_id, occurred_at);
CREATE INDEX movements_by_location ON stock_movements(tenant_id, location_id, occurred_at);
CREATE INDEX movements_by_source ON stock_movements(tenant_id, source_type, source_id);

CREATE TRIGGER stock_movements_no_update BEFORE UPDATE ON stock_movements
BEGIN SELECT RAISE(ABORT,'stock_movements is append-only'); END;
CREATE TRIGGER stock_movements_no_delete BEFORE DELETE ON stock_movements
BEGIN SELECT RAISE(ABORT,'stock_movements is append-only'); END;

-- Database-level guard: no bucket may go below zero unless the movement explicitly allows it.
CREATE TRIGGER stock_movements_guard BEFORE INSERT ON stock_movements
WHEN NEW.allow_negative = 0 AND
  COALESCE((SELECT CASE NEW.bucket WHEN 'on_hand' THEN on_hand WHEN 'damaged' THEN damaged ELSE in_transit END
            FROM inventory_balances WHERE location_id = NEW.location_id AND product_id = NEW.product_id), 0)
  + NEW.qty_delta < 0
BEGIN SELECT RAISE(ABORT,'insufficient stock'); END;

-- Keep the balance in step with the ledger inside the same transaction.
CREATE TRIGGER stock_movements_apply AFTER INSERT ON stock_movements
BEGIN
  INSERT INTO inventory_balances (tenant_id, location_id, product_id, on_hand, damaged, in_transit)
  VALUES (NEW.tenant_id, NEW.location_id, NEW.product_id,
    CASE NEW.bucket WHEN 'on_hand' THEN NEW.qty_delta ELSE 0 END,
    CASE NEW.bucket WHEN 'damaged' THEN NEW.qty_delta ELSE 0 END,
    CASE NEW.bucket WHEN 'in_transit' THEN NEW.qty_delta ELSE 0 END)
  ON CONFLICT (location_id, product_id) DO UPDATE SET
    on_hand    = on_hand    + CASE NEW.bucket WHEN 'on_hand' THEN NEW.qty_delta ELSE 0 END,
    damaged    = damaged    + CASE NEW.bucket WHEN 'damaged' THEN NEW.qty_delta ELSE 0 END,
    in_transit = in_transit + CASE NEW.bucket WHEN 'in_transit' THEN NEW.qty_delta ELSE 0 END,
    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now');
END;

-- Sync (shop server <-> cloud). Written in the same transaction as the business change.
CREATE TABLE sync_outbox (
  id              TEXT PRIMARY KEY,
  tenant_id       TEXT NOT NULL,
  entity          TEXT NOT NULL,
  entity_id       TEXT NOT NULL,
  op              TEXT NOT NULL CHECK (op IN ('insert','update','archive','post')),
  payload_json    TEXT NOT NULL,
  idempotency_key TEXT NOT NULL,
  base_revision   INTEGER,
  status          TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','in_flight','done','failed','conflict')),
  attempts        INTEGER NOT NULL DEFAULT 0,
  last_error      TEXT,
  next_attempt_at TEXT,
  created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  UNIQUE (tenant_id, idempotency_key)
);
CREATE INDEX outbox_pending ON sync_outbox(status, created_at);
CREATE TABLE sync_inbox (
  id           TEXT PRIMARY KEY,
  tenant_id    TEXT NOT NULL,
  entity       TEXT NOT NULL,
  entity_id    TEXT NOT NULL,
  server_revision INTEGER NOT NULL,
  payload_json TEXT NOT NULL,
  applied_at   TEXT,
  UNIQUE (tenant_id, entity, entity_id, server_revision)
);
CREATE TABLE sync_checkpoints (
  tenant_id TEXT NOT NULL, entity_group TEXT NOT NULL, cursor TEXT NOT NULL,
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  PRIMARY KEY (tenant_id, entity_group)
);
CREATE TABLE sync_conflicts (
  id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, entity TEXT NOT NULL, entity_id TEXT NOT NULL,
  local_json TEXT NOT NULL, remote_json TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','resolved')),
  resolved_by TEXT, resolved_at TEXT,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

INSERT INTO schema_migrations (version, name) VALUES (1, 'foundation');
