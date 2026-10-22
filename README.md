# Distribution SaaS

**Contributors and AI agents: read [`AGENTS.md`](AGENTS.md) first (module map, rules, build order) and [`STATUS.md`](STATUS.md).**

Local-first, multi-tenant desktop SaaS for distributors. Architecture: see the Phase 1 doc and `docs/adr`.

## Layout
- `packages/domain` shared TypeScript rules (money, permissions, reference stock ledger)
- `packages/test-vectors` JSON cases shared by TypeScript, SQLite and (later) Postgres tests
- `apps/desktop/src-tauri/crates/db` shop-server SQLite migrations and Rust access layer
- `apps/desktop` the React UI (try it with `npm run dev:ui:mock`; see its README)
- `supabase/migrations` cloud schema, RLS, RPC
- `tools/` cross-language test runners

## Applying a patch
```
git apply --check some.patch     # dry run, changes nothing
git apply some.patch
```
(no Git? `winget install Git.Git`). If line endings cause "patch does not apply", add `--ignore-whitespace`.

## Run the tests that work today
```
tsc -p packages/domain/tsconfig.json                  # typecheck
node --experimental-strip-types --test packages/domain/test/domain.test.ts   # domain tests (Node 22.14; plain `node --test` works on 22.18+)
python3 tools/test_sqlite_ledger.py                   # real SQLite schema tests
npm install && npm run test:ui                        # desktop UI tests (needs Node 22.12 or newer)
node tools/gen-entitlement-vectors.mjs                # regenerate signed test vectors (deterministic)
```
Rust (needs a toolchain): `cd apps/desktop/src-tauri && cargo test -p dsaas-db`
