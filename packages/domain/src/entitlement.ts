// Subscription enforcement rules. The cloud is the only authority; the shop server only evaluates a
// SIGNED entitlement it received from the cloud. All dates are UTC strings "YYYY-MM-DDTHH:MM:SSZ",
// which compare correctly as plain strings, so no date library or timezone logic is needed.
export type Mode = "full" | "grace" | "read_only" | "unlicensed";
export type Reason =
  | "active" | "grace_period" | "suspended" | "cancelled" | "expired"
  | "no_entitlement" | "wrong_tenant" | "invalid_status";

export interface EntitlementLike {
  tenant_id: string;
  status: string;
  full_until: string;   // end of the normal period: min(paid-through date, next required cloud check)
  grace_until: string;  // full_until + grace days: still works, with a warning
}
export interface LicenseDecision { mode: Mode; reason: Reason }

export const canWrite = (m: Mode): boolean => m === "full" || m === "grace";

export function isIsoSeconds(s: string): boolean {
  return /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/.test(s);
}

/**
 * @param highWater the latest time this database has ever observed. If the PC clock is set back
 *                  before it, the high-water mark is used instead, so rewinding the clock cannot
 *                  extend a subscription.
 */
export function evaluate(
  ent: EntitlementLike | null,
  tenantId: string,
  now: string,
  highWater: string | null = null,
): LicenseDecision {
  if (!ent) return { mode: "unlicensed", reason: "no_entitlement" };
  if (ent.tenant_id !== tenantId) return { mode: "unlicensed", reason: "wrong_tenant" };
  const effective = highWater !== null && highWater > now ? highWater : now;
  switch (ent.status) {
    case "suspended": return { mode: "read_only", reason: "suspended" };
    case "cancelled": return { mode: "read_only", reason: "cancelled" };
    case "active":
      if (effective <= ent.full_until) return { mode: "full", reason: "active" };
      if (effective <= ent.grace_until) return { mode: "grace", reason: "grace_period" };
      return { mode: "read_only", reason: "expired" };
    default: return { mode: "unlicensed", reason: "invalid_status" };
  }
}
