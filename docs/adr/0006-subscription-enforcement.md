# ADR 0006: Remote subscription control and offline enforcement

**Status:** accepted. Implemented and tested: TypeScript rules, SQLite storage. Written, awaiting the owner's
run: Rust enforcement (`entitlement.rs`, API wiring), Postgres migration 0004 and its test.

## Decision
The cloud is the only authority. A Super Admin action (suspend, cancel, reinstate, change period) calls
`superadmin_set_subscription`, which bumps a revision, flips `tenants.status` (so cloud RPCs refuse writes at
once) and writes an immutable platform audit row. Shop servers learn of it at their next sync: the Worker
builds the payload with `tenant_entitlement_payload` (service_role only), signs it with an Ed25519 key that
exists only in the cloud, and returns `{payload_b64, signature_b64}`. The shop server holds only the public key.

## Shop server behaviour (modes)
| Mode | When | Allowed |
|---|---|---|
| full | active and now <= `full_until` | everything |
| grace | active and now <= `grace_until` | everything, with a warning banner |
| read_only | suspended, cancelled, or past `grace_until` | view and export; no new business writes, no new devices |
| unlicensed | no valid entitlement, wrong tenant, bad status | same as read_only; activation required |
Nothing is ever deleted when a subscription lapses.

`full_until = min(paid-through date, issue time + offline_check_days)`; `grace_until = full_until + grace_days`.
So an offline shop that missed a revocation keeps full access at most `offline_check_days + grace_days`
(defaults 7 + 7). Suspension takes effect on the next successful sync. These are product settings per plan.

## Protections
- Signed payload stored verbatim and re-verified on every decision; editing the local `status` column does nothing.
- Anti-replay: an older issuance (lower revision, or same revision with earlier `issued_at`) is refused,
  so an old "active" statement cannot overwrite a newer "suspended" one. A SQL trigger also blocks revision decreases.
- Clock guard: a persistent high-water mark of observed time; setting the PC clock back cannot extend a subscription.
- Plan limits (`max_devices` now; users and locations when those modules exist) travel in the entitlement.
- Release builds require the public key via `DSAAS_LICENSE_PUBKEY`; only debug builds fall back to the dev key.

## Known limits (honest list)
- Someone with full control of the shop PC and its database file can still restore an old copy of the whole file.
  Encrypting the database (SQLCipher, planned) and server-side device checks at sync time reduce this; they cannot
  eliminate it for a fully offline machine. The offline window above is the accepted exposure.
- Enforcement today covers pairing and device limits. Each business endpoint added later must call the same
  `decide` check (a shared middleware is the plan).
- The mobile app is protected by `tenants.status` in Postgres, which the Super Admin function updates immediately.
