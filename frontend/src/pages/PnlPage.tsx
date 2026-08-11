import { Fragment, useMemo, useState } from "react";
import { getPnl, type PnlDimension } from "../api";
import { eur, pct } from "../fmt";
import { useFetch } from "../hooks";
import { usePortfolio } from "../PortfolioContext";

const DIMENSIONS: { value: PnlDimension; label: string }[] = [
  { value: "asset_class", label: "Asset class" },
  { value: "country", label: "Country" },
  { value: "region", label: "Region" },
  { value: "sector", label: "Sector" },
  { value: "industry", label: "Industry" },
  { value: "currency", label: "Currency" },
  { value: "issuer_group", label: "Issuer group" },
];

function presetRange(preset: string): { from: string; to: string } {
  const today = new Date();
  const iso = (d: Date) => d.toISOString().slice(0, 10);
  const y = today.getFullYear();
  switch (preset) {
    case "MTD": return { from: iso(new Date(y, today.getMonth(), 1)), to: iso(today) };
    case "QTD": return { from: iso(new Date(y, Math.floor(today.getMonth() / 3) * 3, 1)), to: iso(today) };
    case "YTD": return { from: `${y}-01-01`, to: iso(today) };
    default: return { from: "2000-01-01", to: iso(today) }; // ITD
  }
}

export default function PnlPage() {
  const portfolio = usePortfolio();
  const [range, setRange] = useState(presetRange("YTD"));
  const [dimension, setDimension] = useState<PnlDimension>("asset_class");
  const [open, setOpen] = useState<Record<string, boolean>>({});

  const pnl = useFetch(() => getPnl(portfolio.id, { ...range, dimension }), [portfolio.id, range.from, range.to, dimension]);
  const data = pnl.data;

  const total = useMemo(
    () => (data?.groups ?? []).reduce((s, g) => s + g.total, 0),
    [data],
  );

  const controls = (
    <div className="controls">
      {["MTD", "QTD", "YTD", "ITD"].map((k) => (
        <button key={k} onClick={() => setRange(presetRange(k))}>{k}</button>
      ))}
      <input type="date" value={range.from} onChange={(e) => setRange({ ...range, from: e.target.value })} />
      <input type="date" value={range.to} onChange={(e) => setRange({ ...range, to: e.target.value })} />
      <label>Group by:{" "}
        <select value={dimension} onChange={(e) => setDimension(e.target.value as PnlDimension)}>
          {DIMENSIONS.map((d) => <option key={d.value} value={d.value}>{d.label}</option>)}
        </select>
      </label>
      {/* The unclassified count is dimension-independent: it counts instruments with no
          classification data at all, not the current grouping's "Unclassified" bucket. */}
      {!!data?.unclassified && (
        <span className="warn-badge">{data.unclassified} instruments missing classification data</span>
      )}
    </div>
  );

  if (pnl.error) {
    return (
      <div>
        <h2>P&amp;L</h2>
        {controls}
        <p className="neg">{pnl.error}</p>
      </div>
    );
  }

  if (!data) {
    return (
      <div>
        <h2>P&amp;L</h2>
        <p>Loading…</p>
      </div>
    );
  }

  if (data.empty) {
    return (
      <div>
        <h2>P&amp;L</h2>
        {controls}
        <div className="card">
          {data.warnings.map((w, i) => <p key={i}>{w}</p>)}
        </div>
      </div>
    );
  }

  const p = data.period!;
  const r = data.reconciliation!;
  const snapped = p.actual_from !== p.requested_from || p.actual_to !== p.requested_to;

  return (
    <div>
      <h2>P&amp;L</h2>
      {controls}

      {snapped && (
        <p className="kpi-sub">
          Struck between imported NAV dates {p.actual_from} and {p.actual_to} ({p.snapshots} snapshots).
          You asked for {p.requested_from} → {p.requested_to}.
        </p>
      )}

      <div className="card">
        <table className="tbl">
          <thead>
            <tr>
              <th>Group</th>
              <th>Realized</th><th>Unrealized</th>
              <th>of which FX</th><th>Total</th>
            </tr>
          </thead>
          <tbody>
            {data.groups!.map((g) => (
              <Fragment key={g.key}>
                <tr onClick={() => setOpen({ ...open, [g.key]: !open[g.key] })} style={{ cursor: "pointer" }}>
                  <td>{open[g.key] ? "▾" : "▸"} {g.key}</td>
                  <td>{eur(g.realized)}</td>
                  <td>{eur(g.unrealized)}</td>
                  <td>{eur(g.fx)}</td>
                  <td>{eur(g.total)}</td>
                </tr>
                {open[g.key] && g.instruments.map((i) => (
                  <tr key={i.isin}>
                    <td style={{ paddingLeft: 24, color: "#64748b" }}>
                      {i.name}
                      {i.fx_split_imprecise && (
                        <span title="Partial sale after a mid-period purchase: the FX split for this instrument is approximate."> ⚠</span>
                      )}
                    </td>
                    <td>{eur(i.realized_price + i.realized_fx)}</td>
                    <td>{eur(i.unrealized_price + i.unrealized_fx)}</td>
                    <td>{eur(i.realized_fx + i.unrealized_fx)}</td>
                    <td>{eur(i.realized_price + i.unrealized_price + i.realized_fx + i.unrealized_fx)}</td>
                  </tr>
                ))}
              </Fragment>
            ))}
            <tr>
              <td><b>Total</b></td><td></td><td></td><td></td>
              <td><b>{eur(total)}</b></td>
            </tr>
          </tbody>
        </table>
      </div>

      <div className="card">
        <h3>Reconciliation</h3>
        <table className="tbl">
          <tbody>
            <tr><td>Investment P&amp;L</td><td>{eur(r.investment_pnl)}</td></tr>
            <tr><td>Cash and margin accounts</td><td>{eur(r.cash_and_margin)}</td></tr>
            <tr><td>Accrued fees</td><td>{eur(r.accrued_fees)}</td></tr>
            <tr><td>Provisions</td><td>{eur(r.provisions)}</td></tr>
            <tr><td>Dividend income</td><td>{eur(r.dividend_income)}</td></tr>
            <tr><td><b>Total P&amp;L</b></td><td><b>{eur(r.total_pnl)}</b></td></tr>
            <tr><td>AUM change</td><td>{eur(r.aum_change)}</td></tr>
            <tr><td>less subscriptions / redemptions</td><td>{eur(r.net_flows)}</td></tr>
            <tr>
              <td>Residual</td>
              <td className={r.within_tolerance ? "pos" : "neg"}>
                {eur(r.residual)}
                {r.gross > 0 && <> ({pct(Math.abs(r.residual) / r.gross)})</>}
                {r.within_tolerance ? " reconciled" : " above tolerance"}
              </td>
            </tr>
          </tbody>
        </table>
      </div>

      {data.warnings.map((w, i) => <span key={i} className="warn-badge">{w}</span>)}
    </div>
  );
}
