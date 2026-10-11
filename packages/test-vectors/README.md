# packages/test-vectors
JSON cases consumed by several languages so they cannot drift apart.

| File | Consumers |
|---|---|
| `ledger-sequences.json` | TS reference ledger, `tools/test_sqlite_ledger.py` (real SQLite schema). Postgres RPC should match (`supabase/tests/rls_test.sql` covers the same rules). |
| `permissions.json` | TS `authorize` tests. Rust/SQL enforcement to be added with the roles module. |
| `entitlements.json` | TS rules + signature checks (Node crypto), Rust `entitlement.rs` tests. **Generated:** `node tools/gen-entitlement-vectors.mjs` (deterministic; do not hand-edit). |

Rules: change semantics -> update vectors and every consumer in one patch. The signing key behind `entitlements.json`
is the DEV key and must never be used in production.
