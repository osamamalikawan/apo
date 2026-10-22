import { LicenseProvider } from "./state/license.tsx";
import { useHashRoute } from "./state/router.ts";
import { SessionProvider, useSession } from "./state/session.tsx";
import { ThemeProvider } from "./state/theme.tsx";
import { ToastProvider } from "./state/toast.tsx";
import { findItem, READY, visibleReady } from "./nav.ts";
import { Shell } from "./ui/Shell.tsx";
import { ComingSoon } from "./pages/ComingSoon.tsx";
import { Customers } from "./pages/Customers.tsx";
import { Dashboard } from "./pages/Dashboard.tsx";
import { Inventory } from "./pages/Inventory.tsx";
import { Login } from "./pages/Login.tsx";
import { Settings } from "./pages/Settings.tsx";

export function App({ fetchFn }: { fetchFn?: typeof fetch }) {
  return (
    <ThemeProvider>
      <SessionProvider fetchFn={fetchFn}>
        <ToastProvider>
          <Gate />
        </ToastProvider>
      </SessionProvider>
    </ThemeProvider>
  );
}

function Gate() {
  const { status } = useSession();
  if (status === "loading") {
    return (
      <div className="signin">
        <p className="muted" role="status">Loading…</p>
      </div>
    );
  }
  if (status === "signed-out") return <Login />;
  return (
    <LicenseProvider>
      <Signed />
    </LicenseProvider>
  );
}

function Signed() {
  const { canAny } = useSession();
  const { page } = useHashRoute();
  const item = findItem(page);
  const allowed = visibleReady(canAny).some((n) => n.id === page);

  let title = "Page not found";
  let body = (
    <section className="panel">
      <p>There is no page called “{page}”. <a href="#/dashboard">Go to the dashboard</a>.</p>
    </section>
  );
  if (item?.soon) {
    title = item.label;
    body = <ComingSoon id={item.id} />;
  } else if (item && READY.includes(item) && !allowed) {
    title = item.label;
    body = (
      <section className="panel">
        <p>You do not have access to this page. Ask the owner to give you a role that includes it.</p>
      </section>
    );
  } else if (item) {
    title = item.label;
    body = { dashboard: <Dashboard />, customers: <Customers />, inventory: <Inventory />, settings: <Settings /> }[item.id] ?? body;
  }
  return (
    <Shell page={page} title={title}>
      {body}
    </Shell>
  );
}
