import { useCallback, useState } from "react";

const KEY = "apo.sidebar";

function initial(): boolean {
  try {
    const saved = localStorage.getItem(KEY);
    if (saved === "collapsed") return true;
    if (saved === "expanded") return false;
  } catch {
    /* ignore */
  }
  return window.innerWidth <= 760; // small windows start with the rail closed
}

/** Sidebar open/closed state, remembered between runs. Returns [collapsed, toggle, setCollapsed]. */
export function useSidebar(): [boolean, () => void, (v: boolean) => void] {
  const [collapsed, setState] = useState<boolean>(initial);
  const set = useCallback((v: boolean) => {
    setState(v);
    try {
      localStorage.setItem(KEY, v ? "collapsed" : "expanded");
    } catch {
      /* ignore */
    }
  }, []);
  const toggle = useCallback(() => set(!collapsed), [collapsed, set]);
  return [collapsed, toggle, set];
}
