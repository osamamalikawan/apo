import type { ReactNode } from "react";

export function StatusBadge({ active }: { active: boolean }) {
  return <span className={"badge " + (active ? "green" : "none")}>{active ? "Active" : "Inactive"}</span>;
}

export function Empty({ children }: { children: ReactNode }) {
  return <div className="empty">{children}</div>;
}

export function LockNote() {
  return (
    <p className="note lock-note" role="status">
      The subscription is not active, so changes are turned off. You can still view everything.
    </p>
  );
}

export function Pager({ offset, limit, total, onPage }: { offset: number; limit: number; total: number; onPage: (offset: number) => void }) {
  if (total <= limit) return null;
  const from = offset + 1;
  const to = Math.min(offset + limit, total);
  return (
    <div className="bar-actions" style={{ marginTop: 10 }}>
      <span className="muted sm grow">
        Showing {from} to {to} of {total}
      </span>
      <button type="button" className="btn sm" disabled={offset === 0} onClick={() => onPage(Math.max(0, offset - limit))}>
        Previous
      </button>
      <button type="button" className="btn sm" disabled={to >= total} onClick={() => onPage(offset + limit)}>
        Next
      </button>
    </div>
  );
}

export function Tabs<T extends string>({ tabs, current, onPick, label }: { tabs: { id: T; label: string }[]; current: T; onPick: (id: T) => void; label: string }) {
  return (
    <div className="tabs" role="tablist" aria-label={label}>
      {tabs.map((t) => (
        <button key={t.id} type="button" role="tab" aria-selected={t.id === current} onClick={() => onPick(t.id)}>
          {t.label}
        </button>
      ))}
    </div>
  );
}
