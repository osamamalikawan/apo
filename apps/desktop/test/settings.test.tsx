import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { demoShop } from "../src/api/mock.ts";
import { renderApp, shopWithUser, signedIn } from "./helpers.tsx";

const dialog = () => screen.getByRole("dialog");
const rowOf = async (name: string) => (await within(await screen.findByRole("table")).findByText(name)).closest("tr")!;

/** Settings pages that change users and roles need a password sign-in; this helper does that. */
async function passwordSignIn(shop = demoShop(), hash = "#/settings/users") {
  const r = renderApp({ shop, hash });
  await r.user.click(await screen.findByRole("button", { name: "Sign in with a password instead" }));
  await r.user.type(screen.getByLabelText("Username"), "irfan");
  await r.user.type(screen.getByLabelText("Password"), "irfan-password-1");
  await r.user.click(screen.getByRole("button", { name: "Sign in" }));
  await screen.findByRole("button", { name: "Sign out" });
  return r;
}

describe("Settings: My account", () => {
  it("is the only tab for someone who cannot manage users or roles", async () => {
    const shop = shopWithUser("Counter Only", ["sales.create"]);
    await signedIn({ shop, person: "Counter Only", pin: "2222", hash: "#/settings" });
    expect(await screen.findByRole("tab", { name: "My account" })).toHaveAttribute("aria-selected", "true");
    expect(screen.queryByRole("tab", { name: "Users" })).not.toBeInTheDocument();
    expect(screen.queryByRole("tab", { name: "Roles" })).not.toBeInTheDocument();
  });

  it("sets a PIN after checking the current password", async () => {
    const shop = shopWithUser("No Pin Yet", ["sales.create"]);
    shop.users.find((u) => u.id === "u-test")!.pin = null;
    const { user } = renderApp({ shop, hash: "#/settings" });
    await user.click(await screen.findByRole("button", { name: "Sign in with a password instead" }));
    await user.type(screen.getByLabelText("Username"), "tester");
    await user.type(screen.getByLabelText("Password"), "tester-password-1");
    await user.click(screen.getByRole("button", { name: "Sign in" }));
    await screen.findByRole("heading", { name: "Your PIN" });

    await user.type(screen.getByLabelText("New PIN"), "48ab15"); // letters are ignored
    expect(screen.getByLabelText("New PIN")).toHaveValue("4815");
    await user.type(screen.getAllByLabelText("Current password")[0]!, "wrong password");
    await user.click(screen.getByRole("button", { name: "Save PIN" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/Wrong PIN or password/);
    expect(shop.users.find((u) => u.id === "u-test")!.pin).toBeNull();

    const pw = screen.getAllByLabelText("Current password")[0]!;
    await user.clear(pw);
    await user.type(pw, "tester-password-1");
    await user.click(screen.getByRole("button", { name: "Save PIN" }));
    expect(await screen.findByText("PIN saved. You can now sign in with it.")).toBeInTheDocument();
    expect(shop.users.find((u) => u.id === "u-test")!.pin).toBe("4815");
  });

  it("will not save a PIN shorter than 4 digits", async () => {
    const { user } = await signedIn({ hash: "#/settings" });
    await user.type(await screen.findByLabelText("New PIN"), "123");
    await user.type(screen.getAllByLabelText("Current password")[0]!, "irfan-password-1");
    expect(screen.getByRole("button", { name: "Save PIN" })).toBeDisabled();
  });

  it("changes the password, then signs everyone out with a clear notice", async () => {
    const { user, shop } = await passwordSignIn(demoShop(), "#/settings");
    await screen.findByRole("heading", { name: "Change password" });
    const [, cur] = screen.getAllByLabelText("Current password");
    await user.type(cur!, "irfan-password-1");
    await user.type(screen.getByLabelText("New password"), "a much longer password");
    await user.type(screen.getByLabelText("New password again"), "a different password");
    await user.click(screen.getByRole("button", { name: "Change password" }));
    expect(await screen.findByText("The two new passwords are not the same.")).toBeInTheDocument();
    const again = screen.getByLabelText("New password again");
    await user.clear(again);
    await user.type(again, "a much longer password");
    await user.click(screen.getByRole("button", { name: "Change password" }));
    expect(await screen.findByText("Password changed. Sign in with your new password.")).toBeInTheDocument();
    expect(shop.users[0]!.password).toBe("a much longer password");
    expect(screen.getByText("Who is signing in?")).toBeInTheDocument();
  });

  it("explains, and disables the forms, when signed in with a PIN", async () => {
    await signedIn({ hash: "#/settings" });
    expect(await screen.findByText(/You signed in with a PIN/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save PIN" })).toBeDisabled();
    expect(screen.getByLabelText("New PIN")).toBeDisabled();
  });
});

describe("Settings: Users", () => {
  it("lists people with their roles and locations", async () => {
    await passwordSignIn();
    const row = await rowOf("Sana Malik");
    expect(within(row).getByText("Cashier")).toBeInTheDocument();
    expect(within(row).getByText("Main Shop")).toBeInTheDocument();
    const owner = await rowOf("Irfan Ahmed");
    expect(within(owner).getByText("All locations")).toBeInTheDocument();
  });

  it("is read-only when signed in with a PIN", async () => {
    await signedIn({ hash: "#/settings/users" });
    expect(await screen.findByText(/Sign in with your password to change users/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add user" })).toBeDisabled();
  });

  it("adds a user and reminds the owner to give them a role", async () => {
    const { user, shop } = await passwordSignIn();
    await user.click(await screen.findByRole("button", { name: "Add user" }));
    await user.type(within(dialog()).getByLabelText(/Full name/), "Nadia Khan");
    await user.type(within(dialog()).getByLabelText(/Username/), "Nadia");
    expect(within(dialog()).getByRole("button", { name: "Add user" })).toBeDisabled();
    await user.type(within(dialog()).getByLabelText(/First password/), "a long first password");
    await user.click(within(dialog()).getByRole("button", { name: "Add user" }));
    expect(await screen.findByText(/Added Nadia Khan\. Give them a role/)).toBeInTheDocument();
    expect(shop.users.at(-1)).toMatchObject({ username: "nadia", active: true, role_ids: [] });
    expect(await screen.findByText("No role yet")).toBeInTheDocument();
  });

  it("shows the server's reason when a username is taken", async () => {
    const { user } = await passwordSignIn();
    await user.click(await screen.findByRole("button", { name: "Add user" }));
    await user.type(within(dialog()).getByLabelText(/Full name/), "Another Sana");
    await user.type(within(dialog()).getByLabelText(/Username/), "sana");
    await user.type(within(dialog()).getByLabelText(/First password/), "a long first password");
    await user.click(within(dialog()).getByRole("button", { name: "Add user" }));
    expect(await within(dialog()).findByRole("alert")).toHaveTextContent("username already exists");
  });

  it("assigns roles", async () => {
    const { user, shop } = await passwordSignIn();
    const row = await rowOf("Tariq Hussain");
    await user.click(within(row).getByRole("button", { name: "Roles" }));
    await user.click(within(dialog()).getByRole("checkbox", { name: "Cashier" }));
    await user.click(within(dialog()).getByRole("button", { name: "Save roles" }));
    await waitFor(() => expect(shop.users.find((u) => u.id === "u-tariq")!.role_ids.sort()).toEqual(["r-cashier", "r-keeper"]));
    expect(await screen.findByText("Roles saved for Tariq Hussain")).toBeInTheDocument();
  });

  it("sets location access", async () => {
    const { user, shop } = await passwordSignIn();
    const row = await rowOf("Sana Malik");
    await user.click(within(row).getByRole("button", { name: "Locations" }));
    await user.click(within(dialog()).getByRole("checkbox", { name: /Godown 2/ }));
    await user.click(within(dialog()).getByRole("button", { name: "Save locations" }));
    await waitFor(() => expect(shop.users.find((u) => u.id === "u-sana")!.location_ids.sort()).toEqual(["l-g2", "l-main"]));
  });

  it("asks before deactivating, then deactivates", async () => {
    const { user, shop } = await passwordSignIn();
    const row = await rowOf("Sana Malik");
    await user.click(within(row).getByRole("button", { name: "Deactivate" }));
    expect(within(dialog()).getByText(/signed out straight away/)).toBeInTheDocument();
    await user.click(within(dialog()).getByRole("button", { name: "Deactivate user" }));
    await waitFor(() => expect(shop.users.find((u) => u.id === "u-sana")!.active).toBe(false));
    expect(await screen.findByText("Deactivated Sana Malik")).toBeInTheDocument();
  });

  it("refuses to deactivate the last owner and says why", async () => {
    const { user } = await passwordSignIn();
    const row = await rowOf("Irfan Ahmed");
    await user.click(within(row).getByRole("button", { name: "Deactivate" }));
    await user.click(within(dialog()).getByRole("button", { name: "Deactivate user" }));
    expect(await within(dialog()).findByRole("alert")).toHaveTextContent("cannot deactivate the last owner");
  });

  it("is turned off when the subscription is not active", async () => {
    const shop = demoShop();
    shop.mode = "read_only";
    await passwordSignIn(shop);
    expect(await screen.findByText(/changes are turned off/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add user" })).toBeDisabled();
  });
});

describe("Settings: Roles", () => {
  it("lists roles with plain names for permissions and who holds them", async () => {
    await passwordSignIn(demoShop(), "#/settings/roles");
    const owner = (await screen.findByText("Owner")).closest("tr")!;
    expect(within(owner).getByText("Everything")).toBeInTheDocument();
    expect(within(owner).getByText("Cannot be changed")).toBeInTheDocument();
    const keeper = screen.getByText("Storekeeper").closest("tr")!;
    expect(within(keeper).getByText("All of inventory")).toBeInTheDocument();
    expect(within(keeper).getByRole("button", { name: "Edit" })).toBeEnabled();
  });

  it("creates a role from a permission checklist, grouped by module", async () => {
    const { user, shop } = await passwordSignIn(demoShop(), "#/settings/roles");
    await user.click(await screen.findByRole("button", { name: "Add role" }));
    await user.type(within(dialog()).getByLabelText("Role name"), "Counter staff");
    await user.click(within(dialog()).getByRole("checkbox", { name: "View customers" }));
    await user.click(within(dialog()).getByRole("checkbox", { name: "All sales permissions" }));
    await user.click(within(dialog()).getByRole("button", { name: "Add role" }));
    await screen.findByText("Counter staff");
    expect(shop.roles.find((r) => r.name === "Counter staff")!.permissions).toEqual(["customers.view", "sales.*"]);
  });

  it("checks every box in a module once its 'All' box is on, and locks them", async () => {
    const { user } = await passwordSignIn(demoShop(), "#/settings/roles");
    await user.click(await screen.findByRole("button", { name: "Add role" }));
    await user.click(within(dialog()).getByRole("checkbox", { name: "All sales permissions" }));
    const create = within(dialog()).getByRole("checkbox", { name: "Create sales" });
    expect(create).toBeChecked();
    expect(create).toBeDisabled();
  });

  it("edits a role, and shows the server's reason for a duplicate name", async () => {
    const { user, shop } = await passwordSignIn(demoShop(), "#/settings/roles");
    const row = (await screen.findByText("Cashier")).closest("tr")!;
    await user.click(within(row).getByRole("button", { name: "Edit" }));
    await user.click(within(dialog()).getByRole("checkbox", { name: "View sales" }));
    await user.click(within(dialog()).getByRole("button", { name: "Save role" }));
    await waitFor(() => expect(shop.roles.find((r) => r.id === "r-cashier")!.permissions).toContain("sales.view"));

    await user.click(await screen.findByRole("button", { name: "Add role" }));
    await user.type(within(dialog()).getByLabelText("Role name"), "cashier");
    await user.click(within(dialog()).getByRole("button", { name: "Add role" }));
    expect(await within(dialog()).findByRole("alert")).toHaveTextContent("a role with that name already exists");
  });

  it("will not delete a role that people still hold, and deletes one that nobody holds", async () => {
    const shop = demoShop();
    shop.roles.push({ id: "r-spare", name: "Spare role", permissions: ["reports.view"], member_count: 0, protected: false });
    const { user } = await passwordSignIn(shop, "#/settings/roles");
    const cashier = (await screen.findByText("Cashier")).closest("tr")!;
    await user.click(within(cashier).getByRole("button", { name: "Delete" }));
    expect(within(dialog()).getByText(/1 person has this role/)).toBeInTheDocument();
    await user.click(within(dialog()).getByRole("button", { name: "Delete role" }));
    expect(await within(dialog()).findByRole("alert")).toHaveTextContent("role is assigned to users");
    await user.click(within(dialog()).getByRole("button", { name: "Cancel" }));

    const spare = (await screen.findByText("Spare role")).closest("tr")!;
    await user.click(within(spare).getByRole("button", { name: "Delete" }));
    expect(within(dialog()).getByText("No one has this role.")).toBeInTheDocument();
    await user.click(within(dialog()).getByRole("button", { name: "Delete role" }));
    expect(await screen.findByText("Deleted Spare role")).toBeInTheDocument();
    await waitFor(() => expect(shop.roles.some((r) => r.id === "r-spare")).toBe(false));
  });

  it("only offers permissions the signed-in person holds themselves", async () => {
    const shop = shopWithUser("Role Manager", ["roles.manage", "customers.view", "users.manage"]);
    const { user } = renderApp({ shop, hash: "#/settings/roles" });
    await user.click(await screen.findByRole("button", { name: "Sign in with a password instead" }));
    await user.type(screen.getByLabelText("Username"), "tester");
    await user.type(screen.getByLabelText("Password"), "tester-password-1");
    await user.click(screen.getByRole("button", { name: "Sign in" }));
    await user.click(await screen.findByRole("button", { name: "Add role" }));
    expect(within(dialog()).getByRole("checkbox", { name: "View customers" })).toBeEnabled();
    expect(within(dialog()).getByRole("checkbox", { name: "Create sales" })).toBeDisabled();
    expect(within(dialog()).getByRole("checkbox", { name: "All sales permissions" })).toBeDisabled();
    expect(within(dialog()).getByRole("checkbox", { name: /Everything, including permissions added later/ })).toBeDisabled();
  });

  it("does not let a role manager edit or hand out a role stronger than themselves", async () => {
    const shop = shopWithUser("Role Manager", ["roles.manage", "customers.view", "users.manage"]);
    const { user } = renderApp({ shop, hash: "#/settings/roles" });
    await user.click(await screen.findByRole("button", { name: "Sign in with a password instead" }));
    await user.type(screen.getByLabelText("Username"), "tester");
    await user.type(screen.getByLabelText("Password"), "tester-password-1");
    await user.click(screen.getByRole("button", { name: "Sign in" }));
    const keeper = (await screen.findByText("Storekeeper")).closest("tr")!;
    expect(within(keeper).getByRole("button", { name: "Edit" })).toBeDisabled();
    expect(within(keeper).getByRole("button", { name: "Delete" })).toBeDisabled();
  });

  it("is read-only when signed in with a PIN", async () => {
    await signedIn({ hash: "#/settings/roles" });
    expect(await screen.findByText(/Sign in with your password to change roles/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add role" })).toBeDisabled();
  });
});

describe("Settings: access", () => {
  it("shows Users and Roles tabs to an owner, and the tab in the address opens", async () => {
    await signedIn({ hash: "#/settings/roles" });
    expect(await screen.findByRole("tab", { name: "Roles" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tab", { name: "Users" })).toBeInTheDocument();
  });

  it("falls back to My account when the address names a tab the person may not use", async () => {
    const shop = shopWithUser("Counter Only", ["sales.create"]);
    await signedIn({ shop, person: "Counter Only", pin: "2222", hash: "#/settings/roles" });
    expect(await screen.findByRole("tab", { name: "My account" })).toHaveAttribute("aria-selected", "true");
  });
});

