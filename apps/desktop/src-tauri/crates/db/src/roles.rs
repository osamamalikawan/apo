//! Roles, permissions and per-user scope for the Shop Server.
//!
//! * A **role** is a named set of permission strings; a user can hold several roles and gets the
//!   union. Permissions are re-read on every request (`users::authenticate_session`), so a change
//!   takes effect immediately, with no need to sign anyone out.
//! * Permission strings must come from [`PERMISSIONS`] (or be `*` / `module.*`), so a typo such as
//!   `inventory.transfr` is rejected instead of silently granting nothing.
//! * **No escalation:** you can only grant, edit or take away what you hold yourself. Someone with
//!   `roles.manage` but not `*` can never create an Owner-like role, hand one out, or change one.
//! * The **Owner** role (created by `users::grant_owner`) is protected: it cannot be renamed,
//!   edited or deleted through this module, and at least one active user must always hold `*`.
//! * Location scope: `user_location_access` lists the locations a user may use unless they hold
//!   `locations.all`. You can only change location access within locations you can use yourself.
//!
//! Every function runs in one transaction, so a refused change leaves nothing half-applied.

use crate::users::{audit_entity, has_permission, Principal, UserError};
use crate::DbError;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::collections::BTreeSet;

/// Every permission the server knows. Extend this list when a module adds one; the vectors test
/// below fails if the shared permission vectors use a string that is missing here.
pub const PERMISSIONS: &[(&str, &str)] = &[
    ("users.manage", "Create and deactivate users, set their location access"),
    ("roles.manage", "Create and edit roles, assign roles to users"),
    ("locations.all", "Use every stock location"),
    ("locations.manage", "Create and edit stock locations"),
    ("products.view", "View products"),
    ("products.manage", "Create and edit products"),
    ("customers.view", "View customers"),
    ("customers.manage", "Create and edit customers"),
    ("inventory.view", "View stock"),
    ("inventory.transfer", "Move stock between locations"),
    ("inventory.adjust", "Adjust stock quantities"),
    ("inventory.count", "Run stock counts"),
    ("sales.view", "View sales"),
    ("sales.create", "Create sales"),
    ("payments.receive", "Record payments"),
    ("routes.all", "Use every route"),
    ("reports.view", "View reports"),
];

pub const OWNER_ROLE: &str = "Owner";
const MAX_PERMISSIONS: usize = 100;
const MAX_LOCATIONS: usize = 500;

fn known_module(m: &str) -> bool {
    PERMISSIONS.iter().any(|(p, _)| p.split('.').next() == Some(m))
}

/// Validate, de-duplicate and sort a list of permission strings.
pub fn normalize_permissions(input: &[String]) -> Result<Vec<String>, UserError> {
    if input.len() > MAX_PERMISSIONS {
        return Err(UserError::BadInput("too many permissions"));
    }
    let mut out = BTreeSet::new();
    for raw in input {
        let p = raw.trim();
        let ok = p == "*"
            || match p.strip_suffix(".*") {
                Some(module) => known_module(module),
                None => PERMISSIONS.iter().any(|(k, _)| *k == p),
            };
        if !ok {
            return Err(UserError::BadInput("unknown permission"));
        }
        out.insert(p.to_string());
    }
    Ok(out.into_iter().collect())
}

/// Does `granted` include everything `p` stands for? `*` and `module.*` are only covered by `*` or
/// the identical string; a plain permission is covered the normal way.
fn covers(granted: &[String], p: &str) -> bool {
    if p == "*" {
        granted.iter().any(|g| g == "*")
    } else if p.ends_with(".*") {
        granted.iter().any(|g| g == "*" || g == p)
    } else {
        has_permission(granted, p)
    }
}

fn require_covers(actor: &Principal, perms: &[String]) -> Result<(), UserError> {
    if perms.iter().all(|p| covers(&actor.permissions, p)) {
        Ok(())
    } else {
        Err(UserError::Forbidden("cannot_grant_more_than_you_hold"))
    }
}

fn normalize_role_name(raw: &str) -> Result<String, UserError> {
    let n = raw.trim();
    if !(2..=40).contains(&n.chars().count()) || n.chars().any(|c| c.is_control()) {
        return Err(UserError::BadInput("role name must be 2 to 40 characters"));
    }
    Ok(n.to_string())
}

fn is_owner_name(n: &str) -> bool {
    n.eq_ignore_ascii_case(OWNER_ROLE)
}

