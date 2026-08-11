import { getDrawdowns, getNav, getSummary } from "../api";
import EChart from "../components/EChart";
import KpiCard from "../components/KpiCard";
import { eur, num, pct } from "../fmt";
import { useFetch } from "../hooks";
import { usePortfolio } from "../PortfolioContext";
import { Link } from "react-router-dom";

export default function Overview() {
  const portfolio = usePortfolio();
  const summary = useFetch(() => getSummary(portfolio.id), [portfolio.id]);
  const nav = useFetch(() => getNav(portfolio.id), [portfolio.id]);
  const dd = useFetch(() => getDrawdowns(portfolio.id), [portfolio.id]);
  const s = summary.data;

  if (s?.empty) {
    return (
      <div>
        <h2>Overview</h2>
        <div className="card">No data yet — <Link to={`/p/${portfolio.id}/data`}>import a NAV Recap file</Link> to get started.</div>
      </div>
    );
  }

  return (
    <div>
      <h2>Overview {s?.as_of && <small>as of {s.as_of}</small>}</h2>
      {s?.warnings.map((w, i) => <span key={i} className="warn-badge">{w}</span>)}
      <div className="cards-row">
        <KpiCard label="NAV" value={num(s?.nav)} />
        <KpiCard label="AUM" value={eur(s?.aum)} />
        <KpiCard label="YTD" value={pct(s?.ytd)} />
        <KpiCard label="Vol 1Y" value={pct(s?.vol_1y)} sub={`Inception ${pct(s?.vol_inception)}`} />
        <KpiCard label="Sharpe 1Y" value={num(s?.sharpe_1y)} sub={`Yield/Vol ${num(s?.yield_vol_1y)}`} />
        <KpiCard label="Max drawdown" value={pct(s?.max_drawdown)} />
        <KpiCard
          label={`VaR ${pct((s?.var_ucits?.confidence ?? 0.99), 0)}/${s?.var_ucits?.horizon_days ?? 20}d`}
          value={pct(s?.var_ucits?.historical?.var)}
          sub={`Limit ${pct(s?.var_ucits?.limit)} · used ${pct(s?.var_ucits?.utilization, 0)}`}
        />
      </div>

      <div className="card">
        <h3>NAV</h3>
        <EChart option={{
          tooltip: { trigger: "axis" },
          xAxis: { type: "category", data: (nav.data ?? []).map((r) => r.date) },
          yAxis: { type: "value", scale: true },
          series: [{ type: "line", showSymbol: false, data: (nav.data ?? []).map((r) => r.nav), name: "NAV" }],
          dataZoom: [{ type: "inside" }, { type: "slider" }],
          grid: { left: 50, right: 20, top: 20, bottom: 60 },
        }} />
      </div>

      <div className="card">
        <h3>Drawdown</h3>
        <EChart option={{
          tooltip: { trigger: "axis", valueFormatter: (v) => pct(v as number) },
          xAxis: { type: "category", data: (dd.data?.underwater ?? []).map((p) => p.date) },
          yAxis: { type: "value", axisLabel: { formatter: (v: number) => pct(v, 0) } },
          series: [{
            type: "line", showSymbol: false, name: "Drawdown", areaStyle: { opacity: 0.25 },
            color: "#c62828", data: (dd.data?.underwater ?? []).map((p) => p.value),
          }],
          grid: { left: 55, right: 20, top: 20, bottom: 30 },
        }} height={260} />
      </div>
    </div>
  );
}
