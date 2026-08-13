import { useState } from "react";
import { getConcentration, getFlows, getLiquidity, getRates, type Check, type CheckStatus, type Scenario } from "../api";
import EChart from "../components/EChart";
import { eur, num, pct } from "../fmt";
import { useFetch } from "../hooks";
import { usePortfolio } from "../PortfolioContext";

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

const SCENARIO_LABELS: Record<Scenario["key"], string> = {
  top5: "Top 5 holders",
  fixed: "Fixed shock",
  hybrid_top5: "Top 5 holders, stressed ADV",
  hybrid_fixed: "Fixed shock, stressed ADV",
};

export default function LimitsPage() {
  const portfolio = usePortfolio();
  const [date, setDate] = useState<string | undefined>(undefined);
  const conc = useFetch(() => getConcentration(portfolio.id, date), [portfolio.id, date]);
  const liq = useFetch(() => getLiquidity(portfolio.id, date), [portfolio.id, date]);
  const rates = useFetch(() => getRates(portfolio.id, date), [portfolio.id, date]);
  const flows = useFetch(() => getFlows(portfolio.id), [portfolio.id]);
  const [scenario, setScenario] = useState<string>("fixed");

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
          <p className="kpi-sub">
            Participation rate {pct(liq.data.params.participation_rate)} of ADV · stress factor{" "}
            {pct(liq.data.params.adv_stress_factor)} · horizon {liq.data.params.liquidity_horizon_days}{" "}
            {liq.data.params.day_unit} · settlement deadline {liq.data.params.settlement_deadline_days}{" "}
            {liq.data.params.day_unit}.
          </p>

          {liq.data.coverage && liq.data.asset ? (
            <>
              <p className="kpi-sub">
                ADV measured on <strong>{pct(liq.data.coverage.adv_pct_of_nav)}</strong> of NAV.{" "}
                {liq.data.coverage.fallbacks.length} position(s) on the assumed-days fallback
                {liq.data.coverage.fallbacks.length > 0 && (
                  <details style={{ display: "inline" }}>
                    <summary style={{ display: "inline", cursor: "pointer" }}>details</summary>
                    <ul>
                      {liq.data.coverage.fallbacks.map((f, i) => (
                        <li key={i}>{f.code}: {f.reason}</li>
                      ))}
                    </ul>
                  </details>
                )}
                .
              </p>
              <p className="kpi-sub">
                The stress factor applies only to positions measured from traded volume.
                A position on the assumed-days fallback is already an assumption and is not
                re-stressed, so the stressed profile moves {pct(liq.data.coverage.adv_pct_of_nav)} of the fund.
              </p>
              <p className="kpi-sub">
                {liq.data.coverage.coupon_gaps.length} bond(s) contribute no coupon inflow (frequency
                could not be resolved from INVJCPLUX or accrued-interest inference)
                {liq.data.coverage.coupon_gaps.length > 0 && (
                  <details style={{ display: "inline" }}>
                    <summary style={{ display: "inline", cursor: "pointer" }}>details</summary>
                    <ul>
                      {liq.data.coverage.coupon_gaps.map((f, i) => (
                        <li key={i}>{f.code}: {f.reason}</li>
                      ))}
                    </ul>
                  </details>
                )}
                .
              </p>
              {liq.data.coverage.register.stale && (
                <p className="warn-badge">
                  Shareholder register is stale (as of {liq.data.coverage.register.as_of ?? "unknown"}).
                </p>
              )}

              <EChart option={{
                tooltip: { trigger: "axis", valueFormatter: (x) => pct(x as number) },
                legend: { data: ["Normal", "Stressed", "Normal cumulative", "Stressed cumulative"] },
                xAxis: { type: "category", data: liq.data.asset.normal.buckets.map((b) => BUCKET_LABELS[b.bucket] ?? b.bucket) },
                yAxis: { type: "value", axisLabel: { formatter: (x: number) => pct(x, 0) } },
                series: [
                  { type: "bar", name: "Normal", color: "#1d64c2", data: liq.data.asset.normal.buckets.map((b) => b.weight) },
                  { type: "bar", name: "Stressed", color: "#b26a00", data: liq.data.asset.stressed.buckets.map((b) => b.weight) },
                  { type: "line", name: "Normal cumulative", color: "#2e7d32", data: liq.data.asset.normal.cumulative.map((b) => b.weight) },
                  { type: "line", name: "Stressed cumulative", color: "#c62828", data: liq.data.asset.stressed.cumulative.map((b) => b.weight) },
                ],
                grid: { left: 55, right: 40, top: 40, bottom: 30 },
              }} />
            </>
          ) : (
            <p className="kpi-sub">No positions in this snapshot.</p>
          )}

          {liq.data.scenarios.length > 0 && (
            <>
              <table className="tbl">
                <thead>
                  <tr>
                    <th>Scenario</th><th>Required</th><th>Waterfall days</th><th>Slice days</th><th>Unmet</th><th>Status</th>
                  </tr>
                </thead>
                <tbody>
                  {liq.data.scenarios.map((s) => (
                    <tr
                      key={s.key}
                      onClick={() => setScenario(s.key)}
                      style={{ cursor: "pointer", fontWeight: scenario === s.key ? "bold" : undefined }}
                    >
                      <td>{SCENARIO_LABELS[s.key]}</td>
                      {s.status === "unavailable" ? (
                        <td colSpan={4} className="warn-badge">{s.reason}</td>
                      ) : (
                        <>
                          <td>{eur(s.required_eur ?? null)}</td>
                          <td>{s.waterfall?.days ?? "> horizon"}</td>
                          <td>{num(s.slice_days ?? null, 1)}</td>
                          <td>{eur(s.waterfall?.unmet_eur ?? null)}</td>
                        </>
                      )}
                      <td>
                        <span className={s.status === "ok" ? "pos" : s.status === "breach" ? "neg" : "warn-badge"}>
                          {s.status.toUpperCase()}
                        </span>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>

              {(() => {
                const sel = liq.data.scenarios.find((s) => s.key === scenario);
                if (!sel || sel.status === "unavailable" || !sel.curve) return null;
                return (
                  <EChart option={{
                    tooltip: { trigger: "axis", valueFormatter: (x) => eur(x as number) },
                    xAxis: { type: "category", data: sel.curve.map((c) => c.day) },
                    yAxis: { type: "value", axisLabel: { formatter: (x: number) => eur(x) } },
                    series: [{
                      type: "line", name: "Available", color: "#1d64c2",
                      data: sel.curve.map((c) => c.available_eur),
                      markLine: {
                        silent: true, symbol: "none",
                        lineStyle: { color: "#c62828", type: "dashed" },
                        data: [
                          { yAxis: sel.required_eur, label: { formatter: "Required" } },
                          { xAxis: liq.data.params.settlement_deadline_days, label: { formatter: "Deadline" } },
                        ],
                      },
                    }],
                    grid: { left: 70, right: 40, top: 40, bottom: 30 },
                  }} />
                );
              })()}
            </>
          )}

          {flows.data && (flows.data.status === "unavailable" ? (
            <p className="kpi-sub">
              Observed outflows: {flows.data.reason}. Load JOURSRLUX files on the Data page.
              {flows.data.dates_excluded_no_nav > 0 &&
                ` (${flows.data.dates_excluded_no_nav} date(s) excluded for missing NAV)`}
            </p>
          ) : (
            <p className="kpi-sub">
              Worst observed 20-day outflow{" "}
              <strong>{pct(flows.data.worst?.find((w) => w.window === 20)?.pct_of_nav ?? 0)}</strong>{" "}
              of NAV over {flows.data.n_observations} observations, {flows.data.from} to {flows.data.to}.
              Configured shock is {pct(liq.data.params.redemption_shock, 0)}.
              {flows.data.dates_excluded_no_nav > 0 &&
                ` (${flows.data.dates_excluded_no_nav} date(s) excluded for missing NAV)`}
            </p>
          ))}

          <p className="kpi-sub">
            Negative positions (payables, short cash): {pct(liq.data.negative_memo)}
            {" "}({eur(liq.data.negative_memo_eur)}) — reduces availability from day one, not netted elsewhere.
          </p>
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
            Signed as profit and loss: a negative sensitivity means net assets fall if yields rise 100bp
            (the book is long rates); a positive one means they rise. Shown as “–” when the snapshot’s AUM
            is unknown.
          </p>

          <h4>Bond futures</h4>
          {rates.data.futures_no_spec.length > 0 && (
            <p className="warn-badge">
              {rates.data.futures_no_spec.length} future(s) held with no contract spec at all, so excluded
              from the DV01 above: {rates.data.futures_no_spec.join(", ")}. Re-upload the NAV Recap on the
              Data page to seed their specs, then confirm them.
            </p>
          )}
          {rates.data.futures_missing_any && (
            <p className="warn-badge">
              Some bond futures are missing CTD analytics for this snapshot date and are excluded from DV01 —
              upload the weekly CTD companion file on the Data page to include them.
            </p>
          )}
          {rates.data.futures.length === 0 ? (
            // Never claim absence when the reason is that nothing could be
            // evaluated: the fund may hold bond futures that simply have no spec.
            <p className="kpi-sub">
              {rates.data.futures_no_spec.length > 0
                ? "No bond-future DV01 could be computed for this snapshot — see the note above."
                : "No interest-rate futures in this snapshot."}
            </p>
          ) : (
            <table className="tbl">
              <thead>
                <tr><th>Future</th><th>Qty</th><th>Price</th><th>Point value</th><th>CTD ISIN</th><th>Mod. duration</th><th>Conv. factor</th><th>DV01 €</th></tr>
              </thead>
              <tbody>
                {rates.data.futures.map((f, i) => f.missing ? (
                  <tr key={i}><td>{f.name || f.ticker}</td><td colSpan={7} className="neg">missing CTD analytics for this date</td></tr>
                ) : (
                  <tr key={i}>
                    <td>{f.name || f.ticker}</td>
                    <td>{num(f.qty, 0)}</td>
                    <td>{num(f.price)}</td>
                    <td>{num(f.point_value)}</td>
                    <td>{f.ctd_isin}</td>
                    <td>{num(f.ctd_mod_duration)}</td>
                    <td>{num(f.conversion_factor)}</td>
                    <td>{eur(f.dv01_eur ?? null)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      )}
    </div>
  );
}
