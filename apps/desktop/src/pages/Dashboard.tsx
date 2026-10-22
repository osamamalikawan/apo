import type { Location, Page, UserRow } from "../api/types.ts";
import { useLicense } from "../state/license.tsx";
import { useSession } from "../state/session.tsx";
import { useAsync } from "../state/useAsync.ts";

interface Counts { customers: number | null; products: number | null; locations: number | null; users: number | null }

export function Dashboard() {
  const { api, can, me } = useSession();
  const { license } = useLicense();
  const { data } = useAsync<Counts>(async () => {
    const total = async (path: string) => {
      try {
        return (await api.get<Page<unknown>>(path)).total;
      } catch {
        return null;
      }
    };
    const [customers, products, locations, users] = await Promise.all([
      can("customers.view") ? total("/v1/customers?limit=1") : null,
      can("products.view") ? total("/v1/products?limit=1") : null,
      api.get<Location[]>("/v1/locations").then((l) => l.filter((x) => x.is_active).length, () => null),
      can("users.manage") ? api.get<UserRow[]>("/v1/users").then((u) => u.filter((x) => x.is_active).length, () => null) : null,
    ]);
    return { customers, products, locations, users };
  }, [api]);

  const tiles: [string, number | null | undefined][] = [
    ["Customers", data?.customers],
    ["Products", data?.products],
    ["Locations", data?.locations],
    ["Users", data?.users],
  ];
  const shown = tiles.filter(([, v]) => v !== null && v !== undefined) as [string, number][];
  const empty = shown.filter(([, v]) => v === 0).map(([k]) => k.toLowerCase());
  const l = license?.limits;

  return (
    <>
      <p style={{ marginTop: 0 }}>Welcome, {me?.user.full_name}.</p>
      {shown.length > 0 && (
        <div className="tiles" data-testid="tiles">
          {shown.map(([label, value]) => (
            <div key={label} className="tile">
              <span>{label}</span>
              <b>{value}</b>
            </div>
          ))}
        </div>
      )}
      <div className="cols">
        {empty.length > 0 && (
          <section className="panel" aria-labelledby="start-h">
            <h2 id="start-h">Getting started</h2>
            <p>
              Nothing here yet for: {empty.join(", ")}. Add your customers and products, and set up your locations, so they are ready for sales and stock.
            </p>
            <p className="sm">
              <a href="#/customers">Customers</a> · <a href="#/inventory">Products and locations</a>
            </p>
          </section>
        )}
        {license && (
          <section className="panel" aria-labelledby="plan-h">
            <h2 id="plan-h">Your plan</h2>
            <p>
              {license.mode === "full" && "Active"}
              {license.mode === "grace" && "In the grace period: renew soon to avoid read-only mode"}
              {license.mode === "read_only" && "Not active: you can view everything, but changes are turned off"}
              {license.mode === "unlicensed" && "No subscription found"}
              {license.full_until && license.mode === "full" ? `, until ${license.full_until.slice(0, 10)}` : ""}
            </p>
            {l && (
              <p className="muted">
                Up to {l.max_users} users, {l.max_locations} locations and {l.max_devices} PCs.
              </p>
            )}
          </section>
        )}
        <section className="panel" aria-labelledby="soon-h">
          <h2 id="soon-h">What is next</h2>
          <p className="muted">
            Sales, returns, routes and the other pages from the prototype are listed under “Coming soon” in the sidebar. Each one opens a page that says what it will do and when it arrives.
          </p>
        </section>
      </div>
    </>
  );
}
