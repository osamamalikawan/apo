import { describe, expect, it } from "vitest";
import { covers, describePermission, moduleOf } from "../src/permissions.ts";

describe("covers (mirrors crates/db/src/roles.rs)", () => {
  it("lets * cover everything", () => {
    for (const p of ["*", "sales.*", "sales.create", "users.manage"]) expect(covers(["*"], p)).toBe(true);
  });
  it("covers a plain permission by exact match or by its module wildcard", () => {
    expect(covers(["sales.create"], "sales.create")).toBe(true);
    expect(covers(["sales.*"], "sales.create")).toBe(true);
    expect(covers(["inventory.view"], "inventory.transfer")).toBe(false);
    expect(covers(["inventoryx.*"], "inventory.view")).toBe(false);
    expect(covers([], "sales.create")).toBe(false);
  });
  it("covers a wildcard only with * or the identical wildcard", () => {
    expect(covers(["sales.create", "sales.view"], "sales.*")).toBe(false);
    expect(covers(["sales.*"], "sales.*")).toBe(true);
    expect(covers(["sales.*"], "*")).toBe(false);
    expect(covers(["inventory.*"], "sales.*")).toBe(false);
  });
  it("names permissions for people", () => {
    expect(describePermission("*")).toBe("Everything");
    expect(describePermission("sales.*")).toBe("All of sales");
    expect(describePermission("sales.create")).toBe("sales.create");
    expect(moduleOf("inventory.transfer")).toBe("inventory");
  });
});
