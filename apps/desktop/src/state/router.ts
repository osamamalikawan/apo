import { useCallback, useEffect, useState } from "react";

/** Hash routing: `#/customers`, `#/settings/roles`. Simple and works the same in a browser and in Tauri. */
export function useHashRoute() {
  const [hash, setHash] = useState(() => window.location.hash);
  useEffect(() => {
    const on = () => setHash(window.location.hash);
    window.addEventListener("hashchange", on);
    return () => window.removeEventListener("hashchange", on);
  }, []);
  const parts = hash.replace(/^#\/?/, "").split("/").filter(Boolean);
  const navigate = useCallback((path: string) => {
    window.location.hash = "#/" + path.replace(/^\/+/, "");
  }, []);
  return { page: parts[0] ?? "dashboard", arg: parts[1], navigate };
}
