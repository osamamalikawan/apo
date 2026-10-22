import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App.tsx";
import "./styles.css";

async function start() {
  // `npm run dev:mock` runs against an in-memory demo shop, so the UI can be tried without a Shop Server.
  const fetchFn = __MOCK__ ? (await import("./api/mock.ts")).createMockFetch() : undefined;
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <App fetchFn={fetchFn} />
    </StrictMode>,
  );
}
void start();
