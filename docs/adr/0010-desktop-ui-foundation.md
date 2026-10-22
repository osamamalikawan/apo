# ADR 0010: Desktop UI foundation

Status: accepted. Covers the first screens of `apps/desktop` (build step 8 started early, before the Tauri shell).

## Decision
- **Stack:** React 18 + TypeScript on Vite, as planned. Hash routing (`#/customers`, `#/settings/roles`) so it behaves the same in a
  browser and in Tauri. No UI framework or router library: the screens are small and the prototype's own CSS is reused.
- **The prototype is the design.** Colours, layout, light/dark themes and component styles come from the Irfan Autos HTML prototype.
  Its pages are all in the sidebar. Pages whose server part does not exist yet are marked "Coming soon" and open a page that says
  what they will do and in which build step; they show no invented data.
- **Sidebar and header.** The sidebar collapses to an icon rail with one button at the left of the header (`aria-expanded`,
  `aria-controls`, remembered between runs). On narrow windows it starts collapsed and opens over the content.
- **PIN-style sign-in.** Tap your name, then use the on-screen pad (digits, backspace, enter; the keyboard works too). PINs are 4 to
  8 digits, so the pad has an explicit Sign in key instead of guessing the length. The password form stays as the fallback and is
  the only way in before anyone has a PIN. The tiles come from `GET /v1/login-users`, which needs a paired device but no sign-in and
  lists only active users who have a PIN (id, username, full name). That reveals who works in the shop to whoever sits at a paired PC;
  it is the normal trade-off for a counter PIN screen, and wrong guesses are still limited by the per-account lockout (ADR 0007).
  `GET /v1/whoami` now also returns the shop's name, shown on the sign-in screen and in the sidebar.
- **Transport.** The UI calls relative `/v1/...` URLs. In development the Vite proxy forwards them to the Shop Server, accepts its
  self-signed certificate and injects the paired device's credentials, which live in a git-ignored `.env.local` and never reach the
  page. In the Tauri app the same calls will go through the Rust side, which pins the certificate. The browser never holds device credentials.
- **The UI enforces nothing.** It hides or disables what a person cannot do and mirrors the server's "grant only what you hold"
  rule (`src/permissions.ts`) to avoid offering refused choices. The server decides.
- **Demo shop.** `src/api/mock.ts` is an in-memory shop with the server's shapes, error codes and main rules. It drives the tests
  and `npm run dev:ui:mock`. It is not a second implementation; the Rust tests are the authority.
- **Dev tooling security.** Vite 8 and Vitest 5 are used because older versions let any website send requests to a local dev server
  and read the answers, which matters because the dev proxy injects device credentials. The dev server is checked to send no
  permissive CORS header and to refuse spoofed `Host` headers.

## Consequences
- Sessions live in `sessionStorage` (cleared when the window closes). The server's own limits (12 hours, 2 hours idle) still apply.
- `PUT` replaces whole records, so forms send every field, including ones they do not show (for example a location's responsible user).
- The screens are verified by jsdom tests, not by eye; a visual review of `npm run dev:ui:mock` is part of accepting this work.
