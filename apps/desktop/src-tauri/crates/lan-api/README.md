# crates/lan-api (Rust): Shop Server HTTPS API
- `src/lib.rs`: axum router. `/v1/health` (open), then for `/v1/*`: version handshake (`X-Api-Version`, 426 if incompatible) ->
  device auth (`X-Device-Id`, `X-Device-Secret`) -> handlers: `POST /v1/pair/redeem`, `GET /v1/whoami`, `GET /v1/license/status`.
  Database work goes through `db()` (single writer, off the async runtime). Errors are generic JSON `{error,message,reason}`.
- `src/tls.rs`: self-signed certificate created once, SHA-256 fingerprint for pinning, rustls with the `ring` provider (no C toolchain needed).
- `src/entitlement.rs`: verify Ed25519-signed entitlements, apply (anti-rollback), `decide()` (modes + clock guard). Mirrors `packages/domain/src/entitlement.ts`.
- `src/main.rs`: dev runner `lan-server`; creates a dev tenant, prints fingerprint and a pairing code; `--lan`, `--apply-entitlement`.

**Rules for new endpoints:** authenticate the device, check the user's permission/scope, call `entitlement::decide` for writes (402 `subscription_inactive`
otherwise), require an idempotency key for inventory/finance writes, never return internals in errors.
**Tests:** `cargo test -p dsaas-lan-api` (12 verified before the subscription patch; 25 expected after it).
**Not built:** user login, shared authorization middleware, mDNS discovery, per-IP rate limiting, client-side certificate pinning (belongs to the client app).
