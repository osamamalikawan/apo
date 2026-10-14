# ADR 0009: Master-data conventions (locations, categories, products, customers)

Status: accepted. These rules apply to every master-data record added later (suppliers, routes, price lists...).

## Decision
- **Revisions and optimistic concurrency.** Every record has a `revision`, starting at 1 and rising by one on each change. Updates and
  (de)activations must quote the `expected_revision` the caller last read. A mismatch is `409 stale_revision` and changes nothing, so
  two PCs editing the same record can never silently overwrite each other. A missing revision counts as stale.
- **`PUT` replaces the whole record.** Send every field; an optional field left out (barcode, category, phone...) is cleared, not kept.
  This keeps the rules simple (one validated input, one snapshot) and makes a stale client's mistake visible. Clients should edit a copy
  of the record they read and send it back with its `revision`.
- **Deactivate, never delete.** Records are switched off (`is_active`), never removed; deleting is blocked by a trigger where history
  depends on the record (locations, customers). Locations additionally cannot be deactivated while they hold stock or have active
  sub-locations; bringing a location back needs an active parent and counts against the plan's `max_locations` like creating one.
- **Normalised, case-insensitive identifiers.** Location codes, SKUs and customer codes are stored upper-case, so the ordinary
  UNIQUE constraints behave case-insensitively. A barcode identifies exactly one product per shop (partial unique index).
- **Every change is audited and queued for sync in the same transaction.** Each write adds an `audit_logs` row (before/after) and a
  `sync_outbox` row whose payload is a snapshot of the record at its new revision, with `base_revision` set on updates and the
  idempotency key `entity:id:rN`. Deactivation is queued as `archive`. Nothing drains the outbox yet (build step 6); the snapshot
  format may be refined there, which is why the outbox rows are written now rather than back-filled later.
- **Who may do what.** Reading needs `products.view` / `customers.view` (any signed-in user may list the locations they can use).
  Writing needs `products.manage` / `customers.manage` / `locations.manage` and a subscription that allows writes. Master-data edits do
  not need a password sign-in; a PIN session at the counter may do them. Location endpoints also apply location scope (an
  inaccessible location and a missing one both answer 403, so ids cannot be probed). Creating a location needs `locations.all`.
- **Customers are new on both sides:** Postgres gets a read-only (RLS) `customers` table; writes arrive from the sync Worker.

## Consequences
- Clients must carry `revision` from every read into the next write and handle 409 by reloading.
- Product prices, brand, local-language names, default location and fitment data are not part of this step (see STATUS.md).
- Customers are not route-scoped yet; anyone with `customers.view` sees all of them until routes exist (build step 10).
