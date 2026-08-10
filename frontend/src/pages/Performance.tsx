import { getCalendar, getDrawdowns, type PeriodReturn } from "../api";
import { pct } from "../fmt";
import { useFetch } from "../hooks";
import { usePortfolio } from "../PortfolioContext";

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** background heat color: red for losses, green for gains */
function heat(v: number | undefined): string {
  if (v == null) return "transparent";
  const a = Math.min(Math.abs(v) / 0.05, 1) * 0.45;
  return v >= 0 ? `rgba(10, 125, 51, ${a})` : `rgba(198, 40, 40, ${a})`;
}

function byYear(rows: PeriodReturn[]): Map<number, Map<number, number>> {
  const m = new Map<number, Map<number, number>>();
  for (const r of rows) {
    if (!m.has(r.year)) m.set(r.year, new Map());
    m.get(r.year)!.set(r.period, r.value);
  }
  return m;
}

export default function Performance() {
  const portfolio = usePortfolio();
  const cal = useFetch(() => getCalendar(portfolio.id), [portfolio.id]);
  const dd = useFetch(() => getDrawdowns(portfolio.id), [portfolio.id]);
  const monthly = byYear(cal.data?.monthly ?? []);
  const quarterly = byYear(cal.data?.quarterly ?? []);
  const annual = new Map((cal.data?.annual ?? []).map((r) => [r.year, r.value]));
  const years = [...monthly.keys()].sort((a, b) => b - a).slice(0, 3);

  return (
    <div>
      <h2>Performance</h2>

      <div className="card">
        <h3>Monthly returns</h3>
        <table className="tbl">
          <thead><tr><th>Year</th>{MONTHS.map((m) => <th key={m}>{m}</th>)}<th>Year</th></tr></thead>
          <tbody>
            {years.map((y) => (
              <tr key={y}>
                <td>{y}</td>
                {MONTHS.map((_, i) => {
                  const v = monthly.get(y)?.get(i + 1);
                  return <td key={i} style={{ background: heat(v) }}>{v == null ? "" : pct(v, 1)}</td>;
                })}
                <td className={(annual.get(y) ?? 0) >= 0 ? "pos" : "neg"}>{pct(annual.get(y), 1)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <div className="card">
        <h3>Quarterly returns</h3>
        <table className="tbl">
          <thead><tr><th>Year</th><th>Q1</th><th>Q2</th><th>Q3</th><th>Q4</th></tr></thead>
          <tbody>
            {years.map((y) => (
              <tr key={y}>
                <td>{y}</td>
                {[1, 2, 3, 4].map((q) => {
                  const v = quarterly.get(y)?.get(q);
                  return <td key={q} style={{ background: heat(v) }}>{v == null ? "" : pct(v, 1)}</td>;
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <div className="card">
        <h3>Max drawdown per year</h3>
        <table className="tbl">
          <thead><tr><th>Year</th><th>Max drawdown</th></tr></thead>
          <tbody>
            {(dd.data?.yearly ?? []).map((r) => (
              <tr key={r.year}><td>{r.year}</td><td className="neg">{pct(r.max_drawdown)}</td></tr>
            ))}
            <tr><td><b>Since inception</b></td><td className="neg"><b>{pct(dd.data?.overall_max)}</b></td></tr>
          </tbody>
        </table>
      </div>
    </div>
  );
}
