import { Fragment, useState } from "react";
import DerivativesExposure from "../components/DerivativesExposure";
import { ApiError, emirExportUrl, getEmir, putEmirKpi, type EmirKpi } from "../api";
import { eur, num, pct } from "../fmt";
import { useFetch } from "../hooks";
import { usePortfolio } from "../PortfolioContext";

const VERDICT_LABEL: Record<string, string> = { ok: "OK", watch: "WATCH", breach: "BREACH" };
const TIER_LABEL: Record<string, string> = {
  not_triggered: "Not triggered — no OTC contracts outstanding",
  quarterly: "Quarterly",
  weekly: "Weekly",
  daily: "Daily",
};
const REC_LABEL: Record<EmirKpi["reconciliation"], string> = {
  done: "Done", not_done: "Not done", not_applicable: "N/A",
};

function VerdictChip({ v }: { v: string }) {
  const cls = v === "ok" ? "pos" : v === "watch" ? "warn-badge" : "neg";
  return <span className={cls}>{VERDICT_LABEL[v] ?? v}</span>;
}

function KpiForm({ pid, onSaved }: { pid: number; onSaved: () => void }) {
  const [month, setMonth] = useState(() => new Date().toISOString().slice(0, 7));
  const [unconf, setUnconf] = useState(0);
  const [rec, setRec] = useState<EmirKpi["reconciliation"]>("not_applicable");
  const [disputes, setDisputes] = useState(0);
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);

  async function save() {
    setBusy(true);
    setMsg(null);
    try {
      await putEmirKpi(pid, `${month}-01`, {
        unconfirmed_over_5d: unconf,
        reconciliation: rec,
        disputes,
        note: note.trim() || null,
      });
      setMsg(`Saved ${month}.`);
      onSaved();
    } catch (e) {
      const ae = e as ApiError;
      setMsg(`Error: ${ae.detail ?? ae.message}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="controls">
      <label>Month{" "}
        <input type="month" value={month} onChange={(e) => setMonth(e.target.value)} />
      </label>
      <label>Unconfirmed &gt; 5 days{" "}
        <input type="number" min={0} value={unconf} onChange={(e) => setUnconf(Number(e.target.value))} />
      </label>
      <label>Reconciliation{" "}
        <select value={rec} onChange={(e) => setRec(e.target.value as EmirKpi["reconciliation"])}>
          <option value="done">Done</option>
          <option value="not_done">Not done</option>
          <option value="not_applicable">N/A</option>
        </select>
      </label>
      <label>Disputes{" "}
        <input type="number" min={0} value={disputes} onChange={(e) => setDisputes(Number(e.target.value))} />
      </label>
      <label>Note{" "}
        <input type="text" value={note} onChange={(e) => setNote(e.target.value)} />
      </label>
      <button disabled={busy} onClick={() => void save()}>Save month</button>
      {msg && <span className="kpi-sub">{msg}</span>}
    </div>
  );
}

export default function DerivativesPage() {
  const portfolio = usePortfolio();
  const [date, setDate] = useState<string | undefined>(undefined);
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const emir = useFetch(() => getEmir(portfolio.id, date), [portfolio.id, date]);
  const data = emir.data;

  return (
    <div>
      <h2>Derivatives / EMIR</h2>
      <div className="controls">
        <label>Snapshot:{" "}
          <select value={data?.date ?? ""} onChange={(e) => setDate(e.target.value || undefined)}>
            {(data?.dates ?? []).map((d) => <option key={d} value={d}>{d}</option>)}
          </select>
        </label>
        <a href={emirExportUrl(portfolio.id) + (date ? `?date=${date}` : "")} download>Export evidence workbook</a>
      </div>
      {emir.error && <p className="neg">{emir.error}</p>}
      {!data && !emir.error && <p>Loading…</p>}
      {data?.empty && (
        <div className="card">
          {data.warnings.map((w, i) => <p key={i}>{w}</p>)}
        </div>
      )}
      {data && !data.empty && (
        <>
          <DerivativesExposure date={date} />

          <div className="card">
            <h3>EMIR clearing thresholds</h3>
            <p className="kpi-sub">
              Average of month-end gross notional over the last 12 months
              ({data.months_present} of {data.months_total} months have a snapshot). {data.otc_note}
            </p>
            <table className="tbl">
              <thead>
                <tr>
                  <th>Class</th><th>Avg OTC notional</th><th>Avg total notional</th>
                  <th>Threshold</th><th>% of threshold</th><th>Verdict</th>
                </tr>
              </thead>
              <tbody>
                {(data.classes ?? []).map((c) => (
                  <Fragment key={c.class}>
                    <tr onClick={() => setOpen({ ...open, [c.class]: !open[c.class] })} style={{ cursor: "pointer" }}>
                      <td>{open[c.class] ? "▾" : "▸"} {c.label}</td>
                      <td>{eur(c.avg_otc_eur)}</td>
                      <td>{eur(c.avg_total_eur)}</td>
                      <td>{eur(c.threshold_eur)}</td>
                      <td>{pct(c.pct_of_threshold)}</td>
                      <td><VerdictChip v={c.verdict} /></td>
                    </tr>
                    {open[c.class] && c.months.map((m) => (
                      <tr key={m.month}>
                        <td style={{ paddingLeft: 24, color: "#64748b" }}>
                          {m.month.slice(0, 7)}
                          {m.snapshot_date ? ` (snapshot ${m.snapshot_date})` : ""}
                        </td>
                        <td>{m.otc_eur === null ? "—" : eur(m.otc_eur)}</td>
                        <td>{m.total_eur === null ? "—" : eur(m.total_eur)}</td>
                        <td colSpan={3}>{m.snapshot_date === null && <span className="warn-badge">no snapshot this month</span>}</td>
                      </tr>
                    ))}
                  </Fragment>
                ))}
              </tbody>
            </table>
            {data.warnings.map((w, i) => <span key={i} className="warn-badge">{w}</span>)}
          </div>

          <div className="card">
            <h3>OTC obligations</h3>
            <table className="tbl">
              <tbody>
                <tr><td>Open OTC contracts</td><td>{num(data.monitors!.otc_open_contracts, 0)}</td></tr>
                <tr><td>Portfolio reconciliation</td><td>{TIER_LABEL[data.monitors!.reconciliation]}</td></tr>
                <tr>
                  <td>Compression analysis (≥ 500 contracts)</td>
                  <td>{data.monitors!.compression_required
                    ? <span className="warn-badge">required semiannually</span>
                    : `not required (${data.monitors!.otc_open_contracts} < 500)`}</td>
                </tr>
              </tbody>
            </table>
            <p className="kpi-sub">{data.monitors_note}</p>
          </div>

          <div className="card">
            <h3>Margin accounts</h3>
            <p className="kpi-sub">
              Margin balances from the snapshot, collateralizing {data.futures_count} futures position(s).
            </p>
            {(data.margin ?? []).length === 0 ? <p>No margin accounts in this snapshot.</p> : (
              <table className="tbl">
                <thead>
                  <tr><th>Account</th><th>Currency</th><th>Local value</th><th>EUR value</th></tr>
                </thead>
                <tbody>
                  {(data.margin ?? []).map((m, i) => (
                    <tr key={i}>
                      <td>{m.name ?? "—"}</td>
                      <td>{m.currency ?? "—"}</td>
                      <td>{num(m.valuation_ccy)}</td>
                      <td>{eur(m.valuation_eur)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>

          <div className="card">
            <h3>Monthly EMIR KPIs</h3>
            <p className="kpi-sub">
              Middle-office facts the tool cannot derive: confirmation follow-up, portfolio
              reconciliation and disputes. One record per calendar month, reviewed in the risk committee.
            </p>
            <KpiForm pid={portfolio.id} onSaved={emir.reload} />
            {(data.kpis ?? []).length > 0 && (
              <table className="tbl">
                <thead>
                  <tr><th>Month</th><th>Unconfirmed &gt; 5 days</th><th>Reconciliation</th><th>Disputes</th><th>Note</th></tr>
                </thead>
                <tbody>
                  {(data.kpis ?? []).map((k) => (
                    <tr key={k.month}>
                      <td>{k.month.slice(0, 7)}</td>
                      <td>{num(k.unconfirmed_over_5d, 0)}</td>
                      <td>{REC_LABEL[k.reconciliation]}</td>
                      <td>{num(k.disputes, 0)}</td>
                      <td>{k.note ?? "—"}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
        </>
      )}
    </div>
  );
}
