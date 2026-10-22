# ADR 0007: Users, login and sessions on the Shop Server

Status: accepted. Hard to reverse because clients, stored hashes and audit history depend on it.

## Decision
- **Two layers of identity.** A paired *device* proves "this PC belongs to the shop" (ADR 0004). A signed-in *user* proves "this person is
  at the counter". Every user action needs both. A session token only works from the device that created it.
- **Secrets:** passwords and PINs are stored only as Argon2id PHC strings (19 MiB, 2 passes, 1 lane = the OWASP minimum; the parameters live
  in the hash, so they can be raised later without a migration). Passwords are 10 to 128 characters; PINs are 4 to 8 digits.
- **Sessions:** 32 random bytes, only the SHA-256 hash is stored. Absolute lifetime 12 hours, idle timeout 2 hours. Changing a password,
  deactivating a user and logging out all revoke sessions server-side, so they take effect immediately.
- **Lockout:** 5 wrong attempts lock the account for 15 minutes. Password and PIN share one counter. Wrong, unknown, locked and
  deactivated users all return the same error and do comparable work, so usernames cannot be probed.
- **PIN sessions are weaker on purpose.** A PIN signs in quickly on a counter PC but the session is marked `pin`: it cannot manage users or
  change credentials. A 4-digit PIN is guessable in principle; the lockout (about 5 guesses per 15 minutes) and the paired-device
  requirement are what make that acceptable.
- **No hard deletes.** Users are deactivated, never deleted, so history stays attributable. The last active holder of the all-permissions
  grant ("Owner") cannot be deactivated.
- **First owner and recovery** happen only at the server console (`DSAAS_ADMIN_PASSWORD` for the dev runner). There is no remote reset.
- **Subscription:** signing in, signing out and changing your own password or PIN keep working when a subscription is suspended. Creating
  users is a business write and is refused when the mode does not allow writing; it is also bounded by the plan's `max_users`.

## Consequences
- New users have no permissions until the roles module (build step 2) lets an owner assign roles.
- There is no per-IP rate limit yet; the per-account lockout is the only brute-force control (listed in STATUS).
- The PIN sign-in screen lists PIN users through `GET /v1/login-users` (ADR 0010): device credentials only, names only.
- Session tokens travel in an `Authorization: Bearer` header over the pinned HTTPS channel (ADR 0005).
