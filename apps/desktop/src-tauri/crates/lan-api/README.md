# crates/lan-api (Rust): Shop Server HTTPS API
- `src/lib.rs`: axum router. `/v1/health` (open), then for `/v1/*`: version handshake (`X-Api-Version`, 426 if incompatible) ->
  device auth (`X-Device-Id`, `X-Device-Secret`) -> handlers: `POST /v1/pair/redeem`, `GET /v1/whoami`, `GET /v1/license/status`, `POST /v1/auth/login`.
  Routes that act as a user add a fourth step, session auth (`Authorization: Bearer <token>`, only valid from the device that signed in): `POST /v1/auth/logout`, `GET /v1/me`, `POST /v1/me/password`, `POST /v1/me/pin`, `POST /v1/users` (needs `users.manage`, password sign-in, a subscription that allows writes, and room under the plan's `max_users`), `POST /v1/users/:id/deactivate`.
  Administration (`src/admin.rs`, all through `authz::check`): `GET /v1/permissions`, `GET/POST /v1/roles`, `PUT/DELETE /v1/roles/:id`, `GET /v1/users`, `PUT /v1/users/:id/roles`, `PUT /v1/users/:id/locations`. `GET /v1/me` also returns `all_locations` and `location_ids`. See ADRs 0007 and 0008.
  Master data (`src/master_api.rs`, ADR 0009): `GET/POST /v1/locations`, `GET/PUT /v1/locations/:id`, `POST /v1/locations/:id/deactivate|activate`; `GET/POST /v1/categories`, `PUT /v1/categories/:id`; `GET/POST /v1/products` (`?search=&category_id=&include_inactive=&limit=&offset=`), `GET/PUT /v1/products/:id`, `POST /v1/products/:id/deactivate|activate`; the same shape for `/v1/customers`. Updates quote `expected_revision` (409 `stale_revision` on mismatch); `PUT` replaces the whole record, so send every field (an omitted optional field is cleared).
  Database work goes through `db()` (single writer, off the async runtime). Errors are generic JSON `{error,message,reason}`.
- `src/tls.rs`: self-signed certificate created once, SHA-256 fingerprint for pinning, rustls with the `ring` provider (no C toolchain needed).
- `src/entitlement.rs`: verify Ed25519-signed entitlements, apply (anti-rollback), `decide()` (modes + clock guard). Mirrors `packages/domain/src/entitlement.ts`.
- `src/main.rs`: dev runner `lan-server`; creates a dev tenant, prints fingerprint and a pairing code; `--lan`, `--apply-entitlement`.

**Rules for new endpoints:** authenticate the device, then call `authz::check(.., &Requirement::read/write("module.action"), Scope{..})` inside the `db()` closure.
It checks permission, location/route scope and, for writes, the subscription (402 `subscription_inactive`) in one place. Require an idempotency key for
inventory/finance writes, never return internals in errors.
**Tests:** `cargo test -p dsaas-lan-api` (60 in the build sandbox; 39 were verified on the owner's machine).
**Not built:** password reset for other users, mDNS discovery, per-IP rate limiting, client-side certificate pinning (belongs to the client app).
