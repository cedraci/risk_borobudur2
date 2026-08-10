import { getDerivatives } from "../api";
import { CATEGORY_LABELS as LABELS, eur, num, pct } from "../fmt";
import { useFetch } from "../hooks";
import { usePortfolio } from "../PortfolioContext";

export default function DerivativesExposure({ date }: { date?: string }) {
  const portfolio = usePortfolio();
  const d = useFetch(() => getDerivatives(portfolio.id, date), [portfolio.id, date]);
  const data = d.data;
  // Any excluded contract is a position whose notional could not be computed,
  // so every figure below is a lower bound, not a total. Say so on the number
  // itself rather than only in the banner above it.
  const partial = (data?.excluded.length ?? 0) > 0;
  const atLeast = (x: number) => (partial ? `≥ ${pct(x)}` : pct(x));

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
              {data.excluded.length} contract(s) excluded from the totals for want of a spec, a price,
              a quantity or an FX rate: {data.excluded.join(", ")}. The totals below are partial.
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
                    <td><strong>{partial ? "Total notional (partial)" : "Total notional"}</strong></td>
                    <td><strong>{atLeast(data.total.long_pct)}</strong></td>
                    <td><strong>{atLeast(data.total.short_pct)}</strong></td>
                    <td><strong>{atLeast(data.total.gross_pct)}</strong></td>
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
                  {data.rows.map((r, i) => (
                    <tr key={`${r.ticker}-${i}`}>
                      <td>
                        {r.ticker}
                        {r.unconfirmed && <> <span className="warn-badge" title="Contract spec not confirmed — this notional is provisional.">unconfirmed</span></>}
                      </td>
                      <td>{r.name}</td>
                      <td>{LABELS[r.category]}</td>
                      <td>{r.qty === null ? <span className="neg">missing</span> : num(r.qty, 0)}</td>
                      <td>{r.price === null ? <span className="neg">missing</span> : num(r.price)}</td>
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
