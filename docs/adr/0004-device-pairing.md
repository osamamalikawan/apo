# ADR 0004: Device pairing and credentials

**Status:** accepted; core logic in `crates/db/src/pairing.rs` (Rust tests written, run by owner).

- An authorized user creates a one-time 8-digit code on the server. It expires after 10 minutes,
  works once, and locks after 5 failed attempts (counted against all open pairings of the shop).
- Redeeming returns a device id and a 256-bit random secret, shown once. Only SHA-256 hashes of
  the code (salted with the pairing id) and of the secret are stored. Comparisons are constant-time.
- Failures are deliberately indistinguishable (wrong, expired, used, locked).
- Revocation is immediate and tenant-scoped. A revoked credential is reported as `Revoked` so the
  client can show "this PC was removed by the administrator".
- Not yet built: the HTTPS/TLS transport, certificate-fingerprint confirmation screen, mDNS
  discovery, per-IP rate limiting at the HTTP layer, and the permission check on "create pairing".
