# STATUS
Honest state of every module. "Verified (owner)" = run on the owner's Windows machine. "Verified (sandbox)" = run in the
build sandbox only. "Verified (apo project)" = run against the live Supabase project `apo`. "Written, not run" = no one has executed it yet. Update this file in every patch.

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
| `crates/db` `roles` (permission registry, role CRUD, role and location assignment, access scope) | Verified (sandbox) | `cargo test -p dsaas-db`: 44 passed (29 + 15 new). Two deliberate mutations (no-escalation rule, last-owner guard) were each caught by their own test. Owner re-run pending |
| `crates/lan-api` shared `authz::check` + role/user admin endpoints | Verified (sandbox) | `cargo test -p dsaas-lan-api`: 52 passed (39 + 4 authz + 9 new). Live HTTPS smoke test: create role, typo rejected, assign, cashier refused, Owner protected, audit trail written. Owner re-run pending |
| `crates/db` master data: `locations`, `catalog` (categories, products), `customers` + SQLite migration 0005 | Verified (sandbox) | `cargo test -p dsaas-db`: 67 passed (44 + 23 new). Mutation checks: disabling the stale-revision check fails 7 tests, disabling the stock guard fails its test. Owner re-run pending |
| `crates/lan-api` master-data endpoints (`master_api`) | Verified (sandbox) | `cargo test -p dsaas-lan-api`: 60 passed (52 + 1 authz + 7 new): plan limits, location scope, revisions, permissions, suspension, outbox. Owner re-run pending |
| Supabase 0005 customers + catalog (`0005_customers_and_catalog.sql`) | Verified (apo project) | Applied by the owner; `customers_test.sql` run by the owner: ALL 17 CHECKS PASSED. Afterwards the table, category `revision` column, barcode index and no-delete function exist on `apo`, RLS is enabled and forced, `anon` cannot read, `authenticated` can read but not insert, and no test rows remain |
| `crates/db` `pin_login_users` + `GET /v1/login-users`, `whoami.tenant_name` | Verified (sandbox) | `cargo test -p dsaas-db`: 68 passed; `cargo test -p dsaas-lan-api`: 61 passed. Owner re-run pending |
| Desktop UI (`apps/desktop`): PIN sign-in, shell with header sidebar toggle, dashboard, customers, inventory (products, categories, locations), settings (account, users, roles), Coming soon pages | Verified (sandbox, by tests only) | `npm run test:ui`: 101 passed (jsdom), typecheck clean, production build 194 kB (59 kB gzip), 4 consecutive clean runs. Dev proxy exercised against the real Shop Server: pair, whoami, login-users, password sign-in, set PIN, PIN sign-in, wrong PIN. **Not seen by a human: nobody has looked at the screens yet.** Owner review pending |
| Dev tooling security | Verified (sandbox) | `npm audit`: 0 vulnerabilities (Vite 8.3.3, Vitest 5.0.5; the older versions had 6, two critical). Dev server sends no permissive CORS header and refuses a spoofed `Host` header |
| Supabase 0001-0003 | Verified (owner) | `rls_test.sql`: 24/24 |
| Supabase 0004 subscriptions + Super Admin functions | Verified (apo project) | `superadmin_test.sql`: ALL 20 CHECKS PASSED, after one live fix (see findings). Objects were already applied; the test left no data behind |
| Dev license tools | Verified (sandbox) | signatures cross-checked by Node crypto; deterministic vectors |
| Cloudflare Worker, Super Admin portal, Desktop UI, mobile | Not started | |

## Pending checks (owner)
Done on the owner's machine (before the roles patch): `cargo test` db (29) and lan-api (39), Python SQLite (23), domain node test (40).
After applying the UI patch, re-run `cargo test -p dsaas-db` (expect 68) and `cargo test -p dsaas-lan-api` (expect 61), then `npm install` and `npm run test:ui` (expect 101) from the repo root (Node 22.12 or newer).
1. Look at the UI: `npm run dev:ui:mock`, open http://localhost:5173, sign in with the PIN pad (Irfan Ahmed 1111). Try collapsing the sidebar with the header button, dark mode, every page, a narrow window. Report anything that looks or feels wrong.
2. Live demo with sign-in: `node tools/issue-entitlement.mjs dev-tenant active 30 3 1 --out ent.json`, then in PowerShell from `src-tauri`:
   `$env:DSAAS_ADMIN_PASSWORD = "a long password"; cargo run -p dsaas-lan-api --bin lan-server -- --apply-entitlement ..\..\..\ent.json`
   (expect `license : full (active)` and `owner login  : admin: password set ...`).

## Not started (see AGENTS.md build order)
Inventory transfers/counts,
sales/payments/accounts, sync engine, Cloudflare Worker, the Tauri shell (the React UI runs in a browser for now), the UI pages marked "Coming soon", mDNS discovery, client certificate pinning,
SQLCipher + key in OS credential store, Super Admin portal, routes/planner/targets/reports, mobile app, installer + signed updater.

