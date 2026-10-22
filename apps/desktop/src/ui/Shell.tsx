import { useRef, type ReactNode } from "react";
import { useLicense } from "../state/license.tsx";
import { SOON, visibleReady } from "../nav.ts";
import { useSession } from "../state/session.tsx";
import { useSidebar } from "../state/sidebar.ts";
import { useTheme } from "../state/theme.tsx";

function initials(name: string): string {
  const words = name.split(/\s+/).filter(Boolean);
  return (words.length > 1 ? words[0]![0]! + words[1]![0]! : name.slice(0, 2)).toUpperCase();
}

function LicensePill() {
  const { license } = useLicense();
  if (!license) return null;
  const view = {
    full: { text: "Subscription active", cls: "" },
    grace: { text: "Subscription grace period", cls: "warn" },
    read_only: { text: "Read-only: subscription not active", cls: "off" },
    unlicensed: { text: "No subscription", cls: "off" },
  }[license.mode];
  return (
    <span className={"pill hide-sm " + view.cls} title={license.full_until ? `Active until ${license.full_until.slice(0, 10)}` : undefined}>
      <i />
      {view.text}
    </span>
  );
}

/** Sidebar + header. The button at the left of the header opens and closes the sidebar. */
export function Shell({ page, title, children }: { page: string; title: string; children: ReactNode }) {
  const { me, tenantName, canAny, signOut } = useSession();
  const { theme, toggle: toggleTheme } = useTheme();
  const [collapsed, toggleSidebar, setCollapsed] = useSidebar();
  const main = useRef<HTMLElement>(null);
  const ready = visibleReady(canAny);

  return (
    <div className={"shell" + (collapsed ? " collapsed" : "")}>
      <button type="button" className="skip" onClick={() => main.current?.focus()}>
        Skip to content
      </button>
      <aside id="sidebar" aria-label="Main navigation">
        <div className="brand">
          <span className="bl">{tenantName}</span>
          <span className="bs" aria-hidden="true">
            {initials(tenantName)}
          </span>
        </div>
        <nav>
          {ready.map((n) => (
            <a key={n.id} href={`#/${n.id}`} className={n.id === page ? "on" : ""} aria-current={n.id === page ? "page" : undefined} aria-label={n.label} title={n.label}>
              <span className="ni" aria-hidden="true">{n.badge}</span>
              <span className="nl">{n.label}</span>
            </a>
          ))}
          <hr />
          <div className="group">Coming soon</div>
          {SOON.map((n) => (
            <a key={n.id} href={`#/${n.id}`} className={"soon" + (n.id === page ? " on" : "")} aria-current={n.id === page ? "page" : undefined} aria-label={`${n.label} (coming soon)`} title={`${n.label}: coming soon`}>
              <span className="ni" aria-hidden="true">{n.badge}</span>
              <span className="nl">{n.label}</span>
            </a>
          ))}
        </nav>
      </aside>
      <div className="scrim" onClick={() => setCollapsed(true)} aria-hidden="true" />
      <div className="main">
        <header className="top">
          <button
            type="button"
            className="icon-btn"
            aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"}
            aria-expanded={!collapsed}
            aria-controls="sidebar"
            title={collapsed ? "Expand sidebar" : "Collapse sidebar"}
            onClick={toggleSidebar}
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path d="M4 6h16M4 12h16M4 18h16" />
            </svg>
          </button>
          <h1>{title}</h1>
          <LicensePill />
          <button type="button" className="btn hide-sm" onClick={toggleTheme}>
            {theme === "dark" ? "Light mode" : "Dark mode"}
          </button>
          <span className="muted hide-sm">{me?.user.full_name}</span>
          <button type="button" className="btn" onClick={() => void signOut()}>
            Sign out
          </button>
        </header>
        <main id="view" tabIndex={-1} ref={main}>
          {children}
        </main>
      </div>
    </div>
  );
}
