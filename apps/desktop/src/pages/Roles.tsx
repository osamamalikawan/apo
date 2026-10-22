import { useState } from "react";
import { describeError } from "../api/errors.ts";
import type { PermissionInfo, Role } from "../api/types.ts";
import { covers, describePermission, moduleOf } from "../permissions.ts";
import { useLicense } from "../state/license.tsx";
import { useSession } from "../state/session.tsx";
import { useToast } from "../state/toast.tsx";
import { useAsync } from "../state/useAsync.ts";
import { Empty, LockNote } from "../ui/bits.tsx";
import { ConfirmModal, FormModal } from "../ui/Modal.tsx";

export function Roles() {
  const { api, me } = useSession();
  const { canWrite } = useLicense();
  const toast = useToast();
  const [editing, setEditing] = useState<Role | "new" | null>(null);
  const [deleting, setDeleting] = useState<Role | null>(null);
  const { data, error, loading, reload } = useAsync(async () => {
    const [roles, permissions] = await Promise.all([api.get<Role[]>("/v1/roles"), api.get<PermissionInfo[]>("/v1/permissions")]);
    return { roles, permissions };
  }, [api]);
  const viaPin = me?.method === "pin";
  const editable = canWrite && !viaPin;
  const mine = me?.permissions ?? [];

  return (
    <>
      {viaPin && <p className="note lock-note" role="status">You signed in with a PIN. Sign in with your password to change roles.</p>}
      {!canWrite && <LockNote />}
      <div className="bar-actions">
        <span className="grow muted">A role is a set of permissions. A person can have several roles.</span>
        <button type="button" className="btn primary" disabled={!editable} onClick={() => setEditing("new")}>Add role</button>
      </div>
      <div className="panel">
        {error ? (
          <p className="err inline" role="alert">{describeError(error)}</p>
        ) : loading && !data ? (
          <Empty>Loading roles…</Empty>
        ) : (
          <div className="tscroll">
            <table>
              <thead><tr><th>Role</th><th>Permissions</th><th className="r">People</th><th /></tr></thead>
              <tbody>
                {data?.roles.map((r) => (
                  <tr key={r.id}>
                    <td><b>{r.name}</b></td>
                    <td className="chips">{r.permissions.length ? r.permissions.map((p) => <span key={p}>{describePermission(p)}</span>) : <span className="muted">Nothing yet</span>}</td>
                    <td className="r">{r.member_count}</td>
                    <td className="acts">
                      {r.protected ? (
                        <span className="muted sm">Cannot be changed</span>
                      ) : (
                        <>
                          <button type="button" className="btn sm" disabled={!editable || !r.permissions.every((p) => covers(mine, p))} title={r.permissions.every((p) => covers(mine, p)) ? undefined : "This role has permissions you do not have"} onClick={() => setEditing(r)}>Edit</button>
                          <button type="button" className="btn sm danger" disabled={!editable || !r.permissions.every((p) => covers(mine, p))} onClick={() => setDeleting(r)}>Delete</button>
                        </>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
      {editing && data && (
        <RoleForm
          initial={editing === "new" ? null : editing}
          permissions={data.permissions}
          onClose={() => setEditing(null)}
          onSaved={(m) => { toast(m); reload(); }}
        />
      )}
      {deleting && (
        <ConfirmModal
          title={`Delete the role “${deleting.name}”?`}
          confirmLabel="Delete role"
          onClose={() => setDeleting(null)}
          onConfirm={async () => {
            await api.del(`/v1/roles/${deleting.id}`);
            toast(`Deleted ${deleting.name}`);
            reload();
          }}
        >
          <p>
            {deleting.member_count > 0
              ? `${deleting.member_count} ${deleting.member_count === 1 ? "person has" : "people have"} this role. Take it off them first; the server will not delete a role that is in use.`
              : "No one has this role."}
          </p>
        </ConfirmModal>
      )}
    </>
  );
}

function RoleForm({ initial, permissions, onClose, onSaved }: { initial: Role | null; permissions: PermissionInfo[]; onClose: () => void; onSaved: (m: string) => void }) {
  const { api, me } = useSession();
  const mine = me?.permissions ?? [];
  const [name, setName] = useState(initial?.name ?? "");
  const [picked, setPicked] = useState(new Set(initial?.permissions ?? []));
  const toggle = (p: string) => setPicked((s) => { const n = new Set(s); if (n.has(p)) n.delete(p); else n.add(p); return n; });

  const modules = [...new Set(permissions.map((p) => moduleOf(p.permission)))].sort();
  const everything = picked.has("*");
  return (
    <FormModal
      title={initial ? `Edit role: ${initial.name}` : "Add role"}
      submitLabel={initial ? "Save role" : "Add role"}
      wide
      onClose={onClose}
      canSubmit={name.trim().length >= 2}
      onSubmit={async () => {
        const body = { name, permissions: [...picked] };
        if (initial) await api.put(`/v1/roles/${initial.id}`, body);
        else await api.post("/v1/roles", body);
        onSaved(`Saved ${name}`);
      }}
    >
      <label>Role name<input value={name} onChange={(e) => setName(e.target.value)} required autoFocus /></label>
      <p className="muted">You can only give permissions that you have yourself.</p>
      <label className="inl">
        <input type="checkbox" checked={everything} disabled={!covers(mine, "*")} onChange={() => toggle("*")} />
        Everything, including permissions added later
      </label>
      {modules.map((m) => {
        const all = `${m}.*`;
        const allOn = everything || picked.has(all);
        return (
          <fieldset key={m}>
            <legend>{m}</legend>
            <label className="inl">
              <input type="checkbox" checked={allOn} disabled={everything || !covers(mine, all)} onChange={() => toggle(all)} />
              All {m} permissions
            </label>
            <div className="permgrid">
              {permissions.filter((p) => moduleOf(p.permission) === m).map((p) => (
                <label key={p.permission} className="inl" title={p.description}>
                  <input type="checkbox" checked={allOn || picked.has(p.permission)} disabled={allOn || !covers(mine, p.permission)} onChange={() => toggle(p.permission)} />
                  {p.description}
                </label>
              ))}
            </div>
          </fieldset>
        );
      })}
    </FormModal>
  );
}
