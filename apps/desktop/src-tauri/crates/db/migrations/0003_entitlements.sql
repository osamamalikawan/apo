-- Subscription entitlement, as last received from the cloud (cloud-authoritative, cached locally).
-- The signed payload is stored verbatim and RE-VERIFIED on every use, so editing the `status`
-- column by hand changes nothing: the server trusts only the signature-checked payload.
CREATE TABLE entitlement_current (
  tenant_id     TEXT PRIMARY KEY REFERENCES tenants(id),
  revision      INTEGER NOT NULL CHECK (revision >= 1),
  payload_b64   TEXT NOT NULL,
  signature_b64 TEXT NOT NULL,
  status        TEXT NOT NULL,                 -- copy for reporting only; never used for decisions
  applied_at    TEXT NOT NULL
);
CREATE TRIGGER entitlement_no_rollback BEFORE UPDATE ON entitlement_current
WHEN NEW.revision < OLD.revision
BEGIN SELECT RAISE(ABORT,'entitlement revision cannot decrease'); END;
CREATE TRIGGER entitlement_no_delete BEFORE DELETE ON entitlement_current
BEGIN SELECT RAISE(ABORT,'entitlement cannot be deleted'); END;

-- Latest time this database has ever observed; setting the PC clock back cannot extend a subscription.
CREATE TABLE clock_guard (
  id         INTEGER PRIMARY KEY CHECK (id = 1),
  high_water TEXT NOT NULL
);
INSERT INTO clock_guard (id, high_water) VALUES (1, '1970-01-01T00:00:00Z');
CREATE TRIGGER clock_guard_monotonic BEFORE UPDATE ON clock_guard
WHEN NEW.high_water < OLD.high_water
BEGIN SELECT RAISE(ABORT,'clock high-water mark cannot decrease'); END;

INSERT INTO schema_migrations (version, name) VALUES (3, 'entitlements');
