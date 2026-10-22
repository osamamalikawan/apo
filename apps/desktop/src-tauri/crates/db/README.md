# crates/db (Rust): Shop Server database
Owns the SQLite file. Only the server process may open it (never over a network share).

- `migrations/0001_foundation.sql`: tenancy, users/roles, devices, pairing, audit, locations, products, stock ledger + balances (triggers), sync tables.
- `0002_pairing_attempts.sql`: failed-attempt counter for pairing codes. `0003_entitlements.sql`: signed entitlement cache, rollback guard, clock high-water mark.
- `src/lib.rs`: `open` (WAL, FKs on), `migrate` (transaction per migration), `backup_to` (VACUUM INTO; call before migrating a file DB), `Writer` (single writer thread), re-exports `rusqlite`.
- `0004_users_sessions.sql`: login lockout columns on `users`, `user_sessions` (token hash, device-bound, `password`/`pin` method).
- `src/users.rs`: Argon2id hashing, `login` (lockout, generic errors), sessions (`authenticate_session`, `logout`), `create_user`, `change_password`, `set_pin`, `deactivate_user` (last-owner guard), `set_password_local` (console only), `pin_login_users` (names for the PIN sign-in tiles), `permissions_for_user` + `has_permission` (checked against `packages/test-vectors/permissions.json`). See ADR 0007.
- `src/roles.rs`: permission registry (`PERMISSIONS`), role create/update/delete, `set_user_roles`, `set_user_locations`, `list_roles`, `list_users`. You can only grant what you hold; the Owner role is protected; at least one active `*` holder always remains. See ADR 0008.
- `src/locations.rs`, `src/catalog.rs`, `src/customers.rs`, `src/master.rs`: master data (ADR 0009). Normalised codes/SKUs, `expected_revision` updates, deactivate not delete, audit + `sync_outbox` rows in the same transaction. `0005_customers_and_catalog.sql` adds customers, category revisions and the unique barcode index.
- `src/pairing.rs`: one-time 8-digit codes (10 min, single use, 5 attempts), device credentials (hash-only storage), authenticate/revoke/record_seen.

**Invariants:** ledger and audit tables are append-only (triggers); balances are maintained by trigger and must equal SUM(ledger);
locations are never deleted; composite FKs enforce tenant ownership; new migrations get the next number and are registered in `MIGRATIONS`.
**Tests:** `cargo test -p dsaas-db` (68; the first 29 were verified on the owner's machine, the rest in the build sandbox) + `python tools/test_sqlite_ledger.py` (runs the same SQL).
**Next here:** route access tables (with the routes module), stock transfer/count tables, outbox writers for users/roles, encrypted DB (SQLCipher), key in OS credential store.
