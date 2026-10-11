// DEV ONLY signing key. Production signing happens in the Cloudflare Worker with a secret key that
// never leaves Cloudflare; the shop server embeds only the matching PUBLIC key.
import crypto from "node:crypto";

const seed = (label) => crypto.createHash("sha256").update(label).digest();
const PKCS8_PREFIX = Buffer.from("302e020100300506032b657004220420", "hex");
const SPKI_PREFIX = Buffer.from("302a300506032b6570032100", "hex");

export function keypairFromLabel(label) {
  const priv = crypto.createPrivateKey({ key: Buffer.concat([PKCS8_PREFIX, seed(label)]), format: "der", type: "pkcs8" });
  const raw = crypto.createPublicKey(priv).export({ format: "der", type: "spki" }).subarray(-32);
  return { priv, publicKeyB64: raw.toString("base64") };
}
export const devKey = () => keypairFromLabel("dsaas-dev-license-key-DO-NOT-USE-IN-PRODUCTION");
export const otherKey = () => keypairFromLabel("dsaas-some-other-key");

export function sign(priv, payloadObj) {
  const payload = Buffer.from(JSON.stringify(payloadObj));
  return { payload_b64: payload.toString("base64"), signature_b64: crypto.sign(null, payload, priv).toString("base64") };
}
export function verify(publicKeyB64, payload_b64, signature_b64) {
  const key = crypto.createPublicKey({ key: Buffer.concat([SPKI_PREFIX, Buffer.from(publicKeyB64, "base64")]), format: "der", type: "spki" });
  return crypto.verify(null, Buffer.from(payload_b64, "base64"), key, Buffer.from(signature_b64, "base64"));
}
