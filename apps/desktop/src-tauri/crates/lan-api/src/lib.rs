//! Shop Server HTTP API (v1). NOT YET COMPILED by the author; run `cargo test -p dsaas-lan-api`.
//!
//! The router is tested in-process; `tls` wraps it in HTTPS with a self-signed, pinned certificate.
//!
//! Request pipeline for /v1/* (except /v1/health):
//!   1. version handshake  (X-Api-Version must be within the supported range, else 426)
//!   2. device auth        (X-Device-Id + X-Device-Secret, checked against stored hashes)
//! Pairing codes are created locally on the server (not over the network) by an administrator.

pub mod entitlement;
pub mod tls;

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use dsaas_db::{
    pairing::{self, DeviceAuth, PairError},
    rusqlite::Connection,
    DbError, Writer,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const API_VERSION: u32 = 1;
pub const MIN_API_VERSION: u32 = 1;

#[derive(Clone)]
pub struct AppState {
    pub writer: Arc<Writer>,
    pub tenant_id: Arc<str>,
    pub server_version: Arc<str>,
    /// Ed25519 public key that subscription entitlements must be signed with.
    pub license_pubkey: [u8; 32],
}

#[derive(Clone, Debug)]
pub struct AuthedDevice(pub String);

#[derive(Debug)]
pub enum ApiError {
    BadRequest(&'static str),
    BadPairingCode,
    Unauthorized,
    Revoked,
    UpgradeRequired,
    Internal,
    SubscriptionInactive(&'static str),
    DeviceLimit,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut reason: Option<&'static str> = None;
        let (status, code, msg) = match self {
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, "bad_request", m),
            ApiError::BadPairingCode => (StatusCode::UNAUTHORIZED, "invalid_pairing_code", "invalid or expired pairing code"),
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized", "missing or invalid device credentials"),
            ApiError::Revoked => (StatusCode::FORBIDDEN, "device_revoked", "this device was removed by the administrator"),
            ApiError::UpgradeRequired => (
                StatusCode::UPGRADE_REQUIRED,
                "upgrade_required",
                "this app version is not compatible with the server; please update",
            ),
            ApiError::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error"),
            ApiError::SubscriptionInactive(r) => {
                reason = Some(r);
                (StatusCode::PAYMENT_REQUIRED, "subscription_inactive", "your subscription does not currently allow this action")
            }
            ApiError::DeviceLimit => (StatusCode::FORBIDDEN, "device_limit_reached", "the device limit of your plan has been reached"),
        };
        let body = serde_json::json!({
            "error": code,
            "message": msg,
            "api_version": API_VERSION,
            "min_api_version": MIN_API_VERSION,
            "reason": reason,
        });
        (status, Json(body)).into_response()
    }
}

/// Run a database job on the single writer thread without blocking the async runtime.
async fn db<T, F>(st: &AppState, f: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(&mut Connection) -> Result<T, DbError> + Send + 'static,
{
    let w = st.writer.clone();
    tokio::task::spawn_blocking(move || w.run(f))
        .await
        .map_err(|_| ApiError::Internal)?
        .map_err(|_| ApiError::Internal)
}

const NOW_SQL: &str = "SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')";

fn header(req: &Request, name: &str) -> Option<String> {
    req.headers().get(name).and_then(|v| v.to_str().ok()).map(|s| s.to_string())
}

async fn require_compatible(req: Request, next: Next) -> Response {
    let v = header(&req, "x-api-version").and_then(|s| s.parse::<u32>().ok());
    match v {
        Some(v) if (MIN_API_VERSION..=API_VERSION).contains(&v) => next.run(req).await,
        _ => ApiError::UpgradeRequired.into_response(),
    }
}

async fn require_device(State(st): State<AppState>, mut req: Request, next: Next) -> Response {
    let id = header(&req, "x-device-id");
    let secret = header(&req, "x-device-secret");
    let app_version: String = header(&req, "x-app-version").unwrap_or_default().chars().take(32).collect();
    let (Some(id), Some(secret)) = (id, secret) else {
        return ApiError::Unauthorized.into_response();
    };
    let tenant = st.tenant_id.to_string();
    let id2 = id.clone();
    let result = db(&st, move |c| {
        let a = pairing::authenticate(c, &tenant, &id2, &secret)?;
        if a == DeviceAuth::Valid {
            let now: String = c.query_row(NOW_SQL, [], |r| r.get(0))?;
            pairing::record_seen(c, &tenant, &id2, &now, &app_version)?;
        }
        Ok(a)
    })
    .await;
    match result {
        Ok(DeviceAuth::Valid) => {
            req.extensions_mut().insert(AuthedDevice(id));
            next.run(req).await
        }
        Ok(DeviceAuth::Revoked) => ApiError::Revoked.into_response(),
        Ok(DeviceAuth::Invalid) => ApiError::Unauthorized.into_response(),
        Err(e) => e.into_response(),
    }
}

async fn health(State(st): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "server_version": &*st.server_version,
        "api_version": API_VERSION,
        "min_api_version": MIN_API_VERSION,
    }))
}

