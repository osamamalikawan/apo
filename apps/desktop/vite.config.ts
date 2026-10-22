import { loadEnv } from "vite";
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import path from "node:path";

// Development against a real Shop Server:
//   1. start it:                cargo run -p dsaas-lan-api --bin lan-server
//   2. pair this UI once:       node tools/dev-pair.mjs <pairing code>   (writes apps/desktop/.env.local)
//   3. start the UI:            npm run dev -w apps/desktop
// The browser only talks to this dev server (same origin). The proxy below forwards /v1 to the Shop Server,
// accepts its self-signed certificate, and adds the paired device's credentials, so they never reach the page.
// `npm run dev:mock -w apps/desktop` runs the UI against an in-memory demo shop instead (no server needed).
export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), "APO_");
  return {
    plugins: [react()],
    resolve: { alias: { "@dsaas/domain": path.resolve(import.meta.dirname, "../../packages/domain/src/index.ts") } },
    server: {
      port: 5173,
      proxy: {
        "/v1": {
          target: env.APO_SERVER || "https://127.0.0.1:8787",
          secure: false,
          changeOrigin: true,
          headers: {
            "x-api-version": "1",
            "x-device-id": env.APO_DEVICE_ID || "",
            "x-device-secret": env.APO_DEVICE_SECRET || "",
          },
        },
      },
    },
    define: { __MOCK__: JSON.stringify(mode === "mock") },
    test: {
      environment: "jsdom",
      globals: true,
      setupFiles: ["./test/setup.ts"],
      css: false,
      restoreMocks: true,
      testTimeout: 20000,
    },
  };
});
