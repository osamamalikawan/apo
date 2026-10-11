//! Subscription entitlements on the Shop Server. NOT YET COMPILED by the author.
//!
//! The cloud (Super Admin) is the only authority. It issues a small payload signed with an Ed25519
//! key that never leaves the cloud; this server holds only the PUBLIC key. The signed payload is
//! stored verbatim and re-verified on every decision, so editing the local database cannot grant
//! access. Rules mirror packages/domain/src/entitlement.ts and share test vectors with it.
//!
//! Offline policy: `full_until` = min(paid-through date, issue time + offline check window) and
//! `grace_until` = full_until + grace days. After `grace_until` (or immediately on suspension or
//! cancellation) the server is READ-ONLY: data stays visible and exportable, nothing is deleted,
//! but no new business writes are accepted. Maximum time an offline shop keeps full access after a
//! revocation is therefore the offline check window (default 7 days) plus grace days.

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use dsaas_db::rusqlite::{params, Connection, OptionalExtension};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Limits {
    pub max_devices: i64,
    pub max_users: i64,
    pub max_locations: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Entitlement {
    pub tenant_id: String,
    pub revision: i64,
    pub status: String,
    pub issued_at: String,
    pub full_until: String,
    pub grace_until: String,
    pub limits: Limits,
}

#[derive(Debug, thiserror::Error)]
pub enum EntError {
    #[error("signature check failed")]
    BadSignature,
    #[error("malformed entitlement: {0}")]
    Malformed(String),
    #[error("entitlement belongs to another tenant")]
    WrongTenant,
    #[error("entitlement is older than the one already applied")]
    Rollback,
    #[error("sqlite: {0}")]
    Sqlite(#[from] dsaas_db::rusqlite::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Full,
    Grace,
    ReadOnly,
    Unlicensed,
}

impl Mode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Mode::Full => "full",
            Mode::Grace => "grace",
            Mode::ReadOnly => "read_only",
            Mode::Unlicensed => "unlicensed",
        }
    }
    pub fn can_write(&self) -> bool {
        matches!(self, Mode::Full | Mode::Grace)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    pub mode: Mode,
    pub reason: &'static str,
}

fn d(mode: Mode, reason: &'static str) -> Decision {
    Decision { mode, reason }
}

/// Pure rule evaluation. All times are "YYYY-MM-DDTHH:MM:SSZ" strings, compared as text.
pub fn evaluate(ent: Option<&Entitlement>, tenant: &str, now: &str, high_water: Option<&str>) -> Decision {
    let Some(e) = ent else { return d(Mode::Unlicensed, "no_entitlement") };
    if e.tenant_id != tenant {
        return d(Mode::Unlicensed, "wrong_tenant");
    }
    let effective = match high_water {
        Some(h) if h > now => h,
        _ => now,
    };
    match e.status.as_str() {
        "suspended" => d(Mode::ReadOnly, "suspended"),
        "cancelled" => d(Mode::ReadOnly, "cancelled"),
        "active" => {
            if effective <= e.full_until.as_str() {
                d(Mode::Full, "active")
            } else if effective <= e.grace_until.as_str() {
                d(Mode::Grace, "grace_period")
            } else {
                d(Mode::ReadOnly, "expired")
            }
        }
        _ => d(Mode::Unlicensed, "invalid_status"),
    }
}

pub fn pubkey_from_b64(s: &str) -> Option<[u8; 32]> {
    let bytes = B64.decode(s.trim()).ok()?;
    <[u8; 32]>::try_from(bytes.as_slice()).ok()
}

fn is_iso_seconds(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 20
        && b[19] == b'Z'
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            10 => *c == b'T',
            13 | 16 => *c == b':',
            19 => *c == b'Z',
            _ => c.is_ascii_digit(),
        })
}