fn role_perms(tx: &Connection, tenant: &str, role_id: &str) -> Result<Vec<String>, rusqlite::Error> {
    let mut st = tx.prepare("SELECT permission FROM role_permissions WHERE tenant_id=?1 AND role_id=?2 ORDER BY permission")?;
    let rows = st.query_map(params![tenant, role_id], |r| r.get::<_, String>(0))?;
    rows.collect()
}

fn role_name(tx: &Connection, tenant: &str, role_id: &str) -> Result<Option<String>, rusqlite::Error> {
    tx.query_row("SELECT name FROM roles WHERE tenant_id=?1 AND id=?2", params![tenant, role_id], |r| r.get(0)).optional()
}

fn name_taken(tx: &Connection, tenant: &str, name: &str, except: Option<&str>) -> Result<bool, rusqlite::Error> {
    tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM roles WHERE tenant_id=?1 AND lower(name)=lower(?2) AND id<>COALESCE(?3,''))",
        params![tenant, name, except],
        |r| r.get(0),
    )
}

fn active_owner_count(tx: &Connection, tenant: &str) -> Result<i64, rusqlite::Error> {
    tx.query_row(
        "SELECT COUNT(DISTINCT u.id) FROM users u
         JOIN user_roles ur ON ur.tenant_id=u.tenant_id AND ur.user_id=u.id
         JOIN role_permissions rp ON rp.tenant_id=ur.tenant_id AND rp.role_id=ur.role_id
         WHERE u.tenant_id=?1 AND u.is_active=1 AND rp.permission='*'",
        [tenant],
        |r| r.get(0),
    )
}

fn replace_role_perms(tx: &Transaction, tenant: &str, role_id: &str, perms: &[String]) -> Result<(), rusqlite::Error> {
    tx.execute("DELETE FROM role_permissions WHERE tenant_id=?1 AND role_id=?2", params![tenant, role_id])?;
    for p in perms {
        tx.execute(
            "INSERT INTO role_permissions(tenant_id,role_id,permission) VALUES (?1,?2,?3)",
            params![tenant, role_id, p],
        )?;
    }
    Ok(())
}

// ---------- roles ----------

pub fn create_role(
    conn: &mut Connection,
    tenant: &str,
    actor: &Principal,
    device: Option<&str>,
    name: &str,
    permissions: &[String],
    now: &str,
) -> Result<String, UserError> {
    let name = normalize_role_name(name)?;
    if is_owner_name(&name) {
        return Err(UserError::BadInput("that role name is reserved"));
    }
    let perms = normalize_permissions(permissions)?;
    require_covers(actor, &perms)?;
    let tx = conn.transaction()?;
    if name_taken(&tx, tenant, &name, None)? {
        return Err(UserError::Conflict("a role with that name already exists"));
    }
    let id = uuid::Uuid::new_v4().to_string();
    tx.execute("INSERT INTO roles(id,tenant_id,name) VALUES (?1,?2,?3)", params![id, tenant, name])?;
    replace_role_perms(&tx, tenant, &id, &perms)?;
    let after = serde_json::json!({ "name": name, "permissions": perms }).to_string();
    audit_entity(&tx, tenant, Some(&actor.user_id), device, "role.create", "role", Some(&id), None, Some(&after), now)?;
    tx.commit()?;
    Ok(id)
}

/// Replace a role's name and permissions. You must hold both the old and the new permissions.
pub fn update_role(
    conn: &mut Connection,
    tenant: &str,
    actor: &Principal,
    device: Option<&str>,
    role_id: &str,
    name: &str,
    permissions: &[String],
    now: &str,
) -> Result<(), UserError> {
    let name = normalize_role_name(name)?;
    let perms = normalize_permissions(permissions)?;
    let tx = conn.transaction()?;
    let old_name = role_name(&tx, tenant, role_id)?.ok_or(UserError::NotFound)?;
    if is_owner_name(&old_name) || is_owner_name(&name) {
        return Err(UserError::Conflict("the Owner role cannot be changed"));
    }
    let old_perms = role_perms(&tx, tenant, role_id)?;
    require_covers(actor, &old_perms)?;
    require_covers(actor, &perms)?;
    if name_taken(&tx, tenant, &name, Some(role_id))? {
        return Err(UserError::Conflict("a role with that name already exists"));
    }
    tx.execute("UPDATE roles SET name=?3 WHERE tenant_id=?1 AND id=?2", params![tenant, role_id, name])?;
    replace_role_perms(&tx, tenant, role_id, &perms)?;
    if active_owner_count(&tx, tenant)? == 0 {
        return Err(UserError::Conflict("cannot remove the last owner")); // dropped tx rolls back
    }
    let before = serde_json::json!({ "name": old_name, "permissions": old_perms }).to_string();
    let after = serde_json::json!({ "name": name, "permissions": perms }).to_string();
    audit_entity(&tx, tenant, Some(&actor.user_id), device, "role.update", "role", Some(role_id), Some(&before), Some(&after), now)?;
    tx.commit()?;
    Ok(())
}

