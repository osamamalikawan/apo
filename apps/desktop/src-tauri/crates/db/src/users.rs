//! Users, login and sessions for the Shop Server.
//!
//! * Passwords and PINs are stored only as Argon2id PHC strings (never plaintext, never reversible).
//! * 5 wrong attempts lock the account for 15 minutes. Passwords and PINs share one counter, so a
//!   short PIN cannot be brute-forced by also guessing passwords. Locked, wrong, unknown and
//!   deactivated all return the same `InvalidCredentials`, so nothing reveals which usernames exist.
//! * A successful login creates a session: 32 random bytes, only the SHA-256 hash is stored. A session
//!   belongs to the device that created it, expires after 12 hours, and after 2 hours without use.
//! * A session remembers how it was created (`password` or `pin`). Sensitive actions require a
//!   password session; a PIN is for fast sign-in on a counter PC.
//! * Time is passed in as an ISO-8601 UTC string (`...SS.sssZ`) so behaviour is deterministic.
//!
//! Authorization (what a user may do) lives in `has_permission` below; it mirrors
//! `packages/domain/src/permissions.ts` and is checked against the shared vectors.

use crate::DbError;
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rand::{rngs::OsRng, RngCore};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

pub const MAX_FAILED_LOGINS: i64 = 5;
pub const LOCK_SECS: i64 = 15 * 60;
pub const SESSION_TTL_SECS: i64 = 12 * 3600;
pub const SESSION_IDLE_SECS: i64 = 2 * 3600;

#[derive(Debug, thiserror::Error)]
pub enum UserError {
    /// Deliberately generic: wrong, locked, unknown and deactivated are indistinguishable.
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("{0}")]
    BadInput(&'static str),
    #[error("{0}")]
    Conflict(&'static str),
    #[error("not found")]
    NotFound,
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

impl From<DbError> for UserError {
    fn from(e: DbError) -> Self {
        match e {
            DbError::Sqlite(e) => UserError::Sqlite(e),
            DbError::WriterStopped => UserError::Sqlite(rusqlite::Error::InvalidQuery),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Password,
    Pin,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Password => "password",
            Method::Pin => "pin",
        }
    }
    fn column(self) -> &'static str {
        match self {
            Method::Password => "password_hash",
            Method::Pin => "pin_hash",
        }
    }
}

// ---------- hashing and validation ----------

fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// Argon2id with the library defaults (19 MiB memory, 2 passes, 1 lane: the OWASP minimum).
/// The PHC string records the parameters, so they can be raised later without breaking old hashes.
pub fn hash_secret(secret: &str) -> String {
    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);
    let salt = SaltString::encode_b64(&salt).expect("16 bytes is a valid salt length");
    Argon2::default()
        .hash_password(secret.as_bytes(), &salt)
        .expect("argon2 hashing with default parameters cannot fail")
        .to_string()
}

pub fn verify_secret(phc: &str, secret: &str) -> bool {
    match PasswordHash::new(phc) {
        Ok(parsed) => Argon2::default().verify_password(secret.as_bytes(), &parsed).is_ok(),
        Err(_) => false,
    }
}

/// Burn the same CPU time as a real check when the user does not exist or has no secret set.
fn dummy_verify(secret: &str) {
    static DUMMY: OnceLock<String> = OnceLock::new();
    let phc = DUMMY.get_or_init(|| hash_secret("not-a-real-password"));
    let _ = verify_secret(phc, secret);
}

pub fn normalize_username(raw: &str) -> Result<String, UserError> {
    let u = raw.trim().to_lowercase();
    let ok = (3..=32).contains(&u.chars().count())
        && u.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(u)
    } else {
        Err(UserError::BadInput("username must be 3 to 32 characters: letters, digits, dot, dash, underscore"))
    }
}

pub fn validate_password(username: &str, pw: &str) -> Result<(), UserError> {
    let n = pw.chars().count();
    if !(10..=128).contains(&n) {
        return Err(UserError::BadInput("password must be 10 to 128 characters"));
    }
    let first = pw.chars().next().unwrap_or(' ');
    if pw.chars().all(|c| c == first) {
        return Err(UserError::BadInput("password is too easy to guess"));
    }
    if pw.eq_ignore_ascii_case(username) {
        return Err(UserError::BadInput("password must not be the username"));
    }
    Ok(())
}

pub fn validate_pin(pin: &str) -> Result<(), UserError> {
    let digits = pin.bytes().all(|b| b.is_ascii_digit());
    if !(4..=8).contains(&pin.len()) || !digits {
        return Err(UserError::BadInput("PIN must be 4 to 8 digits"));
    }
    let b = pin.as_bytes();
    if b.iter().all(|x| *x == b[0]) {
        return Err(UserError::BadInput("PIN is too easy to guess"));
    }
    Ok(())
}

