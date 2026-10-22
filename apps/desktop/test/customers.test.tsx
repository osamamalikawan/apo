import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { demoShop } from "../src/api/mock.ts";
import { shopWithUser, signedIn } from "./helpers.tsx";

const dialog = () => screen.getByRole("dialog");

describe("Customers", () => {
  it("lists customers with code, phone and address", async () => {
    await signedIn({ hash: "#/customers" });
    expect(await screen.findByText("Ali Traders")).toBeInTheDocument();
    expect(screen.getByText("C00001")).toBeInTheDocument();
    expect(screen.getByText("0300-1234567")).toBeInTheDocument();
    expect(screen.getByText("Hussain Agahi, Multan")).toBeInTheDocument();
    expect(screen.getAllByRole("row")).toHaveLength(1 + 5);
  });

  it("searches by name, code or phone", async () => {
    const { user } = await signedIn({ hash: "#/customers" });
    await screen.findByText("Ali Traders");
    await user.type(screen.getByRole("searchbox", { name: "Search customers" }), "bilal");
    await waitFor(() => expect(screen.queryByText("Ali Traders")).not.toBeInTheDocument());
    expect(screen.getByText("Bilal Motors")).toBeInTheDocument();
    await user.clear(screen.getByRole("searchbox"));
    await user.type(screen.getByRole("searchbox"), "zzz");
    expect(await screen.findByText("No customers match “zzz”.")).toBeInTheDocument();
  });

  it("adds a customer and the server assigns the next code", async () => {
    const { user } = await signedIn({ hash: "#/customers" });
    await user.click(await screen.findByRole("button", { name: "Add customer" }));
    await user.type(within(dialog()).getByLabelText("Name"), "Gul Auto Parts");
    await user.type(within(dialog()).getByLabelText("Phone"), "0304-5678901");
    await user.click(within(dialog()).getByRole("button", { name: "Add customer" }));
    expect(await screen.findByText("Added Gul Auto Parts (C00006)")).toBeInTheDocument();
    expect(await screen.findByText("Gul Auto Parts")).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("keeps the form open and says what is wrong when the server refuses", async () => {
    const { user } = await signedIn({ hash: "#/customers" });
    await user.click(await screen.findByRole("button", { name: "Add customer" }));
    await user.type(within(dialog()).getByLabelText("Name"), "Copy Cat");
    await user.type(within(dialog()).getByLabelText("Code"), "c00001");
    await user.click(within(dialog()).getByRole("button", { name: "Add customer" }));
    expect(await within(dialog()).findByRole("alert")).toHaveTextContent("a customer with that code already exists");
    expect(within(dialog()).getByLabelText("Name")).toHaveValue("Copy Cat");
  });

  it("will not submit a name that is too short", async () => {
    const { user } = await signedIn({ hash: "#/customers" });
    await user.click(await screen.findByRole("button", { name: "Add customer" }));
    await user.type(within(dialog()).getByLabelText("Name"), "x");
    expect(within(dialog()).getByRole("button", { name: "Add customer" })).toBeDisabled();
  });

  it("closes the form with Cancel and with Escape", async () => {
    const { user } = await signedIn({ hash: "#/customers" });
    await user.click(await screen.findByRole("button", { name: "Add customer" }));
    await user.click(within(dialog()).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Add customer" }));
    dialog().dispatchEvent(new Event("cancel", { cancelable: true }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("edits a customer, sending the whole record with its revision", async () => {
    const { user, shop } = await signedIn({ hash: "#/customers" });
    const row = (await screen.findByText("Ali Traders")).closest("tr")!;
    await user.click(within(row).getByRole("button", { name: "Edit" }));
    const name = within(dialog()).getByLabelText("Name");
    await user.clear(name);
    await user.type(name, "Ali Traders & Sons");
    await user.click(within(dialog()).getByRole("button", { name: "Save changes" }));
    expect(await screen.findByText("Ali Traders & Sons")).toBeInTheDocument();
    const saved = shop.customers[0]!;
    expect([saved.revision, saved.phone, saved.address]).toEqual([2, "0300-1234567", "Hussain Agahi, Multan"]);
  });

  it("explains a stale edit, reloads, and closes the form", async () => {
    const { user, shop } = await signedIn({ hash: "#/customers" });
    const row = (await screen.findByText("Ali Traders")).closest("tr")!;
    await user.click(within(row).getByRole("button", { name: "Edit" }));
    shop.customers[0]!.name = "Ali Traders (changed by someone else)";
    shop.customers[0]!.revision += 1;
    await user.type(within(dialog()).getByLabelText("Notes"), "my note");
    await user.click(within(dialog()).getByRole("button", { name: "Save changes" }));
    expect(await screen.findByText(/Someone else changed this record/)).toBeInTheDocument();
    expect(await screen.findByText("Ali Traders (changed by someone else)")).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("deactivates and reactivates", async () => {
    const { user } = await signedIn({ hash: "#/customers" });
    const row = (await screen.findByText("Dawood Spares")).closest("tr")!;
    await user.click(within(row).getByRole("button", { name: "Deactivate" }));
    await waitFor(() => expect(screen.queryByText("Dawood Spares")).not.toBeInTheDocument());
    await user.click(screen.getByRole("checkbox", { name: "Show inactive" }));
    const again = (await screen.findByText("Dawood Spares")).closest("tr")!;
    expect(within(again).getByText("Inactive")).toBeInTheDocument();
    await user.click(within(again).getByRole("button", { name: "Activate" }));
    await user.click(screen.getByRole("checkbox", { name: "Show inactive" }));
    expect(await screen.findByText("Dawood Spares")).toBeInTheDocument();
  });

  it("shows a read-only list without edit buttons to someone who can only view", async () => {
    const shop = shopWithUser("Viewer", ["customers.view"]);
    await signedIn({ shop, person: "Viewer", pin: "2222", hash: "#/customers" });
    expect(await screen.findByText("Ali Traders")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Add customer" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Edit" })).not.toBeInTheDocument();
  });

  it("turns changes off, with a note, when the subscription is not active", async () => {
    const shop = demoShop();
    shop.mode = "read_only";
    await signedIn({ shop, hash: "#/customers" });
    expect(await screen.findByText(/changes are turned off/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add customer" })).toBeDisabled();
    expect(screen.getAllByRole("button", { name: "Edit" })[0]).toBeDisabled();
    expect(screen.getByText("Ali Traders")).toBeInTheDocument();
  });

  it("pages through a long list", async () => {
    const shop = demoShop();
    for (let i = 6; i <= 60; i++) shop.customers.push({ id: `x${i}`, code: `C${String(i).padStart(5, "0")}`, name: `Customer ${String(i).padStart(2, "0")}`, phone: null, address: null, notes: null, is_active: true, revision: 1 });
    const { user } = await signedIn({ shop, hash: "#/customers" });
    expect(await screen.findByText("Showing 1 to 25 of 60")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Next" }));
    expect(await screen.findByText("Showing 26 to 50 of 60")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Previous" })).toBeEnabled();
  });

  it("stays on the page you chose, even right after the screen opens", async () => {
    const shop = demoShop();
    for (let i = 6; i <= 60; i++) shop.customers.push({ id: `x${i}`, code: `C${String(i).padStart(5, "0")}`, name: `Customer ${String(i).padStart(2, "0")}`, phone: null, address: null, notes: null, is_active: true, revision: 1 });
    const { user } = await signedIn({ shop, hash: "#/customers" });
    await user.click(await screen.findByRole("button", { name: "Next" }));
    await new Promise((r) => setTimeout(r, 450)); // longer than the search box's typing pause
    expect(screen.getByText("Showing 26 to 50 of 60")).toBeInTheDocument();
  });
});
