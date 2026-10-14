//! The single place that decides "may this signed-in user do this, here, right now?".
//!
//! Every endpoint that acts as a user calls [`check`] inside its database closure, so the decision and
//! the write happen on the same single-writer connection (nothing can change in between). Order:
//!   1. password sign-in, if the action is sensitive (PIN sessions are refused),
//!   2. permission, then location scope, then route scope (`Principal::authorize`, which mirrors the
//!      shared vectors in packages/test-vectors/permissions.json),
//!   3. for writes only, the subscription (`entitlement::decide`).
//! The subscription is checked last so that someone without permission learns nothing about it.
//! Reads are never blocked by the subscription (rule 6 in AGENTS.md: nothing is hidden on lapse).

use crate::{entitlement, ApiError};
use dsaas_db::{
    rusqlite::Connection,
    users::{Method, Principal, Scope},
};

#[derive(Debug, Clone, Copy)]
pub struct Requirement {
    /// `None` means "any signed-in user" (only scope is checked, e.g. listing the locations you can use).
    pub permission: Option<&'static str>,
    /// A business write: refused unless the subscription allows writing.
    pub write: bool,
    /// Needs a password sign-in (not a PIN session).
    pub password_session: bool,
}

impl Requirement {
    pub const fn read(permission: &'static str) -> Self {
        Self { permission: Some(permission), write: false, password_session: false }
    }
    /// Any signed-in user, read-only; location/route scope still applies.
    pub const fn signed_in() -> Self {
        Self { permission: None, write: false, password_session: false }
    }
    pub const fn write(permission: &'static str) -> Self {
        Self { permission: Some(permission), write: true, password_session: false }
    }
    /// Also require a password sign-in.
    pub const fn sensitive(self) -> Self {
        Self { password_session: true, ..self }
    }
}

/// What the caller learns from a successful check.
pub struct Granted {
    /// The signed plan, present only for writes (used for plan limits such as `max_users`).
    pub entitlement: Option<entitlement::Entitlement>,
}

pub fn check(
    c: &Connection,
    tenant: &str,
    pubkey: &[u8; 32],
    p: &Principal,
    req: &Requirement,
    scope: Scope,
) -> Result<Granted, ApiError> {
    if req.password_session && p.method != Method::Password {
        return Err(ApiError::PasswordRequired);
    }
    let allowed = match req.permission {
        Some(perm) => p.authorize(perm, scope),
        None => p.check_scope(scope),
    };
    if let Err(d) = allowed {
        return Err(ApiError::Forbidden(d.code()));
    }
    if !req.write {
        return Ok(Granted { entitlement: None });
    }
    let (dec, ent) = entitlement::decide(c, tenant, pubkey).map_err(|_| ApiError::Internal)?;
    if !dec.mode.can_write() {
        return Err(ApiError::SubscriptionInactive(dec.reason));
    }
    Ok(Granted { entitlement: ent })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsaas_db::users::Method;

    fn who(method: Method, perms: &[&str], locs: &[&str]) -> Principal {
        Principal {
            session_id: "s".into(),
            user_id: "u".into(),
            username: "u".into(),
            full_name: "U".into(),
            method,
            permissions: perms.iter().map(|x| x.to_string()).collect(),
            location_ids: locs.iter().map(|x| x.to_string()).collect(),
        }
    }

    /// A connection with no license at all: `decide` says "unlicensed", which cannot write.
    fn bare() -> Connection {
        let mut c = dsaas_db::open_in_memory().unwrap();
        dsaas_db::migrate(&mut c).unwrap();
        c
    }

    const PK: [u8; 32] = [0; 32];

    #[test]
    fn reads_need_only_the_permission_and_never_look_at_the_subscription() {
        let c = bare();
        let r = Requirement::read("roles.manage");
        assert!(check(&c, "t1", &PK, &who(Method::Pin, &["roles.manage"], &[]), &r, Scope::default()).is_ok());
        assert!(matches!(
            check(&c, "t1", &PK, &who(Method::Password, &["sales.view"], &[]), &r, Scope::default()),
            Err(ApiError::Forbidden("no_permission"))
        ));
    }

    #[test]
    fn signed_in_requirements_check_scope_only() {
        let c = bare();
        let r = Requirement::signed_in();
        let nobody = who(Method::Pin, &[], &["main"]);
        let at = |l: &'static str| Scope { location_id: Some(l), route_id: None };
        assert!(check(&c, "t1", &PK, &nobody, &r, Scope::default()).is_ok(), "no permission needed");
        assert!(check(&c, "t1", &PK, &nobody, &r, at("main")).is_ok());
        assert!(matches!(check(&c, "t1", &PK, &nobody, &r, at("g1")), Err(ApiError::Forbidden("location_denied"))));
    }

    #[test]
    fn writes_are_refused_without_a_license_but_only_after_the_permission_check() {
        let c = bare();
        let r = Requirement::write("users.manage");
        assert!(matches!(
            check(&c, "t1", &PK, &who(Method::Password, &["users.manage"], &[]), &r, Scope::default()),
            Err(ApiError::SubscriptionInactive("no_entitlement"))
        ));
        // no permission: the answer must not reveal the subscription state
        assert!(matches!(
            check(&c, "t1", &PK, &who(Method::Password, &[], &[]), &r, Scope::default()),
            Err(ApiError::Forbidden("no_permission"))
        ));
    }

    #[test]
    fn sensitive_actions_refuse_pin_sessions_first() {
        let c = bare();
        let r = Requirement::write("users.manage").sensitive();
        assert!(matches!(
            check(&c, "t1", &PK, &who(Method::Pin, &["*"], &[]), &r, Scope::default()),
            Err(ApiError::PasswordRequired)
        ));
    }

    #[test]
    fn location_scope_is_enforced_with_a_reason() {
        let c = bare();
        let r = Requirement::read("sales.view");
        let cashier = who(Method::Password, &["sales.view"], &["main"]);
        let at = |l: &'static str| Scope { location_id: Some(l), route_id: None };
        assert!(check(&c, "t1", &PK, &cashier, &r, at("main")).is_ok());
        assert!(matches!(check(&c, "t1", &PK, &cashier, &r, at("g1")), Err(ApiError::Forbidden("location_denied"))));
        let all = who(Method::Password, &["sales.view", "locations.all"], &[]);
        assert!(check(&c, "t1", &PK, &all, &r, at("g1")).is_ok());
        let routed = Scope { location_id: None, route_id: Some("r1") };
        assert!(matches!(check(&c, "t1", &PK, &cashier, &r, routed), Err(ApiError::Forbidden("route_denied"))));
    }
}
