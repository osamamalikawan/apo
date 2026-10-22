// Thin typed wrapper around fetch for the Shop Server API.
// The browser talks to its own origin; in development the Vite proxy forwards /v1 to the Shop Server and
// adds the paired device's credentials. In the Tauri app the same calls will go through the Rust side.

export class ApiError extends Error {
  constructor(public status: number, public code: string, message: string, public reason: string | null = null) {
    super(message);
    this.name = "ApiError";
  }
}

export interface Api {
  get<T>(path: string): Promise<T>;
  post<T>(path: string, body?: unknown): Promise<T>;
  put<T>(path: string, body?: unknown): Promise<T>;
  del<T>(path: string): Promise<T>;
}

export interface ApiOptions {
  getToken: () => string | null;
  /** Called when the server says the user session is gone (expired, signed out, user deactivated). */
  onSessionLost: () => void;
  fetchFn?: typeof fetch;
}

export function createApi(o: ApiOptions): Api {
  async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
    const f = o.fetchFn ?? fetch;
    const headers: Record<string, string> = { accept: "application/json" };
    const token = o.getToken();
    if (token) headers.authorization = `Bearer ${token}`;
    if (body !== undefined) headers["content-type"] = "application/json";
    const res = await f(path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
    if (res.status === 204) return undefined as T;
    let json: unknown = null;
    try {
      json = await res.json();
    } catch {
      /* not JSON */
    }
    if (!res.ok) {
      const e = (json ?? {}) as { error?: string; message?: string; reason?: string | null };
      const err = new ApiError(res.status, e.error ?? "http_" + res.status, e.message ?? res.statusText, e.reason ?? null);
      if (res.status === 401 && err.code === "session_required") o.onSessionLost();
      throw err;
    }
    return json as T;
  }
  return {
    get: (p) => request("GET", p),
    post: (p, b) => request("POST", p, b ?? {}),
    put: (p, b) => request("PUT", p, b ?? {}),
    del: (p) => request("DELETE", p),
  };
}