// ---------- audit ----------

fn audit(
    conn: &Connection,
    tenant: &str,
    actor: Option<&str>,
    device: Option<&str>,
    action: &str,
    entity_id: Option<&str>,
    after: Option<&str>,
    now: &str,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO audit_logs(id,tenant_id,actor_id,device_id,action,entity,entity_id,after_json,at)
         VALUES (?1,?2,?3,?4,?5,'user',?6,?7,?8)",
        params![uuid::Uuid::new_v4().to_string(), tenant, actor, device, action, entity_id, after, now],
    )?;
    Ok(())
}

// ---------- permissions ----------

/// "*" grants everything, "module.*" grants a module, otherwise the exact string.
/// Mirrors `hasPermission` in packages/domain/src/permissions.ts.
pub fn has_permission(granted: &[String], needed: &str) -> bool {
    let module = needed.split('.').next().unwrap_or("");
    let module_all = format!("{module}.*");
    granted.iter().any(|g| g == "*" || g == needed || *g == module_all)
}

pub fn permissions_for_user(conn: &Connection, tenant: &str, user_id: &str) -> Result<Vec<String>, DbError> {
    let mut st = conn.prepare(
        "SELECT DISTINCT rp.permission FROM user_roles ur
         JOIN role_permissions rp ON rp.tenant_id = ur.tenant_id AND rp.role_id = ur.role_id
         WHERE ur.tenant_id = ?1 AND ur.user_id = ?2 ORDER BY rp.permission",
    )?;
    let rows = st.query_map(params![tenant, user_id], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Give a user the tenant's "Owner" role (all permissions), creating the role if needed.
pub fn grant_owner(conn: &mut Connection, tenant: &str, user_id: &str) -> Result<(), DbError> {
    let tx = conn.transaction()?;
    let role_id: Option<String> = tx
        .query_row("SELECT id FROM roles WHERE tenant_id=?1 AND name='Owner'", [tenant], |r| r.get(0))
        .optional()?;
    let role_id = match role_id {
        Some(id) => id,
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            tx.execute("INSERT INTO roles(id,tenant_id,name) VALUES (?1,?2,'Owner')", params![id, tenant])?;
            tx.execute(
                "INSERT INTO role_permissions(tenant_id,role_id,permission) VALUES (?1,?2,'*')",
                params![tenant, id],
            )?;
            id
        }
    };
    tx.execute(
        "INSERT OR IGNORE INTO user_roles(tenant_id,user_id,role_id) VALUES (?1,?2,?3)",
        params![tenant, user_id, role_id],
    )?;
    tx.commit()?;
    Ok(())
}

// ---------- user management ----------

pub fn count_active_users(conn: &Connection, tenant: &str) -> Result<i64, DbError> {
    Ok(conn.query_row("SELECT COUNT(*) FROM users WHERE tenant_id=?1 AND is_active=1", [tenant], |r| r.get(0))?)
}

pub fn create_user(
    conn: &mut Connection,
    tenant: &str,
    actor: Option<&str>,
    device: Option<&str>,
    username: &str,
    full_name: &str,
    password: &str,
    now: &str,
) -> Result<String, UserError> {
    let username = normalize_username(username)?;
    let full_name = full_name.trim();
    if full_name.is_empty() || full_name.chars().count() > 80 {
        return Err(UserError::BadInput("full_name must be 1 to 80 characters"));
    }
    validate_password(&username, password)?;
    let hash = hash_secret(password); // slow on purpose; done before the transaction opens
    let tx = conn.transaction()?;
    let taken: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM users WHERE tenant_id=?1 AND username=?2)",
        params![tenant, username],
        |r| r.get(0),
    )?;
    if taken {
        return Err(UserError::Conflict("username already exists"));
    }
    let id = uuid::Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO users(id,tenant_id,username,full_name,password_hash,created_at,updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?6)",
        params![id, tenant, username, full_name, hash, now],
    )?;
    let after = format!(r#"{{"username":"{username}"}}"#); // never includes any hash
    audit(&tx, tenant, actor, device, "user.create", Some(&id), Some(&after), now)?;
    tx.commit()?;
    Ok(id)
}

fn revoke_sessions_tx(tx: &Transaction, tenant: &str, user_id: &str, now: &str) -> Result<(), rusqlite::Error> {
    tx.execute(
        "UPDATE user_sessions SET revoked_at=?3 WHERE tenant_id=?1 AND user_id=?2 AND revoked_at IS NULL",
        params![tenant, user_id, now],
    )?;
    Ok(())
}