/// Check the signature over the exact payload bytes, then parse and sanity-check the payload.
pub fn verify(payload_b64: &str, signature_b64: &str, pubkey: &[u8; 32]) -> Result<Entitlement, EntError> {
    let payload = B64.decode(payload_b64).map_err(|_| EntError::Malformed("payload is not base64".into()))?;
    let sig_bytes = B64.decode(signature_b64).map_err(|_| EntError::Malformed("signature is not base64".into()))?;
    let sig = Signature::from_slice(&sig_bytes).map_err(|_| EntError::BadSignature)?;
    let key = VerifyingKey::from_bytes(pubkey).map_err(|_| EntError::BadSignature)?;
    key.verify(&payload, &sig).map_err(|_| EntError::BadSignature)?;

    let e: Entitlement = serde_json::from_slice(&payload).map_err(|x| EntError::Malformed(x.to_string()))?;
    if e.revision < 1 {
        return Err(EntError::Malformed("revision must be >= 1".into()));
    }
    if !(is_iso_seconds(&e.issued_at) && is_iso_seconds(&e.full_until) && is_iso_seconds(&e.grace_until)) {
        return Err(EntError::Malformed("dates must be UTC seconds, e.g. 2026-10-10T10:00:00Z".into()));
    }
    if e.full_until > e.grace_until {
        return Err(EntError::Malformed("full_until is after grace_until".into()));
    }
    if e.limits.max_devices < 0 || e.limits.max_users < 0 || e.limits.max_locations < 0 {
        return Err(EntError::Malformed("limits must not be negative".into()));
    }
    Ok(e)
}

/// Verify and store a new entitlement. Older issuances are refused so an old "active" statement
/// cannot be replayed over a newer "suspended" one.
pub fn apply(
    conn: &mut Connection,
    tenant: &str,
    payload_b64: &str,
    signature_b64: &str,
    pubkey: &[u8; 32],
) -> Result<Entitlement, EntError> {
    let ent = verify(payload_b64, signature_b64, pubkey)?;
    if ent.tenant_id != tenant {
        return Err(EntError::WrongTenant);
    }
    let tx = conn.transaction()?;
    let stored: Option<(String, String)> = tx
        .query_row(
            "SELECT payload_b64, signature_b64 FROM entitlement_current WHERE tenant_id=?1",
            [tenant],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((p, s)) = stored {
        // If the stored copy no longer verifies (tampering or corruption) it is simply replaced.
        if let Ok(old) = verify(&p, &s, pubkey) {
            let newer = ent.revision > old.revision || (ent.revision == old.revision && ent.issued_at >= old.issued_at);
            if !newer {
                return Err(EntError::Rollback);
            }
        }
    }
    tx.execute(
        "INSERT INTO entitlement_current(tenant_id,revision,payload_b64,signature_b64,status,applied_at)
         VALUES (?1,?2,?3,?4,?5,strftime('%Y-%m-%dT%H:%M:%SZ','now'))
         ON CONFLICT(tenant_id) DO UPDATE SET revision=excluded.revision, payload_b64=excluded.payload_b64,
           signature_b64=excluded.signature_b64, status=excluded.status, applied_at=excluded.applied_at",
        params![tenant, ent.revision, payload_b64, signature_b64, ent.status],
    )?;
    tx.commit()?;
    Ok(ent)
}

/// The stored entitlement, only if its signature still verifies.
pub fn current(conn: &Connection, tenant: &str, pubkey: &[u8; 32]) -> Result<Option<Entitlement>, dsaas_db::rusqlite::Error> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT payload_b64, signature_b64 FROM entitlement_current WHERE tenant_id=?1",
            [tenant],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(row.and_then(|(p, s)| verify(&p, &s, pubkey).ok()))
}

/// What may this shop do right now? Also advances the clock high-water mark.
pub fn decide(
    conn: &Connection,
    tenant: &str,
    pubkey: &[u8; 32],
) -> Result<(Decision, Option<Entitlement>), dsaas_db::rusqlite::Error> {
    let now: String = conn.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%SZ','now')", [], |r| r.get(0))?;
    let high_water: String = conn.query_row("SELECT high_water FROM clock_guard WHERE id=1", [], |r| r.get(0))?;
    conn.execute("UPDATE clock_guard SET high_water=?1 WHERE high_water < ?1", [&now])?;
    let ent = current(conn, tenant, pubkey)?;
    Ok((evaluate(ent.as_ref(), tenant, &now, Some(&high_water)), ent))
}

