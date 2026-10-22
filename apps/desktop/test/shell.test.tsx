import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { createMockFetch, demoShop } from "../src/api/mock.ts";
import { renderApp, shopWithUser, signedIn } from "./helpers.tsx";

const shell = () => document.querySelector(".shell") as HTMLElement;

describe("collapsible sidebar with a button in the header", () => {
  it("has the toggle in the header, before the page title", async () => {
    await signedIn();
    const header = screen.getByRole("banner");
    const toggle = within(header).getByRole("button", { name: "Collapse sidebar" });
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(toggle).toHaveAttribute("aria-controls", "sidebar");
    expect(header.firstElementChild).toBe(toggle);
    expect(within(shell()).getByRole("complementary", { name: "Main navigation" }).id).toBe("sidebar");
  });

  it("collapses and expands, and says which way the next click goes", async () => {
    const { user } = await signedIn();
    expect(shell()).not.toHaveClass("collapsed");
    await user.click(screen.getByRole("button", { name: "Collapse sidebar" }));
    expect(shell()).toHaveClass("collapsed");
    const expand = screen.getByRole("button", { name: "Expand sidebar" });
    expect(expand).toHaveAttribute("aria-expanded", "false");
    await user.click(expand);
    expect(shell()).not.toHaveClass("collapsed");
    expect(screen.getByRole("button", { name: "Collapse sidebar" })).toBeInTheDocument();
  });

  it("remembers the choice between runs", async () => {
    const first = await signedIn();
    await first.user.click(screen.getByRole("button", { name: "Collapse sidebar" }));
    expect(localStorage.getItem("apo.sidebar")).toBe("collapsed");
    first.unmount();
    renderApp({ hash: "#/dashboard", fetchFn: first.fetchFn }); // the session token is still in sessionStorage
    await screen.findByRole("button", { name: "Expand sidebar" });
    expect(shell()).toHaveClass("collapsed");
  });

  it("keeps every link reachable by name while the rail is collapsed", async () => {
    const { user } = await signedIn();
    await user.click(screen.getByRole("button", { name: "Collapse sidebar" }));
    for (const n of ["Dashboard", "Customers", "Inventory", "Settings"]) expect(screen.getByRole("link", { name: n })).toBeInTheDocument();
  });

  it("starts collapsed on a narrow window", async () => {
    const old = window.innerWidth;
    window.innerWidth = 600;
    try {
      await signedIn();
      expect(shell()).toHaveClass("collapsed");
    } finally {
      window.innerWidth = old;
    }
  });
});

describe("navigation", () => {
  it("shows the pages a person may use, and Coming soon for the rest of the prototype's pages", async () => {
    await signedIn();
    const nav = screen.getByRole("navigation");
    for (const n of ["Dashboard", "Customers", "Inventory", "Settings"]) expect(within(nav).getByRole("link", { name: n })).toBeInTheDocument();
    expect(within(nav).getByText("Coming soon")).toBeInTheDocument();
    expect(within(nav).getByRole("link", { name: "Sales (coming soon)" })).toBeInTheDocument();
    expect(within(nav).getAllByRole("link").length).toBe(4 + 13);
  });

  it("hides pages a person has no permission for", async () => {
    const shop = shopWithUser("Counter Only", ["sales.create"]);
    await signedIn({ shop, person: "Counter Only", pin: "2222" });
    const nav = screen.getByRole("navigation");
    expect(within(nav).queryByRole("link", { name: "Customers" })).not.toBeInTheDocument();
    expect(within(nav).queryByRole("link", { name: "Inventory" })).not.toBeInTheDocument();
    expect(within(nav).getByRole("link", { name: "Settings" })).toBeInTheDocument();
  });

  it("tells someone who types the address of a page they may not use", async () => {
    const shop = shopWithUser("Counter Only", ["sales.create"]);
    await signedIn({ shop, person: "Counter Only", pin: "2222", hash: "#/customers" });
    expect(await screen.findByText(/You do not have access to this page/)).toBeInTheDocument();
  });

  it("opens a Coming soon page that says what it will do and when", async () => {
    const { user } = await signedIn();
    await user.click(screen.getByRole("link", { name: "Sales (coming soon)" }));
    expect(await screen.findByRole("heading", { name: "Sales is not built yet" })).toBeInTheDocument();
    expect(screen.getByText(/build step 5/)).toBeInTheDocument();
    expect(screen.getByText(/buy 10, get 1 free/)).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Sales");
  });

  it("handles an unknown address kindly", async () => {
    await signedIn({ hash: "#/nope" });
    expect(await screen.findByText(/There is no page called “nope”/)).toBeInTheDocument();
  });

  it("marks the current page", async () => {
    const { user } = await signedIn();
    await user.click(screen.getByRole("link", { name: "Customers" }));
    expect(await screen.findByRole("link", { name: "Customers" })).toHaveAttribute("aria-current", "page");
  });
});

describe("header", () => {
  it("switches between light and dark mode and remembers it", async () => {
    const { user } = await signedIn();
    await user.click(screen.getByRole("button", { name: "Dark mode" }));
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(localStorage.getItem("apo.theme")).toBe("dark");
    await user.click(screen.getByRole("button", { name: "Light mode" }));
    expect(document.documentElement.dataset.theme).toBe("light");
  });

  it("shows the subscription state", async () => {
    await signedIn();
    expect(await screen.findByText("Subscription active")).toBeInTheDocument();
  });

  it("shows a read-only warning when the subscription is not active", async () => {
    const shop = demoShop();
    shop.mode = "read_only";
    await signedIn({ shop });
    expect(await screen.findByText(/Read-only: subscription not active/)).toBeInTheDocument();
  });

  it("signs out to the PIN screen", async () => {
    const { user } = await signedIn();
    await user.click(screen.getByRole("button", { name: "Sign out" }));
    expect(await screen.findByText("Who is signing in?")).toBeInTheDocument();
    expect(sessionStorage.getItem("apo.token")).toBeNull();
  });

  it("returns to sign-in with a notice when the server ends the session", async () => {
    const shop = demoShop();
    const real = createMockFetch(shop);
    let expired = false;
    const fetchFn = ((input: RequestInfo | URL, init?: RequestInit) => {
      const auth = new Headers(init?.headers).get("authorization");
      if (expired && auth) return Promise.resolve(new Response(JSON.stringify({ error: "session_required", message: "sign in to continue" }), { status: 401 }));
      return real(input, init);
    }) as typeof fetch;
    const { user } = await signedIn({ shop, fetchFn });
    expired = true;
    await user.click(screen.getByRole("link", { name: "Customers" }));
    await waitFor(() => expect(screen.getByText("Your session ended. Sign in again.")).toBeInTheDocument());
    expect(screen.getByText("Who is signing in?")).toBeInTheDocument();
  });

  it("keeps a session across a reload", async () => {
    const first = await signedIn();
    first.unmount();
    renderApp({ fetchFn: first.fetchFn });
    expect(await screen.findByText("Welcome, Irfan Ahmed.")).toBeInTheDocument();
  });
});
