import { useState } from "react";
import { ApiError, createPortfolio, getCodes, getPortfolios, putCodes, updatePortfolio, type Portfolio } from "../api";
import { useFetch } from "../hooks";

const KINDS: Portfolio["kind"][] = ["ucits", "mandate"];

/** Admin card for the Data page: lists every portfolio (including archived),
 * lets you rename or archive/restore one, and create new ones. Calls
 * `onChange` after any successful mutation so the caller can refresh
 * whatever else shows the portfolio list (e.g. the nav selector). */
export default function PortfoliosAdmin({ onChange }: { onChange?: () => void }) {
  const portfolios = useFetch(() => getPortfolios(), []);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [drafts, setDrafts] = useState<Record<number, string>>({});
  const [busyId, setBusyId] = useState<number | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);

  const [newName, setNewName] = useState("");
  const [newKind, setNewKind] = useState<Portfolio["kind"]>("ucits");
  const [creating, setCreating] = useState(false);

  const rows = portfolios.data ?? [];

  function startEdit(p: Portfolio) {
    setErr(null);
    setEditingId(p.id);
    setDrafts((d) => ({ ...d, [p.id]: p.name }));
  }
  function cancelEdit(id: number) {
    setEditingId((cur) => (cur === id ? null : cur));
    setDrafts((d) => { const rest = { ...d }; delete rest[id]; return rest; });
  }

  // Rename: keeps the row's current archived flag untouched — only the name changes.
  async function saveRename(p: Portfolio) {
    const draft = (drafts[p.id] ?? p.name).trim();
    if (!draft) { setErr("Name must not be empty."); return; }
    setErr(null);
    setBusyId(p.id);
    try {
      await updatePortfolio(p.id, draft, p.archived);
      cancelEdit(p.id);
      setMsg(`Renamed to "${draft}".`);
      portfolios.reload();
      onChange?.();
    } catch (e) {
      const ae = e as ApiError;
      setErr(ae.detail ?? ae.message);
    } finally {
      setBusyId((cur) => (cur === p.id ? null : cur));
    }
  }

  // Archive/restore: keeps the row's current name untouched — only the flag flips.
  async function toggleArchive(p: Portfolio) {
    setErr(null);
    setBusyId(p.id);
    try {
      await updatePortfolio(p.id, p.name, !p.archived);
      setMsg(`${p.archived ? "Restored" : "Archived"} "${p.name}".`);
      portfolios.reload();
      onChange?.();
    } catch (e) {
      const ae = e as ApiError;
      setErr(ae.detail ?? ae.message);
    } finally {
      setBusyId((cur) => (cur === p.id ? null : cur));
    }
  }

  async function doCreate() {
    const name = newName.trim();
    if (!name) { setErr("Name must not be empty."); return; }
    setErr(null);
    setCreating(true);
    try {
      await createPortfolio(name, newKind);
      setNewName("");
      setNewKind("ucits");
      setMsg(`Created "${name}".`);
      portfolios.reload();
      onChange?.();
    } catch (e) {
      const ae = e as ApiError;
      setErr(ae.detail ?? ae.message);
    } finally {
      setCreating(false);
    }
  }

  return (
    <div className="card">
      <h3>Portfolios</h3>
      {portfolios.error && <p className="neg">{portfolios.error}</p>}
      {msg && <p className="kpi-sub">{msg}</p>}
      {err && (
        <div className="neg">
          <p>{err}</p>
        </div>
      )}

      <table className="tbl">
        <thead>
          <tr><th>Name</th><th>Kind</th><th>CACEIS code</th><th>Latest NAV</th><th>Status</th><th></th></tr>
        </thead>
        <tbody>
          {rows.map((p) => {
            const isEditing = editingId === p.id;
            const busy = busyId === p.id;
            return (
              <tr key={p.id}>
                <td>
                  {isEditing ? (
                    <input
                      value={drafts[p.id] ?? p.name}
                      disabled={busy}
                      onChange={(e) => setDrafts((d) => ({ ...d, [p.id]: e.target.value }))}
                    />
                  ) : (
                    <>
                      {p.name}{" "}
                      <button disabled={busy} title="Rename" onClick={() => startEdit(p)}>✎</button>
                    </>
                  )}
                </td>
                <td>{p.kind}</td>
                <td><CodeCell portfolioId={p.id} /></td>
                <td>{p.latest_nav_date ?? "—"}</td>
                <td>
                  {p.archived ? <span className="warn-badge">archived</span> : <span className="pos">active</span>}
                </td>
                <td>
                  {isEditing ? (
                    <>
                      <button disabled={busy} onClick={() => void saveRename(p)}>Save</button>
                      <button disabled={busy} onClick={() => cancelEdit(p.id)}>Cancel</button>
                    </>
                  ) : (
                    <button disabled={busy} onClick={() => void toggleArchive(p)}>
                      {p.archived ? "Restore" : "Archive"}
                    </button>
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>

      <h4>Create a portfolio</h4>
      <div className="controls">
        <input
          placeholder="Name"
          value={newName}
          disabled={creating}
          onChange={(e) => setNewName(e.target.value)}
        />
        <select value={newKind} disabled={creating} onChange={(e) => setNewKind(e.target.value as Portfolio["kind"])}>
          {KINDS.map((k) => <option key={k} value={k}>{k}</option>)}
        </select>
        <button disabled={creating || !newName.trim()} onClick={() => void doCreate()}>Create</button>
      </div>
    </div>
  );
}

function CodeCell({ portfolioId }: { portfolioId: number }) {
  const codes = useFetch(() => getCodes(portfolioId), [portfolioId]);
  const [draft, setDraft] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const current = codes.data?.find((c) => c.source === "caceis")?.code ?? "";
  const value = draft ?? current;
  const dirty = draft !== null && draft.trim() !== current;
  async function save() {
    setErr(null);
    try {
      const others = (codes.data ?? []).filter((c) => c.source !== "caceis")
        .map((c) => ({ source: c.source, code: c.code }));
      const next = value.trim() ? [...others, { source: "caceis", code: value.trim() }] : others;
      await putCodes(portfolioId, next);
      setDraft(null);
      codes.reload();
    } catch (e) {
      const ae = e as ApiError;
      setErr(ae.detail ?? ae.message);
    }
  }
  return (
    <>
      <input
        style={{ width: 80 }}
        placeholder="fund code"
        value={value}
        onChange={(e) => setDraft(e.target.value)}
      />
      <button disabled={!dirty} onClick={() => void save()}>Save</button>
      {err && <span className="neg"> {err}</span>}
    </>
  );
}