/// Delete a role that nobody holds.
pub fn delete_role(
    conn: &mut Connection,
    tenant: &str,
    actor: &Principal,
    device: Option<&str>,
    role_id: &str,
    now: &str,
) -> Result<(), UserError> {
    let tx = conn.transaction()?;
    let name = role_name(&tx, tenant, role_id)?.ok_or(UserError::NotFound)?;
    if is_owner_name(&name) {
        return Err(UserError::Conflict("the Owner role cannot be changed"));
    }
    let perms = role_perms(&tx, tenant, role_id)?;
    require_covers(actor, &perms)?;
    let in_use: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM user_roles WHERE tenant_id=?1 AND role_id=?2)",
        params![tenant, role_id],
        |r| r.get(0),
    )?;
    if in_use {
        return Err(UserError::Conflict("role is assigned to users; remove it from them first"));
    }
    tx.execute("DELETE FROM role_permissions WHERE tenant_id=?1 AND role_id=?2", params![tenant, role_id])?;
    tx.execute("DELETE FROM roles WHERE tenant_id=?1 AND id=?2", params![tenant, role_id])?;
    let before = serde_json::json!({ "name": name, "permissions": perms }).to_string();
    audit_entity(&tx, tenant, Some(&actor.user_id), device, "role.delete", "role", Some(role_id), Some(&before), None, now)?;
    tx.commit()?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleInfo {
    pub id: String,
    pub name: String,
    pub permissions: Vec<String>,
    pub member_count: i64,
    pub protected: bool,
}

pub fn list_roles(conn: &Connection, tenant: &str) -> Result<Vec<RoleInfo>, DbError> {
    let mut st = conn.prepare(
        "SELECT r.id, r.name, (SELECT COUNT(*) FROM user_roles ur WHERE ur.tenant_id=r.tenant_id AND ur.role_id=r.id)
         FROM roles r WHERE r.tenant_id=?1 ORDER BY lower(r.name)",
    )?;
    let rows = st.query_map([tenant], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?;
    let mut out = Vec::new();
    for row in rows {
        let (id, name, member_count) = row?;
        let permissions = role_perms(conn, tenant, &id)?;
        let protected = is_owner_name(&name);
        out.push(RoleInfo { id, name, permissions, member_count, protected });
    }
    Ok(out)
}

// ---------- users: roles and locations ----------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInfo {
    pub id: String,
    pub username: String,
    pub full_name: String,
    pub is_active: bool,
    pub role_ids: Vec<String>,
    pub location_ids: Vec<String>,
}

/// Users with their role ids and location ids. Never includes any password or PIN material.
pub fn list_users(conn: &Connection, tenant: &str) -> Result<Vec<UserInfo>, DbError> {
    let mut st = conn.prepare("SELECT id, username, full_name, is_active FROM users WHERE tenant_id=?1 ORDER BY username")?;
    let rows = st.query_map([tenant], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)))?;
    let mut out = Vec::new();
    for row in rows {
        let (id, username, full_name, active) = row?;
        let role_ids = user_role_ids(conn, tenant, &id)?.into_iter().collect();
        let location_ids = user_location_ids(conn, tenant, &id)?.into_iter().collect();
        out.push(UserInfo { id, username, full_name, is_active: active == 1, role_ids, location_ids });
    }
    Ok(out)
}

fn user_role_ids(c: &Connection, tenant: &str, user_id: &str) -> Result<BTreeSet<String>, rusqlite::Error> {
    let mut st = c.prepare("SELECT role_id FROM user_roles WHERE tenant_id=?1 AND user_id=?2")?;
    let rows = st.query_map(params![tenant, user_id], |r| r.get::<_, String>(0))?;
    rows.collect()
}

fn user_location_ids(c: &Connection, tenant: &str, user_id: &str) -> Result<BTreeSet<String>, rusqlite::Error> {
    let mut st = c.prepare("SELECT location_id FROM user_location_access WHERE tenant_id=?1 AND user_id=?2")?;
    let rows = st.query_map(params![tenant, user_id], |r| r.get::<_, String>(0))?;
    rows.collect()
}

