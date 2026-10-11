# STATUS
Honest state of every module. "Verified (owner)" = run on the owner's Windows machine. "Verified (sandbox)" = run in the
build sandbox only. "Written, not run" = no one has executed it yet. Update this file in every patch.

## Modules
| Module | State | Evidence |
|---|---|---|
| `packages/domain` money, permissions, ledger | Verified (owner) | 24 tests + clean typecheck |
| `packages/domain` entitlement rules | Verified (sandbox) | 40 tests total, clean typecheck; owner re-run pending |
| SQLite migrations 0001-0002 | Verified (owner) | 19 Python tests |
| SQLite migration 0003 (entitlements, clock guard) | Verified (sandbox) | 23 Python tests; owner re-run pending |
| `crates/db` (migrations runner, writer, pairing) | Verified (owner) | `cargo test -p dsaas-db`: 9 passed (before 0003 registration; expect 9) |
| `crates/lan-api` HTTP API + HTTPS | Verified (owner) | 12 tests passed |
| `crates/lan-api` subscription enforcement | Compiles on owner's machine (`cargo run` built it); tests not yet reported | expected 25 tests |
| Supabase 0001-0003 | Verified (owner) | `rls_test.sql`: 24/24 |
| Supabase 0004 subscriptions + Super Admin functions | Written, not run | `superadmin_test.sql` (20 checks) |
| Dev license tools | Verified (sandbox) | signatures cross-checked by Node crypto; deterministic vectors |
| Cloudflare Worker, Super Admin portal, Desktop UI, mobile | Not started | |

## Pending checks (owner)
1. `cargo test -p dsaas-db` (expect 9) and `cargo test -p dsaas-lan-api` (expect 25) from `apps\desktop\src-tauri`.
2. `python tools/test_sqlite_ledger.py` (expect 23) and the node test (expect 40) from the root.
3. Supabase: run `0004_subscriptions.sql`, then `supabase/tests/superadmin_test.sql` (expect "ALL 20 CHECKS PASSED").
4. Live demo: `node tools/issue-entitlement.mjs dev-tenant active 30 3 1 --out ent.json`, then from `src-tauri`:
   `cargo run -p dsaas-lan-api --bin lan-server -- --apply-entitlement ..\..\..\ent.json` (expect `license : full (active)`).

## Not started (see AGENTS.md build order)
Users/login (Argon2id), roles + shared authorization middleware, master-data APIs, inventory transfers/counts,
sales/payments/accounts, sync engine, Cloudflare Worker, Tauri shell + React UI, mDNS discovery, client certificate pinning,
SQLCipher + key in OS credential store, Super Admin portal, routes/planner/targets/reports, mobile app, installer + signed updater.

## Known limitations
- Postgres `post_stock_movement` handles single movements only (no transfer/count variants yet).
- Permission strings are free-form until the roles module adds a registry.
- Subscription enforcement currently covers pairing and the device limit; each new write endpoint must add the check.
- A fully offline shop that restores an old copy of its database file is bounded only by the offline window (ADR 0006).
- The server private key is a plain file for now.

## Findings log
- Pairing counter kept counting on locked pairings (found by Rust test): fixed.
- `can_access_location` did not verify location ownership for `locations.all` users (found by `rls_test` #19): fixed in Supabase 0003.
- PowerShell `>` wrote the entitlement file as UTF-16, which the server rejected: issuer tool now has `--out`; server accepts BOM/UTF-16.
