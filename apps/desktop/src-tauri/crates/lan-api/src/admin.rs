//! User and role administration endpoints. Every handler goes through `authz::check`.
//!
//!   GET  /v1/permissions            roles.manage   the permission registry
//!   GET  /v1/roles                  roles.manage
//!   POST /v1/roles                  roles.manage   (password sign-in, subscription must allow writes)
//!   PUT  /v1/roles/:id              roles.manage   (same)
//!   DELETE /v1/roles/:id            roles.manage   (same)
//!   GET  /v1/users                  users.manage
//!   POST /v1/users                  users.manage   (same, and the plan's max_users)
//!   POST /v1/users/:id/deactivate   users.manage   (same)
//!   PUT  /v1/users/:id/roles        roles.manage   (same)
//!   PUT  /v1/users/:id/locations    users.manage   (same)
//! The no-escalation rules (you cannot grant what you do not hold) live in `dsaas_db::roles`.

use crate::authz::{self, Requirement};
use crate::{db, now_utc, user_err, ApiError, AppState, AuthedDevice};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post, put},
    Extension, Json, Router,
};
use dsaas_db::{
    roles,
    users::{self, Principal, Scope},
};
use serde::Deserialize;
use serde_json::{json, Value};

const READ_ROLES: Requirement = Requirement::read("roles.manage");
const WRITE_ROLES: Requirement = Requirement::write("roles.manage").sensitive();
const READ_USERS: Requirement = Requirement::read("users.manage");
const WRITE_USERS: Requirement = Requirement::write("users.manage").sensitive();

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/permissions", get(list_permissions))
        .route("/v1/roles", get(list_roles).post(create_role))
        .route("/v1/roles/:id", put(update_role).delete(delete_role))
        .route("/v1/users", get(list_users).post(create_user))
        .route("/v1/users/:id/deactivate", post(deactivate_user))
        .route("/v1/users/:id/roles", put(set_user_roles))
        .route("/v1/users/:id/locations", put(set_user_locations))
}

fn too_long(s: &str, max: usize) -> Result<(), ApiError> {
    if s.len() > max {
        Err(ApiError::BadRequest("value is too long"))
    } else {
        Ok(())
    }
}

async fn list_permissions(State(st): State<AppState>, Extension(p): Extension<Principal>) -> Result<Json<Value>, ApiError> {
    let tenant = st.tenant_id.to_string();
    let pk = st.license_pubkey;
    db(&st, move |c| Ok(authz::check(c, &tenant, &pk, &p, &READ_ROLES, Scope::default())))
        .await?
        .map(|_| {
            Json(json!(roles::PERMISSIONS
                .iter()
                .map(|(k, d)| json!({ "permission": k, "description": d }))
                .collect::<Vec<_>>()))
        })
}

async fn list_roles(State(st): State<AppState>, Extension(p): Extension<Principal>) -> Result<Json<Value>, ApiError> {
    let tenant = st.tenant_id.to_string();
    let pk = st.license_pubkey;
    let rows = db(&st, move |c| {
        if let Err(e) = authz::check(c, &tenant, &pk, &p, &READ_ROLES, Scope::default()) {
            return Ok(Err(e));
        }
        Ok(Ok(roles::list_roles(c, &tenant)?))
    })
    .await??;
    Ok(Json(json!(rows
        .into_iter()
        .map(|r| json!({ "id": r.id, "name": r.name, "permissions": r.permissions, "member_count": r.member_count, "protected": r.protected }))
        .collect::<Vec<_>>())))
}

#[derive(Deserialize)]
struct RoleReq {
    name: String,
    permissions: Vec<String>,
}

