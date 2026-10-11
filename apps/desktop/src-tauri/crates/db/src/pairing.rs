//! Device pairing and device credentials for the Shop Server. NOT YET COMPILED by the author
//! (no Rust toolchain in the build sandbox); run `cargo test -p dsaas-db`.
//!
//! Flow: an authorized user creates a one-time 8-digit pairing code on the server. A client
//! redeems it and receives a device id + secret (shown once). Only SHA-256 hashes are stored.
//! Codes expire after 10 minutes, work once, and lock after 5 failed attempts.
//! Time is passed in as an ISO-8601 UTC string so behaviour is deterministic and testable.

use crate::DbError;
use rand::{rngs::OsRng, Rng, RngCore};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

pub const PAIRING_TTL_SECS: i64 = 600;
pub const MAX_ATTEMPTS: i64 = 5;

#[derive(Debug, thiserror::Error)]
pub enum PairError {
    /// Deliberately generic: wrong, expired, used and locked codes are indistinguishable.
    #[error("invalid or expired pairing code")]
    Invalid,
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

#[derive(Debug)]
pub struct Pairing {
    pub id: String,
    pub code: String, // shown to the admin once; never stored in plaintext
    pub expires_at: String,
}

#[derive(Debug)]
pub struct Credential {
    pub device_id: String,
    pub secret: String, // returned once; the server keeps only its hash
}

#[derive(Debug, PartialEq, Eq)]
pub enum DeviceAuth {
    Valid,
    Revoked,
    Invalid,
}

fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// Constant-time comparison of two hex strings.
fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

fn code_hash(pairing_id: &str, code: &str) -> String {
    sha256_hex(&format!("{pairing_id}:{code}"))
}

pub fn create_pairing(
    conn: &Connection,
    tenant_id: &str,
    created_by: &str,
    now: &str,
) -> Result<Pairing, DbError> {
    let id = uuid::Uuid::new_v4().to_string();
    let code = format!("{:08}", OsRng.gen_range(0..100_000_000u32));
    let expires_at: String = conn.query_row(
        "SELECT strftime('%Y-%m-%dT%H:%M:%fZ', ?1, ?2)",
        params![now, format!("+{PAIRING_TTL_SECS} seconds")],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT INTO device_pairings(id,tenant_id,code_hash,created_by,expires_at) VALUES (?1,?2,?3,?4,?5)",
        params![id, tenant_id, code_hash(&id, &code), created_by, expires_at],
    )?;
    Ok(Pairing { id, code, expires_at })
}

/// Redeem a code. Wrong guesses count against every open pairing of the tenant.
pub fn redeem(
    conn: &mut Connection,
    tenant_id: &str,
    code: &str,
    device_name: &str,
    kind: &str,
    now: &str,
) -> Result<Credential, PairError> {
    let tx = conn.transaction()?;
    let open: Vec<(String, String)> = {
        let mut st = tx.prepare(
            "SELECT id, code_hash FROM device_pairings
             WHERE tenant_id=?1 AND used_at IS NULL AND attempts < ?2 AND expires_at > ?3",
        )?;
        let rows = st.query_map(params![tenant_id, MAX_ATTEMPTS, now], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    let matched = open.iter().find(|(id, h)| ct_eq(h, &code_hash(id, code)));
    match matched {
        None => {
            tx.execute(
                "UPDATE device_pairings SET attempts = attempts + 1
                 WHERE tenant_id=?1 AND used_at IS NULL AND expires_at > ?2 AND attempts < ?3",
                params![tenant_id, now, MAX_ATTEMPTS],
            )?;
            tx.commit()?; // keep the attempt count even though the redeem failed
            Err(PairError::Invalid)
        }
        Some((pairing_id, _)) => {
            let device_id = uuid::Uuid::new_v4().to_string();
            let mut raw = [0u8; 32];
            OsRng.fill_bytes(&mut raw);
            let secret = hex::encode(raw);
            tx.execute(
                "INSERT INTO devices(id,tenant_id,name,kind,credential_hash,last_seen_at) VALUES (?1,?2,?3,?4,?5,?6)",
                params![device_id, tenant_id, device_name, kind, sha256_hex(&secret), now],
            )?;
            tx.execute(
                "UPDATE device_pairings SET used_at=?1, device_id=?2 WHERE id=?3",
                params![now, device_id, pairing_id],
            )?;
            tx.commit()?;
            Ok(Credential { device_id, secret })
        }
    }
}

pub fn authenticate(
    conn: &Connection,
    tenant_id: &str,
    device_id: &str,
    secret: &str,
) -> Result<DeviceAuth, rusqlite::Error> {
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT credential_hash, revoked_at FROM devices WHERE tenant_id=?1 AND id=?2",
            params![tenant_id, device_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match row {
        None => DeviceAuth::Invalid,
        Some((hash, revoked)) => {
            if !ct_eq(&hash, &sha256_hex(secret)) {
                DeviceAuth::Invalid
            } else if revoked.is_some() {
                DeviceAuth::Revoked
            } else {
                DeviceAuth::Valid
            }
        }
    })
}

pub fn revoke_device(conn: &Connection, tenant_id: &str, device_id: &str, now: &str) -> Result<bool, rusqlite::Error> {
    Ok(conn.execute(
        "UPDATE devices SET revoked_at=?3 WHERE tenant_id=?1 AND id=?2 AND revoked_at IS NULL",
        params![tenant_id, device_id, now],
    )? > 0)
}

pub fn record_seen(conn: &Connection, tenant_id: &str, device_id: &str, now: &str, app_version: &str) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE devices SET last_seen_at=?3, app_version=?4 WHERE tenant_id=?1 AND id=?2",
        params![tenant_id, device_id, now, app_version],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{migrate, open_in_memory};

    const T0: &str = "2026-10-10T10:00:00.000Z";
    const T_5MIN: &str = "2026-10-10T10:05:00.000Z";
    const T_11MIN: &str = "2026-10-10T10:11:00.000Z";

    fn setup() -> Connection {
        let mut c = open_in_memory().unwrap();
        migrate(&mut c).unwrap();
        c.execute_batch(
            "INSERT INTO tenants(id,name) VALUES ('t1','Shop 1'),('t2','Shop 2');
             INSERT INTO users(id,tenant_id,username,full_name) VALUES ('u1','t1','admin','Admin 1'),('u2','t2','admin','Admin 2');",
        )
        .unwrap();
        c
    }

    #[test]
    fn code_works_once_and_returns_a_working_credential() {
        let mut c = setup();
        let p = create_pairing(&c, "t1", "u1", T0).unwrap();
        assert_eq!(p.code.len(), 8);
        assert_eq!(p.expires_at, "2026-10-10T10:10:00.000Z");
        let cred = redeem(&mut c, "t1", &p.code, "Counter PC", "desktop_client", T_5MIN).unwrap();
        assert_eq!(authenticate(&c, "t1", &cred.device_id, &cred.secret).unwrap(), DeviceAuth::Valid);
        // the same code cannot be used a second time
        assert!(matches!(redeem(&mut c, "t1", &p.code, "Other PC", "desktop_client", T_5MIN), Err(PairError::Invalid)));
    }

    #[test]
    fn expired_code_is_rejected() {
        let mut c = setup();
        let p = create_pairing(&c, "t1", "u1", T0).unwrap();
        assert!(matches!(redeem(&mut c, "t1", &p.code, "PC", "desktop_client", T_11MIN), Err(PairError::Invalid)));
    }

    #[test]
    fn five_wrong_guesses_lock_the_pairing_even_for_the_right_code() {
        let mut c = setup();
        let p = create_pairing(&c, "t1", "u1", T0).unwrap();
        let wrong = if p.code == "00000000" { "11111111" } else { "00000000" };
        for _ in 0..MAX_ATTEMPTS {
            assert!(matches!(redeem(&mut c, "t1", wrong, "PC", "desktop_client", T_5MIN), Err(PairError::Invalid)));
        }
        assert!(matches!(redeem(&mut c, "t1", &p.code, "PC", "desktop_client", T_5MIN), Err(PairError::Invalid)));
        let n: i64 = c.query_row("SELECT attempts FROM device_pairings WHERE id=?1", [&p.id], |r| r.get(0)).unwrap();
        assert_eq!(n, MAX_ATTEMPTS);
    }

    #[test]
    fn a_code_from_another_shop_does_not_pair() {
        let mut c = setup();
        let p = create_pairing(&c, "t1", "u1", T0).unwrap();
        assert!(matches!(redeem(&mut c, "t2", &p.code, "PC", "desktop_client", T_5MIN), Err(PairError::Invalid)));
        let n: i64 = c.query_row("SELECT COUNT(*) FROM devices", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn wrong_secret_unknown_device_and_other_tenant_are_invalid() {
        let mut c = setup();
        let p = create_pairing(&c, "t1", "u1", T0).unwrap();
        let cred = redeem(&mut c, "t1", &p.code, "PC", "desktop_client", T_5MIN).unwrap();
        assert_eq!(authenticate(&c, "t1", &cred.device_id, "not-the-secret").unwrap(), DeviceAuth::Invalid);
        assert_eq!(authenticate(&c, "t1", "no-such-device", &cred.secret).unwrap(), DeviceAuth::Invalid);
        assert_eq!(authenticate(&c, "t2", &cred.device_id, &cred.secret).unwrap(), DeviceAuth::Invalid);
    }

    #[test]
    fn revocation_takes_effect_immediately() {
        let mut c = setup();
        let p = create_pairing(&c, "t1", "u1", T0).unwrap();
        let cred = redeem(&mut c, "t1", &p.code, "PC", "desktop_client", T_5MIN).unwrap();
        assert!(revoke_device(&c, "t1", &cred.device_id, T_11MIN).unwrap());
        assert_eq!(authenticate(&c, "t1", &cred.device_id, &cred.secret).unwrap(), DeviceAuth::Revoked);
        assert!(!revoke_device(&c, "t1", &cred.device_id, T_11MIN).unwrap()); // already revoked
        assert!(!revoke_device(&c, "t2", &cred.device_id, T_11MIN).unwrap()); // wrong tenant
    }

    #[test]
    fn secrets_and_codes_are_not_stored_in_plaintext() {
        let mut c = setup();
        let p = create_pairing(&c, "t1", "u1", T0).unwrap();
        let cred = redeem(&mut c, "t1", &p.code, "PC", "desktop_client", T_5MIN).unwrap();
        let stored: String = c.query_row("SELECT credential_hash FROM devices WHERE id=?1", [&cred.device_id], |r| r.get(0)).unwrap();
        assert_ne!(stored, cred.secret);
        assert_eq!(stored.len(), 64);
        let ch: String = c.query_row("SELECT code_hash FROM device_pairings WHERE id=?1", [&p.id], |r| r.get(0)).unwrap();
        assert_ne!(ch, p.code);
    }
}
