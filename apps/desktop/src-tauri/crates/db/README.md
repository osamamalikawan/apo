# crates/db (Rust): Shop Server database
Owns the SQLite file. Only the server process may open it (never over a network share).

- `migrations/0001_foundation.sql`: tenancy, users/roles, devices, pairing, audit, locations, products, stock ledger + balances (triggers), sync tables.
- `0002_pairing_attempts.sql`: failed-attempt counter for pairing codes. `0003_entitlements.sql`: signed entitlement cache, rollback guard, clock high-water mark.
- `src/lib.rs`: `open` (WAL, FKs on), `migrate` (transaction per migration), `backup_to` (VACUUM INTO; call before migrating a file DB), `Writer` (single writer thread), re-exports `rusqlite`.
- `src/pairing.rs`: one-time 8-digit codes (10 min, single use, 5 attempts), device credentials (hash-only storage), authenticate/revoke/record_seen.

**Invariants:** ledger and audit tables are append-only (triggers); balances are maintained by trigger and must equal SUM(ledger);
locations are never deleted; composite FKs enforce tenant ownership; new migrations get the next number and are registered in `MIGRATIONS`.
**Tests:** `cargo test -p dsaas-db` (9, verified on the owner's machine) + `python tools/test_sqlite_ledger.py` (runs the same SQL).
**Next here:** users/sessions (Argon2id), roles/permission tables, stock transfer/count tables, outbox writers, encrypted DB (SQLCipher), key in OS credential store.
