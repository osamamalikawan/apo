# STATUS
Honest state of every module. "Verified (owner)" = run on the owner's Windows machine. "Verified (sandbox)" = run in the
build sandbox only. "Written, not run" = no one has executed it yet. Update this file in every patch.

## Modules
| Module | State | Evidence |
|---|---|---|
| `packages/domain` money, permissions, ledger | Verified (owner) | 24 tests + clean typecheck |
| `packages/domain` entitlement rules | Verified (owner) | 40 tests total passed on the owner's machine (typecheck clean in sandbox; owner typecheck not reported) |
| SQLite migrations 0001-0002 | Verified (owner) | 19 Python tests |
| SQLite migration 0003 (entitlements, clock guard) | Verified (owner) | 23 Python tests |
| `crates/db` migrations runner, writer, pairing | Verified (owner) | included in the 29 db tests below |
| SQLite migration 0004 (login lockout, sessions) | Verified (owner) | 23 Python tests (they apply every migration) + the db tests below |
| `crates/db` `users` (Argon2id, login, lockout, sessions, user admin) | Verified (owner) | `cargo test -p dsaas-db`: 29 passed (9 older + 20 new) |
| `crates/lan-api` HTTP API + HTTPS | Verified (owner) | 12 tests passed |
| `crates/lan-api` subscription enforcement | Verified (owner) | included in the 39 lan-api tests below (25 before the auth work) |
| `crates/lan-api` login, sessions, user admin endpoints | Verified (owner) | `cargo test -p dsaas-lan-api`: 39 passed (25 + 14 new). Live HTTPS smoke test (pair, bad login, login, `/v1/me`, create user) done in the sandbox only |
| Supabase 0001-0003 | Verified (owner) | `rls_test.sql`: 24/24 |
| Supabase 0004 subscriptions + Super Admin functions | Written, not run | `superadmin_test.sql` (20 checks) |
| Dev license tools | Verified (sandbox) | signatures cross-checked by Node crypto; deterministic vectors |
| Cloudflare Worker, Super Admin portal, Desktop UI, mobile | Not started | |

## Pending checks (owner)
Done on the owner's machine: `cargo test` db (29) and lan-api (39), Python SQLite (23), domain node test (40).
1. Supabase: run `0004_subscriptions.sql`, then `supabase/tests/superadmin_test.sql` (expect "ALL 20 CHECKS PASSED").
2. Live demo with sign-in: `node tools/issue-entitlement.mjs dev-tenant active 30 3 1 --out ent.json`, then in PowerShell from `src-tauri`:
   `$env:DSAAS_ADMIN_PASSWORD = "a long password"; cargo run -p dsaas-lan-api --bin lan-server -- --apply-entitlement ..\..\..\ent.json`
   (expect `license : full (active)` and `owner login  : admin: password set ...`).

## Not started (see AGENTS.md build order)
Roles tables/management endpoints + shared authorization middleware (scope checks; today only `users.manage` is checked), master-data APIs, inventory transfers/counts,
sales/payments/accounts, sync engine, Cloudflare Worker, Tauri shell + React UI, mDNS discovery, client certificate pinning,
SQLCipher + key in OS credential store, Super Admin portal, routes/planner/targets/reports, mobile app, installer + signed updater.

## Known limitations
- Postgres `post_stock_movement` handles single movements only (no transfer/count variants yet).
- Permission strings are free-form until the roles module adds a registry. Users created through the API have no roles, hence no permissions, until that module exists (the dev admin is made Owner at startup).
- No per-IP rate limiting on login; only the per-account lockout (ADR 0007). A 4-digit PIN is guessable in principle.
- Location and route scope is not loaded into the session yet (`Principal` carries module permissions only).
- No endpoint to list users or reset another user's password; recovery is console-only (`DSAAS_ADMIN_PASSWORD`).
- Subscription enforcement currently covers pairing and the device limit; each new write endpoint must add the check.
- A fully offline shop that restores an old copy of its database file is bounded only by the offline window (ADR 0006).
- The server private key is a plain file for now.

## Findings log
- Migration 0004 first omitted its `schema_migrations` row (the db test caught it): fixed.
- Housekeeping: removed the stale `supabase/migrations/rls_test.sql` (older 23-check copy of `supabase/tests/rls_test.sql`); `ent.json` is now git-ignored (generated, UTF-16 when written by PowerShell `>`).
- Pairing counter kept counting on locked pairings (found by Rust test): fixed.
- `can_access_location` did not verify location ownership for `locations.all` users (found by `rls_test` #19): fixed in Supabase 0003.
- PowerShell `>` wrote the entitlement file as UTF-16, which the server rejected: issuer tool now has `--out`; server accepts BOM/UTF-16.
