import { useState } from "react";
import { getBacktest, getSettings, getVar, type VarBlock } from "../api";
import EChart from "../components/EChart";
import { eur, num, pct } from "../fmt";
import { useFetch } from "../hooks";
import { usePortfolio } from "../PortfolioContext";

function MethodCard({ title, v, varEur }: { title: string; v: { var: number; es: number } | null | undefined; varEur?: number | null }) {
  return (
    <div className="card kpi">
      <div className="kpi-label">{title}</div>
      <div className="kpi-value">{pct(v?.var)}</div>
      <div className="kpi-sub">ES {pct(v?.es)}{varEur != null ? ` · ${eur(varEur)}` : ""}</div>
    </div>
  );
}

export default function VarPage() {
  const portfolio = usePortfolio();
  const settings = useFetch(() => getSettings(portfolio.id), [portfolio.id]);
  const [confidence, setConfidence] = useState<number | null>(null);
  const [horizon, setHorizon] = useState<number | null>(null);
  const [window, setWindow] = useState<number | null>(null);

  const c = confidence ?? settings.data?.var_confidence ?? 0.99;
  const h = horizon ?? settings.data?.var_horizon_days ?? 20;
  const w = window ?? settings.data?.var_window_days ?? 252;
  const v = useFetch(() => getVar(portfolio.id, { confidence: c, horizon: h, window: w }), [portfolio.id, c, h, w, !!settings.data]);
  const m: VarBlock | null = v.data?.methods ?? null;
  const bt = useFetch(() => getBacktest(portfolio.id), [portfolio.id]);

  return (
    <div>
      <h2>VaR / Expected Shortfall</h2>
      {v.data?.warnings.map((wn, i) => <span key={i} className="warn-badge">{wn}</span>)}
      <div className="controls">
        <label>Confidence:{" "}
          <select value={c} onChange={(e) => setConfidence(Number(e.target.value))}>
            {[0.95, 0.975, 0.99].map((x) => <option key={x} value={x}>{(x * 100).toFixed(1)}%</option>)}
          </select>
        </label>
        <label>Horizon:{" "}
          <select value={h} onChange={(e) => setHorizon(Number(e.target.value))}>
            {[1, 10, 20].map((x) => <option key={x} value={x}>{x}d</option>)}
          </select>
        </label>
        <label>Window:{" "}
          <input type="number" min={30} value={w} onChange={(e) => setWindow(Math.max(30, Number(e.target.value)))} />
        </label>
        <span>UCITS limit: {pct(v.data?.limit)}</span>
      </div>

      <div className="cards-row">
        <MethodCard title="Historical" v={m?.historical} varEur={m?.var_eur} />
        <MethodCard title="Gaussian" v={m?.gaussian} />
        <MethodCard title="Cornish-Fisher" v={m?.cornish_fisher} />
        <div className="card kpi">
          <div className="kpi-label">Limit utilization</div>
          <div className={`kpi-value ${(m?.utilization ?? 0) > 1 ? "neg" : "pos"}`}>{pct(m?.utilization, 0)}</div>
          <div className="kpi-sub">of {pct(m?.limit)} absolute VaR limit</div>
        </div>
      </div>

      <div className="card">
        <h3>Rolling VaR (historical, {(c * 100).toFixed(1)}% / {h}d) vs UCITS limit</h3>
        <EChart option={{
          tooltip: { trigger: "axis", valueFormatter: (x) => pct(x as number) },
          xAxis: { type: "category", data: (v.data?.rolling ?? []).map((p) => p.date) },
          yAxis: { type: "value", axisLabel: { formatter: (x: number) => pct(x, 0) } },
          series: [{
            type: "line", showSymbol: false, name: "VaR", color: "#b26a00",
            data: (v.data?.rolling ?? []).map((p) => p.value),
            markLine: {
              silent: true, symbol: "none",
              lineStyle: { color: "#c62828", type: "dashed" },
              data: [{ yAxis: v.data?.limit ?? 0.2, label: { formatter: "UCITS limit" } }],
            },
          }],
          grid: { left: 55, right: 40, top: 20, bottom: 30 },
        }} />
      </div>

      <div className="card">
        <h3>Limit breaches</h3>
        {(v.data?.breaches?.length ?? 0) === 0 ? <p className="pos">No breaches over the computed history.</p> : (
          <table className="tbl">
            <thead><tr><th>Date</th><th>VaR</th></tr></thead>
            <tbody>
              {(v.data?.breaches ?? []).map((b, i) => (
                <tr key={i}><td>{b.date}</td><td className="neg">{pct(b.value)}</td></tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      <h3>Back-testing (1-day / 99%, window {bt.data?.window ?? "…"})</h3>
      {bt.data?.insufficient ? (
        <div className="card"><p className="warn-badge">Insufficient history for back-testing (needs more than {bt.data.window} daily returns).</p></div>
      ) : bt.data && (
        <>
          <div className="cards-row">
            {([
              ["Historical", bt.data.methods.historical],
              ["Gaussian", bt.data.methods.gaussian],
              ["Cornish-Fisher", bt.data.methods.cornish_fisher],
            ] as const).map(([title, m]) => (
              <div className="card kpi" key={title}>
                <div className="kpi-label">{title}</div>
                <div className={`kpi-value ${m.zone === "green" ? "pos" : "neg"}`}>
                  {m.exceptions}/{m.n} · {m.zone.toUpperCase()}
                </div>
                <div className="kpi-sub">
                  Kupiec p {m.kupiec_p == null ? "n/a" : num(m.kupiec_p, 3)}{m.reject ? " · model rejected" : ""}
                  {m.n < 250 ? ` · partial: ${m.n}/250` : ""}
                </div>
              </div>
            ))}
          </div>
          <div className="card">
            <h3>Daily returns vs −VaR (exceptions marked)</h3>
            <EChart option={{
              tooltip: { trigger: "axis", valueFormatter: (x) => pct(x as number) },
              legend: { data: ["Return", "−VaR hist", "−VaR gauss", "−VaR CF"] },
              xAxis: { type: "category", data: bt.data.series.map((p) => p.date) },
              yAxis: { type: "value", axisLabel: { formatter: (x: number) => pct(x, 1) } },
              series: [
                { type: "line", name: "Return", showSymbol: false, color: "#607d8b", data: bt.data.series.map((p) => p.ret) },
                { type: "line", name: "−VaR hist", showSymbol: false, color: "#b26a00", data: bt.data.series.map((p) => p.var_hist == null ? null : -p.var_hist) },
                { type: "line", name: "−VaR gauss", showSymbol: false, color: "#1d64c2", data: bt.data.series.map((p) => p.var_gauss == null ? null : -p.var_gauss) },
                { type: "line", name: "−VaR CF", showSymbol: false, color: "#6a1b9a", data: bt.data.series.map((p) => p.var_cf == null ? null : -p.var_cf) },
                {
                  type: "scatter", name: "Exception", color: "#c62828", symbolSize: 8,
                  data: bt.data.series.filter((p) => p.exc_hist || p.exc_gauss || p.exc_cf).map((p) => [p.date, p.ret]),
                },
              ],
              grid: { left: 55, right: 40, top: 40, bottom: 30 },
            }} />
          </div>
        </>
      )}
    </div>
  );
}
