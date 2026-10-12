//! Shop Server HTTP API (v1). NOT YET COMPILED by the author; run `cargo test -p dsaas-lan-api`.
//!
//! The router is tested in-process; `tls` wraps it in HTTPS with a self-signed, pinned certificate.
//!
//! Request pipeline for /v1/* (except /v1/health):
//!   1. version handshake  (X-Api-Version must be within the supported range, else 426)
//!   2. device auth        (X-Device-Id + X-Device-Secret, checked against stored hashes)
//!   3. user session       (`Authorization: Bearer <token>` from /v1/auth/login), for the routes that
//!                         act as a user; the session must belong to the calling device
//! Pairing codes are created locally on the server (not over the network) by an administrator.
//!
//! Subscription rule: business writes call `entitlement::decide` and are refused unless the mode allows
//! writing. Sign-in, sign-out and changing your own password/PIN are NOT business writes and keep
//! working when a subscription is suspended (otherwise a lapse would lock the owner out of their data).

pub mod authz;
pub mod entitlement;
pub mod tls;

mod admin;

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
    users::{self, Method, Principal, UserError},
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
    InvalidCredentials,
    SessionRequired,
    /// 403 with a reason code: no_permission, location_denied, route_denied, cannot_grant_more_than_you_hold.
    Forbidden(&'static str),
    PasswordRequired,
    UserLimit,
    Conflict(&'static str),
    NotFound,
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
            // Deliberately vague: wrong password, unknown user, locked and deactivated look the same.
            ApiError::InvalidCredentials => (
                StatusCode::UNAUTHORIZED,
                "invalid_credentials",
                "invalid username or password, or the account is temporarily locked",
            ),
            ApiError::SessionRequired => (StatusCode::UNAUTHORIZED, "session_required", "sign in to continue"),
            ApiError::Forbidden(r) => {
                reason = Some(r);
                (StatusCode::FORBIDDEN, "forbidden", "you do not have permission to do this")
            }
            ApiError::PasswordRequired => (
                StatusCode::FORBIDDEN,
                "password_login_required",
                "sign in with your password (not a PIN) to do this",
            ),
            ApiError::UserLimit => (StatusCode::FORBIDDEN, "user_limit_reached", "the user limit of your plan has been reached"),
            ApiError::Conflict(m) => (StatusCode::CONFLICT, "conflict", m),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not_found", "not found"),
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

fn user_err(e: UserError) -> ApiError {
    match e {
        UserError::InvalidCredentials => ApiError::InvalidCredentials,
        UserError::BadInput(m) => ApiError::BadRequest(m),
        UserError::Conflict(m) => ApiError::Conflict(m),
        UserError::Forbidden(m) => ApiError::Forbidden(m),
        UserError::NotFound => ApiError::NotFound,
        UserError::Sqlite(_) => ApiError::Internal,
    }
}

fn now_utc(c: &Connection) -> Result<String, DbError> {
    Ok(c.query_row(NOW_SQL, [], |r| r.get(0))?)
}

/// Sign-in required: resolve `Authorization: Bearer <token>` to a user for the calling device.
/// Runs after `require_device`, which supplies the device id.
async fn require_session(State(st): State<AppState>, mut req: Request, next: Next) -> Response {
    let token = header(&req, "authorization")
        .and_then(|h| h.strip_prefix("Bearer ").map(|t| t.trim().to_string()))
        .filter(|t| !t.is_empty() && t.len() <= 128);
    let device = req.extensions().get::<AuthedDevice>().map(|d| d.0.clone());
    let (Some(token), Some(device)) = (token, device) else {
        return ApiError::SessionRequired.into_response();
    };
    let tenant = st.tenant_id.to_string();
    let found = db(&st, move |c| {
        let now = now_utc(c)?;
        users::authenticate_session(c, &tenant, &device, &token, &now)
    })
    .await;
    match found {
        Ok(Some(p)) => {
            req.extensions_mut().insert(p);
            next.run(req).await
        }
        Ok(None) => ApiError::SessionRequired.into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Deserialize)]
struct LoginReq {
    username: String,
    password: Option<String>,
    pin: Option<String>,
}

async fn login(
    State(st): State<AppState>,
    Extension(d): Extension<AuthedDevice>,
    Json(req): Json<LoginReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (secret, method) = match (req.password, req.pin) {
        (Some(p), None) => (p, Method::Password),
        (None, Some(p)) => (p, Method::Pin),
        _ => return Err(ApiError::BadRequest("send exactly one of password or pin")),
    };
    if req.username.len() > 64 || secret.is_empty() || secret.len() > 256 {
        return Err(ApiError::BadRequest("username or secret has an invalid length"));
    }
    let tenant = st.tenant_id.to_string();
    let device = d.0;
    let username = req.username;
    let out = db(&st, move |c| {
        let now = now_utc(c)?;
        Ok(match users::login(c, &tenant, &device, &username, &secret, method, &now) {
            Ok(ok) => {
                let perms = users::permissions_for_user(c, &tenant, &ok.user_id)?;
                Ok((ok, perms))
            }
            Err(e) => Err(e),
        })
    })
    .await?
    .map_err(user_err)?;
    let (ok, perms) = out;
    Ok(Json(serde_json::json!({
        "token": ok.token,
        "expires_at": ok.expires_at,
        "method": ok.method.as_str(),
        "user": { "id": ok.user_id, "username": ok.username, "full_name": ok.full_name },
        "permissions": perms,
    })))
}

async fn logout(State(st): State<AppState>, Extension(p): Extension<Principal>) -> Result<StatusCode, ApiError> {
    let tenant = st.tenant_id.to_string();
    db(&st, move |c| {
        let now = now_utc(c)?;
        users::logout(c, &tenant, &p.session_id, &now)
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn me(Extension(p): Extension<Principal>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "user": { "id": p.user_id, "username": p.username, "full_name": p.full_name },
        "method": p.method.as_str(),
        "permissions": p.permissions,
        "all_locations": p.access().all_locations,
        "location_ids": p.location_ids,
    }))
}

#[derive(Deserialize)]
struct ChangePasswordReq {
    current_password: String,
    new_password: String,
}

async fn change_password(
    State(st): State<AppState>,
    Extension(d): Extension<AuthedDevice>,
    Extension(p): Extension<Principal>,
    Json(req): Json<ChangePasswordReq>,
) -> Result<StatusCode, ApiError> {
    if p.method != Method::Password {
        return Err(ApiError::PasswordRequired);
    }
    if req.current_password.len() > 256 || req.new_password.len() > 512 {
        return Err(ApiError::BadRequest("password is too long"));
    }
    let tenant = st.tenant_id.to_string();
    db(&st, move |c| {
        let now = now_utc(c)?;
        Ok(users::change_password(c, &tenant, &p.user_id, Some(&d.0), &req.current_password, &req.new_password, &now))
    })
    .await?
    .map_err(user_err)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct SetPinReq {
    current_password: String,
    pin: String,
}

async fn set_pin(
    State(st): State<AppState>,
    Extension(d): Extension<AuthedDevice>,
    Extension(p): Extension<Principal>,
    Json(req): Json<SetPinReq>,
) -> Result<StatusCode, ApiError> {
    if p.method != Method::Password {
        return Err(ApiError::PasswordRequired);
    }
    if req.current_password.len() > 256 || req.pin.len() > 16 {
        return Err(ApiError::BadRequest("value is too long"));
    }
    let tenant = st.tenant_id.to_string();
    db(&st, move |c| {
        let now = now_utc(c)?;
        Ok(users::set_pin(c, &tenant, &p.user_id, Some(&d.0), &req.current_password, &req.pin, &now))
    })
    .await?
    .map_err(user_err)?;
    Ok(StatusCode::NO_CONTENT)
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
    // Routes that act as a signed-in user. `require_session` is added first so it runs after `require_device`.
    let sessioned = Router::new()
        .route("/v1/auth/logout", post(logout))
        .route("/v1/me", get(me))
        .route("/v1/me/password", post(change_password))
        .route("/v1/me/pin", post(set_pin))
        .merge(admin::routes())
        .route_layer(middleware::from_fn_with_state(state.clone(), require_session));
    let authed = Router::new()
        .route("/v1/whoami", get(whoami))
        .route("/v1/license/status", get(license_status))
        .route("/v1/auth/login", post(login))
        .merge(sessioned)
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
        let boss = users::create_user(&mut c, "t1", None, None, "boss", "The Boss", PW, "2030-01-01T00:00:00.000Z").unwrap();
        users::grant_owner(&mut c, "t1", &boss).unwrap();
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
    const PW: &str = "correct horse battery";

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

    // ---------- users, login and sessions ----------

    fn bearer(token: &str) -> String {
        format!("Bearer {token}")
    }

    fn authed<'a>(id: &'a str, secret: &'a str, bearer: &'a str) -> Vec<(&'a str, &'a str)> {
        let mut h = creds(id, secret);
        h.push(("authorization", bearer));
        h
    }

    async fn sign_in(app: &Router, id: &str, secret: &str, user: &str, pw: &str) -> (StatusCode, serde_json::Value) {
        let body = serde_json::json!({ "username": user, "password": pw }).to_string();
        call(app, req("POST", "/v1/auth/login", &creds(id, secret), Some(&body))).await
    }

    /// A paired device and a signed-in owner token.
    async fn owner_session(app: &Router, writer: &Arc<Writer>) -> (String, String, String) {
        let code = new_code(writer);
        let (id, secret) = paired(app, &code).await;
        let (s, v) = sign_in(app, &id, &secret, "boss", PW).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        (id, secret, v["token"].as_str().unwrap().to_string())
    }

    async fn post_json(app: &Router, uri: &str, h: &[(&str, &str)], body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        call(app, req("POST", uri, h, Some(&body.to_string()))).await
    }

    #[tokio::test]
    async fn login_needs_a_paired_device_first() {
        let (app, _, _) = make();
        let body = serde_json::json!({ "username": "boss", "password": PW }).to_string();
        let (s, e) = call(&app, req("POST", "/v1/auth/login", &[V1], Some(&body))).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        assert_eq!(e["error"], "unauthorized");
    }

    #[tokio::test]
    async fn login_returns_a_token_and_permissions_and_never_a_hash() {
        let (app, writer, _) = make();
        let code = new_code(&writer);
        let (id, secret) = paired(&app, &code).await;
        let (s, v) = sign_in(&app, &id, &secret, "Boss", PW).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["user"]["username"], "boss");
        assert_eq!(v["method"], "password");
        assert_eq!(v["permissions"], serde_json::json!(["*"]));
        assert_eq!(v["token"].as_str().unwrap().len(), 64);
        assert!(!v.to_string().contains("argon2"));
        let b = bearer(v["token"].as_str().unwrap());
        let (s, m) = call(&app, req("GET", "/v1/me", &authed(&id, &secret, &b), None)).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(m["user"]["full_name"], "The Boss");
    }

    #[tokio::test]
    async fn wrong_password_and_unknown_user_get_the_same_answer() {
        let (app, writer, _) = make();
        let code = new_code(&writer);
        let (id, secret) = paired(&app, &code).await;
        let (s1, e1) = sign_in(&app, &id, &secret, "boss", "wrong password!").await;
        let (s2, e2) = sign_in(&app, &id, &secret, "nobody", "wrong password!").await;
        assert_eq!((s1, s2), (StatusCode::UNAUTHORIZED, StatusCode::UNAUTHORIZED));
        assert_eq!(e1, e2);
        assert_eq!(e1["error"], "invalid_credentials");
    }

    #[tokio::test]
    async fn login_body_must_have_exactly_one_secret() {
        let (app, writer, _) = make();
        let code = new_code(&writer);
        let (id, secret) = paired(&app, &code).await;
        for body in [
            serde_json::json!({ "username": "boss" }),
            serde_json::json!({ "username": "boss", "password": PW, "pin": "2580" }),
            serde_json::json!({ "username": "boss", "password": "" }),
        ] {
            let (s, _) = post_json(&app, "/v1/auth/login", &creds(&id, &secret), body).await;
            assert_eq!(s, StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn five_wrong_passwords_lock_the_account_even_for_the_right_one() {
        let (app, writer, _) = make();
        let code = new_code(&writer);
        let (id, secret) = paired(&app, &code).await;
        for _ in 0..5 {
            let (s, _) = sign_in(&app, &id, &secret, "boss", "wrong password!").await;
            assert_eq!(s, StatusCode::UNAUTHORIZED);
        }
        let (s, e) = sign_in(&app, &id, &secret, "boss", PW).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        assert_eq!(e["error"], "invalid_credentials");
    }

    #[tokio::test]
    async fn session_routes_need_a_valid_token_from_the_same_device() {
        let (app, writer, _) = make();
        let (id, secret, token) = owner_session(&app, &writer).await;
        // no token / garbage token / wrong scheme
        for h in [None, Some("Bearer nonsense"), Some("Basic abc"), Some("Bearer ")] {
            let mut hdr = creds(&id, &secret);
            if let Some(v) = h {
                hdr.push(("authorization", v));
            }
            let (s, e) = call(&app, req("GET", "/v1/me", &hdr, None)).await;
            assert_eq!(s, StatusCode::UNAUTHORIZED, "{h:?}");
            assert_eq!(e["error"], "session_required");
        }
        // a second paired device cannot use the first device's token
        let code2 = new_code(&writer);
        let (id2, secret2) = paired(&app, &code2).await;
        let b = bearer(&token);
        let (s, _) = call(&app, req("GET", "/v1/me", &authed(&id2, &secret2, &b), None)).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        // and the device credentials alone are not enough for a session route
        let (s, _) = call(&app, req("GET", "/v1/me", &creds(&id, &secret), None)).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        // existing device-only route is unchanged
        let (s, _) = call(&app, req("GET", "/v1/whoami", &creds(&id, &secret), None)).await;
        assert_eq!(s, StatusCode::OK);
    }

    #[tokio::test]
    async fn logout_ends_the_session() {
        let (app, writer, _) = make();
        let (id, secret, token) = owner_session(&app, &writer).await;
        let b = bearer(&token);
        let (s, _) = call(&app, req("POST", "/v1/auth/logout", &authed(&id, &secret, &b), None)).await;
        assert_eq!(s, StatusCode::NO_CONTENT);
        let (s, _) = call(&app, req("GET", "/v1/me", &authed(&id, &secret, &b), None)).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn an_owner_creates_a_user_who_can_then_sign_in() {
        let (app, writer, _) = make();
        let (id, secret, token) = owner_session(&app, &writer).await;
        let b = bearer(&token);
        let body = serde_json::json!({ "username": "Clerk1", "full_name": "Clerk One", "password": "clerk password 1" });
        let (s, v) = post_json(&app, "/v1/users", &authed(&id, &secret, &b), body.clone()).await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
        assert!(v["id"].as_str().is_some());
        let (s, e) = post_json(&app, "/v1/users", &authed(&id, &secret, &b), body).await;
        assert_eq!((s, e["error"].as_str()), (StatusCode::CONFLICT, Some("conflict")));
        let (s, l) = sign_in(&app, &id, &secret, "clerk1", "clerk password 1").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(l["permissions"], serde_json::json!([]), "new users have no permissions until given a role");
        let weak = serde_json::json!({ "username": "weak", "full_name": "W", "password": "short" });
        let (s, _) = post_json(&app, "/v1/users", &authed(&id, &secret, &b), weak).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_user_without_users_manage_is_forbidden_even_with_a_password_session() {
        let (app, writer, _) = make();
        let (id, secret, token) = owner_session(&app, &writer).await;
        let b = bearer(&token);
        let body = serde_json::json!({ "username": "clerk1", "full_name": "Clerk", "password": "clerk password 1" });
        post_json(&app, "/v1/users", &authed(&id, &secret, &b), body).await;
        let (_, l) = sign_in(&app, &id, &secret, "clerk1", "clerk password 1").await;
        let cb = bearer(l["token"].as_str().unwrap());
        let body = serde_json::json!({ "username": "other", "full_name": "O", "password": "other password 1" });
        let (s, e) = post_json(&app, "/v1/users", &authed(&id, &secret, &cb), body).await;
        assert_eq!((s, e["error"].as_str()), (StatusCode::FORBIDDEN, Some("forbidden")));
    }

    #[tokio::test]
    async fn a_pin_session_cannot_manage_users_or_change_credentials() {
        let (app, writer, _) = make();
        let (id, secret, token) = owner_session(&app, &writer).await;
        let b = bearer(&token);
        let (s, _) = post_json(&app, "/v1/me/pin", &authed(&id, &secret, &b), serde_json::json!({ "current_password": "wrong password!", "pin": "2580" })).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "setting a PIN needs the current password");
        let (s, _) = post_json(&app, "/v1/me/pin", &authed(&id, &secret, &b), serde_json::json!({ "current_password": PW, "pin": "1234567890" })).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, _) = post_json(&app, "/v1/me/pin", &authed(&id, &secret, &b), serde_json::json!({ "current_password": PW, "pin": "2580" })).await;
        assert_eq!(s, StatusCode::NO_CONTENT);

        let (s, l) = post_json(&app, "/v1/auth/login", &creds(&id, &secret), serde_json::json!({ "username": "boss", "pin": "2580" })).await;
        assert_eq!((s, l["method"].as_str()), (StatusCode::OK, Some("pin")));
        let pb = bearer(l["token"].as_str().unwrap());
        let (s, m) = call(&app, req("GET", "/v1/me", &authed(&id, &secret, &pb), None)).await;
        assert_eq!((s, m["method"].as_str()), (StatusCode::OK, Some("pin")));

        let new_user = serde_json::json!({ "username": "sneaky", "full_name": "S", "password": "sneaky password 1" });
        let (s, e) = post_json(&app, "/v1/users", &authed(&id, &secret, &pb), new_user).await;
        assert_eq!((s, e["error"].as_str()), (StatusCode::FORBIDDEN, Some("password_login_required")));
        let chg = serde_json::json!({ "current_password": PW, "new_password": "attacker chosen password" });
        let (s, e) = post_json(&app, "/v1/me/password", &authed(&id, &secret, &pb), chg).await;
        assert_eq!((s, e["error"].as_str()), (StatusCode::FORBIDDEN, Some("password_login_required")));
    }

    #[tokio::test]
    async fn changing_the_password_signs_everyone_out() {
        let (app, writer, _) = make();
        let (id, secret, token) = owner_session(&app, &writer).await;
        let b = bearer(&token);
        let bad = serde_json::json!({ "current_password": "wrong password!", "new_password": "a brand new password" });
        let (s, _) = post_json(&app, "/v1/me/password", &authed(&id, &secret, &b), bad).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        let ok = serde_json::json!({ "current_password": PW, "new_password": "a brand new password" });
        let (s, _) = post_json(&app, "/v1/me/password", &authed(&id, &secret, &b), ok).await;
        assert_eq!(s, StatusCode::NO_CONTENT);
        let (s, _) = call(&app, req("GET", "/v1/me", &authed(&id, &secret, &b), None)).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        assert_eq!(sign_in(&app, &id, &secret, "boss", PW).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(sign_in(&app, &id, &secret, "boss", "a brand new password").await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn deactivating_a_user_signs_them_out_and_the_last_owner_is_protected() {
        let (app, writer, _) = make();
        let (id, secret, token) = owner_session(&app, &writer).await;
        let b = bearer(&token);
        let body = serde_json::json!({ "username": "clerk1", "full_name": "Clerk", "password": "clerk password 1" });
        let (_, created) = post_json(&app, "/v1/users", &authed(&id, &secret, &b), body).await;
        let clerk_id = created["id"].as_str().unwrap().to_string();
        let (_, l) = sign_in(&app, &id, &secret, "clerk1", "clerk password 1").await;
        let cb = bearer(l["token"].as_str().unwrap());

        let uri = format!("/v1/users/{clerk_id}/deactivate");
        let (s, _) = call(&app, req("POST", &uri, &authed(&id, &secret, &b), None)).await;
        assert_eq!(s, StatusCode::NO_CONTENT);
        let (s, _) = call(&app, req("GET", "/v1/me", &authed(&id, &secret, &cb), None)).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        assert_eq!(sign_in(&app, &id, &secret, "clerk1", "clerk password 1").await.0, StatusCode::UNAUTHORIZED);

        let boss_id = writer
            .run(|c| Ok(c.query_row("SELECT id FROM users WHERE username='boss'", [], |r| r.get::<_, String>(0))?))
            .unwrap();
        let (s, e) = call(&app, req("POST", &format!("/v1/users/{boss_id}/deactivate"), &authed(&id, &secret, &b), None)).await;
        assert_eq!((s, e["error"].as_str()), (StatusCode::CONFLICT, Some("conflict")));
        let (s, _) = call(&app, req("POST", "/v1/users/no-such-user/deactivate", &authed(&id, &secret, &b), None)).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_plan_user_limit_is_enforced() {
        let (app, writer, _) = make();
        let (id, secret, token) = owner_session(&app, &writer).await;
        let b = bearer(&token);
        // plan allows 10 users; u1 and boss exist, add 8 more directly
        writer
            .run(|c| {
                for i in 0..8 {
                    c.execute(
                        "INSERT INTO users(id,tenant_id,username,full_name) VALUES (?1,'t1',?2,'Filler')",
                        dsaas_db::rusqlite::params![format!("f{i}"), format!("filler{i}")],
                    )?;
                }
                Ok(())
            })
            .unwrap();
        let body = serde_json::json!({ "username": "oneover", "full_name": "Over", "password": "over the limit 1" });
        let (s, e) = post_json(&app, "/v1/users", &authed(&id, &secret, &b), body).await;
        assert_eq!((s, e["error"].as_str()), (StatusCode::FORBIDDEN, Some("user_limit_reached")));
    }

    #[tokio::test]
    async fn suspension_blocks_creating_users_but_not_signing_in() {
        let (app, writer, _) = make();
        let (id, secret, token) = owner_session(&app, &writer).await;
        let b = bearer(&token);
        apply_env(&writer, "suspended_rev2").unwrap();
        let body = serde_json::json!({ "username": "newhire", "full_name": "New", "password": "new hire password" });
        let (s, e) = post_json(&app, "/v1/users", &authed(&id, &secret, &b), body).await;
        assert_eq!((s, e["error"].as_str()), (StatusCode::PAYMENT_REQUIRED, Some("subscription_inactive")));
        // owners can still sign in, read, and sign out of a suspended shop
        assert_eq!(sign_in(&app, &id, &secret, "boss", PW).await.0, StatusCode::OK);
        let (s, _) = call(&app, req("GET", "/v1/me", &authed(&id, &secret, &b), None)).await;
        assert_eq!(s, StatusCode::OK);
        let (s, _) = call(&app, req("POST", "/v1/auth/logout", &authed(&id, &secret, &b), None)).await;
        assert_eq!(s, StatusCode::NO_CONTENT);
    }

    // ---------- roles, permissions and scope ----------

    async fn put_json(app: &Router, uri: &str, h: &[(&str, &str)], body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        call(app, req("PUT", uri, h, Some(&body.to_string()))).await
    }

    /// Create a user holding one custom role with `perms` (done by the owner), and sign them in.
    /// Returns (user id, role id, bearer header value).
    async fn user_with_role(app: &Router, id: &str, secret: &str, owner_bearer: &str, name: &str, perms: &[&str]) -> (String, String, String) {
        let h = authed(id, secret, owner_bearer);
        let (s, u) = post_json(app, "/v1/users", &h, serde_json::json!({ "username": name, "full_name": name, "password": "clerk password 1" })).await;
        assert_eq!(s, StatusCode::CREATED, "{u}");
        let (s, r) = post_json(app, "/v1/roles", &h, serde_json::json!({ "name": format!("{name} role"), "permissions": perms })).await;
        assert_eq!(s, StatusCode::CREATED, "{r}");
        let (uid, rid) = (u["id"].as_str().unwrap().to_string(), r["id"].as_str().unwrap().to_string());
        let (s, e) = put_json(app, &format!("/v1/users/{uid}/roles"), &h, serde_json::json!({ "role_ids": [rid] })).await;
        assert_eq!(s, StatusCode::NO_CONTENT, "{e}");
        let (s, l) = sign_in(app, id, secret, name, "clerk password 1").await;
        assert_eq!(s, StatusCode::OK);
        (uid, rid, bearer(l["token"].as_str().unwrap()))
    }

    fn add_locations(writer: &Arc<Writer>) {
        writer
            .run(|c| {
                c.execute_batch(
                    "INSERT INTO stock_locations(id,tenant_id,name,code) VALUES
                       ('main','t1','Main','MAIN'),('g1','t1','Godown 1','G1'),('g2','t1','Godown 2','G2');",
                )?;
                Ok(())
            })
            .unwrap();
    }

    #[tokio::test]
    async fn role_admin_needs_the_roles_manage_permission() {
        let (app, writer, _) = make();
        let (id, secret, token) = owner_session(&app, &writer).await;
        let ob = bearer(&token);
        let (_, _, clerk) = user_with_role(&app, &id, &secret, &ob, "clerk1", &["sales.view"]).await;
        for (m, uri) in [("GET", "/v1/roles"), ("GET", "/v1/permissions"), ("GET", "/v1/users")] {
            let (s, e) = call(&app, req(m, uri, &authed(&id, &secret, &clerk), None)).await;
            assert_eq!((s, e["error"].as_str(), e["reason"].as_str()), (StatusCode::FORBIDDEN, Some("forbidden"), Some("no_permission")), "{uri}");
        }
        let (s, p) = call(&app, req("GET", "/v1/permissions", &authed(&id, &secret, &ob), None)).await;
        assert_eq!(s, StatusCode::OK);
        assert!(p.as_array().unwrap().iter().any(|x| x["permission"] == "inventory.transfer" && x["description"].is_string()));
    }

    #[tokio::test]
    async fn a_role_can_be_created_listed_edited_and_deleted() {
        let (app, writer, _) = make();
        let (id, secret, token) = owner_session(&app, &writer).await;
        let h_owner = bearer(&token);
        let h = authed(&id, &secret, &h_owner);
        let (s, r) = post_json(&app, "/v1/roles", &h, serde_json::json!({ "name": "Cashier", "permissions": ["sales.create", "inventory.view", "sales.create"] })).await;
        assert_eq!(s, StatusCode::CREATED, "{r}");
        let rid = r["id"].as_str().unwrap().to_string();

        let (_, list) = call(&app, req("GET", "/v1/roles", &h, None)).await;
        let cashier = list.as_array().unwrap().iter().find(|x| x["id"] == rid.as_str()).unwrap().clone();
        assert_eq!(cashier["permissions"], serde_json::json!(["inventory.view", "sales.create"]));
        assert_eq!((cashier["member_count"].as_i64(), cashier["protected"].as_bool()), (Some(0), Some(false)));
        assert!(list.as_array().unwrap().iter().any(|x| x["protected"] == true && x["name"] == "Owner"));

        // validation and conflicts
        let bad = |n: &str, p: serde_json::Value| serde_json::json!({ "name": n, "permissions": p });
        assert_eq!(post_json(&app, "/v1/roles", &h, bad("Typo", serde_json::json!(["inventory.transfr"]))).await.0, StatusCode::BAD_REQUEST);
        assert_eq!(post_json(&app, "/v1/roles", &h, bad("cashier", serde_json::json!([]))).await.0, StatusCode::CONFLICT);
        assert_eq!(post_json(&app, "/v1/roles", &h, bad("Owner", serde_json::json!([]))).await.0, StatusCode::BAD_REQUEST);

        let (s, _) = put_json(&app, &format!("/v1/roles/{rid}"), &h, bad("Senior cashier", serde_json::json!(["sales.*", "inventory.view"]))).await;
        assert_eq!(s, StatusCode::NO_CONTENT);
        let (s, _) = call(&app, req("DELETE", &format!("/v1/roles/{rid}"), &h, None)).await;
        assert_eq!(s, StatusCode::NO_CONTENT);
        let (s, _) = call(&app, req("DELETE", &format!("/v1/roles/{rid}"), &h, None)).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_role_manager_cannot_grant_more_than_they_hold() {
        let (app, writer, _) = make();
        let (id, secret, token) = owner_session(&app, &writer).await;
        let ob = bearer(&token);
        let (_, _, mgr) = user_with_role(&app, &id, &secret, &ob, "mgr", &["roles.manage", "inventory.*"]).await;
        let h = authed(&id, &secret, &mgr);
        let mk = |n: &str, p: serde_json::Value| serde_json::json!({ "name": n, "permissions": p });
        assert_eq!(post_json(&app, "/v1/roles", &h, mk("Stock", serde_json::json!(["inventory.view"]))).await.0, StatusCode::CREATED);
        for p in [serde_json::json!(["*"]), serde_json::json!(["sales.create"]), serde_json::json!(["users.*"])] {
            let (s, e) = post_json(&app, "/v1/roles", &h, mk("Sneaky", p)).await;
            assert_eq!((s, e["reason"].as_str()), (StatusCode::FORBIDDEN, Some("cannot_grant_more_than_you_hold")));
        }
        // handing themselves the Owner role is refused too
        let (_, list) = call(&app, req("GET", "/v1/roles", &h, None)).await;
        let owner_role = list.as_array().unwrap().iter().find(|x| x["protected"] == true).unwrap()["id"].as_str().unwrap().to_string();
        let (_, users) = call(&app, req("GET", "/v1/users", &authed(&id, &secret, &ob), None)).await;
        let mgr_id = users.as_array().unwrap().iter().find(|u| u["username"] == "mgr").unwrap()["id"].as_str().unwrap().to_string();
        let (s, e) = put_json(&app, &format!("/v1/users/{mgr_id}/roles"), &h, serde_json::json!({ "role_ids": [owner_role] })).await;
        assert_eq!((s, e["reason"].as_str()), (StatusCode::FORBIDDEN, Some("cannot_grant_more_than_you_hold")));
    }

    #[tokio::test]
    async fn assigned_roles_take_effect_on_the_same_session_immediately() {
        let (app, writer, _) = make();
        let (id, secret, token) = owner_session(&app, &writer).await;
        let ob = bearer(&token);
        let (uid, rid, clerk) = user_with_role(&app, &id, &secret, &ob, "clerk1", &["sales.view"]).await;
        let me = |b: String| {
            let app = app.clone();
            let (id, secret) = (id.clone(), secret.clone());
            async move { call(&app, req("GET", "/v1/me", &authed(&id, &secret, &b), None)).await.1 }
        };
        assert_eq!(me(clerk.clone()).await["permissions"], serde_json::json!(["sales.view"]));
        let h = authed(&id, &secret, &ob);
        put_json(&app, &format!("/v1/roles/{rid}"), &h, serde_json::json!({ "name": "clerk1 role", "permissions": ["sales.view", "sales.create"] })).await;
        assert_eq!(me(clerk.clone()).await["permissions"], serde_json::json!(["sales.create", "sales.view"]));
        let (s, _) = put_json(&app, &format!("/v1/users/{uid}/roles"), &h, serde_json::json!({ "role_ids": [] })).await;
        assert_eq!(s, StatusCode::NO_CONTENT);
        assert_eq!(me(clerk).await["permissions"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn the_owner_role_and_the_last_owner_are_protected() {
        let (app, writer, _) = make();
        let (id, secret, token) = owner_session(&app, &writer).await;
        let h_owner = bearer(&token);
        let h = authed(&id, &secret, &h_owner);
        let (_, list) = call(&app, req("GET", "/v1/roles", &h, None)).await;
        let owner_role = list.as_array().unwrap().iter().find(|x| x["protected"] == true).unwrap()["id"].as_str().unwrap().to_string();
        let (s, _) = put_json(&app, &format!("/v1/roles/{owner_role}"), &h, serde_json::json!({ "name": "Owner", "permissions": ["sales.view"] })).await;
        assert_eq!(s, StatusCode::CONFLICT);
        let (s, _) = call(&app, req("DELETE", &format!("/v1/roles/{owner_role}"), &h, None)).await;
        assert_eq!(s, StatusCode::CONFLICT);
        let boss_id = writer.run(|c| Ok(c.query_row("SELECT id FROM users WHERE username='boss'", [], |r| r.get::<_, String>(0))?)).unwrap();
        let (s, e) = put_json(&app, &format!("/v1/users/{boss_id}/roles"), &h, serde_json::json!({ "role_ids": [] })).await;
        assert_eq!((s, e["error"].as_str()), (StatusCode::CONFLICT, Some("conflict")));
        let (_, m) = call(&app, req("GET", "/v1/me", &h, None)).await;
        assert_eq!(m["permissions"], serde_json::json!(["*"]), "refused change left the owner intact");
    }

    #[tokio::test]
    async fn location_access_can_be_set_and_is_visible_on_me() {
        let (app, writer, _) = make();
        add_locations(&writer);
        let (id, secret, token) = owner_session(&app, &writer).await;
        let ob = bearer(&token);
        let (uid, _, clerk) = user_with_role(&app, &id, &secret, &ob, "clerk1", &["sales.view"]).await;
        let h = authed(&id, &secret, &ob);
        let (s, _) = put_json(&app, &format!("/v1/users/{uid}/locations"), &h, serde_json::json!({ "location_ids": ["nowhere"] })).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, _) = put_json(&app, &format!("/v1/users/{uid}/locations"), &h, serde_json::json!({ "location_ids": ["main", "g1"] })).await;
        assert_eq!(s, StatusCode::NO_CONTENT);
        let (_, m) = call(&app, req("GET", "/v1/me", &authed(&id, &secret, &clerk), None)).await;
        assert_eq!((m["all_locations"].as_bool(), m["location_ids"].clone()), (Some(false), serde_json::json!(["g1", "main"])));
        let (_, om) = call(&app, req("GET", "/v1/me", &h, None)).await;
        assert_eq!(om["all_locations"], true, "the owner's * covers every location");
        let (s, _) = put_json(&app, "/v1/users/ghost/locations", &h, serde_json::json!({ "location_ids": [] })).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_limited_supervisor_can_only_manage_locations_they_have() {
        let (app, writer, _) = make();
        add_locations(&writer);
        let (id, secret, token) = owner_session(&app, &writer).await;
        let ob = bearer(&token);
        let (sup_id, _, sup) = user_with_role(&app, &id, &secret, &ob, "sup", &["users.manage"]).await;
        let (w_id, _, _) = user_with_role(&app, &id, &secret, &ob, "worker", &["sales.view"]).await;
        let owner_h = authed(&id, &secret, &ob);
        put_json(&app, &format!("/v1/users/{sup_id}/locations"), &owner_h, serde_json::json!({ "location_ids": ["g1"] })).await;
        // the supervisor's session already exists: permissions and locations are re-read per request
        let sh = authed(&id, &secret, &sup);
        let (s, _) = put_json(&app, &format!("/v1/users/{w_id}/locations"), &sh, serde_json::json!({ "location_ids": ["g1"] })).await;
        assert_eq!(s, StatusCode::NO_CONTENT);
        let (s, e) = put_json(&app, &format!("/v1/users/{w_id}/locations"), &sh, serde_json::json!({ "location_ids": ["g1", "g2"] })).await;
        assert_eq!((s, e["reason"].as_str()), (StatusCode::FORBIDDEN, Some("cannot_grant_more_than_you_hold")));
    }

    #[tokio::test]
    async fn user_listing_shows_roles_and_locations_and_never_secrets() {
        let (app, writer, _) = make();
        let (id, secret, token) = owner_session(&app, &writer).await;
        let ob = bearer(&token);
        let (uid, rid, _) = user_with_role(&app, &id, &secret, &ob, "clerk1", &["sales.view"]).await;
        let (s, v) = call(&app, req("GET", "/v1/users", &authed(&id, &secret, &ob), None)).await;
        assert_eq!(s, StatusCode::OK);
        let raw = v.to_string();
        assert!(!raw.contains("argon2") && !raw.contains("hash") && !raw.contains("password"));
        let clerk = v.as_array().unwrap().iter().find(|u| u["id"] == uid.as_str()).unwrap();
        assert_eq!((clerk["username"].as_str(), clerk["is_active"].as_bool(), clerk["role_ids"].clone()), (Some("clerk1"), Some(true), serde_json::json!([rid])));
    }

    #[tokio::test]
    async fn pin_sessions_and_suspension_are_handled_by_the_shared_gate() {
        let (app, writer, _) = make();
        let (id, secret, token) = owner_session(&app, &writer).await;
        let ob = bearer(&token);
        let h = authed(&id, &secret, &ob);
        post_json(&app, "/v1/me/pin", &h, serde_json::json!({ "current_password": PW, "pin": "2580" })).await;
        let (_, l) = post_json(&app, "/v1/auth/login", &creds(&id, &secret), serde_json::json!({ "username": "boss", "pin": "2580" })).await;
        let pb = bearer(l["token"].as_str().unwrap());
        let ph = authed(&id, &secret, &pb);
        // a PIN session can read role data but not change it
        assert_eq!(call(&app, req("GET", "/v1/roles", &ph, None)).await.0, StatusCode::OK);
        let (s, e) = post_json(&app, "/v1/roles", &ph, serde_json::json!({ "name": "X role", "permissions": [] })).await;
        assert_eq!((s, e["error"].as_str()), (StatusCode::FORBIDDEN, Some("password_login_required")));

        // suspended shop: reads continue, writes stop with 402
        apply_env(&writer, "suspended_rev2").unwrap();
        assert_eq!(call(&app, req("GET", "/v1/roles", &h, None)).await.0, StatusCode::OK);
        assert_eq!(call(&app, req("GET", "/v1/users", &h, None)).await.0, StatusCode::OK);
        let (s, e) = post_json(&app, "/v1/roles", &h, serde_json::json!({ "name": "Late role", "permissions": [] })).await;
        assert_eq!((s, e["error"].as_str(), e["reason"].as_str()), (StatusCode::PAYMENT_REQUIRED, Some("subscription_inactive"), Some("suspended")));
    }
}
