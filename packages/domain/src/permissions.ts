// Permission strings are "module.action". "*" grants everything, "module.*" grants a module.
// Scopes (locations, routes) are independent of module permissions.
export interface AccessContext {
  permissions: readonly string[];
  allLocations: boolean;
  locationIds: readonly string[];
  allRoutes: boolean;
  routeIds: readonly string[];
}
export interface Scope { locationId?: string; routeId?: string }
export type Decision = { allowed: true } | { allowed: false; reason: "no_permission" | "location_denied" | "route_denied" };

export function hasPermission(granted: readonly string[], needed: string): boolean {
  const mod = needed.split(".")[0];
  return granted.some((g) => g === "*" || g === needed || g === `${mod}.*`);
}

export function authorize(ctx: AccessContext, needed: string, scope: Scope = {}): Decision {
  if (!hasPermission(ctx.permissions, needed)) return { allowed: false, reason: "no_permission" };
  if (scope.locationId !== undefined && !ctx.allLocations && !ctx.locationIds.includes(scope.locationId))
    return { allowed: false, reason: "location_denied" };
  if (scope.routeId !== undefined && !ctx.allRoutes && !ctx.routeIds.includes(scope.routeId))
    return { allowed: false, reason: "route_denied" };
  return { allowed: true };
}
