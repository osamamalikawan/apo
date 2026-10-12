# ADR 0008: Roles, permissions and the shared authorization check

Status: accepted. Builds on ADR 0007 (users and sessions) and ADR 0006 (subscription enforcement).

## Decision
- **Permissions are strings** `module.action`; `*` grants everything and `module.*` a whole module. The server keeps a **registry**
  (`dsaas_db::roles::PERMISSIONS`). A role may only contain registered strings, `*` or `module.*` of a registered module, so a typo is
  rejected instead of silently granting nothing. Adding a permission means adding it to the registry; a test fails if the shared
  vectors use a string that is not registered.
- **Roles are named sets of permissions; a user holds any number of roles and gets the union.** Permissions and location access are
  re-read on every request, so a change applies immediately to sessions that are already signed in.
- **No escalation.** You can only grant, edit, delete or take away what you hold yourself (`*` and `module.*` are only covered by `*`
  or the identical string). Someone with `roles.manage` but not `*` can never create an Owner-like role, give one out, or change one.
  Location access follows the same rule: unless you hold `locations.all`, you may only add or remove locations you can use yourself.
- **Owner is protected.** The Owner role cannot be renamed, edited or deleted through the API, and every change that could remove a
  role or permission re-counts active holders of `*`; if that would leave none, the whole change is refused and rolled back.
- **Scope.** Location scope comes from `user_location_access` unless the user holds `locations.all` (or `*`). Route scope has no
  tables yet (build step 10): until then only `routes.all` or `*` passes a route check, and everything else is denied (default-deny).
- **One shared gate.** Every endpoint that acts as a user calls `authz::check` inside its database closure, in this order:
  password sign-in (for sensitive actions), permission, location scope, route scope, and for writes the subscription. The subscription
  comes last so someone without permission learns nothing about it. Reads are never blocked by the subscription.
- **Same rules in two languages.** `AccessContext::authorize` in Rust mirrors `authorize` in `packages/domain/src/permissions.ts`
  and is tested against the same vectors (`packages/test-vectors/permissions.json`), including scope reasons.
- Every role and assignment change is written to the audit log with before/after.

## Consequences
- New users start with no roles. An owner (or a role manager) assigns roles through `PUT /v1/users/:id/roles`.
- Cutting off a user's access takes effect on their next request; their session stays valid but becomes powerless.
  Deactivating the user still ends the session outright (ADR 0007).
- The Postgres side (cloud) keeps its own `has_permission` / `can_access_location`; the registry is not mirrored there yet.
