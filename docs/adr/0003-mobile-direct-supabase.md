# ADR 0003: Mobile talks to Supabase directly; Postgres is its security boundary

**Status:** accepted; RLS and RPC written (`supabase/migrations/0001_foundation.sql`), not yet executed.

- RLS is enabled and forced on every tenant table. Clients (`anon`, `authenticated`) have SELECT only.
- Writes go through `SECURITY DEFINER` RPCs (first one: `post_stock_movement`) that check
  membership, tenant status, permission, location scope, negative-stock permission and idempotency.
- The service-role key lives only in Cloudflare secrets and CI.
- Desktop shop servers still use the Worker, which calls the same RPCs or equivalent logic.