## Known limitations
- Postgres `post_stock_movement` handles single movements only (no transfer/count variants yet).
- The permission registry (`dsaas_db::roles::PERMISSIONS`) must be extended whenever a module adds a permission. It is not mirrored in Postgres yet.
- New users start with no roles; assign them through `PUT /v1/users/:id/roles`. Role changes apply on the next request; sessions are not cut (deactivating the user does end them).
- No per-IP rate limiting on login; only the per-account lockout (ADR 0007). A 4-digit PIN is guessable in principle.
- Route scope has no tables yet (build step 10): only `routes.all` or `*` passes a route check, everything else is denied. Customers are therefore not route-scoped: anyone with `customers.view` sees all of them.
- Location scope is enforced by the location endpoints; the inventory and sales endpoints must pass a location scope to `authz::check` too.
- No endpoint to reset another user's password; recovery is console-only (`DSAAS_ADMIN_PASSWORD`).
- Subscription enforcement covers pairing, the device limit and every endpoint that calls `authz::check` with a write requirement (the user, role and master-data endpoints so far). Each new write endpoint must do the same.
- Products carry only SKU, name, barcode, category and unit. There are no prices, brand, local-language names, default location or vehicle fitment yet; prices belong to the sales module's price lists. Customers have no credit limit, balance or route yet.
- Master-data writes enqueue `sync_outbox` rows (a snapshot at the new revision); users, roles and role assignments do not enqueue yet, and nothing drains the outbox until the sync engine exists.
- Master-data validation (SKU, code, phone, barcode rules) lives in Rust only; there are no shared TypeScript vectors until a TS client needs them.
- The prototype's data model is richer than the server's so far. Not yet on the server: Urdu product names, brands and companies (with discount percentages and per-customer discounts), buy-N-get-1 packages, default location per product, prices and costs, customer owner/credit limit/area/route/balance/remarks, racks, salesmen. The UI pages for those arrive with the server parts (build steps 4, 5, 10); the product and customer forms show only what the server stores.
- The UI has no idle auto-lock yet (the server ends sessions after 2 hours idle). Sessions live in `sessionStorage`.
- `GET /v1/login-users` lists names of PIN users to any paired device (ADR 0010); an attacker at a paired PC can see who works in the shop and can lock an account by guessing (5 tries, 15 minutes).
- The UI runs in a browser in development; certificate pinning and device credentials move to the Tauri/Rust side with the shell.
- A fully offline shop that restores an old copy of its database file is bounded only by the offline window (ADR 0006).
- The server private key is a plain file for now.

## Findings log
- A race found while stabilising the UI tests: the search box's typing pause also fired on first load and reset paging to page 1 if someone clicked Next within 250 ms. Fixed (it now does nothing when the search text is unchanged) with a regression test that fails without the fix.
- The upgrade to Vite 8 and Vitest 5 needed vitest 5.0.5 exactly: 5.0.0 to 5.0.4 hit an npm 10 resolver bug (`Cannot read properties of null (reading 'edgesOut')`). 5.0.5 is about a week old; it is a test runner and is never installed on shop PCs.
- A wrong assumption in a new test (plan allows 5 locations, not 4) exposed a real loophole: reactivating a location skipped the `max_locations` check. Reactivation is now checked like creation, and a no-op reactivation at the limit is still allowed.
- The registry guard test first failed on `sales.createx` in the shared vectors: that is a deliberate look-alike negative case, so the guard now checks only granted and expected-allowed permissions.
- Live `apo` database: `tenant_entitlement_payload(uuid)` was executable by `authenticated`, so any signed-in user could build entitlement payloads. Migration 0004 revokes that, so the database had drifted from the file (cause unknown; likely an earlier partial or older run). `superadmin_test.sql` check #12 caught it. Fixed on 2026-10-11 by re-applying the revoke/grant lines from 0004; the test then passed 20/20. The other three 0004 functions already matched the file.
- `apo` has no rows in any table and an empty migration history (everything was applied through the SQL Editor).
- Two migration folders exist on purpose: `supabase/migrations` (Postgres, you paste them) and `apps/desktop/src-tauri/crates/db/migrations` (SQLite, the server applies them). The numbers are unrelated; pasting a SQLite file into Supabase fails with `relation "users" does not exist`.
- Postgres migrations are not re-runnable: running 0004 twice fails with `relation "subscription_plans" already exists`. Check what exists (`to_regclass`, `to_regprocedure`) before re-running one.
- Migration 0004 first omitted its `schema_migrations` row (the db test caught it): fixed.
- Housekeeping: removed the stale `supabase/migrations/rls_test.sql` (older 23-check copy of `supabase/tests/rls_test.sql`); `ent.json` is now git-ignored (generated, UTF-16 when written by PowerShell `>`).
- Pairing counter kept counting on locked pairings (found by Rust test): fixed.
- `can_access_location` did not verify location ownership for `locations.all` users (found by `rls_test` #19): fixed in Supabase 0003.
- PowerShell `>` wrote the entitlement file as UTF-16, which the server rejected: issuer tool now has `--out`; server accepts BOM/UTF-16.
