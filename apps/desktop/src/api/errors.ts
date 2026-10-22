import { ApiError } from "./client.ts";

/** What to tell a person when a call fails: plain words, and what to do next. */
export function describeError(e: unknown): string {
  if (e instanceof ApiError) {
    switch (e.code) {
      case "invalid_credentials":
        return "Wrong PIN or password, or the account is locked for a few minutes.";
      case "session_required":
        return "Your session ended. Sign in again.";
      case "unauthorized":
        return "This PC is not paired with the shop server. Pair it first, then try again.";
      case "password_login_required":
        return "Sign in with your password (not a PIN) to do this.";
      case "stale_revision":
        return "Someone else changed this record. The latest version has been loaded; check it and save again.";
      case "user_limit_reached":
        return "Your plan's user limit is reached. Deactivate a user or upgrade the plan.";
      case "location_limit_reached":
        return "Your plan's location limit is reached. Deactivate a location or upgrade the plan.";
      case "device_limit_reached":
        return "Your plan's device limit is reached.";
      case "subscription_inactive":
        return "The subscription is not active, so changes are turned off. You can still view everything.";
      case "forbidden":
        switch (e.reason) {
          case "location_denied":
            return "You do not have access to that location.";
          case "route_denied":
            return "You do not have access to that route.";
          case "cannot_grant_more_than_you_hold":
            return "You can only give permissions and locations that you have yourself.";
          case "needs_all_locations":
            return "Only users with access to every location can add locations.";
          default:
            return "You do not have permission to do this.";
        }
      case "not_found":
        return "That record no longer exists.";
      default:
        return e.message || "Something went wrong. Try again.";
    }
  }
  if (e instanceof TypeError) return "Cannot reach the shop server. Check that it is running, then try again.";
  return "Something went wrong. Try again.";
}

export function isStale(e: unknown): boolean {
  return e instanceof ApiError && e.code === "stale_revision";
}
