import { getDerivatives, type Category } from "../api";
import { eur, num, pct } from "../fmt";
import { useFetch } from "../hooks";

const LABELS: Record<Category, string> = {
  equity: "Equity",
  interest_rate: "Interest rate",
  fx: "Foreign exchange",
  credit: "Credit",
  commodity: "Commodity",
  other: "Other",
};

export default function DerivativesExposure({ date }: { date?: string }) {
  const d = useFetch(() => getDerivatives(date), [date]);
  const data = d.data;

  return (
    <div className="card">
      <h3>Derivatives exposure</h3>
      {d.error && <p className="neg">{d.error}</p>}
      {data && (
        <>
          <p className="kpi-sub">{data.note}</p>

          {data.unconfirmed.length > 0 && (
            <p className="warn-badge">
              Unconfirmed contract specs: {data.unconfirmed.join(", ")}. Confirm them on the Data page.
            </p>
          )}
          {data.excluded.length > 0 && (
            <p className="warn-badge">
              Excluded from the totals for want of a spec or FX rate: {data.excluded.join(", ")}.
            </p>
          )}

          {data.rows.length === 0 ? (
            <p>No derivative positions in this snapshot.</p>
          ) : (
            <>
              <table className="tbl">
                <thead>
                  <tr><th>Category</th><th>Long</th><th>Short</th><th>Gross</th></tr>
                </thead>
                <tbody>
                  {data.categories.filter((c) => c.gross_pct > 0).map((c) => (
                    <tr key={c.category}>
                      <td>{LABELS[c.category]}</td>
                      <td>{c.long_pct > 0 ? pct(c.long_pct) : "—"}</td>
                      <td>{c.short_pct > 0 ? pct(c.short_pct) : "—"}</td>
                      <td>{pct(c.gross_pct)}</td>
                    </tr>
                  ))}
                  <tr>
                    <td><strong>Total notional</strong></td>
                    <td><strong>{pct(data.total.long_pct)}</strong></td>
                    <td><strong>{pct(data.total.short_pct)}</strong></td>
                    <td><strong>{pct(data.total.gross_pct)}</strong></td>
                  </tr>
                </tbody>
              </table>

              <h4>Contracts</h4>
              <table className="tbl">
                <thead>
                  <tr>
                    <th>Ticker</th><th>Name</th><th>Category</th><th>Qty</th>
                    <th>Price</th><th>Point value</th><th>Notional €</th><th>% NAV</th>
                  </tr>
                </thead>
                <tbody>
                  {data.rows.map((r) => (
                    <tr key={r.ticker}>
                      <td>{r.ticker}</td>
                      <td>{r.name}</td>
                      <td>{LABELS[r.category]}</td>
                      <td>{num(r.qty, 0)}</td>
                      <td>{num(r.price)}</td>
                      <td>{r.spec_missing ? <span className="neg">spec missing</span> : num(r.point_value, 0)}</td>
                      <td>{eur(r.notional_eur)}</td>
                      <td>{r.pct_nav === null ? "—" : pct(r.pct_nav)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </>
          )}
        </>
      )}
    </div>
  );
}
