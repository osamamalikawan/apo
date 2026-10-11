# packages/domain (TypeScript)
Pure, dependency-free business rules shared by UI, Worker and mobile. No I/O, no clock reads (time is a parameter).

- `money.ts`: bigint minor units, `fromDecimal/toDecimal`, `applyBps` (round half away from zero), `allocate` (largest remainder).
- `permissions.ts`: `module.action` strings, `*` and `module.*` wildcards, scopes (locations, routes) via `authorize`.
- `ledger.ts`: reference in-memory stock ledger (3 buckets, idempotency, overdraw rule) + `transferMovements`.
- `entitlement.ts`: `evaluate()` -> mode (full/grace/read_only/unlicensed) + reason, `canWrite`, `isIsoSeconds`.

**Contract:** behaviour is pinned by `packages/test-vectors`; Rust (`lan-api`), SQLite and Postgres must agree with it.
**Run:** from root: `tsc -p packages/domain/tsconfig.json` and the node test command in AGENTS.md. Imports use `.ts` extensions.
**Status:** verified (24 tests on the owner's machine; 40 with entitlements in the sandbox, owner re-run pending).
**Extend:** add a rule, add vectors first, then make all consumers pass. Keep it free of Node-only APIs (it must run in browsers/Workers).