#[derive(Deserialize)]
struct RedeemReq {
    code: String,
    device_name: String,
}

#[derive(Serialize)]
struct RedeemResp {
    device_id: String,
    secret: String,
    tenant_id: String,
}

enum Outcome {
    Paired(pairing::Credential),
    BadCode,
    Blocked(&'static str),
    DeviceLimit,
}

async fn redeem(State(st): State<AppState>, Json(req): Json<RedeemReq>) -> Result<Json<RedeemResp>, ApiError> {
    let name = req.device_name.trim().to_string();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(ApiError::BadRequest("device_name must be 1 to 64 characters"));
    }
    // A malformed code gets the same answer as a wrong one.
    if req.code.len() != 8 || !req.code.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ApiError::BadPairingCode);
    }
    let tenant = st.tenant_id.to_string();
    let code = req.code;
    let pk = st.license_pubkey;
    let outcome = db(&st, move |c| {
        // 1. Does the subscription allow new devices at all? (checked before the code is spent)
        let (decision, ent) = entitlement::decide(c, &tenant, &pk)?;
        if !decision.mode.can_write() {
            return Ok(Outcome::Blocked(decision.reason));
        }
        // 2. Plan limit on active devices.
        let max = ent.map(|e| e.limits.max_devices).unwrap_or(0);
        let n: i64 = c.query_row(
            "SELECT COUNT(*) FROM devices WHERE tenant_id=?1 AND kind='desktop_client' AND revoked_at IS NULL",
            [&tenant],
            |r| r.get(0),
        )?;
        if n >= max {
            return Ok(Outcome::DeviceLimit);
        }
        let now: String = c.query_row(NOW_SQL, [], |r| r.get(0))?;
        Ok(match pairing::redeem(c, &tenant, &code, &name, "desktop_client", &now) {
            Ok(cred) => Outcome::Paired(cred),
            Err(PairError::Invalid) => Outcome::BadCode,
            Err(PairError::Sqlite(e)) => return Err(DbError::Sqlite(e)),
        })
    })
    .await?;
    match outcome {
        Outcome::Paired(cred) => Ok(Json(RedeemResp {
            device_id: cred.device_id,
            secret: cred.secret,
            tenant_id: st.tenant_id.to_string(),
        })),
        Outcome::BadCode => Err(ApiError::BadPairingCode),
        Outcome::Blocked(reason) => Err(ApiError::SubscriptionInactive(reason)),
        Outcome::DeviceLimit => Err(ApiError::DeviceLimit),
    }
}

/// Readable in every mode so clients can show "suspended" or "grace period" banners.
async fn license_status(State(st): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let tenant = st.tenant_id.to_string();
    let pk = st.license_pubkey;
    let (dec, ent) = db(&st, move |c| Ok(entitlement::decide(c, &tenant, &pk)?)).await?;
    Ok(Json(serde_json::json!({
        "mode": dec.mode.as_str(),
        "reason": dec.reason,
        "can_write": dec.mode.can_write(),
        "revision": ent.as_ref().map(|e| e.revision),
        "full_until": ent.as_ref().map(|e| e.full_until.clone()),
        "grace_until": ent.as_ref().map(|e| e.grace_until.clone()),
        "limits": ent.as_ref().map(|e| &e.limits),
    })))
}

