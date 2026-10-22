import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { demoShop } from "../src/api/mock.ts";
import { shopWithUser, signedIn } from "./helpers.tsx";

const dialog = () => screen.getByRole("dialog");

describe("Inventory: products", () => {
  it("opens on Products and lists them with category and unit", async () => {
    await signedIn({ hash: "#/inventory" });
    expect(await screen.findByText("Brake pad set, front")).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Products" })).toHaveAttribute("aria-selected", "true");
    const row = screen.getByText("Brake pad set, front").closest("tr")!;
    expect(within(row).getByText("BP-100")).toBeInTheDocument();
    expect(within(row).getByText("Brake parts")).toBeInTheDocument();
    expect(within(row).getByText("set")).toBeInTheDocument();
    expect(screen.getByText(/Stock levels, transfers and stock counts arrive with build step 4/)).toBeInTheDocument();
  });

  it("finds a product by an exactly scanned barcode, a SKU fragment or part of the name", async () => {
    const { user } = await signedIn({ hash: "#/inventory" });
    await screen.findByText("Brake pad set, front");
    const box = screen.getByRole("searchbox", { name: "Search products" });
    await user.type(box, "8901234567893");
    await waitFor(() => expect(screen.queryByText("Brake pad set, front")).not.toBeInTheDocument());
    expect(screen.getByText("Spark plug")).toBeInTheDocument();
    await user.clear(box);
    await user.type(box, "oil");
    expect(await screen.findByText("Oil filter")).toBeInTheDocument();
    expect(screen.queryByText("Spark plug")).not.toBeInTheDocument();
  });

  it("filters by category", async () => {
    const { user } = await signedIn({ hash: "#/inventory" });
    await screen.findByText("Spark plug");
    await user.selectOptions(screen.getByRole("combobox", { name: "Category" }), "Filters");
    await waitFor(() => expect(screen.queryByText("Spark plug")).not.toBeInTheDocument());
    expect(screen.getByText("Oil filter")).toBeInTheDocument();
    expect(screen.getByText("Air filter")).toBeInTheDocument();
  });

  it("adds a product with a category and normalises the SKU", async () => {
    const { user, shop } = await signedIn({ hash: "#/inventory" });
    await user.click(await screen.findByRole("button", { name: "Add product" }));
    await user.type(within(dialog()).getByLabelText("SKU"), "ch-77");
    await user.type(within(dialog()).getByLabelText("Name"), "Chain sprocket kit");
    await user.selectOptions(within(dialog()).getByLabelText("Category"), "Engine parts");
    await user.click(within(dialog()).getByRole("button", { name: "Add product" }));
    expect(await screen.findByText("Added Chain sprocket kit (CH-77)")).toBeInTheDocument();
    expect(shop.products.at(-1)).toMatchObject({ sku: "CH-77", category_id: "k-engine", unit: "pcs" });
  });

  it("refuses a duplicate SKU and keeps the form", async () => {
    const { user } = await signedIn({ hash: "#/inventory" });
    await user.click(await screen.findByRole("button", { name: "Add product" }));
    await user.type(within(dialog()).getByLabelText("SKU"), "bp-100");
    await user.type(within(dialog()).getByLabelText("Name"), "Another pad");
    await user.click(within(dialog()).getByRole("button", { name: "Add product" }));
    expect(await within(dialog()).findByRole("alert")).toHaveTextContent("a product with that SKU already exists");
  });

  it("edits a product without losing its other fields", async () => {
    const { user, shop } = await signedIn({ hash: "#/inventory" });
    const row = (await screen.findByText("Brake pad set, front")).closest("tr")!;
    await user.click(within(row).getByRole("button", { name: "Edit" }));
    const name = within(dialog()).getByLabelText("Name");
    await user.clear(name);
    await user.type(name, "Brake pad set, front axle");
    await user.click(within(dialog()).getByRole("button", { name: "Save changes" }));
    expect(await screen.findByText("Brake pad set, front axle")).toBeInTheDocument();
    expect(shop.products[0]).toMatchObject({ barcode: "8901234567890", category_id: "k-brake", unit: "set", revision: 2 });
  });

  it("deactivates a product so it drops out of the list", async () => {
    const { user } = await signedIn({ hash: "#/inventory" });
    const row = (await screen.findByText("Headlight bulb")).closest("tr")!;
    await user.click(within(row).getByRole("button", { name: "Deactivate" }));
    await waitFor(() => expect(screen.queryByText("Headlight bulb")).not.toBeInTheDocument());
  });

  it("adds and renames categories", async () => {
    const { user, shop } = await signedIn({ hash: "#/inventory" });
    await user.click(await screen.findByRole("button", { name: "Categories" }));
    await user.type(within(dialog()).getByLabelText("New category name"), "Tyres");
    await user.click(within(dialog()).getByRole("button", { name: "Add" }));
    expect(await within(dialog()).findByDisplayValue("Tyres")).toBeInTheDocument();
    const box = within(dialog()).getByLabelText("Name of Filters");
    await user.clear(box);
    await user.type(box, "Filters and strainers");
    await user.click(within(box.closest("div")!).getByRole("button", { name: "Rename" }));
    await waitFor(() => expect(shop.categories.find((c) => c.id === "k-filter")?.name).toBe("Filters and strainers"));
    expect(shop.categories.some((c) => c.name === "Tyres")).toBe(true);
  });

  it("hides the Products tab from someone without product permissions", async () => {
    const shop = shopWithUser("Stock Viewer", ["inventory.view"]);
    await signedIn({ shop, person: "Stock Viewer", pin: "2222", hash: "#/inventory" });
    expect(await screen.findByRole("tab", { name: "Locations" })).toHaveAttribute("aria-selected", "true");
    expect(screen.queryByRole("tab", { name: "Products" })).not.toBeInTheDocument();
  });
});

