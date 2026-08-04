import { useState } from "react";
import { getConcentration, getLiquidity, getRates, type Check, type CheckStatus } from "../api";
import EChart from "../components/EChart";
import { eur, num, pct } from "../fmt";
import { useFetch } from "../hooks";

const STATUS_LABEL: Record<CheckStatus, string> = { ok: "OK", watch: "WATCH", breach: "BREACH" };

function StatusChip({ s }: { s: CheckStatus }) {
  const cls = s === "ok" ? "pos" : s === "watch" ? "warn-badge" : "neg";
  return <span className={cls}>{STATUS_LABEL[s]}</span>;
}

function CheckCard({ c }: { c: Check }) {
  return (
    <div className="card">
      <h3>{c.scope_label} <StatusChip s={c.status} /></h3>
      {c.rows.length === 0 ? <p>No positions in scope.</p> : (
        <table className="tbl">
          <thead><tr><th>Group</th><th>Weight</th><th>vs limit {pct(c.limit, 0)}</th><th>Status</th></tr></thead>
          <tbody>
            {c.rows.map((r, i) => (
              <tr key={i}>
                <td>{r.group}</td>
                <td>{pct(r.weight)}</td>
                <td>
                  <div style={{ background: "#eee", height: 8, width: 120 }}>
                    <div style={{
                      background: r.status === "breach" ? "#c62828" : r.status === "watch" ? "#b26a00" : "#2e7d32",
                      height: 8,
                      width: Math.min(120, (r.weight / c.limit) * 120),
                    }} />
                  </div>
                </td>
                <td><StatusChip s={r.status} /></td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

const BUCKET_LABELS: Record<string, string> = {
  d1: "1 day", d2_7: "2-7 days", d8_30: "8-30 days", d30p: "> 30 days",
};

export default function LimitsPage() {
  const [date, setDate] = useState<string | undefined>(undefined);
  const conc = useFetch(() => getConcentration(date), [date]);
  const liq = useFetch(() => getLiquidity(date), [date]);
  const rates = useFetch(() => getRates(date), [date]);

  return (
    <div>
      <h2>Limits</h2>
      <div className="controls">
        <label>Snapshot:{" "}
          <select value={conc.data?.date ?? ""} onChange={(e) => setDate(e.target.value || undefined)}>
            {(conc.data?.dates ?? []).map((d) => <option key={d} value={d}>{d}</option>)}
          </select>
        </label>
      </div>

      <h3>Concentration</h3>
      {conc.error && <p className="neg">{conc.error}</p>}
      {(conc.data?.checks ?? []).map((c) => <CheckCard key={c.check} c={c} />)}
      {conc.data && <p className="kpi-sub">{conc.data.excluded_note}</p>}

      <h3>Liquidity</h3>
      {liq.error && <p className="neg">{liq.error}</p>}
      {liq.data && (
        <div className="card">
          <p>
            Redemption stress {pct(liq.data.shock, 0)} vs assets liquidatable in ≤ 7 days:{" "}
            <StatusChip s={liq.data.stress_status === "ok" ? "ok" : "breach"} />
          </p>
          <EChart option={{
            tooltip: { trigger: "axis", valueFormatter: (x) => pct(x as number) },
            legend: { data: ["Bucket", "Cumulative"] },
            xAxis: { type: "category", data: liq.data.buckets.map((b) => BUCKET_LABELS[b.bucket] ?? b.bucket) },
            yAxis: { type: "value", axisLabel: { formatter: (x: number) => pct(x, 0) } },
            series: [
              { type: "bar", name: "Bucket", color: "#1d64c2", data: liq.data.buckets.map((b) => b.weight) },
              {
                type: "line", name: "Cumulative", color: "#2e7d32",
                data: liq.data.cumulative.map((b) => b.weight),
                markLine: {
                  silent: true, symbol: "none",
                  lineStyle: { color: "#c62828", type: "dashed" },
                  data: [{ yAxis: liq.data.shock, label: { formatter: "Stress" } }],
                },
              },
            ],
            grid: { left: 55, right: 40, top: 40, bottom: 30 },
          }} />
          <p className="kpi-sub">Negative positions (payables, short cash): {pct(liq.data.negative_memo)} — shown as memo, not netted.</p>
        </div>
      )}

      <h3>Rates</h3>
      {rates.error && <p className="neg">{rates.error}</p>}
      {rates.data && (
        <div className="card">
          {rates.data.missing_any && (
            <p className="warn-badge">Some bonds lack reference data — fill coupon/maturity/frequency on the Data page.</p>
          )}
          <table className="tbl">
            <thead><tr><th>Bond</th><th>Coupon</th><th>Maturity</th><th>Price</th><th>YTM</th><th>Mod. duration</th><th>DV01 €</th><th>Weight</th></tr></thead>
            <tbody>
              {rates.data.bonds.map((b, i) => b.missing ? (
                <tr key={i}><td>{b.name ?? b.code}</td><td colSpan={7} className="neg">missing reference data</td></tr>
              ) : (
                <tr key={i}>
                  <td>{b.name ?? b.code}</td>
                  <td>{num(b.coupon_pct)}%</td>
                  <td>{b.maturity}</td>
                  <td>{num(b.price)}</td>
                  <td>{pct(b.ytm)}</td>
                  <td>{num(b.mod_duration)}</td>
                  <td>{eur(b.dv01_eur ?? null)}</td>
                  <td>{pct(b.weight)}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <p>
            Portfolio DV01: <strong>{eur(rates.data.total_dv01_eur)}</strong> · NAV sensitivity per +100bp:{" "}
            <strong>{pct(rates.data.nav_sensitivity_100bp)}</strong>
          </p>
          <p className="kpi-sub">
            Not included (no notional/CTD data in the source file): {rates.data.futures_note.join(", ") || "—"}
          </p>
        </div>
      )}
    </div>
  );
}