fn user_exists(tx: &Connection, tenant: &str, user_id: &str) -> Result<bool, rusqlite::Error> {
    tx.query_row("SELECT EXISTS(SELECT 1 FROM users WHERE tenant_id=?1 AND id=?2)", params![tenant, user_id], |r| r.get(0))
}

/// Replace the set of roles a user holds. Every role added or removed must be one you fully hold.
pub fn set_user_roles(
    conn: &mut Connection,
    tenant: &str,
    actor: &Principal,
    device: Option<&str>,
    user_id: &str,
    role_ids: &[String],
    now: &str,
) -> Result<(), UserError> {
    if role_ids.len() > 50 {
        return Err(UserError::BadInput("too many roles"));
    }
    let wanted: BTreeSet<String> = role_ids.iter().cloned().collect();
    let tx = conn.transaction()?;
    if !user_exists(&tx, tenant, user_id)? {
        return Err(UserError::NotFound);
    }
    for r in &wanted {
        if role_name(&tx, tenant, r)?.is_none() {
            return Err(UserError::BadInput("unknown role"));
        }
    }
    let current = user_role_ids(&tx, tenant, user_id)?;
    let changed: Vec<&String> = wanted.symmetric_difference(&current).collect();
    for r in &changed {
        require_covers(actor, &role_perms(&tx, tenant, r)?)?;
    }
    for r in wanted.difference(&current) {
        tx.execute("INSERT INTO user_roles(tenant_id,user_id,role_id) VALUES (?1,?2,?3)", params![tenant, user_id, r])?;
    }
    for r in current.difference(&wanted) {
        tx.execute("DELETE FROM user_roles WHERE tenant_id=?1 AND user_id=?2 AND role_id=?3", params![tenant, user_id, r])?;
    }
    if active_owner_count(&tx, tenant)? == 0 {
        return Err(UserError::Conflict("cannot remove the last owner")); // dropped tx rolls back
    }
    let before = serde_json::json!({ "role_ids": current }).to_string();
    let after = serde_json::json!({ "role_ids": wanted }).to_string();
    audit_entity(&tx, tenant, Some(&actor.user_id), device, "user.roles_set", "user", Some(user_id), Some(&before), Some(&after), now)?;
    tx.commit()?;
    Ok(())
}

