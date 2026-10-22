import { useState } from "react";
import { hasPermission } from "@dsaas/domain";
import { describeError } from "../api/errors.ts";
import type { Location, Role, UserRow } from "../api/types.ts";
import { covers } from "../permissions.ts";
import { useLicense } from "../state/license.tsx";
import { useSession } from "../state/session.tsx";
import { useToast } from "../state/toast.tsx";
import { useAsync } from "../state/useAsync.ts";
import { Empty, LockNote, StatusBadge } from "../ui/bits.tsx";
import { ConfirmModal, FormModal } from "../ui/Modal.tsx";

type Dialog = { kind: "add" } | { kind: "roles"; user: UserRow } | { kind: "locations"; user: UserRow } | { kind: "deactivate"; user: UserRow };

export function Users() {
  const { api, me, can } = useSession();
  const { canWrite } = useLicense();
  const toast = useToast();
  const [dialog, setDialog] = useState<Dialog | null>(null);

  const { data, error, loading, reload } = useAsync(async () => {
    const [users, roles, locations] = await Promise.all([
      api.get<UserRow[]>("/v1/users"),
      can("roles.manage") ? api.get<Role[]>("/v1/roles") : Promise.resolve([] as Role[]),
      api.get<Location[]>("/v1/locations"),
    ]);
    return { users, roles, locations };
  }, [api]);

  const viaPin = me?.method === "pin";
  const editable = canWrite && !viaPin;
  const roles = data?.roles ?? [];
  const locations = data?.locations ?? [];
  const roleName = (id: string) => roles.find((r) => r.id === id)?.name ?? "Role";
  const hasAllLocations = (u: UserRow) =>
    u.role_ids.some((id) => hasPermission(roles.find((r) => r.id === id)?.permissions ?? [], "locations.all"));

  return (
    <>
      {viaPin && <p className="note lock-note" role="status">You signed in with a PIN. Sign in with your password to change users.</p>}
      {!canWrite && <LockNote />}
      <div className="bar-actions">
        <span className="grow muted">People who can sign in on this shop's PCs.</span>
        <button type="button" className="btn primary" disabled={!editable} onClick={() => setDialog({ kind: "add" })}>
          Add user
        </button>
      </div>
      <div className="panel">
        {error ? (
          <p className="err inline" role="alert">{describeError(error)}</p>
        ) : loading && !data ? (
          <Empty>Loading users…</Empty>
        ) : (
          <div className="tscroll">
            <table>
              <thead>
                <tr><th>Name</th><th>Roles</th><th>Locations</th><th>Status</th><th /></tr>
              </thead>
              <tbody>
                {data?.users.map((u) => (
                  <tr key={u.id}>
                    <td><b>{u.full_name}</b><div className="muted sm">{u.username}</div></td>
                    <td className="chips">{u.role_ids.length ? u.role_ids.map((id) => <span key={id}>{roleName(id)}</span>) : <span className="muted">No role yet</span>}</td>
                    <td className="chips">
                      {hasAllLocations(u) ? <span>All locations</span> : u.location_ids.length ? u.location_ids.map((id) => <span key={id}>{locations.find((l) => l.id === id)?.name ?? "Location"}</span>) : <span className="muted">None</span>}
                    </td>
                    <td><StatusBadge active={u.is_active} /></td>
                    <td className="acts">
                      {can("roles.manage") && <button type="button" className="btn sm" disabled={!editable} onClick={() => setDialog({ kind: "roles", user: u })}>Roles</button>}
                      <button type="button" className="btn sm" disabled={!editable} onClick={() => setDialog({ kind: "locations", user: u })}>Locations</button>
                      {u.is_active && <button type="button" className="btn sm danger" disabled={!editable} onClick={() => setDialog({ kind: "deactivate", user: u })}>Deactivate</button>}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>

      {dialog?.kind === "add" && (
        <AddUser
          onClose={() => setDialog(null)}
          onAdded={(name) => {
            toast(`Added ${name}. Give them a role so they can do things.`);
            reload();
          }}
        />
      )}
      {dialog?.kind === "roles" && (
        <RolesDialog user={dialog.user} roles={roles} onClose={() => setDialog(null)} onSaved={() => { toast(`Roles saved for ${dialog.user.full_name}`); reload(); }} />
      )}
      {dialog?.kind === "locations" && (
        <LocationsDialog user={dialog.user} locations={locations} onClose={() => setDialog(null)} onSaved={() => { toast(`Locations saved for ${dialog.user.full_name}`); reload(); }} />
      )}
      {dialog?.kind === "deactivate" && (
        <ConfirmModal
          title={`Deactivate ${dialog.user.full_name}?`}
          confirmLabel="Deactivate user"
          onClose={() => setDialog(null)}
          onConfirm={async () => {
            await api.post(`/v1/users/${dialog.user.id}/deactivate`);
            toast(`Deactivated ${dialog.user.full_name}`);
            reload();
          }}
        >
          <p>They will be signed out straight away and can no longer sign in. Their history stays.</p>
        </ConfirmModal>
      )}
    </>
  );
}

function AddUser({ onClose, onAdded }: { onClose: () => void; onAdded: (name: string) => void }) {
  const { api } = useSession();
  const [fullName, setFullName] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  return (
    <FormModal
      title="Add user"
      submitLabel="Add user"
      onClose={onClose}
      canSubmit={fullName.trim().length > 1 && username.trim().length >= 3 && password.length >= 10}
      onSubmit={async () => {
        await api.post("/v1/users", { username, full_name: fullName, password });
        onAdded(fullName);
      }}
    >
      <label>Full name<input value={fullName} onChange={(e) => setFullName(e.target.value)} required autoFocus /></label>
      <label>
        Username
        <input value={username} onChange={(e) => setUsername(e.target.value)} autoCapitalize="none" required />
        <span className="muted sm">3 to 32 characters: letters, digits, dot, dash, underscore.</span>
      </label>
      <label>
        First password
        <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} autoComplete="new-password" required />
        <span className="muted sm">At least 10 characters. They can set a PIN after signing in.</span>
      </label>
    </FormModal>
  );
}

function RolesDialog({ user, roles, onClose, onSaved }: { user: UserRow; roles: Role[]; onClose: () => void; onSaved: () => void }) {
  const { api, me } = useSession();
  const [picked, setPicked] = useState(new Set(user.role_ids));
  const mine = me?.permissions ?? [];
  const toggle = (id: string) => setPicked((s) => { const n = new Set(s); if (n.has(id)) n.delete(id); else n.add(id); return n; });
  return (
    <FormModal
      title={`Roles for ${user.full_name}`}
      submitLabel="Save roles"
      onClose={onClose}
      onSubmit={async () => {
        await api.put(`/v1/users/${user.id}/roles`, { role_ids: [...picked] });
        onSaved();
      }}
    >
      {roles.length === 0 && <Empty>No roles yet. Create one under Settings, Roles.</Empty>}
      <fieldset>
        <legend>Roles</legend>
        {roles.map((r) => {
          const allowed = r.permissions.every((p) => covers(mine, p));
          return (
            <label key={r.id} className="inl" title={allowed ? undefined : "You can only give roles whose permissions you have yourself"}>
              <input type="checkbox" checked={picked.has(r.id)} disabled={!allowed && !user.role_ids.includes(r.id)} onChange={() => toggle(r.id)} />
              {r.name}
              {r.protected && <span className="muted sm"> (full access)</span>}
            </label>
          );
        })}
      </fieldset>
    </FormModal>
  );
}

function LocationsDialog({ user, locations, onClose, onSaved }: { user: UserRow; locations: Location[]; onClose: () => void; onSaved: () => void }) {
  const { api } = useSession();
  const visible = new Set(locations.map((l) => l.id));
  const [picked, setPicked] = useState(new Set(user.location_ids.filter((id) => visible.has(id))));
  const toggle = (id: string) => setPicked((s) => { const n = new Set(s); if (n.has(id)) n.delete(id); else n.add(id); return n; });
  return (
    <FormModal
      title={`Locations for ${user.full_name}`}
      submitLabel="Save locations"
      onClose={onClose}
      onSubmit={async () => {
        // Locations this signed-in user cannot see stay as they are; only the ones shown here are edited.
        const hidden = user.location_ids.filter((id) => !visible.has(id));
        await api.put(`/v1/users/${user.id}/locations`, { location_ids: [...hidden, ...picked] });
        onSaved();
      }}
    >
      <p className="muted">Users with the "locations.all" permission can use every location regardless of this list.</p>
      <fieldset>
        <legend>Locations this person can use</legend>
        {locations.length === 0 && <Empty>No locations yet.</Empty>}
        {locations.map((l) => (
          <label key={l.id} className="inl">
            <input type="checkbox" checked={picked.has(l.id)} onChange={() => toggle(l.id)} />
            {l.name} <span className="muted sm">({l.code})</span>
          </label>
        ))}
      </fieldset>
    </FormModal>
  );
}
