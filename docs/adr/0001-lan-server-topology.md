# ADR 0001: Shop Server owns the database; clients are thin

**Status:** accepted

One installer, two modes. In server mode the app runs an embedded HTTPS API and is the only
process that opens the SQLite file. Client PCs have no operational database and call the server
over the LAN (TLS, pinned certificate, per-device credential from a one-time pairing code).

Only the server syncs with the cloud (through the Cloudflare Worker). Header "Sync now" on any
device sends a command to the server.

**Why:** a single writer removes same-shop conflicts (two cashiers cannot both sell the last
unit) and gives one outbox and one cloud identity per shop. SQLite must never be shared over a
network folder.

**Cost:** clients cannot work when the server PC is off. Mitigations: daily backups, a documented
"promote another PC to server" procedure, UPS and static-IP guidance.
