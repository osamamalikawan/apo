#!/usr/bin/env node
// Pair the desktop UI (development) with a running Shop Server.
//
//   node tools/dev-pair.mjs <pairing code> [server url]
//
// The pairing code is printed by the Shop Server when it starts (see STATUS.md, "Live demo").
// This redeems it once and writes the new device's credentials to apps/desktop/.env.local, which only the
// Vite dev proxy reads (they never reach the browser). Development only: the Shop Server's certificate is
// self-signed, so this script does not verify it. The Tauri app pins the certificate fingerprint instead.
import https from "node:https";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const [code, server = "https://127.0.0.1:8787"] = process.argv.slice(2);
if (!code) {
  console.error("Usage: node tools/dev-pair.mjs <pairing code> [server url]");
  process.exit(1);
}

const body = JSON.stringify({ code: code.trim(), device_name: "Dev UI" });
const req = https.request(
  new URL("/v1/pair/redeem", server),
  {
    method: "POST",
    rejectUnauthorized: false,
    headers: { "content-type": "application/json", "x-api-version": "1", "content-length": Buffer.byteLength(body) },
  },
  (res) => {
    let data = "";
    res.on("data", (c) => (data += c));
    res.on("end", () => {
      let json = {};
      try {
        json = JSON.parse(data);
      } catch {
        /* not JSON */
      }
      if (res.statusCode !== 200) {
        console.error(`The server refused the code (${res.statusCode}): ${json.message ?? data}`);
        console.error("Codes work once and expire after a few minutes. Restart the Shop Server to get a new one.");
        process.exit(1);
      }
      const here = path.dirname(fileURLToPath(import.meta.url));
      const file = path.resolve(here, "../apps/desktop/.env.local");
      fs.writeFileSync(file, `APO_SERVER=${server}\nAPO_DEVICE_ID=${json.device_id}\nAPO_DEVICE_SECRET=${json.secret}\n`, { mode: 0o600 });
      console.log(`Paired. Wrote ${file}`);
      console.log("Now run: npm run dev -w apps/desktop   (then open http://localhost:5173)");
    });
  },
);
req.on("error", (e) => {
  console.error(`Cannot reach ${server}: ${e.message}`);
  console.error("Is the Shop Server running? Start it with: cargo run -p dsaas-lan-api --bin lan-server");
  process.exit(1);
});
req.end(body);
