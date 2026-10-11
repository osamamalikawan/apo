//! TLS for the Shop Server. NOT YET COMPILED by the author; run `cargo test -p dsaas-lan-api`.
//!
//! The server creates a self-signed certificate once and keeps it. Clients do NOT trust it through
//! a certificate authority: during pairing the admin compares the SHA-256 fingerprint shown on the
//! server with the one shown on the client, and the client then pins that fingerprint.
//! Regenerating the certificate changes the fingerprint, so clients must re-pair.
//!
//! Key storage note: `server-key.der` is a plain file in the app data directory (mode 0600 on
//! Unix; on Windows it relies on the user profile's permissions). Moving it into the OS credential
//! store (DPAPI) is planned and listed in STATUS.md.

use axum::Router;
use axum_server::{tls_rustls::RustlsConfig, Handle};
use sha2::{Digest, Sha256};
use std::{error::Error, fs, io, net::SocketAddr, path::Path};

#[derive(Clone)]
pub struct TlsIdentity {
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
}

impl TlsIdentity {
    pub fn generate() -> Result<Self, Box<dyn Error>> {
        let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?;
        Ok(TlsIdentity {
            cert_der: ck.cert.der().to_vec(),
            key_der: ck.key_pair.serialize_der(),
        })
    }

    /// Load the stored identity, or create and store a new one.
    pub fn load_or_create(dir: &Path) -> Result<Self, Box<dyn Error>> {
        let cert_path = dir.join("server-cert.der");
        let key_path = dir.join("server-key.der");
        if cert_path.exists() && key_path.exists() {
            return Ok(TlsIdentity { cert_der: fs::read(&cert_path)?, key_der: fs::read(&key_path)? });
        }
        fs::create_dir_all(dir)?;
        let id = Self::generate()?;
        fs::write(&cert_path, &id.cert_der)?;
        write_private(&key_path, &id.key_der)?;
        Ok(id)
    }

    /// SHA-256 of the certificate, formatted for people to compare: "AB:CD:...".
    pub fn fingerprint(&self) -> String {
        Sha256::digest(&self.cert_der)
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":")
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
        f.write_all(bytes)
    }
    #[cfg(not(unix))]
    {
        fs::write(path, bytes)
    }
}

pub fn install_crypto_provider() {
    // Err means a provider was already installed, which is fine.
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Serve the API over HTTPS until the process ends or `handle` shuts it down.
pub async fn serve(addr: SocketAddr, app: Router, ident: TlsIdentity, handle: Handle) -> io::Result<()> {
    install_crypto_provider();
    let config = RustlsConfig::from_der(vec![ident.cert_der], ident.key_der).await?;
    axum_server::bind_rustls(addr, config)
        .handle(handle)
        .serve(app.into_make_service())
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{router, AppState};
    use dsaas_db::{migrate, open_in_memory, Writer};
    use rustls::pki_types::{CertificateDer, ServerName};
    use rustls::{ClientConfig, RootCertStore};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio_rustls::TlsConnector;

    fn state() -> AppState {
        let mut c = open_in_memory().unwrap();
        migrate(&mut c).unwrap();
        c.execute_batch("INSERT INTO tenants(id,name) VALUES ('t1','Shop 1');").unwrap();
        AppState { writer: Arc::new(Writer::spawn(c)), tenant_id: "t1".into(), server_version: "0.0.1".into(), license_pubkey: [0u8; 32] }
    }

    async fn start(ident: &TlsIdentity) -> (u16, Handle) {
        let handle = Handle::new();
        tokio::spawn(serve("127.0.0.1:0".parse().unwrap(), router(state()), ident.clone(), handle.clone()));
        let addr = handle.listening().await.expect("server did not start");
        (addr.port(), handle)
    }

    fn connector(trusted_der: &[u8]) -> TlsConnector {
        install_crypto_provider();
        let mut roots = RootCertStore::empty();
        roots.add(CertificateDer::from(trusted_der.to_vec())).unwrap();
        let cfg = ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
        TlsConnector::from(Arc::new(cfg))
    }

    #[tokio::test]
    async fn https_works_for_a_client_that_trusts_the_servers_certificate() {
        let ident = TlsIdentity::generate().unwrap();
        let (port, _h) = start(&ident).await;
        let tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let name = ServerName::try_from("localhost".to_string()).unwrap();
        let mut tls = connector(&ident.cert_der).connect(name, tcp).await.expect("handshake failed");
        tls.write_all(b"GET /v1/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").await.unwrap();
        let mut buf = Vec::new();
        let _ = tls.read_to_end(&mut buf).await; // a missing close_notify may surface as an error; the body is what matters
        let text = String::from_utf8_lossy(&buf);
        assert!(text.starts_with("HTTP/1.1 200"), "got: {text}");
        assert!(text.contains("\"status\":\"ok\""), "got: {text}");
    }

    #[tokio::test]
    async fn a_client_pinned_to_a_different_certificate_is_refused() {
        let real = TlsIdentity::generate().unwrap();
        let impostor_view = TlsIdentity::generate().unwrap(); // the client expects THIS certificate
        let (port, _h) = start(&real).await;
        let tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let name = ServerName::try_from("localhost".to_string()).unwrap();
        let res = connector(&impostor_view.cert_der).connect(name, tcp).await;
        assert!(res.is_err(), "client accepted a certificate it was not pinned to");
    }

    #[tokio::test]
    async fn plain_http_to_the_tls_port_does_not_return_api_data() {
        let ident = TlsIdentity::generate().unwrap();
        let (port, _h) = start(&ident).await;
        let mut tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        tcp.write_all(b"GET /v1/health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").await.unwrap();
        let mut buf = Vec::new();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), tcp.read_to_end(&mut buf)).await;
        assert!(!String::from_utf8_lossy(&buf).contains("\"status\":\"ok\""));
    }

    #[test]
    fn identity_is_persisted_and_fingerprint_is_stable_and_well_formed() {
        let dir = std::env::temp_dir().join(format!(
            "dsaas-tls-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let a = TlsIdentity::load_or_create(&dir).unwrap();
        let b = TlsIdentity::load_or_create(&dir).unwrap();
        assert_eq!(a.cert_der, b.cert_der);
        assert_eq!(a.fingerprint(), b.fingerprint());
        let f = a.fingerprint();
        assert_eq!(f.len(), 32 * 3 - 1);
        assert!(f.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase() || c == ':'));
        assert_ne!(f, TlsIdentity::generate().unwrap().fingerprint());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
