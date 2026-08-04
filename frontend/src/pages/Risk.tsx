import { useState } from "react";
import { getDrawdowns, getRolling, type NavPoint } from "../api";
import EChart from "../components/EChart";
import { num, pct } from "../fmt";
import { useFetch } from "../hooks";

function line(points: NavPoint[], name: string, percent: boolean, color?: string) {
  return {
    tooltip: { trigger: "axis" as const, valueFormatter: (v: unknown) => (percent ? pct(v as number) : num(v as number)) },
    xAxis: { type: "category" as const, data: points.map((p) => p.date) },
    yAxis: { type: "value" as const, scale: true, axisLabel: { formatter: (v: number) => (percent ? pct(v, 0) : num(v, 1)) } },
    series: [{ type: "line" as const, showSymbol: false, name, color, data: points.map((p) => p.value) }],
    grid: { left: 55, right: 20, top: 20, bottom: 30 },
  };
}

export default function Risk() {
  const [window, setWindow] = useState(60);
  const rolling = useFetch(() => getRolling(window), [window]);
  const dd = useFetch(() => getDrawdowns(), []);

  return (
    <div>
      <h2>Risk</h2>
      <div className="controls">
        <label>Rolling window:{" "}
          <select value={window} onChange={(e) => setWindow(Number(e.target.value))}>
            {[20, 60, 120, 252].map((w) => <option key={w} value={w}>{w} days</option>)}
          </select>
        </label>
      </div>

      <div className="card">
        <h3>Annualized volatility ({window}d rolling)</h3>
        <EChart option={line(rolling.data?.vol ?? [], "Vol", true)} height={260} />
      </div>
      <div className="card">
        <h3>Sharpe ratio ({window}d rolling)</h3>
        <EChart option={line(rolling.data?.sharpe ?? [], "Sharpe", false, "#7b1fa2")} height={260} />
      </div>
      <div className="card">
        <h3>Yield / volatility ({window}d rolling)</h3>
        <EChart option={line(rolling.data?.yield_vol ?? [], "Yield/Vol", false, "#00695c")} height={260} />
      </div>

      <div className="card">
        <h3>Top 5 drawdowns over short periods (≤ {dd.data?.max_days ?? 50} days)</h3>
        <table className="tbl">
          <thead><tr><th>#</th><th>Peak</th><th>Trough</th><th>Depth</th><th>Days</th><th>Recovered</th></tr></thead>
          <tbody>
            {(dd.data?.top_short ?? []).map((e, i) => (
              <tr key={i}>
                <td>{i + 1}</td><td>{e.peak_date}</td><td>{e.trough_date}</td>
                <td className="neg">{pct(e.depth)}</td><td>{e.duration_days}</td>
                <td>{e.recovery_date ?? "ongoing"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
