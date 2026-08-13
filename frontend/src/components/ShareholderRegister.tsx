import { useEffect, useState } from "react";
import { ApiError, getShareholders, putShareholders, type Shareholder } from "../api";
import { useFetch } from "../hooks";
import { usePortfolio } from "../PortfolioContext";

function todayIso() {
  return new Date().toISOString().slice(0, 10);
}

export default function ShareholderRegister() {
  const portfolio = usePortfolio();
  const reg = useFetch(() => getShareholders(portfolio.id), [portfolio.id]);
  const [draft, setDraft] = useState<Shareholder[] | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // A draft is scoped to the portfolio it was edited for — switching
  // portfolios must not carry a half-edited register onto a different fund.
  useEffect(() => { setDraft(null); setMsg(null); }, [portfolio.id]);

  if (reg.data === null && !reg.error) return <div className="card"><h3>Shareholder register</h3><p>Loading…</p></div>;

  const rows = draft ?? reg.data ?? [];
  const total = rows.reduce((a, r) => a + (Number.isFinite(r.pct_of_nav) ? r.pct_of_nav : 0), 0);
  const overLimit = total > 100;

  function setRow(i: number, patch: Partial<Shareholder>) {
    setDraft(rows.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  }
  function addRow() {
    setDraft([...rows, { label: "", pct_of_nav: 0, as_of: todayIso() }]);
  }
  function removeRow(i: number) {
    setDraft(rows.filter((_, j) => j !== i));
  }

  async function save() {
    setBusy(true);
    setMsg(null);
    try {
      await putShareholders(portfolio.id, rows);
      setDraft(null);
      setMsg("Saved.");
      reg.reload();
    } catch (e) {
      const ae = e as ApiError;
      setMsg(`Error: ${ae.detail ?? ae.message}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="card">
      <h3>Shareholder register</h3>
      <p className="kpi-sub">
        Maintained by hand: the depositary feed is share-class level and carries no
        investor-level holdings. Nothing reconciles these percentages against the
        fund's outstanding shares, so a stale register moves the top-five scenarios
        without warning beyond the as-of date shown on Limits.
      </p>
      {reg.error && <p className="neg">{reg.error}</p>}
      {msg && <p className={msg.startsWith("Error") ? "neg" : "pos"}>{msg}</p>}
      <table className="tbl">
        <thead><tr><th>Label</th><th>% of NAV</th><th>As-of</th><th></th></tr></thead>
        <tbody>
          {rows.map((r, i) => (
            <tr key={i}>
              <td>
                <input value={r.label} onChange={(e) => setRow(i, { label: e.target.value })} />
              </td>
              <td>
                <input
                  type="number" min={0} max={100} step={0.01}
                  value={r.pct_of_nav}
                  onChange={(e) => setRow(i, { pct_of_nav: Number(e.target.value) })}
                />
              </td>
              <td>
                <input type="date" value={r.as_of} onChange={(e) => setRow(i, { as_of: e.target.value })} />
              </td>
              <td><button onClick={() => removeRow(i)}>Remove</button></td>
            </tr>
          ))}
        </tbody>
      </table>
      <div className="controls">
        <button onClick={addRow}>Add entry</button>
        <span className={overLimit ? "neg" : ""}>Total: {total.toFixed(2)}% of NAV</span>
        <button disabled={busy || overLimit || draft === null} onClick={() => void save()}>Save</button>
      </div>
    </div>
  );
}
