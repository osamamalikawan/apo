# Desktop UI (`apps/desktop`)

The shop's screens: React + TypeScript on Vite. It talks only to the Shop Server API (`apps/desktop/src-tauri/crates/lan-api`).
The Tauri shell that will wrap it (build step 8) is not built yet; until then the UI runs in a browser.

## Run it

Try every page with no server at all, using an in-memory demo shop (the sample data is Irfan Autos'):
```
npm install                      # once, from the repo root
npm run dev:ui:mock              # then open http://localhost:5173
```
Demo people and PINs: Irfan Ahmed 1111 (owner), Sana Malik 3333 (cashier), Tariq Hussain 4444 (storekeeper).
Their passwords are `irfan-password-1`, `sana-password-1`, `tariq-password-1` (needed to change users, roles, PINs).

Run it against a real Shop Server:
```
# 1. start the server and note the pairing code it prints (see STATUS.md, "Live demo")
cd apps\desktop\src-tauri
$env:DSAAS_ADMIN_PASSWORD = "a long password"; cargo run -p dsaas-lan-api --bin lan-server -- --apply-entitlement ..\..\..\ent.json
# 2. from the repo root, pair this UI once (writes apps/desktop/.env.local, which is git-ignored)
node tools/dev-pair.mjs <pairing code>
# 3. start the UI
npm run dev:ui
```
The browser only talks to the dev server. Its proxy forwards `/v1` to the Shop Server, accepts the server's self-signed certificate
and adds the paired device's credentials, so the credentials never reach the page. Sign in as `admin` with your password, then set a
PIN under Settings, My account.

## Tests
`npm run test:ui` (101 tests, jsdom) and `npm run typecheck:ui`. The tests drive the real screens through an in-memory shop
(`src/api/mock.ts`); that mock mirrors the server's shapes and main rules but is not a second implementation: the Rust tests are the authority.
The test environment cannot judge how the screens *look*; someone needs to open `npm run dev:ui:mock` and look.

## Where things are
| Path | What |
|---|---|
| `src/App.tsx`, `src/nav.ts` | the app shell, routing (`#/page/tab`), which pages exist and who sees them |
| `src/ui/Shell.tsx` | sidebar and header; the header's first button collapses and expands the sidebar |
| `src/pages/Login.tsx`, `src/ui/PinPad.tsx` | PIN-style sign-in (tap your name, then the pad; password form as a fallback) |
| `src/pages/*` | Dashboard, Customers, Inventory (Products, Locations), Settings (My account, Users, Roles), Coming soon |
| `src/api/` | the typed client, plain-language error messages, the demo shop |
| `src/state/` | session, theme, sidebar, toasts, subscription status |
| `src/styles.css` | the prototype's design tokens and components |

## Rules for this folder
- Every call goes through `src/api/client.ts`. The UI holds no business rules: it hides or disables what a person cannot do, but the
  server decides (ADR 0008). Permission checks in the UI are for display only.
- `PUT` replaces the whole record: send every field and the `expected_revision` you read (ADR 0009). On `409 stale_revision`, say so, reload, close the form.
- Say what happened in plain words (`src/api/errors.ts`) and what to do next. Buttons say what they do ("Save customer"), not "Submit".
- Keyboard first: every action works without a mouse, focus is visible, dialogs are native `<dialog>`.
- A page whose server part does not exist stays a "Coming soon" page in `src/nav.ts`; do not fake data in a real page.
