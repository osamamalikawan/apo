//! Development runner for the Shop Server API over HTTPS.
//!
//! Usage: cargo run -p dsaas-lan-api --bin lan-server -- [db_path] [port] [--lan] [--apply-entitlement file.json]
//!   default: listens on 127.0.0.1 only. `--lan` listens on all network interfaces so other PCs on
//!   the shop network can connect (Windows Firewall asks for permission the first time).
//!   `--apply-entitlement` simulates a cloud sync delivering a signed subscription entitlement
//!   (create one with tools/issue-entitlement.mjs). Without a valid one the server is unlicensed
//!   and refuses to pair new devices.
//! On start it creates a dev tenant/admin if the database is empty, prints the certificate
//! fingerprint and a one-time pairing code.
//! The admin has no password until you set one from this console: start with the environment variable
//! DSAAS_ADMIN_PASSWORD (10+ characters). This is also the recovery path if the owner forgets the
//! password. It is never possible over the network. In PowerShell:
//!   $env:DSAAS_ADMIN_PASSWORD = "choose a long password"; cargo run -p dsaas-lan-api --bin lan-server

use axum_server::Handle;
use dsaas_db::{migrate, open, pairing, rusqlite::OptionalExtension, users, Writer};
use dsaas_lan_api::{
    entitlement,
    router,
    tls::{serve, TlsIdentity},
    AppState,
};
use std::{net::SocketAddr, path::Path, sync::Arc};

/// Read a text file as UTF-8, also accepting a UTF-8 BOM or UTF-16 (what Windows PowerShell's ">" writes).
fn read_text_file(path: &str) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    let bad = |e: &dyn std::fmt::Display| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string());
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        let little = bytes[0] == 0xFF;
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|p| if little { u16::from_le_bytes([p[0], p[1]]) } else { u16::from_be_bytes([p[0], p[1]]) })
            .collect();
        return String::from_utf16(&units).map_err(|e| bad(&e));
    }
    let body: &[u8] = &bytes;
    let body = body.strip_prefix(&[0xEFu8, 0xBB, 0xBF]).unwrap_or(body);
    String::from_utf8(body.to_vec()).map_err(|e| bad(&e))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut lan = false;
    let mut ent_file: Option<String> = None;
    let mut pos: Vec<String> = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--lan" => lan = true,
            "--apply-entitlement" => ent_file = it.next(),
            _ => pos.push(a),
        }
    }
    let db_path = pos.first().cloned().unwrap_or_else(|| "shop-dev.db".to_string());
    let port: u16 = pos.get(1).and_then(|p| p.parse().ok()).unwrap_or(8787);

    let mut conn = open(Path::new(&db_path))?;
    migrate(&mut conn)?;

    let existing: Option<String> = conn.query_row("SELECT id FROM tenants LIMIT 1", [], |r| r.get(0)).optional()?;
    if existing.is_none() {
        conn.execute_batch(
            "INSERT INTO tenants(id,name) VALUES ('dev-tenant','Dev Shop');
             INSERT INTO users(id,tenant_id,username,full_name) VALUES ('dev-admin','dev-tenant','admin','Dev Admin');",
        )?;
    }
    let tenant_id: String = conn.query_row("SELECT id FROM tenants LIMIT 1", [], |r| r.get(0))?;
    let admin_id: String = conn.query_row("SELECT id FROM users WHERE tenant_id=?1 LIMIT 1", [&tenant_id], |r| r.get(0))?;
    users::grant_owner(&mut conn, &tenant_id, &admin_id)?;
    let boot_now: String = conn.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| r.get(0))?;
    let admin_login = match std::env::var("DSAAS_ADMIN_PASSWORD") {
        Ok(pw) if !pw.is_empty() => match users::set_password_local(&mut conn, &tenant_id, &admin_id, &pw, &boot_now) {
            Ok(()) => "password set from DSAAS_ADMIN_PASSWORD (remove the variable once you have signed in)".to_string(),
            Err(e) => format!("password NOT set: {e}"),
        },
        _ => {
            let has: bool = conn.query_row(
                "SELECT password_hash IS NOT NULL FROM users WHERE id=?1",
                [&admin_id],
                |r| r.get(0),
            )?;
            if has { "password already set".to_string() } else { "no password yet: set DSAAS_ADMIN_PASSWORD and restart".to_string() }
        }
    };
    let admin_name: String = conn.query_row("SELECT username FROM users WHERE id=?1", [&admin_id], |r| r.get(0))?;

    let pubkey = entitlement::license_pubkey()?;
    if let Some(file) = ent_file {
        let v: serde_json::Value = serde_json::from_str(&read_text_file(&file)?)?;
        let payload = v["payload_b64"].as_str().ok_or("entitlement file has no payload_b64")?;
        let sig = v["signature_b64"].as_str().ok_or("entitlement file has no signature_b64")?;
        match entitlement::apply(&mut conn, &tenant_id, payload, sig, &pubkey) {
            Ok(e) => println!("Applied entitlement revision {} (status: {})", e.revision, e.status),
            Err(e) => println!("Entitlement NOT applied: {e}"),
        }
    }
    let (decision, _) = entitlement::decide(&conn, &tenant_id, &pubkey)?;

    let now: String = conn.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| r.get(0))?;
    let p = pairing::create_pairing(&conn, &tenant_id, &admin_id, &now)?;

    let ident = TlsIdentity::load_or_create(&Path::new(&db_path).with_extension("tls"))?;
    let host = if lan { [0, 0, 0, 0] } else { [127, 0, 0, 1] };
    let addr = SocketAddr::from((host, port));

    println!("Shop Server (dev, HTTPS)");
    println!("  database     : {db_path}");
    println!("  tenant id    : {tenant_id}");
    println!("  listening on : https://{addr}{}", if lan { "  (all network interfaces)" } else { "  (this PC only)" });
    println!("  fingerprint  : {}", ident.fingerprint());
    println!("  license      : {} ({})", decision.mode.as_str(), decision.reason);
    println!("  pairing code : {}   (valid until {})", p.code, p.expires_at);
    println!("  owner login  : {admin_name}: {admin_login}");

    let state = AppState {
        writer: Arc::new(Writer::spawn(conn)),
        tenant_id: tenant_id.as_str().into(),
        server_version: env!("CARGO_PKG_VERSION").into(),
        license_pubkey: pubkey,
    };
    serve(addr, router(state), ident, Handle::new()).await?;
    Ok(())
}
