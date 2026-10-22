import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import type { License } from "../api/types.ts";
import { useSession } from "./session.tsx";

interface LicenseValue { license: License | null; canWrite: boolean }
const Ctx = createContext<LicenseValue>({ license: null, canWrite: true });

export function useLicense(): LicenseValue {
  return useContext(Ctx);
}

/** Subscription status. While it is unknown, writes are allowed in the UI and the server stays the judge. */
export function LicenseProvider({ children }: { children: ReactNode }) {
  const { api } = useSession();
  const [license, setLicense] = useState<License | null>(null);
  useEffect(() => {
    let alive = true;
    api.get<License>("/v1/license/status").then((l) => alive && setLicense(l)).catch(() => {});
    return () => {
      alive = false;
    };
  }, [api]);
  return <Ctx.Provider value={{ license, canWrite: license ? license.can_write : true }}>{children}</Ctx.Provider>;
}
