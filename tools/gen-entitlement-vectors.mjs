// Regenerates packages/test-vectors/entitlements.json. Ed25519 signatures are deterministic, so
// the output is reproducible. Run: node tools/gen-entitlement-vectors.mjs
import fs from "node:fs";
import { devKey, otherKey, sign } from "./dev-license-key.mjs";

const k = devKey();
const base = {
  tenant_id: "t1", revision: 1, status: "active", issued_at: "2026-10-01T00:00:00Z",
  full_until: "2099-12-31T00:00:00Z", grace_until: "2100-01-07T00:00:00Z",
  limits: { max_devices: 2, max_users: 10, max_locations: 5 }, modules: ["sales", "inventory"],
};
const make = (over) => { const p = { ...base, ...over }; return { payload: p, ...sign(k.priv, p) }; };

const envelopes = {
  active_rev1: make({}),
  active_rev1_older: make({ issued_at: "2026-09-01T00:00:00Z" }),
  active_rev1_newer: make({ issued_at: "2026-10-05T00:00:00Z" }),
  suspended_rev2: make({ revision: 2, status: "suspended" }),
  active_rev3: make({ revision: 3 }),
  expired_rev1: make({ full_until: "2020-01-01T00:00:00Z", grace_until: "2020-01-08T00:00:00Z" }),
  wrong_tenant: make({ tenant_id: "other" }),
};
// valid signature over the ORIGINAL payload, but the payload text has been altered afterwards
const orig = envelopes.active_rev1;
const alteredPayload = Buffer.from(orig.payload_b64, "base64").toString().replace("2099-12-31", "2199-12-31");
const tampered = { payload_b64: Buffer.from(alteredPayload).toString("base64"), signature_b64: orig.signature_b64 };

const E = (status, full, grace, tenant = "t1") => ({ tenant_id: tenant, status, full_until: full, grace_until: grace });
const FU = "2026-11-01T00:00:00Z", GU = "2026-11-08T00:00:00Z";
const evaluation_cases = [
  { name: "no entitlement", ent: null, now: "2026-10-10T00:00:00Z", high_water: null, mode: "unlicensed", reason: "no_entitlement" },
  { name: "active within period", ent: E("active", FU, GU), now: "2026-10-10T00:00:00Z", high_water: null, mode: "full", reason: "active" },
  { name: "full_until is inclusive", ent: E("active", FU, GU), now: FU, high_water: null, mode: "full", reason: "active" },
  { name: "one second after period -> grace", ent: E("active", FU, GU), now: "2026-11-01T00:00:01Z", high_water: null, mode: "grace", reason: "grace_period" },
  { name: "grace_until is inclusive", ent: E("active", FU, GU), now: GU, high_water: null, mode: "grace", reason: "grace_period" },
  { name: "after grace -> read only", ent: E("active", FU, GU), now: "2026-11-08T00:00:01Z", high_water: null, mode: "read_only", reason: "expired" },
  { name: "suspended beats valid dates", ent: E("suspended", FU, GU), now: "2026-10-10T00:00:00Z", high_water: null, mode: "read_only", reason: "suspended" },
  { name: "cancelled", ent: E("cancelled", FU, GU), now: "2026-10-10T00:00:00Z", high_water: null, mode: "read_only", reason: "cancelled" },
  { name: "another tenant's entitlement", ent: E("active", FU, GU, "other"), now: "2026-10-10T00:00:00Z", high_water: null, mode: "unlicensed", reason: "wrong_tenant" },
  { name: "unknown status", ent: E("weird", FU, GU), now: "2026-10-10T00:00:00Z", high_water: null, mode: "unlicensed", reason: "invalid_status" },
  { name: "clock rolled back past high-water mark: still expired", ent: E("active", FU, GU), now: "2026-10-10T00:00:00Z", high_water: "2026-12-01T00:00:00Z", mode: "read_only", reason: "expired" },
  { name: "high-water mark inside period changes nothing", ent: E("active", FU, GU), now: "2026-10-10T00:00:00Z", high_water: "2026-10-12T00:00:00Z", mode: "full", reason: "active" },
];

const out = {
  public_key_b64: k.publicKeyB64,
  other_public_key_b64: otherKey().publicKeyB64,
  envelopes, tampered, evaluation_cases,
};
fs.writeFileSync(new URL("../packages/test-vectors/entitlements.json", import.meta.url), JSON.stringify(out, null, 2) + "\n");
console.log("dev public key:", k.publicKeyB64);
