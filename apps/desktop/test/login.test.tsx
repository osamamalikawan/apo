import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { demoShop } from "../src/api/mock.ts";
import { renderApp, signInWithPin } from "./helpers.tsx";

const pad = () => screen.getByRole("group", { name: "PIN pad" });

describe("PIN-style sign-in", () => {
  it("shows the shop name and a tile for each person who has a PIN", async () => {
    renderApp();
    expect(await screen.findByRole("heading", { name: "Irfan Autos" })).toBeInTheDocument();
    expect(screen.getByText("Who is signing in?")).toBeInTheDocument();
    for (const n of ["Irfan Ahmed", "Sana Malik", "Tariq Hussain"]) expect(screen.getByRole("button", { name: n })).toBeInTheDocument();
  });

  it("does not list people who have no PIN", async () => {
    const shop = demoShop();
    shop.users[1]!.pin = null;
    renderApp({ shop });
    await screen.findByRole("button", { name: "Irfan Ahmed" });
    expect(screen.queryByRole("button", { name: "Sana Malik" })).not.toBeInTheDocument();
  });

  it("signs in by tapping a name and then the keys on the pad", async () => {
    const { user } = renderApp();
    await user.click(await screen.findByRole("button", { name: "Irfan Ahmed" }));
    expect(screen.getByText("Enter your PIN")).toBeInTheDocument();
    expect(within(pad()).getByText("4 to 8 digits")).toBeInTheDocument();
    for (const d of "1111") await user.click(within(pad()).getByRole("button", { name: d }));
    expect(screen.getByText("4 digits entered")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Sign in" }));
    expect(await screen.findByText("Welcome, Irfan Ahmed.")).toBeInTheDocument();
  });

  it("also takes the keyboard: digits and Enter", async () => {
    const { user } = renderApp();
    await signInWithPin(user, "Sana Malik", "3333");
    expect(await screen.findByText("Welcome, Sana Malik.")).toBeInTheDocument();
  });

  it("does not let Enter press a key that was just tapped with the mouse", async () => {
    const { user } = renderApp();
    await user.click(await screen.findByRole("button", { name: "Irfan Ahmed" }));
    for (const d of "1111") await user.click(within(pad()).getByRole("button", { name: d }));
    await user.keyboard("{Enter}");
    expect(await screen.findByText("Welcome, Irfan Ahmed.")).toBeInTheDocument();
  });

  it("keeps Sign in disabled until 4 digits and stops at 8", async () => {
    const { user } = renderApp();
    await user.click(await screen.findByRole("button", { name: "Irfan Ahmed" }));
    await user.keyboard("123");
    expect(screen.getByRole("button", { name: "Sign in" })).toBeDisabled();
    await user.keyboard("4");
    expect(screen.getByRole("button", { name: "Sign in" })).toBeEnabled();
    await user.keyboard("567890");
    expect(screen.getByText("8 digits entered")).toBeInTheDocument();
  });

  it("deletes the last digit with the delete key and with Backspace", async () => {
    const { user } = renderApp();
    await user.click(await screen.findByRole("button", { name: "Irfan Ahmed" }));
    await user.keyboard("12345");
    await user.click(screen.getByRole("button", { name: "Delete last digit" }));
    expect(screen.getByText("4 digits entered")).toBeInTheDocument();
    await user.keyboard("{Backspace}");
    expect(screen.getByText("3 digits entered")).toBeInTheDocument();
  });

  it("never shows the digits themselves", async () => {
    const { user } = renderApp();
    await user.click(await screen.findByRole("button", { name: "Irfan Ahmed" }));
    await user.keyboard("9876");
    expect(within(pad()).queryByText("9876")).not.toBeInTheDocument();
    expect(document.body.textContent).not.toContain("9876");
  });

  it("explains a wrong PIN in plain words and clears the digits", async () => {
    const { user } = renderApp();
    await user.click(await screen.findByRole("button", { name: "Irfan Ahmed" }));
    await user.keyboard("9999{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent(/Wrong PIN or password, or the account is locked/);
    expect(screen.queryByText(/digits entered/)).not.toBeInTheDocument();
    await user.keyboard("1111{Enter}");
    expect(await screen.findByText("Welcome, Irfan Ahmed.")).toBeInTheDocument();
  });

  it("shows the same message when the account has been locked by repeated wrong PINs", async () => {
    const { user } = renderApp();
    await user.click(await screen.findByRole("button", { name: "Irfan Ahmed" }));
    for (let i = 0; i < 5; i++) await user.keyboard("0000{Enter}");
    await user.keyboard("1111{Enter}"); // the right PIN no longer works while locked
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent(/locked for a few minutes/));
    expect(screen.queryByText("Welcome, Irfan Ahmed.")).not.toBeInTheDocument();
  });

  it("lets someone pick a different person", async () => {
    const { user } = renderApp();
    await user.click(await screen.findByRole("button", { name: "Irfan Ahmed" }));
    await user.click(screen.getByRole("button", { name: /Not you\? Choose someone else/ }));
    expect(screen.getByText("Who is signing in?")).toBeInTheDocument();
  });
});

describe("password sign-in fallback", () => {
  it("is offered under the tiles and signs in", async () => {
    const { user } = renderApp();
    await user.click(await screen.findByRole("button", { name: "Sign in with a password instead" }));
    await user.type(screen.getByLabelText("Username"), "irfan");
    await user.type(screen.getByLabelText("Password"), "irfan-password-1");
    await user.click(screen.getByRole("button", { name: "Sign in" }));
    expect(await screen.findByText("Welcome, Irfan Ahmed.")).toBeInTheDocument();
  });

  it("is the only choice when nobody has a PIN yet, and says why", async () => {
    const shop = demoShop();
    shop.users.forEach((u) => (u.pin = null));
    const { user } = renderApp({ shop });
    expect(await screen.findByText(/No one has set a PIN yet/)).toBeInTheDocument();
    await user.type(screen.getByLabelText("Username"), "irfan");
    await user.type(screen.getByLabelText("Password"), "irfan-password-1");
    await user.click(screen.getByRole("button", { name: "Sign in" }));
    expect(await screen.findByText("Welcome, Irfan Ahmed.")).toBeInTheDocument();
  });

  it("is prefilled with the username when someone chooses 'Use my password' from the PIN pad", async () => {
    const { user } = renderApp();
    await user.click(await screen.findByRole("button", { name: "Tariq Hussain" }));
    await user.click(screen.getByRole("button", { name: "Use my password" }));
    expect(screen.getByLabelText("Username")).toHaveValue("tariq");
  });

  it("shows a wrong password plainly and empties the password box", async () => {
    const { user } = renderApp();
    await user.click(await screen.findByRole("button", { name: "Sign in with a password instead" }));
    await user.type(screen.getByLabelText("Username"), "irfan");
    await user.type(screen.getByLabelText("Password"), "not-the-password");
    await user.click(screen.getByRole("button", { name: "Sign in" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/Wrong PIN or password/);
    expect(screen.getByLabelText("Password")).toHaveValue("");
  });
});

describe("when the PC is not paired", () => {
  it("says so and offers to try again", async () => {
    const shop = demoShop();
    shop.deviceKnown = false;
    const { user } = renderApp({ shop });
    expect(await screen.findByRole("alert")).toHaveTextContent(/not paired with the shop server/);
    shop.deviceKnown = true;
    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("button", { name: "Irfan Ahmed" })).toBeInTheDocument();
  });
});
