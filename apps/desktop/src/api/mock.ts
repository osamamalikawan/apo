// An in-memory shop that answers like the Shop Server, for tests and for `npm run dev:mock`.
// It follows the real API's shapes, error codes and main rules (revisions, uniqueness, permissions,
// "you can only grant what you hold") but is NOT a second implementation of the server: the Rust tests
// are the authority. Keep it small.
import { hasPermission } from "@dsaas/domain";
import { covers, moduleOf } from "../permissions.ts";
import type { Category, Customer, Location, Product, Role } from "./types.ts";

interface MUser {
  id: string; username: string; full_name: string; password: string; pin: string | null;
  active: boolean; role_ids: string[]; location_ids: string[];
}
interface Session { token: string; userId: string; method: "password" | "pin" }
type Json = Record<string, unknown>;

export const PERMISSION_LIST: [string, string][] = [
  ["users.manage", "Create and deactivate users, set their location access"],
  ["roles.manage", "Create and edit roles, assign roles to users"],
  ["locations.all", "Use every stock location"],
  ["locations.manage", "Create and edit stock locations"],
  ["products.view", "View products"],
  ["products.manage", "Create and edit products"],
  ["customers.view", "View customers"],
  ["customers.manage", "Create and edit customers"],
  ["inventory.view", "View stock"],
  ["inventory.transfer", "Move stock between locations"],
  ["inventory.adjust", "Adjust stock quantities"],
  ["inventory.count", "Run stock counts"],
  ["sales.view", "View sales"],
  ["sales.create", "Create sales"],
  ["payments.receive", "Record payments"],
  ["routes.all", "Use every route"],
  ["reports.view", "View reports"],
];

export interface MockShop {
  tenantName: string;
  users: MUser[];
  roles: Role[];
  locations: Location[];
  categories: Category[];
  products: Product[];
  customers: Customer[];
  mode: "full" | "grace" | "read_only" | "unlicensed";
  deviceKnown: boolean;
}

let counter = 0;
const nid = (p: string) => `${p}${++counter}`;

export function demoShop(): MockShop {
  const owner: Role = { id: "r-owner", name: "Owner", permissions: ["*"], member_count: 1, protected: true };
  const cashier: Role = { id: "r-cashier", name: "Cashier", permissions: ["customers.view", "products.view", "sales.create"], member_count: 1, protected: false };
  const keeper: Role = { id: "r-keeper", name: "Storekeeper", permissions: ["inventory.*", "products.view", "products.manage"], member_count: 1, protected: false };
  const loc = (id: string, name: string, code: string, type = "storage"): Location => ({ id, name, code, location_type: type, description: null, parent_location_id: null, responsible_user_id: null, is_active: true, revision: 1 });
  const cat = (id: string, name: string): Category => ({ id, name, revision: 1 });
  const prod = (id: string, sku: string, name: string, barcode: string | null, category_id: string, unit = "pcs"): Product => ({ id, sku, name, barcode, category_id, unit, is_active: true, revision: 1 });
  const cust = (n: number, name: string, phone: string | null, address: string | null): Customer => ({ id: `c${n}`, code: `C${String(n).padStart(5, "0")}`, name, phone, address, notes: null, is_active: true, revision: 1 });
  return {
    tenantName: "Irfan Autos",
    users: [
      { id: "u-irfan", username: "irfan", full_name: "Irfan Ahmed", password: "irfan-password-1", pin: "1111", active: true, role_ids: ["r-owner"], location_ids: [] },
      { id: "u-sana", username: "sana", full_name: "Sana Malik", password: "sana-password-1", pin: "3333", active: true, role_ids: ["r-cashier"], location_ids: ["l-main"] },
      { id: "u-tariq", username: "tariq", full_name: "Tariq Hussain", password: "tariq-password-1", pin: "4444", active: true, role_ids: ["r-keeper"], location_ids: ["l-g1"] },
    ],
    roles: [owner, cashier, keeper],
    locations: [loc("l-main", "Main Shop", "MAIN", "shop"), loc("l-g1", "Godown 1", "G1"), loc("l-g2", "Godown 2", "G2")],
    categories: [cat("k-brake", "Brake parts"), cat("k-engine", "Engine parts"), cat("k-filter", "Filters"), cat("k-elec", "Electrical")],
    products: [
      prod("p1", "BP-100", "Brake pad set, front", "8901234567890", "k-brake", "set"),
      prod("p2", "BP-200", "Brake pad set, rear", "8901234567891", "k-brake", "set"),
      prod("p3", "OF-10", "Oil filter", "8901234567892", "k-filter"),
      prod("p4", "AF-20", "Air filter", null, "k-filter"),
      prod("p5", "SP-01", "Spark plug", "8901234567893", "k-engine"),
      prod("p6", "CP-05", "Clutch plate set", null, "k-engine", "set"),
      prod("p7", "HB-12", "Headlight bulb", "8901234567894", "k-elec"),
    ],
    customers: [
      cust(1, "Ali Traders", "0300-1234567", "Hussain Agahi, Multan"),
      cust(2, "Bilal Motors", "0301-2345678", "Chowk Kumharanwala, Multan"),
      cust(3, "Chaudhry Autos", null, "Shujabad Road, Multan"),
      cust(4, "Dawood Spares", "0302-3456789", "Old Bahawalpur Road, Multan"),
      cust(5, "Farooq Bike House", "0303-4567890", null),
    ],
    mode: "full",
    deviceKnown: true,
  };
}