describe("Inventory: locations", () => {
  const open = async (opts: Parameters<typeof signedIn>[0] = {}) => {
    const r = await signedIn({ hash: "#/inventory", ...opts });
    await r.user.click(await screen.findByRole("tab", { name: "Locations" }));
    return r;
  };

  it("lists the locations with their codes", async () => {
    await open();
    expect(await screen.findByText("Main Shop")).toBeInTheDocument();
    expect(screen.getByText("G2")).toBeInTheDocument();
  });

  it("adds a location and shows the plan limit when it is reached", async () => {
    const { user, shop } = await open();
    await screen.findByText("Main Shop");
    await user.click(screen.getByRole("button", { name: "Add location" }));
    await user.type(within(dialog()).getByLabelText("Name"), "Market stall");
    await user.type(within(dialog()).getByLabelText("Code"), "mkt");
    await user.click(within(dialog()).getByRole("button", { name: "Add location" }));
    expect(await screen.findByText("Added Market stall (MKT)")).toBeInTheDocument();
    expect(shop.locations.at(-1)).toMatchObject({ code: "MKT", location_type: "storage" });
    // fill the plan (5), then one more is refused with a message that says what to do
    for (const c of ["X1", "X2"]) shop.locations.push({ ...shop.locations[0]!, id: c, code: c, name: c });
    await user.click(screen.getByRole("button", { name: "Add location" }));
    await user.type(within(dialog()).getByLabelText("Name"), "One too many");
    await user.type(within(dialog()).getByLabelText("Code"), "OVER");
    await user.click(within(dialog()).getByRole("button", { name: "Add location" }));
    expect(await within(dialog()).findByRole("alert")).toHaveTextContent(/location limit is reached/);
  });

  it("sends the responsible user back unchanged when editing", async () => {
    const shop = demoShop();
    shop.locations[0]!.responsible_user_id = "u-irfan";
    const { user } = await open({ shop });
    const row = (await screen.findByText("Main Shop")).closest("tr")!;
    await user.click(within(row).getByRole("button", { name: "Edit" }));
    await user.type(within(dialog()).getByLabelText("Description"), "Front of the market");
    await user.click(within(dialog()).getByRole("button", { name: "Save changes" }));
    await screen.findByText("Front of the market");
    expect(shop.locations[0]).toMatchObject({ responsible_user_id: "u-irfan", description: "Front of the market", revision: 2 });
  });

  it("shows a limited user only their own location and no add button", async () => {
    await open({ person: "Tariq Hussain", pin: "4444" });
    expect(await screen.findByText("Godown 1")).toBeInTheDocument();
    expect(screen.queryByText("Main Shop")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Add location" })).not.toBeInTheDocument();
  });

  it("deactivates a location", async () => {
    const { user } = await open();
    const row = (await screen.findByText("Godown 2")).closest("tr")!;
    await user.click(within(row).getByRole("button", { name: "Deactivate" }));
    await waitFor(() => expect(within((screen.getByText("Godown 2")).closest("tr")!).getByText("Inactive")).toBeInTheDocument());
  });
});

describe("Dashboard", () => {
  it("shows the counts a person may see", async () => {
    await signedIn();
    const tiles = await screen.findByTestId("tiles");
    await waitFor(() => expect(within(tiles).getByText("5")).toBeInTheDocument());
    expect(within(tiles).getByText("Customers")).toBeInTheDocument();
    expect(within(tiles).getByText("Products")).toBeInTheDocument();
    expect(within(tiles).getByText("Locations")).toBeInTheDocument();
    expect(within(tiles).getByText("Users")).toBeInTheDocument();
    expect(await screen.findByText(/Up to 10 users, 5 locations and 2 PCs/)).toBeInTheDocument();
  });

  it("hides counts a person may not see, and the plan section is informative not alarming", async () => {
    const shop = shopWithUser("Counter Only", ["sales.create"]);
    await signedIn({ shop, person: "Counter Only", pin: "2222" });
    const tiles = await screen.findByTestId("tiles");
    expect(within(tiles).queryByText("Customers")).not.toBeInTheDocument();
    expect(within(tiles).queryByText("Users")).not.toBeInTheDocument();
  });

  it("nudges a new shop to add its first records", async () => {
    const shop = demoShop();
    shop.customers = [];
    shop.products = [];
    await signedIn({ shop });
    expect(await screen.findByText(/Nothing here yet for: customers, products/)).toBeInTheDocument();
  });
});
