import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../src/App.tsx";
import { createMockFetch, demoShop, type MockShop } from "../src/api/mock.ts";

export type User = ReturnType<typeof userEvent.setup>;

export function renderApp(opts: { shop?: MockShop; hash?: string; fetchFn?: typeof fetch } = {}) {
  const shop = opts.shop ?? demoShop();
  const fetchFn = opts.fetchFn ?? createMockFetch(shop);
  window.location.hash = opts.hash ?? "";
  const user = userEvent.setup();
  const utils = render(<App fetchFn={fetchFn} />);
  return { shop, user, fetchFn, ...utils };
}

export async function signInWithPin(user: User, person: string, pin: string) {
  await user.click(await screen.findByRole("button", { name: person }));
  await user.keyboard(pin + "{Enter}");
  await screen.findByRole("button", { name: "Sign out" });
}

/** Render the app already signed in as `person` (the demo shop's owner by default). */
export async function signedIn(opts: { shop?: MockShop; hash?: string; person?: string; pin?: string; fetchFn?: typeof fetch } = {}) {
  const r = renderApp(opts);
  await signInWithPin(r.user, opts.person ?? "Irfan Ahmed", opts.pin ?? "1111");
  return r;
}

/** A shop with one extra user who holds exactly the given permissions. */
export function shopWithUser(name: string, perms: string[], extra: Partial<MockShop> = {}): MockShop {
  const shop = demoShop();
  shop.roles.push({ id: "r-test", name: "Test role", permissions: perms, member_count: 1, protected: false });
  shop.users.push({ id: "u-test", username: "tester", full_name: name, password: "tester-password-1", pin: "2222", active: true, role_ids: ["r-test"], location_ids: ["l-main"] });
  return Object.assign(shop, extra);
}
