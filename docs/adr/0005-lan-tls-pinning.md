# ADR 0005: LAN TLS with a self-signed, pinned certificate

**Status:** accepted; implemented in `crates/lan-api/src/tls.rs`, tests written, run by owner.

- The server generates a self-signed certificate once (`server-cert.der`, `server-key.der` next to the database).
- No certificate authority is involved. At pairing time the admin compares the SHA-256 fingerprint
  shown on the server with the one shown on the client; the client then pins it.
- TLS uses rustls with the `ring` provider to avoid C/NASM toolchain requirements on Windows.
- Regenerating the certificate changes the fingerprint; every client must re-pair.
- Known gaps: the private key is a plain file (OS credential store planned); no certificate rotation;
  no mDNS discovery yet; the client side of pinning (a custom verifier) is built with the client app.
