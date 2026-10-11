# ADR 0002: Append-only stock ledger, derived balances

**Status:** accepted and implemented in SQLite (migration 0001); Postgres version written, not yet run.

- `stock_movements` is append-only (update/delete triggers abort). Each row has a tenant-scoped
  `idempotency_key`; a retry hits the unique constraint and is reported as `duplicate`.
- `inventory_balances` is maintained by a trigger in the same transaction and can always be
  rebuilt from the ledger. Tests assert balance == SUM(ledger).
- Three buckets: `on_hand`, `damaged`, `in_transit`. In-transit is its own bucket so transferred
  stock is never counted twice.
- A bucket cannot go below zero unless the movement sets `allow_negative`, which requires an
  explicit permission. The guard exists in the database, not only in app code.
- Tenant ownership is enforced with composite foreign keys `(tenant_id, id)`.
- Locations are never deleted (trigger); they are deactivated, keeping stock and history.
- Quantities are integers. The unit scale (for example thousandths for weighed goods) is an
  application decision made when fractional units are introduced.
- Shared vectors in `packages/test-vectors` run against the TypeScript reference ledger and
  the real SQLite schema; the Postgres RPC must pass the same vectors.
