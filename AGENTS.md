# AGENTS.md: start here

Local-first, multi-tenant SaaS for distributors/wholesalers. One **Shop Server** (Tauri/Rust + SQLite) per shop,
thin **Client PCs** on the LAN, **Supabase** (Postgres) as the cloud hub, a **Cloudflare Worker** as the
desktop-to-cloud gateway, a future **mobile app** talking to Supabase directly, and a **Super Admin** control
panel that can revoke subscriptions remotely. Read `docs/adr/` for the why; `STATUS.md` for what is real today.

## Module map (work one module at a time)
| Module | Path | Owns | Read first |
|---|---|---|---|
| Domain rules (TS) | `packages/domain` | money, permissions, reference stock ledger, entitlement rules | its README |
| Shared test vectors | `packages/test-vectors` | JSON cases every language must pass | its README |
| Shop DB (Rust + SQLite) | `apps/desktop/src-tauri/crates/db` | migrations, single writer, device pairing, users/login/sessions, roles/permissions/scope | its README |
| Shop API (Rust) | `apps/desktop/src-tauri/crates/lan-api` | HTTPS API, device auth, user sessions, shared authorization (`authz`), user/role admin, TLS, subscription enforcement | its README |
| Cloud (Postgres) | `supabase/` | schema, RLS, RPCs, Super Admin functions, SQL tests | its README |
| Dev tools | `tools/` | SQLite test runner, dev license signer, vector generator | its README |
| Cloudflare Worker | `services/worker` | **not started** (signs entitlements, sync gateway) | ADR 0003, 0006 |
| Desktop UI | `apps/desktop` (React in `src/`) | **not started** | |
| Super Admin portal | `apps/superadmin` | **not started** | ADR 0006 |
| Mobile app | `apps/mobile` | **not started** (later) | ADR 0003 |

## Non-negotiable rules
1. **Never claim something works unless it was run.** Mark code "written, not run" in `STATUS.md` until tests pass.
   The build sandbox used so far has no network/Rust toolchain; the owner (Windows) runs `cargo` and Supabase SQL.
2. **Tenancy:** every tenant-owned row has `tenant_id`; related tables use composite FKs `(tenant_id, id)`;
   never trust a client-supplied tenant id.
3. **Authorization at every boundary** (LAN API, Worker, Postgres RLS/RPC). Hiding a button is not security.
4. **Stock = append-only ledger** (`stock_movements`) with idempotency keys; balances are derived. Never overwrite a quantity.
5. **Money = integer minor units** (bigint / i64) + currency. No floats. Quantities are integers.
6. **Subscription and permissions:** every endpoint that acts as a user goes through `authz::check` (lan-api) with a `Requirement`
   (`read`/`write`, `.sensitive()` for password-only actions), which checks permission, location/route scope and, for writes,
   `entitlement::decide` in one place. Do not call `Principal::can` ad hoc. New permission strings go in `dsaas_db::roles::PERMISSIONS`.
   Reads stay available when suspended. Nothing is deleted on lapse (ADR 0006).
7. **Two separate migration folders, same numbering style.** `supabase/migrations` is Postgres (pasted into Supabase); `apps/desktop/src-tauri/crates/db/migrations` is SQLite (applied by the server). Never mix them.
   **Migrations are append-only.** Never edit an applied migration; add the next number. SQLite and Postgres are
   separate files kept in logical parity.
8. **Shared vectors:** if you change a rule that has vectors (`packages/test-vectors`), update every consumer
   (TS tests, `tools/test_sqlite_ledger.py`, Rust tests, SQL tests) in the same patch.
9. **Secrets:** never commit real secrets. The key in `tools/dev-license-key.mjs` is DEV ONLY; production signing
   key lives only in Cloudflare; release builds must set `DSAAS_LICENSE_PUBKEY`.
10. **Times:** ISO-8601 UTC text. Entitlements use seconds precision (`YYYY-MM-DDTHH:MM:SSZ`); most DB timestamps use
    milliseconds (`...SS.sssZ`). Do not compare the two formats as strings.

## How to deliver work
- Produce a **unified-diff patch** against the current repo (`git diff --cached --binary`), verify it with
  `git apply --check` on a clean copy, run what can be run, and say plainly what was not run.
- Update `STATUS.md` (module table + pending checks) and the module README in the same patch.
- Add or update an ADR in `docs/adr/` for any decision that is hard to reverse.

## Adding a business module (checklist)
1. ADR if it changes architecture. 2. SQLite migration + Postgres migration (RLS, RPC-only writes where mobile can
reach it). 3. Rules in `packages/domain` + vectors. 4. Service functions in `crates/db` with tests.
5. Endpoints in `crates/lan-api`: device auth, user permission/scope check, entitlement check, idempotency key.
6. Sync: write the outbox row in the same transaction as the change. 7. Tests at every layer; update `STATUS.md`.

## Suggested build order (dependencies first)
1. ~~Users and login on the LAN server (Argon2id, sessions, PIN)~~ **done, see ADR 0007** 2. ~~Roles/permissions tables + a shared authorization
middleware (permission + location/route scope + entitlement)~~ **done, see ADR 0008** 3. Master data: locations, products, customers
4. Inventory service: transfers, counts, reservations (spec in the original multi-location brief)
5. Sales, payments, accounts ledger 6. Sync engine (outbox/inbox, hourly + manual, conflicts)
7. Cloudflare Worker (auth, sync, entitlement signing) 8. Tauri shell + React UI (header: version, sync status)
9. Super Admin portal 10. Routes, weekly planner, targets, reports 11. Mobile app 12. Installer + signed updater.

## Command cheat sheet (Windows PowerShell)
| From | Command |
|---|---|
| project root | `python tools/test_sqlite_ledger.py` |
| project root | `tsc -p packages/domain/tsconfig.json` (or `npx -p typescript tsc -p packages/domain/tsconfig.json`) |
| project root | `node --experimental-strip-types --test packages/domain/test/domain.test.ts` (Node 22.14; plain `node --test ...` on 22.18+) |
| project root | `git apply --check x.patch` then `git apply x.patch` |
| `apps\desktop\src-tauri` | `cargo test -p dsaas-db` and `cargo test -p dsaas-lan-api` |
| `apps\desktop\src-tauri` | `cargo run -p dsaas-lan-api --bin lan-server -- [--lan] [--apply-entitlement file]` |
| Supabase SQL Editor | paste `supabase/migrations/000N_*.sql`, then the matching file in `supabase/tests/` |
Rust must be 1.78+ (Cargo.lock is version 4). The first `cargo test` is slow while crates compile.
Windows notes: use `curl.exe` (not `curl`); do not use `>` to write JSON (UTF-16): use the tool's `--out` flag.