async fn create_role(
    State(st): State<AppState>,
    Extension(d): Extension<AuthedDevice>,
    Extension(p): Extension<Principal>,
    Json(req): Json<RoleReq>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    too_long(&req.name, 200)?;
    let tenant = st.tenant_id.to_string();
    let pk = st.license_pubkey;
    let id = db(&st, move |c| {
        if let Err(e) = authz::check(c, &tenant, &pk, &p, &WRITE_ROLES, Scope::default()) {
            return Ok(Err(e));
        }
        let now = now_utc(c)?;
        Ok(roles::create_role(c, &tenant, &p, Some(&d.0), &req.name, &req.permissions, &now).map_err(user_err))
    })
    .await??;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

async fn update_role(
    State(st): State<AppState>,
    Extension(d): Extension<AuthedDevice>,
    Extension(p): Extension<Principal>,
    Path(role_id): Path<String>,
    Json(req): Json<RoleReq>,
) -> Result<StatusCode, ApiError> {
    too_long(&req.name, 200)?;
    let tenant = st.tenant_id.to_string();
    let pk = st.license_pubkey;
    db(&st, move |c| {
        if let Err(e) = authz::check(c, &tenant, &pk, &p, &WRITE_ROLES, Scope::default()) {
            return Ok(Err(e));
        }
        let now = now_utc(c)?;
        Ok(roles::update_role(c, &tenant, &p, Some(&d.0), &role_id, &req.name, &req.permissions, &now).map_err(user_err))
    })
    .await??;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_role(
    State(st): State<AppState>,
    Extension(d): Extension<AuthedDevice>,
    Extension(p): Extension<Principal>,
    Path(role_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let tenant = st.tenant_id.to_string();
    let pk = st.license_pubkey;
    db(&st, move |c| {
        if let Err(e) = authz::check(c, &tenant, &pk, &p, &WRITE_ROLES, Scope::default()) {
            return Ok(Err(e));
        }
        let now = now_utc(c)?;
        Ok(roles::delete_role(c, &tenant, &p, Some(&d.0), &role_id, &now).map_err(user_err))
    })
    .await??;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_users(State(st): State<AppState>, Extension(p): Extension<Principal>) -> Result<Json<Value>, ApiError> {
    let tenant = st.tenant_id.to_string();
    let pk = st.license_pubkey;
    let rows = db(&st, move |c| {
        if let Err(e) = authz::check(c, &tenant, &pk, &p, &READ_USERS, Scope::default()) {
            return Ok(Err(e));
        }
        Ok(Ok(roles::list_users(c, &tenant)?))
    })
    .await??;
    Ok(Json(json!(rows
        .into_iter()
        .map(|u| json!({
            "id": u.id, "username": u.username, "full_name": u.full_name, "is_active": u.is_active,
            "role_ids": u.role_ids, "location_ids": u.location_ids,
        }))
        .collect::<Vec<_>>())))
}

#[derive(Deserialize)]
struct CreateUserReq {
    username: String,
    full_name: String,
    password: String,
}

async fn create_user(
    State(st): State<AppState>,
    Extension(d): Extension<AuthedDevice>,
    Extension(p): Extension<Principal>,
    Json(req): Json<CreateUserReq>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    too_long(&req.username, 64)?;
    too_long(&req.full_name, 256)?;
    too_long(&req.password, 512)?;
    let tenant = st.tenant_id.to_string();
    let pk = st.license_pubkey;
    let id = db(&st, move |c| {
        let granted = match authz::check(c, &tenant, &pk, &p, &WRITE_USERS, Scope::default()) {
            Ok(g) => g,
            Err(e) => return Ok(Err(e)),
        };
        let max = granted.entitlement.map(|e| e.limits.max_users).unwrap_or(0);
        if users::count_active_users(c, &tenant)? >= max {
            return Ok(Err(ApiError::UserLimit));
        }
        let now = now_utc(c)?;
        Ok(users::create_user(c, &tenant, Some(&p.user_id), Some(&d.0), &req.username, &req.full_name, &req.password, &now)
            .map_err(user_err))
    })
    .await??;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

async fn deactivate_user(
    State(st): State<AppState>,
    Extension(d): Extension<AuthedDevice>,
    Extension(p): Extension<Principal>,
    Path(user_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let tenant = st.tenant_id.to_string();
    let pk = st.license_pubkey;
    db(&st, move |c| {
        if let Err(e) = authz::check(c, &tenant, &pk, &p, &WRITE_USERS, Scope::default()) {
            return Ok(Err(e));
        }
        let now = now_utc(c)?;
        Ok(users::deactivate_user(c, &tenant, &p.user_id, Some(&d.0), &user_id, &now).map_err(user_err))
    })
    .await??;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct UserRolesReq {
    role_ids: Vec<String>,
}

async fn set_user_roles(
    State(st): State<AppState>,
    Extension(d): Extension<AuthedDevice>,
    Extension(p): Extension<Principal>,
    Path(user_id): Path<String>,
    Json(req): Json<UserRolesReq>,
) -> Result<StatusCode, ApiError> {
    let tenant = st.tenant_id.to_string();
    let pk = st.license_pubkey;
    db(&st, move |c| {
        if let Err(e) = authz::check(c, &tenant, &pk, &p, &WRITE_ROLES, Scope::default()) {
            return Ok(Err(e));
        }
        let now = now_utc(c)?;
        Ok(roles::set_user_roles(c, &tenant, &p, Some(&d.0), &user_id, &req.role_ids, &now).map_err(user_err))
    })
    .await??;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct UserLocationsReq {
    location_ids: Vec<String>,
}

async fn set_user_locations(
    State(st): State<AppState>,
    Extension(d): Extension<AuthedDevice>,
    Extension(p): Extension<Principal>,
    Path(user_id): Path<String>,
    Json(req): Json<UserLocationsReq>,
) -> Result<StatusCode, ApiError> {
    let tenant = st.tenant_id.to_string();
    let pk = st.license_pubkey;
    db(&st, move |c| {
        if let Err(e) = authz::check(c, &tenant, &pk, &p, &WRITE_USERS, Scope::default()) {
            return Ok(Err(e));
        }
        let now = now_utc(c)?;
        Ok(roles::set_user_locations(c, &tenant, &p, Some(&d.0), &user_id, &req.location_ids, &now).map_err(user_err))
    })
    .await??;
    Ok(StatusCode::NO_CONTENT)
}
