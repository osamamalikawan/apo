# tools/
- `test_sqlite_ledger.py`: applies every SQLite migration to a real in-memory DB and checks ledger/tenancy/pairing/entitlement rules; runs the shared ledger vectors.
- `dev-license-key.mjs`: DEV-ONLY Ed25519 key helpers (sign/verify). Production signing happens in the Cloudflare Worker.
- `gen-entitlement-vectors.mjs`: regenerates `packages/test-vectors/entitlements.json` (deterministic).
- `issue-entitlement.mjs`: dev stand-in for the cloud issuer. `node tools/issue-entitlement.mjs dev-tenant active 30 3 1 --out ent.json`
  (use `--out`; PowerShell's `>` writes UTF-16). Apply with `lan-server ... --apply-entitlement ent.json`.
