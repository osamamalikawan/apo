# supabase/ (cloud Postgres)
Apply migrations in order in the SQL Editor; each has a test script in `tests/` that rolls itself back and reports
`ALL n CHECKS PASSED` as its (intentional) error message.

| Migration | Content | Test | State |
|---|---|---|---|
| 0001 | tenants, members, roles, locations, products, ledger, balances, RLS, `post_stock_movement` | `rls_test.sql` | applied and verified |
| 0002 | revoke anon access | (covered by rls_test #22) | applied and verified |
| 0003 | fix: location/product ownership checks | `rls_test.sql` (24 checks) | applied and verified |
| 0004 | plans, tenant_subscriptions, platform_admins, audit, `superadmin_set_subscription`, `tenant_entitlement_payload` | `superadmin_test.sql` (20 checks) | applied and verified on project `apo` (20/20) |
| 0005 | customers (read-only via RLS), category revisions, unique barcode per shop | `customers_test.sql` (17 checks) | written; dry-run verified on `apo`, not applied yet |

**Security model:** RLS enabled and forced on every table; clients have SELECT only; writes go through `SECURITY DEFINER` RPCs that check
membership, tenant status, permission, scope and idempotency. Mobile talks to these directly. Service-role key lives only in the Worker.
**Rules:** new tables need RLS + explicit revokes (Supabase grants everything to `authenticated` by default); new RPCs revoke from `public, anon`, and RPCs that signed-in users must not call (like `tenant_entitlement_payload`) also revoke from `authenticated`.
**Applying by hand:** migrations are not re-runnable (a second run fails with "already exists"), so check what exists first (`to_regclass('public.x')`, `to_regprocedure('public.f(uuid)')`). After applying, always run the matching test: a hand-applied function once kept a grant the migration removes, and only the test caught it. Do not paste files from `apps/desktop/.../db/migrations` here; those are SQLite.