/// Replace the locations a user may use (irrelevant to users holding `locations.all`).
/// Unless you hold `locations.all` yourself, you may only add or remove locations you can use.
pub fn set_user_locations(
    conn: &mut Connection,
    tenant: &str,
    actor: &Principal,
    device: Option<&str>,
    user_id: &str,
    location_ids: &[String],
    now: &str,
) -> Result<(), UserError> {
    if location_ids.len() > MAX_LOCATIONS {
        return Err(UserError::BadInput("too many locations"));
    }
    let wanted: BTreeSet<String> = location_ids.iter().cloned().collect();
    let tx = conn.transaction()?;
    if !user_exists(&tx, tenant, user_id)? {
        return Err(UserError::NotFound);
    }
    for l in &wanted {
        let ok: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM stock_locations WHERE tenant_id=?1 AND id=?2)",
            params![tenant, l],
            |r| r.get(0),
        )?;
        if !ok {
            return Err(UserError::BadInput("unknown location"));
        }
    }
    let current = user_location_ids(&tx, tenant, user_id)?;
    if !has_permission(&actor.permissions, "locations.all") {
        let mine: BTreeSet<&String> = actor.location_ids.iter().collect();
        if wanted.symmetric_difference(&current).any(|l| !mine.contains(l)) {
            return Err(UserError::Forbidden("cannot_grant_more_than_you_hold"));
        }
    }
    tx.execute("DELETE FROM user_location_access WHERE tenant_id=?1 AND user_id=?2", params![tenant, user_id])?;
    for l in &wanted {
        tx.execute("INSERT INTO user_location_access(tenant_id,user_id,location_id) VALUES (?1,?2,?3)", params![tenant, user_id, l])?;
    }
    let before = serde_json::json!({ "location_ids": current }).to_string();
    let after = serde_json::json!({ "location_ids": wanted }).to_string();
    audit_entity(&tx, tenant, Some(&actor.user_id), device, "user.locations_set", "user", Some(user_id), Some(&before), Some(&after), now)?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::users::{authenticate_session, create_user, grant_owner, login, Method};
    use crate::{migrate, open_in_memory};

    const T0: &str = "2030-01-01T00:00:00.000Z";
    const PW: &str = "correct horse battery";

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    /// Tenant t1 with locations main/g1/g2, device d1 and an owner "boss".
    fn setup() -> (Connection, Principal) {
        let mut c = open_in_memory().unwrap();
        migrate(&mut c).unwrap();
        c.execute_batch(
            "INSERT INTO tenants(id,name) VALUES ('t1','Shop');
             INSERT INTO devices(id,tenant_id,name,kind,credential_hash) VALUES ('d1','t1','PC','desktop_client','h');
             INSERT INTO stock_locations(id,tenant_id,name,code) VALUES
               ('main','t1','Main','MAIN'),('g1','t1','Godown 1','G1'),('g2','t1','Godown 2','G2');",
        )
        .unwrap();
        let boss = create_user(&mut c, "t1", None, None, "boss", "Boss", PW, T0).unwrap();
        grant_owner(&mut c, "t1", &boss).unwrap();
        let p = principal(&mut c, "boss");
        (c, p)
    }

    /// Sign in as `name` (all test users share PW) and resolve the session, exactly as the API does.
    fn principal(c: &mut Connection, name: &str) -> Principal {
        let ok = login(c, "t1", "d1", name, PW, Method::Password, T0).unwrap();
        authenticate_session(c, "t1", "d1", &ok.token, T0).unwrap().unwrap()
    }

    fn user(c: &mut Connection, by: &Principal, name: &str) -> String {
        create_user(c, "t1", Some(&by.user_id), None, name, name, PW, T0).unwrap()
    }

    fn role(c: &mut Connection, by: &Principal, name: &str, perms: &[&str]) -> String {
        create_role(c, "t1", by, None, name, &s(perms), T0).unwrap()
    }

    /// A user holding the given role, signed in.
    fn user_with(c: &mut Connection, boss: &Principal, name: &str, perms: &[&str]) -> Principal {
        let u = user(c, boss, name);
        let r = role(c, boss, &format!("{name}-role"), perms);
        set_user_roles(c, "t1", boss, None, &u, &[r], T0).unwrap();
        principal(c, name)
    }

    #[test]
    fn registry_is_well_formed_and_covers_every_permission_in_the_shared_vectors() {
        let mut seen = BTreeSet::new();
        for (p, d) in PERMISSIONS {
            assert!(p.split('.').count() == 2 && !d.is_empty(), "{p}");
            assert!(seen.insert(*p), "duplicate {p}");
        }
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../../../../../../packages/test-vectors/permissions.json")).unwrap();
        // Granted permissions and permissions expected to be allowed must be real. Denied cases may use
        // look-alikes on purpose (for example "sales.createx" must not match "sales.create").
        let mut used: Vec<String> = v["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["allowed"].as_bool() == Some(true))
            .map(|c| c["perm"].as_str().unwrap().into())
            .collect();
        for ctx in v["contexts"].as_object().unwrap().values() {
            used.extend(ctx["permissions"].as_array().unwrap().iter().map(|p| p.as_str().unwrap().to_string()));
        }
        assert!(normalize_permissions(&used).is_ok(), "a permission used by the vectors is missing from PERMISSIONS");
    }

    #[test]
    fn permission_strings_are_validated_sorted_and_deduplicated() {
        assert_eq!(
            normalize_permissions(&s(&["sales.create", " inventory.* ", "sales.create", "*"])).unwrap(),
            s(&["*", "inventory.*", "sales.create"])
        );
        for bad in ["inventory.transfr", "nonsense.*", "sales", "", "sales.create.extra", "INVENTORY.VIEW"] {
            assert!(normalize_permissions(&s(&[bad])).is_err(), "{bad}");
        }
        assert!(normalize_permissions(&[]).unwrap().is_empty());
    }

    #[test]
    fn creating_roles_validates_and_rejects_duplicates_and_the_reserved_name() {
        let (mut c, boss) = setup();
        let id = role(&mut c, &boss, "Cashier", &["sales.create", "inventory.view"]);
        assert_eq!(list_roles(&c, "t1").unwrap().iter().find(|r| r.id == id).unwrap().permissions, s(&["inventory.view", "sales.create"]));
        let mk = |c: &mut Connection, n: &str, p: &[&str]| create_role(c, "t1", &boss, None, n, &s(p), T0);
        assert!(matches!(mk(&mut c, "CASHIER", &[]), Err(UserError::Conflict(_))), "case-insensitive");
        assert!(matches!(mk(&mut c, "owner", &[]), Err(UserError::BadInput(_))));
        assert!(matches!(mk(&mut c, "x", &[]), Err(UserError::BadInput(_))));
        assert!(matches!(mk(&mut c, "Typo", &["inventory.transfr"]), Err(UserError::BadInput(_))));
        assert!(mk(&mut c, "Empty role", &[]).is_ok());
    }

    #[test]
    fn nobody_can_grant_more_than_they_hold() {
        let (mut c, boss) = setup();
        let mgr = user_with(&mut c, &boss, "mgr", &["roles.manage", "users.manage", "inventory.*"]);
        let mk = |c: &mut Connection, n: &str, p: &[&str]| create_role(c, "t1", &mgr, None, n, &s(p), T0);
        // within their own permissions: fine
        assert!(mk(&mut c, "Stockman", &["inventory.view", "inventory.transfer"]).is_ok());
        assert!(mk(&mut c, "Stock wildcard", &["inventory.*"]).is_ok());
        // beyond them: refused, including wildcards and the Owner-style "*"
        for p in ["sales.create", "*", "users.*", "locations.all", "payments.receive"] {
            assert!(matches!(mk(&mut c, "Sneaky", &[p]), Err(UserError::Forbidden(_))), "{p}");
        }
        // they cannot hand out, edit or delete a role stronger than themselves
        let sales = role(&mut c, &boss, "Sales", &["sales.create"]);
        let u = user(&mut c, &boss, "clerk");
        assert!(matches!(set_user_roles(&mut c, "t1", &mgr, None, &u, &[sales.clone()], T0), Err(UserError::Forbidden(_))));
        assert!(matches!(update_role(&mut c, "t1", &mgr, None, &sales, "Sales", &s(&["inventory.view"]), T0), Err(UserError::Forbidden(_))));
        assert!(matches!(delete_role(&mut c, "t1", &mgr, None, &sales, T0), Err(UserError::Forbidden(_))));
        // and cannot sneak the Owner role in
        let owner_role = list_roles(&c, "t1").unwrap().into_iter().find(|r| r.protected).unwrap().id;
        assert!(matches!(set_user_roles(&mut c, "t1", &mgr, None, &u, &[owner_role], T0), Err(UserError::Forbidden(_))));
        // nothing leaked through the refusals
        assert!(user_role_ids(&c, "t1", &u).unwrap().is_empty());
    }

    #[test]
    fn role_changes_apply_immediately_to_signed_in_users() {
        let (mut c, boss) = setup();
        let u = user(&mut c, &boss, "clerk");
        let r = role(&mut c, &boss, "Clerk", &["sales.view"]);
        set_user_roles(&mut c, "t1", &boss, None, &u, &[r.clone()], T0).unwrap();
        let ok = login(&mut c, "t1", "d1", "clerk", PW, Method::Password, T0).unwrap();
        let perms = |c: &Connection| authenticate_session(c, "t1", "d1", &ok.token, T0).unwrap().unwrap().permissions;
        assert_eq!(perms(&c), s(&["sales.view"]));
        update_role(&mut c, "t1", &boss, None, &r, "Clerk", &s(&["sales.view", "sales.create"]), T0).unwrap();
        assert_eq!(perms(&c), s(&["sales.create", "sales.view"]), "same session, new permissions");
        set_user_roles(&mut c, "t1", &boss, None, &u, &[], T0).unwrap();
        assert!(perms(&c).is_empty());
    }

    #[test]
    fn the_owner_role_is_protected_and_the_last_owner_cannot_be_removed() {
        let (mut c, boss) = setup();
        let owner_role = list_roles(&c, "t1").unwrap().into_iter().find(|r| r.protected).unwrap();
        assert_eq!(owner_role.permissions, s(&["*"]));
        assert!(matches!(update_role(&mut c, "t1", &boss, None, &owner_role.id, "Owner", &s(&["sales.view"]), T0), Err(UserError::Conflict(_))));
        assert!(matches!(update_role(&mut c, "t1", &boss, None, &owner_role.id, "Boss role", &s(&["*"]), T0), Err(UserError::Conflict(_))));
        assert!(matches!(delete_role(&mut c, "t1", &boss, None, &owner_role.id, T0), Err(UserError::Conflict(_))));
        // the only owner cannot drop their own Owner role
        assert!(matches!(set_user_roles(&mut c, "t1", &boss, None, &boss.user_id, &[], T0), Err(UserError::Conflict(_))));
        assert_eq!(user_role_ids(&c, "t1", &boss.user_id).unwrap().len(), 1, "refusal rolled back");
        // with a second owner it is allowed, and then the second one is the last
        let two = user(&mut c, &boss, "boss2");
        set_user_roles(&mut c, "t1", &boss, None, &two, &[owner_role.id.clone()], T0).unwrap();
        set_user_roles(&mut c, "t1", &boss, None, &boss.user_id, &[], T0).unwrap();
        let boss2 = principal(&mut c, "boss2");
        assert!(matches!(set_user_roles(&mut c, "t1", &boss2, None, &two, &[], T0), Err(UserError::Conflict(_))));
        // boss2 is now the only owner, but can add another one (and then step down)
        let extra = user(&mut c, &boss2, "boss3");
        assert!(set_user_roles(&mut c, "t1", &boss2, None, &extra, &[owner_role.id], T0).is_ok());
    }

    #[test]
    fn a_custom_star_role_can_be_edited_while_a_real_owner_remains() {
        let (mut c, boss) = setup();
        // a custom role with "*" makes its holder an owner until it is edited; the Owner role still protects the tenant
        let star = role(&mut c, &boss, "Super", &["*"]);
        let u = user(&mut c, &boss, "super1");
        set_user_roles(&mut c, "t1", &boss, None, &u, &[star.clone()], T0).unwrap();
        update_role(&mut c, "t1", &boss, None, &star, "Super", &s(&["sales.view"]), T0).unwrap(); // boss still owner: fine
        assert_eq!(list_roles(&c, "t1").unwrap().iter().find(|r| r.id == star).unwrap().permissions, s(&["sales.view"]));
    }

    #[test]
    fn roles_in_use_cannot_be_deleted_and_unused_ones_can() {
        let (mut c, boss) = setup();
        let r = role(&mut c, &boss, "Temp", &["sales.view"]);
        let u = user(&mut c, &boss, "clerk");
        set_user_roles(&mut c, "t1", &boss, None, &u, &[r.clone()], T0).unwrap();
        assert!(matches!(delete_role(&mut c, "t1", &boss, None, &r, T0), Err(UserError::Conflict(_))));
        set_user_roles(&mut c, "t1", &boss, None, &u, &[], T0).unwrap();
        delete_role(&mut c, "t1", &boss, None, &r, T0).unwrap();
        assert!(list_roles(&c, "t1").unwrap().iter().all(|x| x.id != r));
        assert!(matches!(delete_role(&mut c, "t1", &boss, None, &r, T0), Err(UserError::NotFound)));
        let left: i64 = c.query_row("SELECT COUNT(*) FROM role_permissions WHERE role_id=?1", [&r], |x| x.get(0)).unwrap();
        assert_eq!(left, 0);
    }

    #[test]
    fn assigning_roles_validates_users_and_roles() {
        let (mut c, boss) = setup();
        let r = role(&mut c, &boss, "Clerk", &["sales.view"]);
        assert!(matches!(set_user_roles(&mut c, "t1", &boss, None, "ghost", &[r.clone()], T0), Err(UserError::NotFound)));
        let u = user(&mut c, &boss, "clerk");
        assert!(matches!(set_user_roles(&mut c, "t1", &boss, None, &u, &["no-such-role".into()], T0), Err(UserError::BadInput(_))));
        set_user_roles(&mut c, "t1", &boss, None, &u, &[r.clone(), r.clone()], T0).unwrap(); // duplicates collapse
        assert_eq!(user_role_ids(&c, "t1", &u).unwrap().len(), 1);
        set_user_roles(&mut c, "t1", &boss, None, &u, &[r], T0).unwrap(); // no change is fine
    }

    #[test]
    fn location_access_is_validated_and_limited_to_what_the_actor_can_use() {
        let (mut c, boss) = setup();
        let u = user(&mut c, &boss, "worker");
        assert!(matches!(set_user_locations(&mut c, "t1", &boss, None, &u, &["nowhere".into()], T0), Err(UserError::BadInput(_))));
        assert!(matches!(set_user_locations(&mut c, "t1", &boss, None, "ghost", &[], T0), Err(UserError::NotFound)));
        set_user_locations(&mut c, "t1", &boss, None, &u, &s(&["main", "g1"]), T0).unwrap(); // owner: any location
        assert_eq!(user_location_ids(&c, "t1", &u).unwrap(), ["g1", "main"].iter().map(|x| x.to_string()).collect());

        // a supervisor limited to g1 can only touch g1
        let sup = user_with(&mut c, &boss, "sup", &["users.manage"]);
        set_user_locations(&mut c, "t1", &boss, None, &sup.user_id, &s(&["g1"]), T0).unwrap();
        let sup = principal(&mut c, "sup");
        assert_eq!(sup.location_ids, s(&["g1"]));
        let w2 = user(&mut c, &boss, "worker2");
        set_user_locations(&mut c, "t1", &sup, None, &w2, &s(&["g1"]), T0).unwrap();
        assert!(matches!(set_user_locations(&mut c, "t1", &sup, None, &w2, &s(&["g1", "g2"]), T0), Err(UserError::Forbidden(_))), "cannot add g2");
        assert!(matches!(set_user_locations(&mut c, "t1", &sup, None, &u, &s(&["g1"]), T0), Err(UserError::Forbidden(_))), "cannot remove main from worker");
        assert_eq!(user_location_ids(&c, "t1", &u).unwrap().len(), 2, "refusal changed nothing");
        // clearing is a change like any other
        set_user_locations(&mut c, "t1", &sup, None, &w2, &[], T0).unwrap();
    }

    #[test]
    fn listings_show_roles_and_locations_but_no_secrets() {
        let (mut c, boss) = setup();
        let u = user(&mut c, &boss, "clerk");
        let r = role(&mut c, &boss, "Clerk", &["sales.view"]);
        set_user_roles(&mut c, "t1", &boss, None, &u, &[r.clone()], T0).unwrap();
        set_user_locations(&mut c, "t1", &boss, None, &u, &s(&["main"]), T0).unwrap();
        let users = list_users(&c, "t1").unwrap();
        let clerk = users.iter().find(|x| x.id == u).unwrap();
        assert_eq!((clerk.role_ids.clone(), clerk.location_ids.clone(), clerk.is_active), (vec![r.clone()], s(&["main"]), true));
        assert!(format!("{users:?}").find("argon2").is_none());
        let roles = list_roles(&c, "t1").unwrap();
        assert_eq!(roles.iter().find(|x| x.id == r).unwrap().member_count, 1);
        assert_eq!(roles.iter().filter(|x| x.protected).count(), 1);
    }

    #[test]
    fn every_change_is_audited_with_before_and_after() {
        let (mut c, boss) = setup();
        let u = user(&mut c, &boss, "clerk");
        let r = role(&mut c, &boss, "Clerk", &["sales.view"]);
        update_role(&mut c, "t1", &boss, None, &r, "Clerk 2", &s(&["sales.create"]), T0).unwrap();
        set_user_roles(&mut c, "t1", &boss, None, &u, &[r.clone()], T0).unwrap();
        set_user_locations(&mut c, "t1", &boss, None, &u, &s(&["main"]), T0).unwrap();
        set_user_roles(&mut c, "t1", &boss, None, &u, &[], T0).unwrap();
        delete_role(&mut c, "t1", &boss, None, &r, T0).unwrap();
        let mut st = c
            .prepare("SELECT action, actor_id, COALESCE(before_json,''), COALESCE(after_json,'') FROM audit_logs WHERE action LIKE 'role.%' OR action LIKE 'user.roles%' OR action LIKE 'user.locations%' ORDER BY rowid")
            .unwrap();
        let rows: Vec<(String, String, String, String)> =
            st.query_map([], |x| Ok((x.get(0)?, x.get(1)?, x.get(2)?, x.get(3)?))).unwrap().map(|x| x.unwrap()).collect();
        let actions: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(actions, ["role.create", "role.update", "user.roles_set", "user.locations_set", "user.roles_set", "role.delete"]);
        assert!(rows.iter().all(|r| r.1 == boss.user_id));
        let upd = &rows[1];
        assert!(upd.2.contains("sales.view") && upd.3.contains("sales.create") && upd.3.contains("Clerk 2"));
    }

    #[test]
    fn failed_changes_are_rolled_back_completely() {
        let (mut c, boss) = setup();
        let r = role(&mut c, &boss, "Clerk", &["sales.view"]);
        let before: i64 = c.query_row("SELECT COUNT(*) FROM audit_logs", [], |x| x.get(0)).unwrap();
        // the second permission is invalid: nothing from this call may persist
        assert!(update_role(&mut c, "t1", &boss, None, &r, "Renamed", &s(&["sales.view", "bogus.thing"]), T0).is_err());
        let r2 = list_roles(&c, "t1").unwrap().into_iter().find(|x| x.id == r).unwrap();
        assert_eq!((r2.name.as_str(), r2.permissions), ("Clerk", s(&["sales.view"])));
        let after: i64 = c.query_row("SELECT COUNT(*) FROM audit_logs", [], |x| x.get(0)).unwrap();
        assert_eq!(before, after);
    }
}
