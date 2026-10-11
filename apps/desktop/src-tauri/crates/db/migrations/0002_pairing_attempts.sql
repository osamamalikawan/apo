-- Pairing codes are short, so each pairing counts failed attempts and locks after a limit.
ALTER TABLE device_pairings ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0;
CREATE INDEX pairings_open ON device_pairings(tenant_id, used_at, expires_at);
INSERT INTO schema_migrations (version, name) VALUES (2, 'pairing_attempts');
