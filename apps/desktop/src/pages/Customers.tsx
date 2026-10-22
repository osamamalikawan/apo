import { useEffect, useState } from "react";
import { describeError, isStale } from "../api/errors.ts";
import type { Customer, Page } from "../api/types.ts";
import { useLicense } from "../state/license.tsx";
import { useSession } from "../state/session.tsx";
import { useToast } from "../state/toast.tsx";
import { useAsync } from "../state/useAsync.ts";
import { Empty, LockNote, Pager, StatusBadge } from "../ui/bits.tsx";
import { FormModal } from "../ui/Modal.tsx";

const LIMIT = 25;

export function Customers() {
  const { api, can } = useSession();
  const { canWrite } = useLicense();
  const toast = useToast();
  const [search, setSearch] = useState("");
  const [q, setQ] = useState("");
  const [inactive, setInactive] = useState(false);
  const [offset, setOffset] = useState(0);
  const [editing, setEditing] = useState<Customer | "new" | null>(null);

  // Wait for a pause in typing before searching. Nothing to do when the text is already what was searched:
  // otherwise this timer would send someone who just clicked "Next" back to page 1.
  useEffect(() => {
    const next = search.trim();
    if (next === q) return;
    const t = setTimeout(() => {
      setQ(next);
      setOffset(0);
    }, 250);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [search]);

  const { data, error, loading, reload } = useAsync(() => {
    const p = new URLSearchParams({ limit: String(LIMIT), offset: String(offset) });
    if (q) p.set("search", q);
    if (inactive) p.set("include_inactive", "true");
    return api.get<Page<Customer>>(`/v1/customers?${p}`);
  }, [api, q, inactive, offset]);

  const manage = can("customers.manage");
  const editable = manage && canWrite;

  async function toggle(c: Customer) {
    try {
      await api.post(`/v1/customers/${c.id}/${c.is_active ? "deactivate" : "activate"}`, { expected_revision: c.revision });
      toast(`${c.is_active ? "Deactivated" : "Activated"} ${c.name}`);
    } catch (e) {
      toast(describeError(e), true);
    }
    reload();
  }

  return (
    <>
      {manage && !canWrite && <LockNote />}
      <div className="bar-actions">
        <input type="search" aria-label="Search customers" placeholder="Search by name, code or phone" value={search} onChange={(e) => setSearch(e.target.value)} style={{ minWidth: 260 }} />
        <label className="inl">
          <input type="checkbox" checked={inactive} onChange={(e) => { setInactive(e.target.checked); setOffset(0); }} /> Show inactive
        </label>
        <span className="grow" />
        {manage && (
          <button type="button" className="btn primary" disabled={!editable} onClick={() => setEditing("new")}>
            Add customer
          </button>
        )}
      </div>
      <div className="panel">
        {error ? (
          <p className="err inline" role="alert">{describeError(error)}</p>
        ) : loading && !data ? (
          <Empty>Loading customers…</Empty>
        ) : data && data.items.length === 0 ? (
          <Empty>{q ? `No customers match “${q}”.` : "No customers yet."}</Empty>
        ) : (
          <div className="tscroll">
            <table>
              <thead>
                <tr><th>Code</th><th>Name</th><th>Phone</th><th>Address</th><th>Status</th>{manage && <th />}</tr>
              </thead>
              <tbody>
                {data?.items.map((c) => (
                  <tr key={c.id}>
                    <td>{c.code}</td>
                    <td><b>{c.name}</b>{c.notes && <div className="muted sm">{c.notes}</div>}</td>
                    <td>{c.phone ?? <span className="muted">-</span>}</td>
                    <td>{c.address ?? <span className="muted">-</span>}</td>
                    <td><StatusBadge active={c.is_active} /></td>
                    {manage && (
                      <td className="acts">
                        <button type="button" className="btn sm" disabled={!editable} onClick={() => setEditing(c)}>Edit</button>
                        <button type="button" className={"btn sm" + (c.is_active ? " danger" : "")} disabled={!editable} onClick={() => void toggle(c)}>
                          {c.is_active ? "Deactivate" : "Activate"}
                        </button>
                      </td>
                    )}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        {data && <Pager offset={offset} limit={LIMIT} total={data.total} onPage={setOffset} />}
      </div>
      {editing && (
        <CustomerForm
          initial={editing === "new" ? null : editing}
          onClose={() => setEditing(null)}
          onSaved={(msg) => {
            toast(msg);
            reload();
          }}
          onStale={(e) => {
            toast(describeError(e), true);
            setEditing(null);
            reload();
          }}
        />
      )}
    </>
  );
}

function CustomerForm({ initial, onClose, onSaved, onStale }: { initial: Customer | null; onClose: () => void; onSaved: (msg: string) => void; onStale: (e: unknown) => void }) {
  const { api } = useSession();
  const [code, setCode] = useState(initial?.code ?? "");
  const [name, setName] = useState(initial?.name ?? "");
  const [phone, setPhone] = useState(initial?.phone ?? "");
  const [address, setAddress] = useState(initial?.address ?? "");
  const [notes, setNotes] = useState(initial?.notes ?? "");

  async function save() {
    // PUT replaces the whole record, so every field is sent.
    const body = { code, name, phone: phone || null, address: address || null, notes: notes || null };
    try {
      if (initial) {
        await api.put(`/v1/customers/${initial.id}`, { ...body, expected_revision: initial.revision });
        onSaved(`Saved ${name}`);
      } else {
        const created = await api.post<Customer>("/v1/customers", body);
        onSaved(`Added ${created.name} (${created.code})`);
      }
    } catch (e) {
      if (isStale(e)) {
        onStale(e);
        return;
      }
      throw e;
    }
  }

  return (
    <FormModal title={initial ? "Edit customer" : "Add customer"} submitLabel={initial ? "Save changes" : "Add customer"} onClose={onClose} onSubmit={save} canSubmit={name.trim().length >= 2}>
      <div className="fgrid">
        <label>Name<input value={name} onChange={(e) => setName(e.target.value)} required autoFocus /></label>
        <label>
          Code
          <input value={code} onChange={(e) => setCode(e.target.value)} placeholder={initial ? undefined : "Leave blank to assign the next code"} />
        </label>
        <label>Phone<input value={phone} onChange={(e) => setPhone(e.target.value)} inputMode="tel" /></label>
        <label>Address<input value={address} onChange={(e) => setAddress(e.target.value)} /></label>
      </div>
      <label>Notes<textarea rows={3} value={notes} onChange={(e) => setNotes(e.target.value)} /></label>
    </FormModal>
  );
}
