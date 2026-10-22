import { useEffect, useState } from "react";
import { describeError, isStale } from "../api/errors.ts";
import type { Category, Page, Product } from "../api/types.ts";
import { useLicense } from "../state/license.tsx";
import { useSession } from "../state/session.tsx";
import { useToast } from "../state/toast.tsx";
import { useAsync } from "../state/useAsync.ts";
import { Empty, LockNote, Pager, StatusBadge } from "../ui/bits.tsx";
import { FormModal } from "../ui/Modal.tsx";

const LIMIT = 25;

export function Products() {
  const { api, can } = useSession();
  const { canWrite } = useLicense();
  const toast = useToast();
  const [search, setSearch] = useState("");
  const [q, setQ] = useState("");
  const [category, setCategory] = useState("");
  const [inactive, setInactive] = useState(false);
  const [offset, setOffset] = useState(0);
  const [editing, setEditing] = useState<Product | "new" | null>(null);
  const [managingCategories, setManagingCategories] = useState(false);

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

  const cats = useAsync(() => api.get<Category[]>("/v1/categories"), [api]);
  const { data, error, loading, reload } = useAsync(() => {
    const p = new URLSearchParams({ limit: String(LIMIT), offset: String(offset) });
    if (q) p.set("search", q);
    if (category) p.set("category_id", category);
    if (inactive) p.set("include_inactive", "true");
    return api.get<Page<Product>>(`/v1/products?${p}`);
  }, [api, q, category, inactive, offset]);

  const manage = can("products.manage");
  const editable = manage && canWrite;
  const catName = (id: string | null) => cats.data?.find((c) => c.id === id)?.name ?? "";

  async function toggle(p: Product) {
    try {
      await api.post(`/v1/products/${p.id}/${p.is_active ? "deactivate" : "activate"}`, { expected_revision: p.revision });
      toast(`${p.is_active ? "Deactivated" : "Activated"} ${p.name}`);
    } catch (e) {
      toast(describeError(e), true);
    }
    reload();
  }

  return (
    <>
      {manage && !canWrite && <LockNote />}
      <div className="bar-actions">
        <input type="search" aria-label="Search products" placeholder="Search by name, SKU or scan a barcode" value={search} onChange={(e) => setSearch(e.target.value)} style={{ minWidth: 280 }} />
        <select aria-label="Category" value={category} onChange={(e) => { setCategory(e.target.value); setOffset(0); }}>
          <option value="">All categories</option>
          {cats.data?.map((c) => (
            <option key={c.id} value={c.id}>{c.name}</option>
          ))}
        </select>
        <label className="inl">
          <input type="checkbox" checked={inactive} onChange={(e) => { setInactive(e.target.checked); setOffset(0); }} /> Show inactive
        </label>
        <span className="grow" />
        {manage && (
          <>
            <button type="button" className="btn" disabled={!editable} onClick={() => setManagingCategories(true)}>Categories</button>
            <button type="button" className="btn primary" disabled={!editable} onClick={() => setEditing("new")}>Add product</button>
          </>
        )}
      </div>
      <div className="panel">
        {error ? (
          <p className="err inline" role="alert">{describeError(error)}</p>
        ) : loading && !data ? (
          <Empty>Loading products…</Empty>
        ) : data && data.items.length === 0 ? (
          <Empty>{q || category ? "No products match this search." : "No products yet."}</Empty>
        ) : (
          <div className="tscroll">
            <table>
              <thead>
                <tr><th>SKU</th><th>Name</th><th>Barcode</th><th>Category</th><th>Unit</th><th>Status</th>{manage && <th />}</tr>
              </thead>
              <tbody>
                {data?.items.map((p) => (
                  <tr key={p.id}>
                    <td>{p.sku}</td>
                    <td><b>{p.name}</b></td>
                    <td>{p.barcode ?? <span className="muted">-</span>}</td>
                    <td>{catName(p.category_id) || <span className="muted">-</span>}</td>
                    <td>{p.unit}</td>
                    <td><StatusBadge active={p.is_active} /></td>
                    {manage && (
                      <td className="acts">
                        <button type="button" className="btn sm" disabled={!editable} onClick={() => setEditing(p)}>Edit</button>
                        <button type="button" className={"btn sm" + (p.is_active ? " danger" : "")} disabled={!editable} onClick={() => void toggle(p)}>
                          {p.is_active ? "Deactivate" : "Activate"}
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
        <ProductForm
          initial={editing === "new" ? null : editing}
          categories={cats.data ?? []}
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
      {managingCategories && <CategoriesModal categories={cats.data ?? []} onClose={() => { setManagingCategories(false); cats.reload(); }} />}
    </>
  );
}

function ProductForm(props: { initial: Product | null; categories: Category[]; onClose: () => void; onSaved: (m: string) => void; onStale: (e: unknown) => void }) {
  const { api } = useSession();
  const { initial } = props;
  const [sku, setSku] = useState(initial?.sku ?? "");
  const [name, setName] = useState(initial?.name ?? "");
  const [barcode, setBarcode] = useState(initial?.barcode ?? "");
  const [categoryId, setCategoryId] = useState(initial?.category_id ?? "");
  const [unit, setUnit] = useState(initial?.unit ?? "pcs");

  async function save() {
    const body = { sku, name, barcode: barcode || null, category_id: categoryId || null, unit };
    try {
      if (initial) {
        await api.put(`/v1/products/${initial.id}`, { ...body, expected_revision: initial.revision });
        props.onSaved(`Saved ${name}`);
      } else {
        const p = await api.post<Product>("/v1/products", body);
        props.onSaved(`Added ${p.name} (${p.sku})`);
      }
    } catch (e) {
      if (isStale(e)) {
        props.onStale(e);
        return;
      }
      throw e;
    }
  }

  return (
    <FormModal title={initial ? "Edit product" : "Add product"} submitLabel={initial ? "Save changes" : "Add product"} onClose={props.onClose} onSubmit={save} canSubmit={sku.trim().length > 0 && name.trim().length >= 2}>
      <div className="fgrid">
        <label>SKU<input value={sku} onChange={(e) => setSku(e.target.value)} required autoFocus /></label>
        <label>Name<input value={name} onChange={(e) => setName(e.target.value)} required /></label>
        <label>Barcode<input value={barcode} onChange={(e) => setBarcode(e.target.value)} inputMode="numeric" /></label>
        <label>
          Category
          <select value={categoryId} onChange={(e) => setCategoryId(e.target.value)}>
            <option value="">No category</option>
            {props.categories.map((c) => (
              <option key={c.id} value={c.id}>{c.name}</option>
            ))}
          </select>
        </label>
        <label>
          Unit
          <input value={unit} onChange={(e) => setUnit(e.target.value)} list="units" />
          <datalist id="units">
            {["pcs", "set", "pair", "box", "litre", "kg", "meter"].map((u) => <option key={u} value={u} />)}
          </datalist>
        </label>
      </div>
    </FormModal>
  );
}

function CategoriesModal({ categories, onClose }: { categories: Category[]; onClose: () => void }) {
  const { api } = useSession();
  const toast = useToast();
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [fresh, setFresh] = useState("");
  const [rows, setRows] = useState(categories);

  async function rename(c: Category) {
    const name = drafts[c.id];
    if (name === undefined || name.trim() === c.name) return;
    try {
      const updated = await api.put<Category>(`/v1/categories/${c.id}`, { expected_revision: c.revision, name });
      setRows((rs) => rs.map((r) => (r.id === c.id ? updated : r)));
      setDrafts(({ [c.id]: _gone, ...rest }) => rest);
      toast(`Renamed to ${updated.name}`);
    } catch (e) {
      toast(describeError(e), true);
    }
  }

  async function add() {
    try {
      const created = await api.post<Category>("/v1/categories", { name: fresh });
      setRows((rs) => [...rs, created]);
      setFresh("");
      toast(`Added category ${created.name}`);
    } catch (e) {
      toast(describeError(e), true);
    }
  }

  return (
    <FormModal title="Categories" submitLabel="Done" onClose={onClose} onSubmit={async () => {}}>
      {rows.length === 0 && <Empty>No categories yet.</Empty>}
      {rows.map((c) => (
        <div key={c.id} className="bar-actions" style={{ marginBottom: 0 }}>
          <input aria-label={`Name of ${c.name}`} className="grow" value={drafts[c.id] ?? c.name} onChange={(e) => setDrafts((d) => ({ ...d, [c.id]: e.target.value }))} />
          <button type="button" className="btn sm" disabled={drafts[c.id] === undefined || drafts[c.id]!.trim() === c.name} onClick={() => void rename(c)}>
            Rename
          </button>
        </div>
      ))}
      <div className="bar-actions" style={{ marginBottom: 0 }}>
        <input aria-label="New category name" className="grow" placeholder="New category name" value={fresh} onChange={(e) => setFresh(e.target.value)} />
        <button type="button" className="btn sm" disabled={fresh.trim().length < 2} onClick={() => void add()}>
          Add
        </button>
      </div>
    </FormModal>
  );
}
