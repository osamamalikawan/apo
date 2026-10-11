// DEV stand-in for the cloud issuer (production: the Cloudflare Worker signs with a secret key).
// Usage:
//   node tools/issue-entitlement.mjs <tenant_id> <status> [days_valid=30] [max_devices=3] [revision=1] [--out file.json]
// With --out the file is written as plain UTF-8 (recommended on Windows: PowerShell's ">" redirect
// writes UTF-16, which is not what you want). Then:  lan-server shop.db --apply-entitlement file.json
import fs from "node:fs";
import { devKey, sign } from "./dev-license-key.mjs";

const args = process.argv.slice(2);
const oi = args.indexOf("--out");
const out = oi >= 0 ? args.splice(oi, 2)[1] : null;
const [tenant, status = "active", days = "30", maxDevices = "3", revision = "1"] = args;
if (!tenant) {
  console.error("usage: issue-entitlement.mjs <tenant_id> <status> [days] [max_devices] [revision] [--out file.json]");
  process.exit(1);
}
const iso = (d) => d.toISOString().replace(/\.\d{3}Z$/, "Z");
const now = new Date();
const fullUntil = new Date(now.getTime() + Number(days) * 86400000);
const graceUntil = new Date(fullUntil.getTime() + 7 * 86400000);
const payload = {
  tenant_id: tenant, revision: Number(revision), status, issued_at: iso(now),
  full_until: iso(fullUntil), grace_until: iso(graceUntil),
  limits: { max_devices: Number(maxDevices), max_users: 20, max_locations: 10 }, modules: ["sales", "inventory"],
};
const text = JSON.stringify(sign(devKey().priv, payload), null, 2) + "\n";
if (out) { fs.writeFileSync(out, text, "utf8"); console.log(`wrote ${out} (tenant ${tenant}, status ${status}, revision ${revision})`); }
else process.stdout.write(text);
