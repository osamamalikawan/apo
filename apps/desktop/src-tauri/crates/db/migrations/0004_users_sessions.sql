-- Migration 0004: login lockout counters on users, and server-side sessions.
-- Sessions are opaque random tokens; only their SHA-256 hash is stored. A session belongs to
-- the paired device that created it and is useless from any other device.
ALTER TABLE users ADD COLUMN failed_logins INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN locked_until  TEXT;

CREATE TABLE user_sessions (
  id           TEXT PRIMARY KEY,
  tenant_id    TEXT NOT NULL REFERENCES tenants(id),
  user_id      TEXT NOT NULL,
  device_id    TEXT NOT NULL,
  token_hash   TEXT NOT NULL UNIQUE,
  method       TEXT NOT NULL CHECK (method IN ('password','pin')),
  created_at   TEXT NOT NULL,
  last_used_at TEXT NOT NULL,
  expires_at   TEXT NOT NULL,
  revoked_at   TEXT,
  FOREIGN KEY (tenant_id, user_id)   REFERENCES users(tenant_id, id),
  FOREIGN KEY (tenant_id, device_id) REFERENCES devices(tenant_id, id)
);
CREATE INDEX sessions_by_user ON user_sessions(tenant_id, user_id);

INSERT INTO schema_migrations (version, name) VALUES (4, 'users_sessions');