/// Public key used to verify entitlements. Release builds MUST supply it via DSAAS_LICENSE_PUBKEY
/// (or a build-time constant added later); only debug builds fall back to the dev key.
pub const DEV_PUBLIC_KEY_B64: &str = "WxqE929qPN3MFhvR97ZQg+VluUYcYd+8p9zl4wZGUKY=";

#[cfg(debug_assertions)]
fn default_pubkey_b64() -> Option<&'static str> {
    Some(DEV_PUBLIC_KEY_B64)
}
#[cfg(not(debug_assertions))]
fn default_pubkey_b64() -> Option<&'static str> {
    None
}

pub fn license_pubkey() -> Result<[u8; 32], String> {
    let raw = match std::env::var("DSAAS_LICENSE_PUBKEY") {
        Ok(v) => v,
        Err(_) => default_pubkey_b64()
            .map(|s| s.to_string())
            .ok_or_else(|| "DSAAS_LICENSE_PUBKEY must be set in release builds".to_string())?,
    };
    pubkey_from_b64(&raw).ok_or_else(|| "license public key is not a base64-encoded 32-byte key".to_string())
}

#[cfg(test)]
pub(crate) mod testvec {
    pub fn load() -> serde_json::Value {
        serde_json::from_str(include_str!("../../../../../../packages/test-vectors/entitlements.json")).unwrap()
    }
    pub fn pubkey(v: &serde_json::Value) -> [u8; 32] {
        super::pubkey_from_b64(v["public_key_b64"].as_str().unwrap()).unwrap()
    }
    pub fn env(v: &serde_json::Value, name: &str) -> (String, String) {
        let e = &v["envelopes"][name];
        (e["payload_b64"].as_str().unwrap().to_string(), e["signature_b64"].as_str().unwrap().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsaas_db::{migrate, open_in_memory};

    fn db() -> Connection {
        let mut c = open_in_memory().unwrap();
        migrate(&mut c).unwrap();
        c.execute_batch("INSERT INTO tenants(id,name) VALUES ('t1','Shop 1');").unwrap();
        c
    }

    #[test]
    fn dev_public_key_constant_matches_the_vectors() {
        let v = testvec::load();
        assert_eq!(DEV_PUBLIC_KEY_B64, v["public_key_b64"].as_str().unwrap());
    }

    #[test]
    fn shared_evaluation_vectors() {
        let v = testvec::load();
        for case in v["evaluation_cases"].as_array().unwrap() {
            let ent = if case["ent"].is_null() {
                None
            } else {
                let e = &case["ent"];
                Some(Entitlement {
                    tenant_id: e["tenant_id"].as_str().unwrap().into(),
                    revision: 1,
                    status: e["status"].as_str().unwrap().into(),
                    issued_at: "2026-10-01T00:00:00Z".into(),
                    full_until: e["full_until"].as_str().unwrap().into(),
                    grace_until: e["grace_until"].as_str().unwrap().into(),
                    limits: Limits { max_devices: 1, max_users: 1, max_locations: 1 },
                })
            };
            let hw = case["high_water"].as_str();
            let got = evaluate(ent.as_ref(), "t1", case["now"].as_str().unwrap(), hw);
            assert_eq!(got.mode.as_str(), case["mode"].as_str().unwrap(), "{}", case["name"]);
            assert_eq!(got.reason, case["reason"].as_str().unwrap(), "{}", case["name"]);
        }
    }

    #[test]
    fn signatures_from_the_issuer_verify_and_forgeries_do_not() {
        let v = testvec::load();
        let pk = testvec::pubkey(&v);
        for name in ["active_rev1", "suspended_rev2", "active_rev3", "expired_rev1"] {
            let (p, s) = testvec::env(&v, name);
            assert!(verify(&p, &s, &pk).is_ok(), "{name}");
        }
        let (p, s) = testvec::env(&v, "active_rev1");
        let other = pubkey_from_b64(v["other_public_key_b64"].as_str().unwrap()).unwrap();
        assert!(matches!(verify(&p, &s, &other), Err(EntError::BadSignature)));
        let t = &v["tampered"];
        assert!(matches!(
            verify(t["payload_b64"].as_str().unwrap(), t["signature_b64"].as_str().unwrap(), &pk),
            Err(EntError::BadSignature)
        ));
        assert!(matches!(verify("!!!", &s, &pk), Err(EntError::Malformed(_))));
    }

    #[test]
    fn apply_enforces_tenant_and_refuses_rollback_but_allows_refresh() {
        let v = testvec::load();
        let pk = testvec::pubkey(&v);
        let mut c = db();
        let ap = |c: &mut Connection, n: &str| {
            let (p, s) = testvec::env(&v, n);
            apply(c, "t1", &p, &s, &pk)
        };
        assert!(matches!(ap(&mut c, "wrong_tenant"), Err(EntError::WrongTenant)));
        assert!(ap(&mut c, "active_rev1").is_ok());
        assert!(ap(&mut c, "active_rev1_newer").is_ok(), "a newer issuance at the same revision is a refresh");
        assert!(matches!(ap(&mut c, "active_rev1_older"), Err(EntError::Rollback)));
        assert!(ap(&mut c, "suspended_rev2").is_ok());
        assert!(matches!(ap(&mut c, "active_rev1"), Err(EntError::Rollback)), "old 'active' must not replace 'suspended'");
        assert_eq!(decide(&c, "t1", &pk).unwrap().0.reason, "suspended");
        assert!(ap(&mut c, "active_rev3").is_ok());
        let (dec, ent) = decide(&c, "t1", &pk).unwrap();
        assert_eq!(dec.mode, Mode::Full);
        assert_eq!(ent.unwrap().limits.max_devices, 2);
    }

    #[test]
    fn no_entitlement_means_unlicensed_and_editing_the_database_changes_nothing() {
        let v = testvec::load();
        let pk = testvec::pubkey(&v);
        let mut c = db();
        assert_eq!(decide(&c, "t1", &pk).unwrap().0.mode, Mode::Unlicensed);

        let (p, s) = testvec::env(&v, "suspended_rev2");
        apply(&mut c, "t1", &p, &s, &pk).unwrap();
        // someone edits the status column to 'active': the signed payload still says suspended
        c.execute("UPDATE entitlement_current SET status='active'", []).unwrap();
        assert_eq!(decide(&c, "t1", &pk).unwrap().0.reason, "suspended");
        // someone swaps in a tampered payload: signature no longer verifies, so no license
        let t = &v["tampered"];
        c.execute(
            "UPDATE entitlement_current SET payload_b64=?1, signature_b64=?2",
            [t["payload_b64"].as_str().unwrap(), t["signature_b64"].as_str().unwrap()],
        )
        .unwrap();
        assert_eq!(decide(&c, "t1", &pk).unwrap().0.mode, Mode::Unlicensed);
    }

    #[test]
    fn rewinding_the_clock_cannot_revive_an_expired_subscription() {
        let v = testvec::load();
        let pk = testvec::pubkey(&v);
        let mut c = db();
        let (p, s) = testvec::env(&v, "active_rev1"); // valid until 2099
        apply(&mut c, "t1", &p, &s, &pk).unwrap();
        assert_eq!(decide(&c, "t1", &pk).unwrap().0.mode, Mode::Full);
        // this database has (hypothetically) already seen a time after grace_until
        c.execute("UPDATE clock_guard SET high_water='2150-01-01T00:00:00Z'", []).unwrap();
        let (dec, _) = decide(&c, "t1", &pk).unwrap();
        assert_eq!((dec.mode, dec.reason), (Mode::ReadOnly, "expired"));
        // and the stored mark never goes backwards
        let hw: String = c.query_row("SELECT high_water FROM clock_guard", [], |r| r.get(0)).unwrap();
        assert_eq!(hw, "2150-01-01T00:00:00Z");
    }

    #[test]
    fn expired_vector_is_read_only() {
        let v = testvec::load();
        let pk = testvec::pubkey(&v);
        let mut c = db();
        let (p, s) = testvec::env(&v, "expired_rev1");
        apply(&mut c, "t1", &p, &s, &pk).unwrap();
        let (dec, _) = decide(&c, "t1", &pk).unwrap();
        assert_eq!((dec.mode, dec.reason), (Mode::ReadOnly, "expired"));
    }
}
