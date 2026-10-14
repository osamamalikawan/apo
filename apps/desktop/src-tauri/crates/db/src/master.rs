//! Helpers shared by the master-data modules (locations, catalog, customers).
//!
//! Conventions for every master-data record:
//! * `revision` starts at 1 and goes up by one on every change. Updates must quote the revision the
//!   caller last saw (`expected_revision`); a mismatch is `UserError::Stale`, so two PCs editing the
//!   same record can never silently overwrite each other.
//! * Records are deactivated, never deleted.
//! * Each change writes an audit row and a `sync_outbox` row in the same transaction. The outbox payload is
//!   a snapshot of the record at its new revision; the sync engine (build step 6) decides how to ship it.

use crate::users::UserError;
use rusqlite::{params, Connection};

/// Trim, bound the length, refuse control characters.
pub(crate) fn clean(raw: &str, min: usize, max: usize, msg: &'static str) -> Result<String, UserError> {
    let t = raw.trim();
    let n = t.chars().count();
    if n < min || n > max || t.chars().any(|c| c.is_control()) {
        return Err(UserError::BadInput(msg));
    }
    Ok(t.to_string())
}

/// Optional text: missing or blank means "none".
pub(crate) fn clean_opt(raw: Option<&str>, max: usize, msg: &'static str) -> Result<Option<String>, UserError> {
    match raw.map(str::trim) {
        None | Some("") => Ok(None),
        Some(t) => clean(t, 1, max, msg).map(Some),
    }
}

/// Upper-cased code made of A-Z 0-9 and the given extra characters. Stored upper-case, so the
/// plain UNIQUE constraints are effectively case-insensitive.
pub(crate) fn code(raw: &str, min: usize, max: usize, extra: &str, msg: &'static str) -> Result<String, UserError> {
    let t = raw.trim().to_uppercase();
    let n = t.chars().count();
    if n < min || n > max || !t.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || extra.contains(c)) {
        return Err(UserError::BadInput(msg));
    }
    Ok(t)
}

pub(crate) fn check_revision(current: Option<i64>, expected: i64) -> Result<(), UserError> {
    match current {
        None => Err(UserError::NotFound),
        Some(r) if r != expected => Err(UserError::Stale),
        Some(_) => Ok(()),
    }
}

/// Queue a change for the cloud. `op` is insert | update | archive.
pub(crate) fn enqueue(
    tx: &Connection,
    tenant: &str,
    entity: &str,
    entity_id: &str,
    op: &str,
    snapshot: &serde_json::Value,
    base_revision: Option<i64>,
    new_revision: i64,
) -> Result<(), rusqlite::Error> {
    tx.execute(
        "INSERT INTO sync_outbox(id,tenant_id,entity,entity_id,op,payload_json,idempotency_key,base_revision)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            uuid::Uuid::new_v4().to_string(),
            tenant,
            entity,
            entity_id,
            op,
            snapshot.to_string(),
            format!("{entity}:{entity_id}:r{new_revision}"),
            base_revision
        ],
    )?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod testutil {
    use crate::users::{Method, Principal};
    use crate::{migrate, open_in_memory};
    use rusqlite::Connection;

    pub const T0: &str = "2030-01-01T00:00:00.000Z";

    /// Tenant t1 with one device and one user, "u1".
    pub fn setup() -> Connection {
        let mut c = open_in_memory().unwrap();
        migrate(&mut c).unwrap();
        c.execute_batch(
            "INSERT INTO tenants(id,name) VALUES ('t1','Shop');
             INSERT INTO users(id,tenant_id,username,full_name) VALUES ('u1','t1','u1','User One');",
        )
        .unwrap();
        c
    }

    pub fn actor(perms: &[&str], locations: &[&str]) -> Principal {
        Principal {
            session_id: "s".into(),
            user_id: "u1".into(),
            username: "u1".into(),
            full_name: "User One".into(),
            method: Method::Password,
            permissions: perms.iter().map(|p| p.to_string()).collect(),
            location_ids: locations.iter().map(|p| p.to_string()).collect(),
        }
    }

    /// (action, entity, entity_id) of every audit row, oldest first.
    pub fn audit(c: &Connection) -> Vec<(String, String)> {
        let mut st = c.prepare("SELECT action, entity FROM audit_logs ORDER BY rowid").unwrap();
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        rows.map(|r| r.unwrap()).collect()
    }

    /// (entity, op, base_revision, key) of every outbox row, oldest first.
    pub fn outbox(c: &Connection) -> Vec<(String, String, Option<i64>, String)> {
        let mut st = c.prepare("SELECT entity, op, base_revision, idempotency_key FROM sync_outbox ORDER BY rowid").unwrap();
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
        rows.map(|r| r.unwrap()).collect()
    }
}