async fn whoami(State(st): State<AppState>, Extension(d): Extension<AuthedDevice>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "device_id": d.0, "tenant_id": &*st.tenant_id }))
}

pub fn router(state: AppState) -> Router {
    let authed = Router::new()
        .route("/v1/whoami", get(whoami))
        .route("/v1/license/status", get(license_status))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_device));
    let versioned = Router::new()
        .route("/v1/pair/redeem", post(redeem))
        .merge(authed)
        .layer(middleware::from_fn(require_compatible));
    Router::new()
        .route("/v1/health", get(health))
        .merge(versioned)
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use crate::entitlement::testvec;
    use dsaas_db::{migrate, open_in_memory, pairing};
    use tower::ServiceExt;

    fn make() -> (Router, Arc<Writer>, String) {
        make_with(Some("active_rev1"))
    }

    fn make_with(envelope: Option<&str>) -> (Router, Arc<Writer>, String) {
        let vecs = testvec::load();
        let pk = testvec::pubkey(&vecs);
        let mut c = open_in_memory().unwrap();
        migrate(&mut c).unwrap();
        c.execute_batch(
            "INSERT INTO tenants(id,name) VALUES ('t1','Shop 1');
             INSERT INTO users(id,tenant_id,username,full_name) VALUES ('u1','t1','admin','Admin');",
        )
        .unwrap();
        if let Some(name) = envelope {
            let (p, s) = testvec::env(&vecs, name);
            crate::entitlement::apply(&mut c, "t1", &p, &s, &pk).unwrap();
        }
        let now: String = c.query_row(NOW_SQL, [], |r| r.get(0)).unwrap();
        let p = pairing::create_pairing(&c, "t1", "u1", &now).unwrap();
        let writer = Arc::new(Writer::spawn(c));
        let state = AppState {
            writer: writer.clone(),
            tenant_id: "t1".into(),
            server_version: "0.0.1".into(),
            license_pubkey: pk,
        };
        (router(state), writer, p.code)
    }

    fn req(method: &str, uri: &str, headers: &[(&str, &str)], body: Option<&str>) -> HttpRequest<Body> {
        let mut b = HttpRequest::builder().method(method).uri(uri);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        if body.is_some() {
            b = b.header("content-type", "application/json");
        }
        b.body(Body::from(body.unwrap_or("").to_string())).unwrap()
    }

    async fn call(app: &Router, r: HttpRequest<Body>) -> (StatusCode, serde_json::Value) {
        let resp = app.clone().oneshot(r).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    const V1: (&str, &str) = ("x-api-version", "1");

    async fn pair(app: &Router, code: &str) -> (StatusCode, serde_json::Value) {
        let body = format!(r#"{{"code":"{code}","device_name":"Counter PC"}}"#);
        call(app, req("POST", "/v1/pair/redeem", &[V1], Some(&body))).await
    }

    fn creds<'a>(id: &'a str, secret: &'a str) -> Vec<(&'a str, &'a str)> {
        vec![V1, ("x-device-id", id), ("x-device-secret", secret), ("x-app-version", "0.0.1")]
    }

    #[tokio::test]
    async fn health_is_open_and_reports_versions() {
        let (app, _, _) = make();
        let (s, v) = call(&app, req("GET", "/v1/health", &[], None)).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["api_version"], 1);
        assert_eq!(v["status"], "ok");
    }

    #[tokio::test]
    async fn incompatible_or_missing_api_version_gets_426() {
        let (app, _, _) = make();
        let (s, v) = call(&app, req("POST", "/v1/pair/redeem", &[], Some(r#"{"code":"12345678","device_name":"x"}"#))).await;
        assert_eq!(s, StatusCode::UPGRADE_REQUIRED);
        assert_eq!(v["error"], "upgrade_required");
        let (s, _) = call(&app, req("GET", "/v1/whoami", &[("x-api-version", "99")], None)).await;
        assert_eq!(s, StatusCode::UPGRADE_REQUIRED);
    }

    #[tokio::test]
    async fn pairing_then_authenticated_call_works_and_code_is_single_use() {
        let (app, _, code) = make();
        let (s, v) = pair(&app, &code).await;
        assert_eq!(s, StatusCode::OK);
        let id = v["device_id"].as_str().unwrap().to_string();
        let secret = v["secret"].as_str().unwrap().to_string();
        assert_eq!(v["tenant_id"], "t1");

        let (s, w) = call(&app, req("GET", "/v1/whoami", &creds(&id, &secret), None)).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(w["device_id"], id.as_str());

        let (s, e) = pair(&app, &code).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        assert_eq!(e["error"], "invalid_pairing_code");
    }

    #[tokio::test]
    async fn wrong_malformed_and_unknown_codes_look_identical() {
        let (app, _, code) = make();
        let wrong = if code == "00000000" { "11111111" } else { "00000000" };
        for bad in [wrong, "123", "abcdefgh", "123456789"] {
            let (s, e) = pair(&app, bad).await;
            assert_eq!(s, StatusCode::UNAUTHORIZED, "code {bad}");
            assert_eq!(e["error"], "invalid_pairing_code");
        }
    }

    #[tokio::test]
    async fn empty_device_name_is_a_bad_request() {
        let (app, _, code) = make();
        let body = format!(r#"{{"code":"{code}","device_name":"   "}}"#);
        let (s, _) = call(&app, req("POST", "/v1/pair/redeem", &[V1], Some(&body))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn missing_or_wrong_credentials_are_rejected() {
        let (app, _, code) = make();
        let (_, v) = pair(&app, &code).await;
        let id = v["device_id"].as_str().unwrap().to_string();
        let (s, _) = call(&app, req("GET", "/v1/whoami", &[V1], None)).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        let (s, _) = call(&app, req("GET", "/v1/whoami", &creds(&id, "wrong-secret"), None)).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        let (s, _) = call(&app, req("GET", "/v1/whoami", &creds("no-such-device", "x"), None)).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn revoked_device_is_blocked_immediately() {
        let (app, writer, code) = make();
        let (_, v) = pair(&app, &code).await;
        let id = v["device_id"].as_str().unwrap().to_string();
        let secret = v["secret"].as_str().unwrap().to_string();
        let (s, _) = call(&app, req("GET", "/v1/whoami", &creds(&id, &secret), None)).await;
        assert_eq!(s, StatusCode::OK);

        let id2 = id.clone();
        writer
            .run(move |c| Ok(pairing::revoke_device(c, "t1", &id2, "2030-01-01T00:00:00.000Z")?))
            .unwrap();
        let (s, e) = call(&app, req("GET", "/v1/whoami", &creds(&id, &secret), None)).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        assert_eq!(e["error"], "device_revoked");
    }

    #[tokio::test]
    async fn successful_calls_record_app_version_and_last_seen() {
        let (app, writer, code) = make();
        let (_, v) = pair(&app, &code).await;
        let id = v["device_id"].as_str().unwrap().to_string();
        let secret = v["secret"].as_str().unwrap().to_string();
        call(&app, req("GET", "/v1/whoami", &creds(&id, &secret), None)).await;
        let (ver, seen): (Option<String>, Option<String>) = writer
            .run(|c| {
                Ok(c.query_row("SELECT app_version, last_seen_at FROM devices", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?)
            })
            .unwrap();
        assert_eq!(ver.as_deref(), Some("0.0.1"));
        assert!(seen.is_some());
    }

    // ---------- subscription enforcement ----------

    fn apply_env(writer: &Arc<Writer>, name: &str) -> Result<(), crate::entitlement::EntError> {
        let v = testvec::load();
        let pk = testvec::pubkey(&v);
        let (p, s) = testvec::env(&v, name);
        writer.run(move |c| Ok(crate::entitlement::apply(c, "t1", &p, &s, &pk))).unwrap().map(|_| ())
    }

    fn new_code(writer: &Arc<Writer>) -> String {
        writer
            .run(|c| {
                let now: String = c.query_row(NOW_SQL, [], |r| r.get(0))?;
                Ok(pairing::create_pairing(c, "t1", "u1", &now)?.code)
            })
            .unwrap()
    }

    async fn paired(app: &Router, code: &str) -> (String, String) {
        let (s, v) = pair(app, code).await;
        assert_eq!(s, StatusCode::OK, "pairing failed: {v}");
        (v["device_id"].as_str().unwrap().to_string(), v["secret"].as_str().unwrap().to_string())
    }

    #[tokio::test]
    async fn license_status_reports_the_entitlement() {
        let (app, _, code) = make();
        let (id, secret) = paired(&app, &code).await;
        let (s, v) = call(&app, req("GET", "/v1/license/status", &creds(&id, &secret), None)).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["mode"], "full");
        assert_eq!(v["can_write"], true);
        assert_eq!(v["revision"], 1);
        assert_eq!(v["limits"]["max_devices"], 2);
    }

    #[tokio::test]
    async fn without_any_entitlement_nothing_can_be_paired() {
        let (app, _, code) = make_with(None);
        let (s, e) = pair(&app, &code).await;
        assert_eq!(s, StatusCode::PAYMENT_REQUIRED);
        assert_eq!(e["error"], "subscription_inactive");
        assert_eq!(e["reason"], "no_entitlement");
    }

    #[tokio::test]
    async fn suspension_blocks_new_devices_keeps_existing_ones_readable_and_reinstatement_restores() {
        let (app, writer, code) = make();
        let (id, secret) = paired(&app, &code).await;

        apply_env(&writer, "suspended_rev2").unwrap();
        let (_, st) = call(&app, req("GET", "/v1/license/status", &creds(&id, &secret), None)).await;
        assert_eq!((st["mode"].as_str(), st["reason"].as_str(), st["can_write"].as_bool()), (Some("read_only"), Some("suspended"), Some(false)));
        let (s, w) = call(&app, req("GET", "/v1/whoami", &creds(&id, &secret), None)).await;
        assert_eq!(s, StatusCode::OK, "existing devices keep read access: {w}");

        let code2 = new_code(&writer);
        let (s, e) = pair(&app, &code2).await;
        assert_eq!(s, StatusCode::PAYMENT_REQUIRED);
        assert_eq!(e["reason"], "suspended");

        apply_env(&writer, "active_rev3").unwrap();
        let (s, _) = pair(&app, &code2).await; // the code was not consumed by the refused attempt
        assert_eq!(s, StatusCode::OK);
    }

    #[tokio::test]
    async fn expired_subscription_is_read_only() {
        let (app, writer, code) = make();
        apply_env(&writer, "expired_rev1").unwrap();
        let (s, e) = pair(&app, &code).await;
        assert_eq!(s, StatusCode::PAYMENT_REQUIRED);
        assert_eq!(e["reason"], "expired");
    }

    #[tokio::test]
    async fn plan_device_limit_is_enforced_and_freed_by_revocation() {
        let (app, writer, code1) = make(); // plan allows 2 devices
        let (id1, _) = paired(&app, &code1).await;
        let code2 = new_code(&writer);
        paired(&app, &code2).await;
        let code3 = new_code(&writer);
        let (s, e) = pair(&app, &code3).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        assert_eq!(e["error"], "device_limit_reached");

        let id1b = id1.clone();
        writer.run(move |c| Ok(pairing::revoke_device(c, "t1", &id1b, "2030-01-01T00:00:00.000Z")?)).unwrap();
        let (s, _) = pair(&app, &code3).await; // same code still valid: it was not spent
        assert_eq!(s, StatusCode::OK);
    }

    #[tokio::test]
    async fn a_forged_entitlement_cannot_be_applied() {
        let (_, writer, _) = make_with(None);
        let v = testvec::load();
        let pk = testvec::pubkey(&v);
        let t = v["tampered"].clone();
        let (p, s) = (t["payload_b64"].as_str().unwrap().to_string(), t["signature_b64"].as_str().unwrap().to_string());
        let r = writer.run(move |c| Ok(crate::entitlement::apply(c, "t1", &p, &s, &pk))).unwrap();
        assert!(matches!(r, Err(crate::entitlement::EntError::BadSignature)));
    }
}
