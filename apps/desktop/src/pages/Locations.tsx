import { useState } from "react";
import { describeError, isStale } from "../api/errors.ts";
import type { Location } from "../api/types.ts";
import { useLicense } from "../state/license.tsx";
import { useSession } from "../state/session.tsx";
import { useToast } from "../state/toast.tsx";
import { useAsync } from "../state/useAsync.ts";
import { Empty, LockNote, StatusBadge } from "../ui/bits.tsx";
import { FormModal } from "../ui/Modal.tsx";

export function Locations() {
  const { api, can } = useSession();
  const { canWrite } = useLicense();
  const toast = useToast();
  const [editing, setEditing] = useState<Location | "new" | null>(null);
  const { data, error, loading, reload } = useAsync(() => api.get<Location[]>("/v1/locations"), [api]);
  const manage = can("locations.manage");
  const editable = manage && canWrite;
  const nameOf = (id: string | null) => data?.find((l) => l.id === id)?.name;

  async function toggle(l: Location) {
    try {
      await api.post(`/v1/locations/${l.id}/${l.is_active ? "deactivate" : "activate"}`, { expected_revision: l.revision });
      toast(`${l.is_active ? "Deactivated" : "Activated"} ${l.name}`);
    } catch (e) {
      toast(describeError(e), true);
    }
    reload();
  }

  return (
    <>
      {manage && !canWrite && <LockNote />}
      <div className="bar-actions">
        <span className="muted grow">You see the locations you have access to.</span>
        {manage && (
          <button type="button" className="btn primary" disabled={!editable} onClick={() => setEditing("new")}>
            Add location
          </button>
        )}
      </div>
      <div className="panel">
        {error ? (
          <p className="err inline" role="alert">{describeError(error)}</p>
        ) : loading && !data ? (
          <Empty>Loading locations…</Empty>
        ) : data && data.length === 0 ? (
          <Empty>No locations to show.</Empty>
        ) : (
          <div className="tscroll">
            <table>
              <thead>
                <tr><th>Code</th><th>Name</th><th>Type</th><th>Inside</th><th>Status</th>{manage && <th />}</tr>
              </thead>
              <tbody>
                {data?.map((l) => (
                  <tr key={l.id}>
                    <td>{l.code}</td>
                    <td><b>{l.name}</b>{l.description && <div className="muted sm">{l.description}</div>}</td>
                    <td>{l.location_type}</td>
                    <td>{nameOf(l.parent_location_id) ?? <span className="muted">-</span>}</td>
                    <td><StatusBadge active={l.is_active} /></td>
                    {manage && (
                      <td className="acts">
                        <button type="button" className="btn sm" disabled={!editable} onClick={() => setEditing(l)}>Edit</button>
                        <button type="button" className={"btn sm" + (l.is_active ? " danger" : "")} disabled={!editable} onClick={() => void toggle(l)}>
                          {l.is_active ? "Deactivate" : "Activate"}
                        </button>
                      </td>
                    )}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
      {editing && (
        <LocationForm
          initial={editing === "new" ? null : editing}
          all={data ?? []}
          onClose={() => setEditing(null)}
          onSaved={(m) => {
            toast(m);
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

function LocationForm(props: { initial: Location | null; all: Location[]; onClose: () => void; onSaved: (m: string) => void; onStale: (e: unknown) => void }) {
  const { api } = useSession();
  const { initial } = props;
  const [name, setName] = useState(initial?.name ?? "");
  const [code, setCode] = useState(initial?.code ?? "");
  const [type, setType] = useState(initial?.location_type ?? "storage");
  const [parent, setParent] = useState(initial?.parent_location_id ?? "");
  const [description, setDescription] = useState(initial?.description ?? "");

  async function save() {
    // PUT replaces the whole record: the responsible user is not editable here, so it is sent back unchanged.
    const body = {
      name, code, location_type: type, description: description || null,
      parent_location_id: parent || null, responsible_user_id: initial?.responsible_user_id ?? null,
    };
    try {
      if (initial) {
        await api.put(`/v1/locations/${initial.id}`, { ...body, expected_revision: initial.revision });
        props.onSaved(`Saved ${name}`);
      } else {
        const l = await api.post<Location>("/v1/locations", body);
        props.onSaved(`Added ${l.name} (${l.code})`);
      }
    } catch (e) {
      if (isStale(e)) {
        props.onStale(e);
        return;
      }
      throw e;
    }
  }

  const parents = props.all.filter((l) => l.is_active && l.id !== initial?.id);
  return (
    <FormModal title={initial ? "Edit location" : "Add location"} submitLabel={initial ? "Save changes" : "Add location"} onClose={props.onClose} onSubmit={save} canSubmit={name.trim().length >= 2 && code.trim().length >= 2}>
      <div className="fgrid">
        <label>Name<input value={name} onChange={(e) => setName(e.target.value)} required autoFocus /></label>
        <label>Code<input value={code} onChange={(e) => setCode(e.target.value)} required placeholder="For example MAIN or G1" /></label>
        <label>
          Type
          <input value={type} onChange={(e) => setType(e.target.value)} list="location-types" />
          <datalist id="location-types">
            {["storage", "shop", "vehicle"].map((t) => <option key={t} value={t} />)}
          </datalist>
        </label>
        <label>
          Inside
          <select value={parent} onChange={(e) => setParent(e.target.value)}>
            <option value="">Nothing (top level)</option>
            {parents.map((l) => (
              <option key={l.id} value={l.id}>{l.name}</option>
            ))}
          </select>
        </label>
      </div>
      <label>Description<textarea rows={2} value={description} onChange={(e) => setDescription(e.target.value)} /></label>
    </FormModal>
  );
}
