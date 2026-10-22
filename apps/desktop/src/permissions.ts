import { hasPermission } from "@dsaas/domain";

/**
 * Does `granted` include everything `p` stands for? `*` and `module.*` are only covered by `*` or the
 * identical string. Mirrors `covers` in crates/db/src/roles.rs, which is what the server enforces:
 * the UI uses it only to avoid offering choices the server would refuse.
 */
export function covers(granted: readonly string[], p: string): boolean {
  if (p === "*") return granted.includes("*");
  if (p.endsWith(".*")) return granted.includes("*") || granted.includes(p);
  return hasPermission(granted, p);
}

export function moduleOf(permission: string): string {
  return permission.split(".")[0] ?? permission;
}

export function describePermission(p: string): string {
  if (p === "*") return "Everything";
  if (p.endsWith(".*")) return `All of ${moduleOf(p)}`;
  return p;
}