/// Wrong-attempt bookkeeping shared by login, change-password and set-PIN.
/// Returns whether `secret` matched. Counters are updated inside `tx`; the caller must commit
/// even on a mismatch so failures are remembered.
fn check_secret(
    tx: &Transaction,
    tenant: &str,
    user_id: &str,
    method: Method,
    secret: &str,
    now: &str,
) -> Result<bool, rusqlite::Error> {
    let (hash, active, locked_until): (Option<String>, i64, Option<String>) = tx.query_row(
        &format!("SELECT {}, is_active, locked_until FROM users WHERE tenant_id=?1 AND id=?2", method.column()),
        params![tenant, user_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let locked = locked_until.as_deref().map(|l| l > now).unwrap_or(false);
    let ok = if active == 1 && !locked {
        match &hash {
            Some(h) => verify_secret(h, secret),
            None => {
                dummy_verify(secret);
                false
            }
        }
    } else {
        dummy_verify(secret);
        false
    };
    if ok {
        tx.execute(
            "UPDATE users SET failed_logins=0, locked_until=NULL WHERE tenant_id=?1 AND id=?2",
            params![tenant, user_id],
        )?;
        return Ok(true);
    }
    if active == 1 && !locked {
        let failed: i64 = tx.query_row(
            "UPDATE users SET failed_logins = failed_logins + 1 WHERE tenant_id=?1 AND id=?2 RETURNING failed_logins",
            params![tenant, user_id],
            |r| r.get(0),
        )?;
        if failed >= MAX_FAILED_LOGINS {
            tx.execute(
                "UPDATE users SET failed_logins=0, locked_until=strftime('%Y-%m-%dT%H:%M:%fZ', ?3, ?4)
                 WHERE tenant_id=?1 AND id=?2",
                params![tenant, user_id, now, format!("+{LOCK_SECS} seconds")],
            )?;
            audit(tx, tenant, Some(user_id), None, "auth.locked", Some(user_id), None, now)?;
        }
    }
    Ok(false)
}

pub struct LoginOk {
    pub token: String, // returned once; the server keeps only its hash
    pub session_id: String,
    pub user_id: String,
    pub username: String,
    pub full_name: String,
    pub method: Method,
    pub expires_at: String,
}

pub fn login(
    conn: &mut Connection,
    tenant: &str,
    device_id: &str,
    username: &str,
    secret: &str,
    method: Method,
    now: &str,
) -> Result<LoginOk, UserError> {
    let Ok(username) = normalize_username(username) else {
        dummy_verify(secret);
        return Err(UserError::InvalidCredentials);
    };
    let tx = conn.transaction()?;
    let user: Option<(String, String)> = tx
        .query_row(
            "SELECT id, full_name FROM users WHERE tenant_id=?1 AND username=?2",
            params![tenant, username],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((user_id, full_name)) = user else {
        dummy_verify(secret);
        return Err(UserError::InvalidCredentials);
    };
    if !check_secret(&tx, tenant, &user_id, method, secret, now)? {
        audit(&tx, tenant, None, Some(device_id), "auth.login_failed", Some(&user_id), None, now)?;
        tx.commit()?; // keep the counter even though the login failed
        return Err(UserError::InvalidCredentials);
    }
    let mut raw = [0u8; 32];
    OsRng.fill_bytes(&mut raw);
    let token = hex::encode(raw);
    let session_id = uuid::Uuid::new_v4().to_string();
    let expires_at: String = tx.query_row(
        "SELECT strftime('%Y-%m-%dT%H:%M:%fZ', ?1, ?2)",
        params![now, format!("+{SESSION_TTL_SECS} seconds")],
        |r| r.get(0),
    )?;
    tx.execute(
        "INSERT INTO user_sessions(id,tenant_id,user_id,device_id,token_hash,method,created_at,last_used_at,expires_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?7,?8)",
        params![session_id, tenant, user_id, device_id, sha256_hex(&token), method.as_str(), now, expires_at],
    )?;
    audit(&tx, tenant, Some(&user_id), Some(device_id), "auth.login", Some(&user_id), None, now)?;
    tx.commit()?;
    Ok(LoginOk { token, session_id, user_id, username, full_name, method, expires_at })
}

#[derive(Debug, Clone)]
pub struct Principal {
    pub session_id: String,
    pub user_id: String,
    pub username: String,
    pub full_name: String,
    pub method: Method,
    pub permissions: Vec<String>,
}

impl Principal {
    pub fn can(&self, needed: &str) -> bool {
        has_permission(&self.permissions, needed)
    }
}

/// Resolve a bearer token to a user, only for the device that created the session.
/// Returns `None` for unknown, other-device, expired, idle, revoked or deactivated-user sessions.
pub fn authenticate_session(
    conn: &Connection,
    tenant: &str,
    device_id: &str,
    token: &str,
    now: &str,
) -> Result<Option<Principal>, DbError> {
    let row: Option<(String, String, String, String, String)> = conn
        .query_row(
            "SELECT s.id, u.id, u.username, u.full_name, s.method
             FROM user_sessions s JOIN users u ON u.tenant_id = s.tenant_id AND u.id = s.user_id
             WHERE s.token_hash=?1 AND s.tenant_id=?2 AND s.device_id=?3
               AND s.revoked_at IS NULL AND s.expires_at > ?4
               AND s.last_used_at > strftime('%Y-%m-%dT%H:%M:%fZ', ?4, ?5)
               AND u.is_active = 1",
            params![sha256_hex(token), tenant, device_id, now, format!("-{SESSION_IDLE_SECS} seconds")],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let Some((session_id, user_id, username, full_name, method)) = row else {
        return Ok(None);
    };
    conn.execute("UPDATE user_sessions SET last_used_at=?2 WHERE id=?1", params![session_id, now])?;
    let permissions = permissions_for_user(conn, tenant, &user_id)?;
    let method = if method == "pin" { Method::Pin } else { Method::Password };
    Ok(Some(Principal { session_id, user_id, username, full_name, method, permissions }))
}

pub fn logout(conn: &Connection, tenant: &str, session_id: &str, now: &str) -> Result<(), DbError> {
    conn.execute(
        "UPDATE user_sessions SET revoked_at=?3 WHERE tenant_id=?1 AND id=?2 AND revoked_at IS NULL",
        params![tenant, session_id, now],
    )?;
    Ok(())
}

/// Change a user's own password. Needs the current password (counted like a login attempt).
/// All of the user's sessions end, including the current one: they sign in again with the new password.
pub fn change_password(
    conn: &mut Connection,
    tenant: &str,
    user_id: &str,
    device_id: Option<&str>,
    current: &str,
    new_password: &str,
    now: &str,
) -> Result<(), UserError> {
    let username: String = conn
        .query_row("SELECT username FROM users WHERE tenant_id=?1 AND id=?2", params![tenant, user_id], |r| r.get(0))
        .optional()?
        .ok_or(UserError::NotFound)?;
    validate_password(&username, new_password)?;
    let new_hash = hash_secret(new_password);
    let tx = conn.transaction()?;
    if !check_secret(&tx, tenant, user_id, Method::Password, current, now)? {
        tx.commit()?;
        return Err(UserError::InvalidCredentials);
    }
    tx.execute(
        "UPDATE users SET password_hash=?3, revision=revision+1, updated_at=?4 WHERE tenant_id=?1 AND id=?2",
        params![tenant, user_id, new_hash, now],
    )?;
    revoke_sessions_tx(&tx, tenant, user_id, now)?;
    audit(&tx, tenant, Some(user_id), device_id, "user.password_changed", Some(user_id), None, now)?;
    tx.commit()?;
    Ok(())
}

/// Set or replace a user's own PIN. Needs the current password.
pub fn set_pin(
    conn: &mut Connection,
    tenant: &str,
    user_id: &str,
    device_id: Option<&str>,
    current_password: &str,
    pin: &str,
    now: &str,
) -> Result<(), UserError> {
    validate_pin(pin)?;
    let pin_hash = hash_secret(pin);
    let tx = conn.transaction()?;
    if !check_secret(&tx, tenant, user_id, Method::Password, current_password, now)? {
        tx.commit()?;
        return Err(UserError::InvalidCredentials);
    }
    tx.execute(
        "UPDATE users SET pin_hash=?3, revision=revision+1, updated_at=?4 WHERE tenant_id=?1 AND id=?2",
        params![tenant, user_id, pin_hash, now],
    )?;
    audit(&tx, tenant, Some(user_id), device_id, "user.pin_set", Some(user_id), None, now)?;
    tx.commit()?;
    Ok(())
}

/// Set a user's password without knowing the old one. LOCAL USE ONLY (first-run bootstrap and
/// "the owner forgot the password" recovery, run from the server console): never expose over the network.
pub fn set_password_local(
    conn: &mut Connection,
    tenant: &str,
    user_id: &str,
    password: &str,
    now: &str,
) -> Result<(), UserError> {
    let username: String = conn
        .query_row("SELECT username FROM users WHERE tenant_id=?1 AND id=?2", params![tenant, user_id], |r| r.get(0))
        .optional()?
        .ok_or(UserError::NotFound)?;
    validate_password(&username, password)?;
    let hash = hash_secret(password);
    let tx = conn.transaction()?;
    tx.execute(
        "UPDATE users SET password_hash=?3, failed_logins=0, locked_until=NULL, is_active=1,
                revision=revision+1, updated_at=?4 WHERE tenant_id=?1 AND id=?2",
        params![tenant, user_id, hash, now],
    )?;
    revoke_sessions_tx(&tx, tenant, user_id, now)?;
    audit(&tx, tenant, None, None, "user.password_reset_local", Some(user_id), None, now)?;
    tx.commit()?;
    Ok(())
}

/// Deactivate a user (users are never deleted: history must stay attributable) and end their sessions.
/// Refuses to deactivate the last active user that holds the all-permissions ("*") grant.
pub fn deactivate_user(
    conn: &mut Connection,
    tenant: &str,
    actor: &str,
    device_id: Option<&str>,
    user_id: &str,
    now: &str,
) -> Result<(), UserError> {
    let tx = conn.transaction()?;
    let active: Option<i64> = tx
        .query_row("SELECT is_active FROM users WHERE tenant_id=?1 AND id=?2", params![tenant, user_id], |r| r.get(0))
        .optional()?;
    match active {
        None => return Err(UserError::NotFound),
        Some(0) => return Ok(()), // already inactive: idempotent
        Some(_) => {}
    }
    let is_owner = !permissions_for_user(&tx, tenant, user_id)?.iter().all(|p| p != "*");
    if is_owner {
        let other_owners: i64 = tx.query_row(
            "SELECT COUNT(DISTINCT u.id) FROM users u
             JOIN user_roles ur ON ur.tenant_id=u.tenant_id AND ur.user_id=u.id
             JOIN role_permissions rp ON rp.tenant_id=ur.tenant_id AND rp.role_id=ur.role_id
             WHERE u.tenant_id=?1 AND u.is_active=1 AND u.id<>?2 AND rp.permission='*'",
            params![tenant, user_id],
            |r| r.get(0),
        )?;
        if other_owners == 0 {
            return Err(UserError::Conflict("cannot deactivate the last owner"));
        }
    }
    tx.execute(
        "UPDATE users SET is_active=0, revision=revision+1, updated_at=?3 WHERE tenant_id=?1 AND id=?2",
        params![tenant, user_id, now],
    )?;
    revoke_sessions_tx(&tx, tenant, user_id, now)?;
    audit(&tx, tenant, Some(actor), device_id, "user.deactivate", Some(user_id), None, now)?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{migrate, open_in_memory};

    const T0: &str = "2030-01-01T00:00:00.000Z";
    const GOOD: &str = "correct horse battery";

    /// Tenant t1, a paired device d1 (and d2), an owner "boss" with a password.
    fn setup() -> (Connection, String) {
        let mut c = open_in_memory().unwrap();
        migrate(&mut c).unwrap();
        c.execute_batch(
            "INSERT INTO tenants(id,name) VALUES ('t1','Shop');
             INSERT INTO devices(id,tenant_id,name,kind,credential_hash) VALUES
               ('d1','t1','PC 1','desktop_client','h'),('d2','t1','PC 2','desktop_client','h');",
        )
        .unwrap();
        let boss = create_user(&mut c, "t1", None, None, "Boss", "The Boss", GOOD, T0).unwrap();
        grant_owner(&mut c, "t1", &boss).unwrap();
        (c, boss)
    }

    fn at(secs: i64) -> String {
        // T0 plus `secs`, formatted like the database does.
        let c = Connection::open_in_memory().unwrap();
        c.query_row(
            "SELECT strftime('%Y-%m-%dT%H:%M:%fZ', ?1, ?2)",
            params![T0, format!("{secs:+} seconds")],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn pw_login(c: &mut Connection, user: &str, pw: &str, now: &str) -> Result<LoginOk, UserError> {
        login(c, "t1", "d1", user, pw, Method::Password, now)
    }

    #[test]
    fn secrets_are_argon2id_hashes_and_never_plaintext() {
        let (c, _) = setup();
        let h: String = c.query_row("SELECT password_hash FROM users WHERE username='boss'", [], |r| r.get(0)).unwrap();
        assert!(h.starts_with("$argon2id$"), "{h}");
        assert!(!h.contains(GOOD));
        assert!(verify_secret(&h, GOOD));
        assert!(!verify_secret(&h, "wrong"));
        assert_ne!(hash_secret(GOOD), hash_secret(GOOD), "salts must differ");
        assert!(!verify_secret("not a phc string", GOOD));
    }

    #[test]
    fn login_works_and_wrong_unknown_look_identical() {
        let (mut c, boss) = setup();
        let ok = pw_login(&mut c, " BOSS ", GOOD, T0).unwrap(); // username is normalized
        assert_eq!(ok.user_id, boss);
        assert_eq!(ok.token.len(), 64);
        for (u, p) in [("boss", "wrong password!"), ("nobody", GOOD), ("", GOOD), ("bad name!", GOOD)] {
            assert!(matches!(pw_login(&mut c, u, p, T0), Err(UserError::InvalidCredentials)), "{u}");
        }
    }

    #[test]
    fn token_is_stored_only_as_a_hash() {
        let (mut c, _) = setup();
        let ok = pw_login(&mut c, "boss", GOOD, T0).unwrap();
        let stored: String = c.query_row("SELECT token_hash FROM user_sessions", [], |r| r.get(0)).unwrap();
        assert_ne!(stored, ok.token);
        assert_eq!(stored, sha256_hex(&ok.token));
    }

    #[test]
    fn five_wrong_attempts_lock_even_the_right_password_until_the_lock_expires() {
        let (mut c, _) = setup();
        for _ in 0..5 {
            assert!(pw_login(&mut c, "boss", "nope nope nope", T0).is_err());
        }
        assert!(matches!(pw_login(&mut c, "boss", GOOD, &at(60)), Err(UserError::InvalidCredentials)));
        assert!(matches!(pw_login(&mut c, "boss", GOOD, &at(LOCK_SECS - 1)), Err(UserError::InvalidCredentials)));
        assert!(pw_login(&mut c, "boss", GOOD, &at(LOCK_SECS + 1)).is_ok());
        // success cleared the counters
        let (f, l): (i64, Option<String>) =
            c.query_row("SELECT failed_logins, locked_until FROM users WHERE username='boss'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((f, l), (0, None));
    }

    #[test]
    fn a_success_resets_the_failure_count() {
        let (mut c, _) = setup();
        for _ in 0..4 {
            assert!(pw_login(&mut c, "boss", "nope nope nope", T0).is_err());
        }
        assert!(pw_login(&mut c, "boss", GOOD, T0).is_ok());
        for _ in 0..4 {
            assert!(pw_login(&mut c, "boss", "nope nope nope", T0).is_err());
        }
        assert!(pw_login(&mut c, "boss", GOOD, T0).is_ok()); // 4 + 4 failures never reached 5 in a row
    }

    #[test]
    fn pin_login_shares_the_lockout_counter_with_the_password() {
        let (mut c, boss) = setup();
        set_pin(&mut c, "t1", &boss, Some("d1"), GOOD, "2580", T0).unwrap();
        let ok = login(&mut c, "t1", "d1", "boss", "2580", Method::Pin, T0).unwrap();
        assert_eq!(ok.method, Method::Pin);
        // a PIN is not a password and a password is not a PIN
        assert!(login(&mut c, "t1", "d1", "boss", "2580", Method::Password, T0).is_err());
        assert!(login(&mut c, "t1", "d1", "boss", GOOD, Method::Pin, T0).is_err());
        for _ in 0..3 {
            assert!(login(&mut c, "t1", "d1", "boss", "1111", Method::Pin, T0).is_err());
        }
        // 2 failures above + 3 here = 5: now even the right PIN is refused
        assert!(login(&mut c, "t1", "d1", "boss", "2580", Method::Pin, &at(10)).is_err());
    }

    #[test]
    fn a_user_without_a_pin_cannot_log_in_with_one() {
        let (mut c, _) = setup();
        assert!(login(&mut c, "t1", "d1", "boss", "2580", Method::Pin, T0).is_err());
    }

    #[test]
    fn session_resolves_for_its_own_device_only() {
        let (mut c, boss) = setup();
        let ok = pw_login(&mut c, "boss", GOOD, T0).unwrap();
        let p = authenticate_session(&c, "t1", "d1", &ok.token, &at(5)).unwrap().unwrap();
        assert_eq!(p.user_id, boss);
        assert_eq!(p.permissions, vec!["*".to_string()]);
        assert!(p.can("anything.at_all"));
        assert!(authenticate_session(&c, "t1", "d2", &ok.token, &at(5)).unwrap().is_none(), "other device");
        assert!(authenticate_session(&c, "t2", "d1", &ok.token, &at(5)).unwrap().is_none(), "other tenant");
        assert!(authenticate_session(&c, "t1", "d1", "0123456789", &at(5)).unwrap().is_none(), "unknown token");
    }

    #[test]
    fn sessions_expire_absolutely_and_when_idle() {
        let (mut c, _) = setup();
        let ok = pw_login(&mut c, "boss", GOOD, T0).unwrap();
        // idle: nothing for more than 2 hours
        assert!(authenticate_session(&c, "t1", "d1", &ok.token, &at(SESSION_IDLE_SECS + 1)).unwrap().is_none());
        // activity keeps it alive until the 12 hour limit
        let ok = pw_login(&mut c, "boss", GOOD, T0).unwrap();
        let mut t = 0;
        while t + 3600 < SESSION_TTL_SECS {
            t += 3600;
            assert!(authenticate_session(&c, "t1", "d1", &ok.token, &at(t)).unwrap().is_some(), "t={t}");
        }
        assert!(authenticate_session(&c, "t1", "d1", &ok.token, &at(SESSION_TTL_SECS + 1)).unwrap().is_none());
    }

    #[test]
    fn logout_revokes_the_session() {
        let (mut c, _) = setup();
        let ok = pw_login(&mut c, "boss", GOOD, T0).unwrap();
        logout(&c, "t1", &ok.session_id, &at(1)).unwrap();
        assert!(authenticate_session(&c, "t1", "d1", &ok.token, &at(2)).unwrap().is_none());
    }

    #[test]
    fn changing_the_password_ends_all_sessions_and_needs_the_current_one() {
        let (mut c, boss) = setup();
        let a = pw_login(&mut c, "boss", GOOD, T0).unwrap();
        let b = login(&mut c, "t1", "d2", "boss", GOOD, Method::Password, T0).unwrap();
        assert!(matches!(
            change_password(&mut c, "t1", &boss, Some("d1"), "wrong current", "a brand new password", &at(1)),
            Err(UserError::InvalidCredentials)
        ));
        assert!(authenticate_session(&c, "t1", "d1", &a.token, &at(2)).unwrap().is_some());
        change_password(&mut c, "t1", &boss, Some("d1"), GOOD, "a brand new password", &at(3)).unwrap();
        assert!(authenticate_session(&c, "t1", "d1", &a.token, &at(4)).unwrap().is_none());
        assert!(authenticate_session(&c, "t1", "d2", &b.token, &at(4)).unwrap().is_none());
        assert!(pw_login(&mut c, "boss", GOOD, &at(5)).is_err());
        assert!(pw_login(&mut c, "boss", "a brand new password", &at(5)).is_ok());
    }

    #[test]
    fn current_password_guesses_on_change_password_count_toward_the_lockout() {
        let (mut c, boss) = setup();
        for _ in 0..5 {
            let _ = change_password(&mut c, "t1", &boss, Some("d1"), "guess guess guess", "a brand new password", T0);
        }
        assert!(pw_login(&mut c, "boss", GOOD, &at(30)).is_err(), "account is locked");
    }

    #[test]
    fn deactivation_blocks_login_and_kills_existing_sessions() {
        let (mut c, boss) = setup();
        let cashier = create_user(&mut c, "t1", Some(&boss), Some("d1"), "cashier1", "Cashier One", "cashier password", T0).unwrap();
        let s = pw_login(&mut c, "cashier1", "cashier password", T0).unwrap();
        assert!(authenticate_session(&c, "t1", "d1", &s.token, &at(1)).unwrap().is_some());
        deactivate_user(&mut c, "t1", &boss, Some("d1"), &cashier, &at(2)).unwrap();
        assert!(authenticate_session(&c, "t1", "d1", &s.token, &at(3)).unwrap().is_none());
        assert!(matches!(pw_login(&mut c, "cashier1", "cashier password", &at(4)), Err(UserError::InvalidCredentials)));
        deactivate_user(&mut c, "t1", &boss, Some("d1"), &cashier, &at(5)).unwrap(); // idempotent
    }

    #[test]
    fn the_last_owner_cannot_be_deactivated_but_a_second_owner_can() {
        let (mut c, boss) = setup();
        assert!(matches!(deactivate_user(&mut c, "t1", &boss, None, &boss, T0), Err(UserError::Conflict(_))));
        let two = create_user(&mut c, "t1", Some(&boss), None, "boss2", "Second Boss", "second boss password", T0).unwrap();
        grant_owner(&mut c, "t1", &two).unwrap();
        deactivate_user(&mut c, "t1", &boss, None, &two, T0).unwrap();
        assert!(matches!(deactivate_user(&mut c, "t1", &boss, None, &boss, T0), Err(UserError::Conflict(_))));
    }

    #[test]
    fn create_user_validates_and_rejects_duplicates() {
        let (mut c, boss) = setup();
        let mk = |c: &mut Connection, u: &str, n: &str, p: &str| create_user(c, "t1", Some(&boss), None, u, n, p, T0);
        assert!(matches!(mk(&mut c, "boss", "Dup", "another password"), Err(UserError::Conflict(_))));
        assert!(matches!(mk(&mut c, "BOSS", "Dup", "another password"), Err(UserError::Conflict(_))), "case-insensitive");
        assert!(matches!(mk(&mut c, "ab", "Short", "another password"), Err(UserError::BadInput(_))));
        assert!(matches!(mk(&mut c, "has space", "X", "another password"), Err(UserError::BadInput(_))));
        assert!(matches!(mk(&mut c, "newuser", "  ", "another password"), Err(UserError::BadInput(_))));
        assert!(matches!(mk(&mut c, "newuser", "N", "short"), Err(UserError::BadInput(_))));
        assert!(matches!(mk(&mut c, "newuser", "N", "aaaaaaaaaaaa"), Err(UserError::BadInput(_))));
        assert!(matches!(mk(&mut c, "longusername", "N", "LongUserName"), Err(UserError::BadInput(_))));
        assert_eq!(count_active_users(&c, "t1").unwrap(), 1);
    }

    #[test]
    fn local_reset_unlocks_reactivates_and_ends_sessions() {
        let (mut c, boss) = setup();
        let s = pw_login(&mut c, "boss", GOOD, T0).unwrap();
        for _ in 0..5 {
            let _ = pw_login(&mut c, "boss", "nope nope nope", T0);
        }
        c.execute("UPDATE users SET is_active=0 WHERE id=?1", [&boss]).unwrap();
        set_password_local(&mut c, "t1", &boss, "recovered password", &at(1)).unwrap();
        assert!(authenticate_session(&c, "t1", "d1", &s.token, &at(2)).unwrap().is_none());
        assert!(pw_login(&mut c, "boss", "recovered password", &at(3)).is_ok());
        assert!(matches!(set_password_local(&mut c, "t1", "ghost", "recovered password", T0), Err(UserError::NotFound)));
        assert!(matches!(set_password_local(&mut c, "t1", &boss, "short", T0), Err(UserError::BadInput(_))));
    }

    #[test]
    fn pin_rules() {
        for bad in ["123", "123456789", "12a4", "1111", "", "12 34"] {
            assert!(validate_pin(bad).is_err(), "{bad}");
        }
        for good in ["2580", "13579", "86420975"] {
            assert!(validate_pin(good).is_ok(), "{good}");
        }
    }

    #[test]
    fn audit_trail_records_events_without_secrets() {
        let (mut c, boss) = setup();
        let _ = pw_login(&mut c, "boss", "wrong password!", T0);
        let _ = pw_login(&mut c, "boss", GOOD, T0);
        set_pin(&mut c, "t1", &boss, Some("d1"), GOOD, "2580", T0).unwrap();
        let mut st = c.prepare("SELECT action, COALESCE(after_json,'') FROM audit_logs ORDER BY rowid").unwrap();
        let rows: Vec<(String, String)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(|r| r.unwrap()).collect();
        let actions: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(actions, ["user.create", "auth.login_failed", "auth.login", "user.pin_set"]);
        for (_, after) in &rows {
            assert!(!after.contains("argon2") && !after.contains(GOOD) && !after.contains("2580"));
        }
    }

    #[test]
    fn permission_strings_follow_the_shared_vectors() {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../../../../../../packages/test-vectors/permissions.json")).unwrap();
        let mut checked = 0;
        for case in v["cases"].as_array().unwrap() {
            let ctx = &v["contexts"][case["ctx"].as_str().unwrap()];
            let granted: Vec<String> =
                ctx["permissions"].as_array().unwrap().iter().map(|p| p.as_str().unwrap().to_string()).collect();
            // The vectors also cover location/route scope (not this module); the permission part alone
            // is denied only when the expected reason is "no_permission".
            let expected = case["reason"].as_str() != Some("no_permission");
            let perm = case["perm"].as_str().unwrap();
            assert_eq!(has_permission(&granted, perm), expected, "{} {}", case["ctx"], perm);
            checked += 1;
        }
        assert!(checked >= 5);
    }

    #[test]
    fn permissions_come_from_roles_and_wildcards_work() {
        let g = |p: &[&str]| p.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(has_permission(&g(&["inventory.*"]), "inventory.transfer"));
        assert!(!has_permission(&g(&["inventory.*"]), "sales.create"));
        assert!(!has_permission(&g(&["inventory.view"]), "inventory.transfer"));
        assert!(!has_permission(&g(&["inventoryx.*"]), "inventory.view"));
        assert!(!has_permission(&g(&[]), "x.y"));

        let (mut c, boss) = setup();
        let u = create_user(&mut c, "t1", Some(&boss), None, "clerk", "Clerk", "clerk password!", T0).unwrap();
        assert!(permissions_for_user(&c, "t1", &u).unwrap().is_empty());
        c.execute_batch(
            "INSERT INTO roles(id,tenant_id,name) VALUES ('r1','t1','Cashier');
             INSERT INTO role_permissions(tenant_id,role_id,permission) VALUES ('t1','r1','sales.create'),('t1','r1','inventory.view');",
        )
        .unwrap();
        c.execute("INSERT INTO user_roles(tenant_id,user_id,role_id) VALUES ('t1',?1,'r1')", [&u]).unwrap();
        assert_eq!(permissions_for_user(&c, "t1", &u).unwrap(), vec!["inventory.view", "sales.create"]);
    }
}