export function emptyShop(): MockShop {
  const s = demoShop();
  return { ...s, locations: [], categories: [], products: [], customers: [] };
}

export function createMockFetch(shop: MockShop = demoShop()): typeof fetch {
  const sessions = new Map<string, Session>();
  const failed = new Map<string, number>();
  const now = () => new Date().toISOString();

  const reply = (status: number, body?: unknown) =>
    new Response(status === 204 ? null : JSON.stringify(body ?? {}), { status, headers: { "content-type": "application/json" } });
  const err = (status: number, error: string, message: string, reason: string | null = null) =>
    reply(status, { api_version: 1, error, message, min_api_version: 1, reason });
  const forbidden = (reason = "no_permission") => err(403, "forbidden", "you do not have permission to do this", reason);

  const permsOf = (u: MUser) => [...new Set(u.role_ids.flatMap((id) => shop.roles.find((r) => r.id === id)?.permissions ?? []))].sort();
  const withCounts = (r: Role): Role => ({ ...r, member_count: shop.users.filter((u) => u.role_ids.includes(r.id)).length });
  const page = <T,>(items: T[], q: URLSearchParams) => {
    const limit = Math.min(Number(q.get("limit")) || 50, 200);
    const offset = Number(q.get("offset")) || 0;
    return { items: items.slice(offset, offset + limit), total: items.length };
  };
  const text = (v: unknown) => (typeof v === "string" ? v.trim() : "");
  const blank = (v: unknown) => (text(v) === "" ? null : text(v));

  return async (input, init) => {
    const url = new URL(typeof input === "string" ? input : input instanceof URL ? input.href : (input as Request).url, "http://mock");
    const path = url.pathname;
    const method = (init?.method ?? "GET").toUpperCase();
    const body = (init?.body ? JSON.parse(String(init.body)) : {}) as Json;
    const auth = new Headers(init?.headers as HeadersInit | undefined).get("authorization");
    const session = auth?.startsWith("Bearer ") ? sessions.get(auth.slice(7)) : undefined;
    const user = session ? shop.users.find((u) => u.id === session.userId && u.active) : undefined;

    if (!shop.deviceKnown) return err(401, "unauthorized", "device authentication required");
    if (path === "/v1/whoami") return reply(200, { device_id: "mock-device", tenant_id: "mock-tenant", tenant_name: shop.tenantName });
    if (path === "/v1/login-users")
      return reply(200, shop.users.filter((u) => u.active && u.pin).map(({ id, username, full_name }) => ({ id, username, full_name })));
    if (path === "/v1/license/status") {
      const can_write = shop.mode === "full" || shop.mode === "grace";
      return reply(200, { mode: shop.mode, reason: can_write ? null : "suspended", can_write, revision: 1, full_until: "2031-01-01T00:00:00Z", grace_until: "2031-01-15T00:00:00Z", limits: { max_devices: 2, max_users: 10, max_locations: 5 } });
    }

    if (path === "/v1/auth/login" && method === "POST") {
      const u = shop.users.find((x) => x.username === text(body.username).toLowerCase());
      const secret = String(body.pin ?? body.password ?? "");
      const useP = body.pin !== undefined;
      const bad = () => err(401, "invalid_credentials", "invalid username or password, or the account is temporarily locked");
      if (!u || !u.active) return bad();
      if ((failed.get(u.id) ?? 0) >= 5) return bad();
      const ok = useP ? u.pin === secret : u.password === secret;
      if (!ok) {
        failed.set(u.id, (failed.get(u.id) ?? 0) + 1);
        return bad();
      }
      failed.set(u.id, 0);
      const token = nid("tok") + Math.random().toString(16).slice(2);
      sessions.set(token, { token, userId: u.id, method: useP ? "pin" : "password" });
      return reply(200, { token, expires_at: now(), method: useP ? "pin" : "password", user: { id: u.id, username: u.username, full_name: u.full_name }, permissions: permsOf(u) });
    }

    if (!session || !user) return err(401, "session_required", "sign in to continue");
    const can = (p: string) => hasPermission(permsOf(user), p);
    const need = (p: string) => (can(p) ? null : forbidden());
    const writable = () => (shop.mode === "full" || shop.mode === "grace" ? null : err(402, "subscription_inactive", "subscription is not active", "suspended"));
    const passwordOnly = () => (session.method === "password" ? null : err(403, "password_login_required", "sign in with your password (not a PIN) to do this"));
    const stale = (rev: unknown, actual: number) => (Number(rev) !== actual ? err(409, "stale_revision", "this record was changed by someone else; reload it and try again") : null);
    const mine = permsOf(user);

    if (path === "/v1/auth/logout") { sessions.delete(session.token); return reply(204); }
    if (path === "/v1/me" && method === "GET")
      return reply(200, { user: { id: user.id, username: user.username, full_name: user.full_name }, method: session.method, permissions: mine, all_locations: can("locations.all"), location_ids: user.location_ids });
    if (path === "/v1/me/pin" && method === "POST") {
      const g = passwordOnly(); if (g) return g;
      if (body.current_password !== user.password) return err(401, "invalid_credentials", "invalid username or password, or the account is temporarily locked");
      if (!/^\d{4,8}$/.test(String(body.pin))) return err(400, "bad_request", "PIN must be 4 to 8 digits");
      user.pin = String(body.pin);
      return reply(204);
    }
    if (path === "/v1/me/password" && method === "POST") {
      const g = passwordOnly(); if (g) return g;
      if (body.current_password !== user.password) return err(401, "invalid_credentials", "invalid username or password, or the account is temporarily locked");
      if (String(body.new_password).length < 10) return err(400, "bad_request", "password must be 10 to 128 characters");
      user.password = String(body.new_password);
      for (const [t, s] of sessions) if (s.userId === user.id) sessions.delete(t);
      return reply(204);
    }

    // ---- locations ----
    if (path === "/v1/locations" && method === "GET")
      return reply(200, shop.locations.filter((l) => can("locations.all") || user.location_ids.includes(l.id)));
    if (path === "/v1/locations" && method === "POST") {
      const g = need("locations.manage") ?? writable(); if (g) return g;
      if (!can("locations.all")) return forbidden("needs_all_locations");
      if (shop.locations.filter((l) => l.is_active).length >= 5) return err(403, "location_limit_reached", "the location limit of your plan has been reached");
      const code = text(body.code).toUpperCase();
      if (text(body.name).length < 2 || code.length < 2) return err(400, "bad_request", "name and code are required");
      if (shop.locations.some((l) => l.code === code)) return err(409, "conflict", "a location with that code already exists");
      const l: Location = { id: nid("l"), name: text(body.name), code, location_type: text(body.location_type) || "storage", description: blank(body.description), parent_location_id: blank(body.parent_location_id), responsible_user_id: blank(body.responsible_user_id), is_active: true, revision: 1 };
      shop.locations.push(l);
      return reply(201, l);
    }
    let m = path.match(/^\/v1\/locations\/([^/]+)(?:\/(deactivate|activate))?$/);
    if (m) {
      const l = shop.locations.find((x) => x.id === m![1]);
      if (!l || !(can("locations.all") || user.location_ids.includes(l.id))) return forbidden("location_denied");
      if (method === "GET") return reply(200, l);
      const g = need("locations.manage") ?? writable(); if (g) return g;
      const s = stale(body.expected_revision, l.revision); if (s) return s;
      if (m[2]) { l.is_active = m[2] === "activate"; l.revision++; return reply(200, l); }
      const code = text(body.code).toUpperCase();
      if (shop.locations.some((x) => x.code === code && x.id !== l.id)) return err(409, "conflict", "a location with that code already exists");
      Object.assign(l, { name: text(body.name), code, location_type: text(body.location_type) || "storage", description: blank(body.description), parent_location_id: blank(body.parent_location_id), responsible_user_id: blank(body.responsible_user_id) });
      l.revision++;
      return reply(200, l);
    }

    // ---- categories ----
    if (path === "/v1/categories") {
      if (method === "GET") return need("products.view") ?? reply(200, [...shop.categories].sort((a, b) => a.name.localeCompare(b.name)));
      const g = need("products.manage") ?? writable(); if (g) return g;
      const name = text(body.name);
      if (name.length < 2) return err(400, "bad_request", "category name must be 2 to 60 characters");
      if (shop.categories.some((c) => c.name.toLowerCase() === name.toLowerCase())) return err(409, "conflict", "a category with that name already exists");
      const c: Category = { id: nid("k"), name, revision: 1 };
      shop.categories.push(c);
      return reply(201, c);
    }
    m = path.match(/^\/v1\/categories\/([^/]+)$/);
    if (m && method === "PUT") {
      const g = need("products.manage") ?? writable(); if (g) return g;
      const c = shop.categories.find((x) => x.id === m![1]);
      if (!c) return err(404, "not_found", "not found");
      const s = stale(body.expected_revision, c.revision); if (s) return s;
      c.name = text(body.name); c.revision++;
      return reply(200, c);
    }

    // ---- products ----
    if (path === "/v1/products" && method === "GET") {
      const g = need("products.view"); if (g) return g;
      const q = url.searchParams, s = (q.get("search") ?? "").toLowerCase();
      const items = shop.products.filter((p) =>
        (q.get("include_inactive") === "true" || p.is_active) &&
        (!q.get("category_id") || p.category_id === q.get("category_id")) &&
        (!s || p.name.toLowerCase().includes(s) || p.sku.toLowerCase().includes(s) || p.barcode === q.get("search")),
      ).sort((a, b) => a.name.localeCompare(b.name));
      return reply(200, page(items, q));
    }
    if (path === "/v1/products" && method === "POST") {
      const g = need("products.manage") ?? writable(); if (g) return g;
      const sku = text(body.sku).toUpperCase();
      if (!sku || text(body.name).length < 2) return err(400, "bad_request", "SKU and name are required");
      if (shop.products.some((p) => p.sku === sku)) return err(409, "conflict", "a product with that SKU already exists");
      const barcode = blank(body.barcode);
      if (barcode && shop.products.some((p) => p.barcode === barcode)) return err(409, "conflict", "that barcode already belongs to another product");
      const p: Product = { id: nid("p"), sku, name: text(body.name), barcode, category_id: blank(body.category_id), unit: text(body.unit) || "pcs", is_active: true, revision: 1 };
      shop.products.push(p);
      return reply(201, p);
    }
    m = path.match(/^\/v1\/products\/([^/]+)(?:\/(deactivate|activate))?$/);
    if (m) {
      const g0 = need("products.view"); if (g0) return g0;
      const p = shop.products.find((x) => x.id === m![1]);
      if (!p) return err(404, "not_found", "not found");
      if (method === "GET") return reply(200, p);
      const g = need("products.manage") ?? writable(); if (g) return g;
      const s = stale(body.expected_revision, p.revision); if (s) return s;
      if (m[2]) { p.is_active = m[2] === "activate"; p.revision++; return reply(200, p); }
      const sku = text(body.sku).toUpperCase();
      if (shop.products.some((x) => x.sku === sku && x.id !== p.id)) return err(409, "conflict", "a product with that SKU already exists");
      Object.assign(p, { sku, name: text(body.name), barcode: blank(body.barcode), category_id: blank(body.category_id), unit: text(body.unit) || "pcs" });
      p.revision++;
      return reply(200, p);
    }

    // ---- customers ----
    if (path === "/v1/customers" && method === "GET") {
      const g = need("customers.view"); if (g) return g;
      const q = url.searchParams, s = (q.get("search") ?? "").toLowerCase();
      const items = shop.customers.filter((c) => (q.get("include_inactive") === "true" || c.is_active) && (!s || c.name.toLowerCase().includes(s) || c.code.toLowerCase().includes(s) || (c.phone ?? "").includes(s))).sort((a, b) => a.name.localeCompare(b.name));
      return reply(200, page(items, q));
    }
    if (path === "/v1/customers" && method === "POST") {
      const g = need("customers.manage") ?? writable(); if (g) return g;
      if (text(body.name).length < 2) return err(400, "bad_request", "name must be 2 to 120 characters");
      let code = text(body.code).toUpperCase();
      if (!code) { let n = shop.customers.length; do { n++; code = `C${String(n).padStart(5, "0")}`; } while (shop.customers.some((c) => c.code === code)); }
      if (shop.customers.some((c) => c.code === code)) return err(409, "conflict", "a customer with that code already exists");
      const c: Customer = { id: nid("c"), code, name: text(body.name), phone: blank(body.phone), address: blank(body.address), notes: blank(body.notes), is_active: true, revision: 1 };
      shop.customers.push(c);
      return reply(201, c);
    }
    m = path.match(/^\/v1\/customers\/([^/]+)(?:\/(deactivate|activate))?$/);
    if (m) {
      const g0 = need("customers.view"); if (g0) return g0;
      const c = shop.customers.find((x) => x.id === m![1]);
      if (!c) return err(404, "not_found", "not found");
      if (method === "GET") return reply(200, c);
      const g = need("customers.manage") ?? writable(); if (g) return g;
      const s = stale(body.expected_revision, c.revision); if (s) return s;
      if (m[2]) { c.is_active = m[2] === "activate"; c.revision++; return reply(200, c); }
      const code = text(body.code).toUpperCase() || c.code;
      if (shop.customers.some((x) => x.code === code && x.id !== c.id)) return err(409, "conflict", "a customer with that code already exists");
      Object.assign(c, { code, name: text(body.name), phone: blank(body.phone), address: blank(body.address), notes: blank(body.notes) });
      c.revision++;
      return reply(200, c);
    }

    // ---- permissions, roles, users ----
    if (path === "/v1/permissions") return need("roles.manage") ?? reply(200, PERMISSION_LIST.map(([permission, description]) => ({ permission, description })));
    if (path === "/v1/roles" && method === "GET") return need("roles.manage") ?? reply(200, shop.roles.map(withCounts));
    if (path === "/v1/roles" && method === "POST") {
      const g = need("roles.manage") ?? writable() ?? passwordOnly(); if (g) return g;
      const perms = (body.permissions as string[]) ?? [];
      if (!perms.every((p) => covers(mine, p))) return forbidden("cannot_grant_more_than_you_hold");
      if (perms.some((p) => p !== "*" && !PERMISSION_LIST.some(([k]) => k === p) && !(p.endsWith(".*") && PERMISSION_LIST.some(([k]) => moduleOf(k) === moduleOf(p))))) return err(400, "bad_request", "unknown permission");
      const name = text(body.name);
      if (name.length < 2 || name.toLowerCase() === "owner") return err(400, "bad_request", "that role name is reserved");
      if (shop.roles.some((r) => r.name.toLowerCase() === name.toLowerCase())) return err(409, "conflict", "a role with that name already exists");
      const r: Role = { id: nid("r"), name, permissions: [...new Set(perms)].sort(), member_count: 0, protected: false };
      shop.roles.push(r);
      return reply(201, { id: r.id });
    }
    m = path.match(/^\/v1\/roles\/([^/]+)$/);
    if (m) {
      const g = need("roles.manage") ?? writable() ?? passwordOnly(); if (g) return g;
      const r = shop.roles.find((x) => x.id === m![1]);
      if (!r) return err(404, "not_found", "not found");
      if (r.protected) return err(409, "conflict", "the Owner role cannot be changed");
      if (!r.permissions.every((p) => covers(mine, p))) return forbidden("cannot_grant_more_than_you_hold");
      if (method === "DELETE") {
        if (shop.users.some((u) => u.role_ids.includes(r.id))) return err(409, "conflict", "role is assigned to users; remove it from them first");
        shop.roles.splice(shop.roles.indexOf(r), 1);
        return reply(204);
      }
      const perms = (body.permissions as string[]) ?? [];
      if (!perms.every((p) => covers(mine, p))) return forbidden("cannot_grant_more_than_you_hold");
      r.name = text(body.name); r.permissions = [...new Set(perms)].sort();
      return reply(204);
    }
    if (path === "/v1/users" && method === "GET") {
      const g = need("users.manage"); if (g) return g;
      return reply(200, shop.users.map(({ id, username, full_name, active, role_ids, location_ids }) => ({ id, username, full_name, is_active: active, role_ids, location_ids })));
    }
    if (path === "/v1/users" && method === "POST") {
      const g = need("users.manage") ?? writable() ?? passwordOnly(); if (g) return g;
      const username = text(body.username).toLowerCase();
      if (!/^[a-z0-9._-]{3,32}$/.test(username)) return err(400, "bad_request", "username must be 3 to 32 characters: letters, digits, dot, dash, underscore");
      if (String(body.password).length < 10) return err(400, "bad_request", "password must be 10 to 128 characters");
      if (shop.users.some((u) => u.username === username)) return err(409, "conflict", "username already exists");
      if (shop.users.filter((u) => u.active).length >= 10) return err(403, "user_limit_reached", "the user limit of your plan has been reached");
      const u: MUser = { id: nid("u"), username, full_name: text(body.full_name), password: String(body.password), pin: null, active: true, role_ids: [], location_ids: [] };
      shop.users.push(u);
      return reply(201, { id: u.id });
    }
    m = path.match(/^\/v1\/users\/([^/]+)\/(deactivate|roles|locations)$/);
    if (m) {
      const target = shop.users.find((x) => x.id === m![1]);
      if (m[2] === "roles") {
        const g = need("roles.manage") ?? writable() ?? passwordOnly(); if (g) return g;
      } else {
        const g = need("users.manage") ?? writable() ?? passwordOnly(); if (g) return g;
      }
      if (!target) return err(404, "not_found", "not found");
      if (m[2] === "deactivate") {
        const owners = shop.users.filter((u) => u.active && permsOf(u).includes("*"));
        if (permsOf(target).includes("*") && owners.length <= 1) return err(409, "conflict", "cannot deactivate the last owner");
        target.active = false;
        for (const [t, s] of sessions) if (s.userId === target.id) sessions.delete(t);
        return reply(204);
      }
      if (m[2] === "roles") {
        const ids = (body.role_ids as string[]) ?? [];
        const changed = [...ids.filter((i) => !target.role_ids.includes(i)), ...target.role_ids.filter((i) => !ids.includes(i))];
        for (const id of changed) if (!(shop.roles.find((r) => r.id === id)?.permissions ?? []).every((p) => covers(mine, p))) return forbidden("cannot_grant_more_than_you_hold");
        target.role_ids = ids;
        return reply(204);
      }
      target.location_ids = (body.location_ids as string[]) ?? [];
      return reply(204);
    }

    return err(404, "not_found", "not found");
  };
}
