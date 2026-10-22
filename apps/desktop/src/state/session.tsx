import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { hasPermission } from "@dsaas/domain";
import { createApi, type Api } from "../api/client.ts";
import type { LoginResult, Me, SignInMethod, Whoami } from "../api/types.ts";

const TOKEN_KEY = "apo.token";

export type SessionStatus = "loading" | "signed-out" | "signed-in";

export interface SessionValue {
  api: Api;
  status: SessionStatus;
  me: Me | null;
  tenantName: string;
  /** Shown on the sign-in screen after a session ends on its own. */
  notice: string | null;
  signIn(username: string, secret: string, method: SignInMethod): Promise<void>;
  signOut(notice?: string): Promise<void>;
  can(permission: string): boolean;
  canAny(permissions: readonly string[]): boolean;
}

const Ctx = createContext<SessionValue | null>(null);

export function useSession(): SessionValue {
  const v = useContext(Ctx);
  if (!v) throw new Error("useSession must be used inside <SessionProvider>");
  return v;
}

function readToken(): string | null {
  try {
    return sessionStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}
function writeToken(t: string | null) {
  try {
    if (t) sessionStorage.setItem(TOKEN_KEY, t);
    else sessionStorage.removeItem(TOKEN_KEY);
  } catch {
    /* storage unavailable: the session just will not survive a reload */
  }
}

export function SessionProvider({ children, fetchFn }: { children: ReactNode; fetchFn?: typeof fetch }) {
  const token = useRef<string | null>(readToken());
  const [status, setStatus] = useState<SessionStatus>(token.current ? "loading" : "signed-out");
  const [me, setMe] = useState<Me | null>(null);
  const [tenantName, setTenantName] = useState("Irfan Autos");
  const [notice, setNotice] = useState<string | null>(null);

  const endSession = useCallback((why?: string) => {
    token.current = null;
    writeToken(null);
    setMe(null);
    setStatus("signed-out");
    setNotice(why ?? null);
  }, []);
  const lost = useRef(endSession);
  lost.current = endSession;

  const api = useMemo(
    () => createApi({ getToken: () => token.current, onSessionLost: () => lost.current("Your session ended. Sign in again."), fetchFn }),
    [fetchFn],
  );

  useEffect(() => {
    let alive = true;
    api.get<Whoami>("/v1/whoami").then((w) => alive && w.tenant_name && setTenantName(w.tenant_name)).catch(() => {});
    if (token.current) {
      api
        .get<Me>("/v1/me")
        .then((m) => {
          if (!alive) return;
          setMe(m);
          setStatus("signed-in");
        })
        .catch(() => alive && endSession());
    }
    return () => {
      alive = false;
    };
  }, [api, endSession]);

  const signIn = useCallback(
    async (username: string, secret: string, method: SignInMethod) => {
      const body = method === "pin" ? { username, pin: secret } : { username, password: secret };
      const res = await api.post<LoginResult>("/v1/auth/login", body);
      token.current = res.token;
      writeToken(res.token);
      try {
        setMe(await api.get<Me>("/v1/me"));
      } catch (e) {
        endSession();
        throw e;
      }
      setNotice(null);
      setStatus("signed-in");
    },
    [api, endSession],
  );

  const signOut = useCallback(
    async (why?: string) => {
      try {
        if (token.current) await api.post("/v1/auth/logout");
      } catch {
        /* the session is being dropped either way */
      }
      endSession(why);
    },
    [api, endSession],
  );

  const value = useMemo<SessionValue>(() => {
    const granted = me?.permissions ?? [];
    return {
      api,
      status,
      me,
      tenantName,
      notice,
      signIn,
      signOut,
      can: (p) => hasPermission(granted, p),
      canAny: (ps) => ps.some((p) => hasPermission(granted, p)),
    };
  }, [api, status, me, tenantName, notice, signIn, signOut]);

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}
