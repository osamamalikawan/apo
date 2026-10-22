import { describe, expect, it, vi } from "vitest";
import { ApiError, createApi } from "../src/api/client.ts";
import { describeError } from "../src/api/errors.ts";

const json = (status: number, body: unknown) => new Response(status === 204 ? null : JSON.stringify(body), { status });

describe("api client", () => {
  it("sends the bearer token and JSON bodies, and returns parsed JSON", async () => {
    const f = vi.fn(async () => json(200, { ok: true }));
    const api = createApi({ getToken: () => "tok", onSessionLost: () => {}, fetchFn: f as unknown as typeof fetch });
    expect(await api.post("/v1/x", { a: 1 })).toEqual({ ok: true });
    const [path, init] = f.mock.calls[0] as unknown as [string, RequestInit];
    expect(path).toBe("/v1/x");
    expect(init.method).toBe("POST");
    expect((init.headers as Record<string, string>).authorization).toBe("Bearer tok");
    expect(init.body).toBe('{"a":1}');
  });

  it("sends no authorization header when signed out", async () => {
    const f = vi.fn(async () => json(200, []));
    await createApi({ getToken: () => null, onSessionLost: () => {}, fetchFn: f as unknown as typeof fetch }).get("/v1/login-users");
    expect((f.mock.calls[0] as unknown as [string, RequestInit])[1].headers).not.toHaveProperty("authorization");
  });

  it("treats 204 as no content", async () => {
    const api = createApi({ getToken: () => "t", onSessionLost: () => {}, fetchFn: (async () => json(204, null)) as unknown as typeof fetch });
    expect(await api.post("/v1/auth/logout")).toBeUndefined();
  });

  it("turns error bodies into ApiError with code and reason", async () => {
    const api = createApi({ getToken: () => "t", onSessionLost: () => {}, fetchFn: (async () => json(403, { error: "forbidden", message: "no", reason: "location_denied" })) as unknown as typeof fetch });
    await expect(api.get("/v1/locations/x")).rejects.toMatchObject({ status: 403, code: "forbidden", reason: "location_denied" });
  });

  it("reports a lost session only for session_required, not for a wrong PIN", async () => {
    const lost = vi.fn();
    let body: unknown = { error: "invalid_credentials", message: "x" };
    const api = createApi({ getToken: () => "t", onSessionLost: lost, fetchFn: (async () => json(401, body)) as unknown as typeof fetch });
    await expect(api.post("/v1/auth/login", {})).rejects.toBeInstanceOf(ApiError);
    expect(lost).not.toHaveBeenCalled();
    body = { error: "session_required", message: "sign in" };
    await expect(api.get("/v1/me")).rejects.toBeInstanceOf(ApiError);
    expect(lost).toHaveBeenCalledTimes(1);
  });

  it("survives an error response that is not JSON", async () => {
    const api = createApi({ getToken: () => "t", onSessionLost: () => {}, fetchFn: (async () => new Response("<html>bad gateway</html>", { status: 502, statusText: "Bad Gateway" })) as unknown as typeof fetch });
    await expect(api.get("/v1/me")).rejects.toMatchObject({ status: 502, code: "http_502" });
  });
});

describe("describeError", () => {
  const e = (code: string, reason: string | null = null, message = "server text") => new ApiError(400, code, message, reason);
  it("explains the common failures in plain words", () => {
    expect(describeError(e("invalid_credentials"))).toMatch(/Wrong PIN or password/);
    expect(describeError(e("stale_revision"))).toMatch(/Someone else changed this record/);
    expect(describeError(e("password_login_required"))).toMatch(/password \(not a PIN\)/);
    expect(describeError(e("subscription_inactive"))).toMatch(/can still view everything/);
    expect(describeError(e("user_limit_reached"))).toMatch(/user limit/);
    expect(describeError(e("location_limit_reached"))).toMatch(/location limit/);
    expect(describeError(e("unauthorized"))).toMatch(/not paired/);
  });
  it("distinguishes the reasons behind a 403", () => {
    expect(describeError(e("forbidden", "location_denied"))).toMatch(/access to that location/);
    expect(describeError(e("forbidden", "cannot_grant_more_than_you_hold"))).toMatch(/have yourself/);
    expect(describeError(e("forbidden", "needs_all_locations"))).toMatch(/every location/);
    expect(describeError(e("forbidden", "no_permission"))).toMatch(/do not have permission/);
  });
  it("passes the server's own words through for validation problems", () => {
    expect(describeError(e("bad_request", null, "name must be 2 to 120 characters"))).toBe("name must be 2 to 120 characters");
    expect(describeError(e("conflict", null, "a customer with that code already exists"))).toBe("a customer with that code already exists");
  });
  it("says when the server cannot be reached", () => {
    expect(describeError(new TypeError("Failed to fetch"))).toMatch(/Cannot reach the shop server/);
  });
});
