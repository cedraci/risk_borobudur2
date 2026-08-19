import { useState } from "react";
import {
  ApiError, getFlows, getImports, getPositions, getSettings, getRefs, putRef, putSettings, uploadFiles,
  type FileImportResult, type Settings,
} from "../api";
import BloombergPanel from "../components/BloombergPanel";
import FuturesContracts from "../components/FuturesContracts";
import PortfoliosAdmin from "../components/PortfoliosAdmin";
import ShareholderRegister from "../components/ShareholderRegister";
import Unavailable from "../components/Unavailable";
import { useFetch } from "../hooks";
import { usePortfolio, useReloadPortfolios } from "../PortfolioContext";
import { eur, num, pct } from "../fmt";

export default function DataPage() {
  const portfolio = usePortfolio();
  const reloadPortfolios = useReloadPortfolios();
  const [over, setOver] = useState(false);
  const [busy, setBusy] = useState(false);
  const [results, setResults] = useState<FileImportResult[] | null>(null);
  const [uploadErr, setUploadErr] = useState<string | null>(null);
  const [posDate, setPosDate] = useState<string | undefined>(undefined);

  const imports = useFetch(() => getImports(portfolio.id), [portfolio.id]);
  const positions = useFetch(() => getPositions(portfolio.id, posDate), [portfolio.id, posDate]);
  const settings = useFetch(() => getSettings(portfolio.id), [portfolio.id]);
  const refs = useFetch(() => getRefs(), []);

  async function doUpload(files: File[]) {
    if (files.length === 0) return;
    setBusy(true);
    setResults(null);
    setUploadErr(null);
    try {
      setResults(await uploadFiles(portfolio.id, files));
      imports.reload();
      positions.reload();
    } catch (e) {
      const ae = e as ApiError;
      setUploadErr(ae.detail ?? ae.message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div>
      <h2>Data</h2>

      <PortfoliosAdmin onChange={reloadPortfolios} />

      <div
        className={`card drop ${over ? "over" : ""}`}
        onDragOver={(e) => { e.preventDefault(); setOver(true); }}
        onDragLeave={() => setOver(false)}
        onDrop={(e) => {
          e.preventDefault();
          setOver(false);
          void doUpload(Array.from(e.dataTransfer.files));
        }}
      >
        <h3>Import — {portfolio.name}</h3>
        <p>{busy ? "Importing…" : "Drop files here (NAV Recap .xlsx, CACEIS HISINVLUX / HISTOVLLUX .csv) — CACEIS files auto-route to the portfolio mapped to their fund code."}</p>
        <input
          type="file"
          accept=".xlsx,.csv"
          multiple
          disabled={busy}
          onChange={(e) => void doUpload(Array.from(e.target.files ?? []))}
        />
        {positions.forbidden && <Unavailable reason={positions.forbidden} />}
        {uploadErr && <p className="neg">Upload failed: {uploadErr}</p>}
        {results && (
          <table className="tbl">
            <thead><tr><th>File</th><th>Kind</th><th>Portfolio</th><th>Result</th></tr></thead>
            <tbody>
              {results.map((r, i) => (
                <tr key={i}>
                  <td>{r.filename}</td>
                  <td>{r.kind ?? "—"}</td>
                  <td>{r.portfolio_name ?? "—"}</td>
                  <td>
                    {r.error ? (
                      <>
                        <span className="neg">{r.error}</span>
                        {r.error_rows && (
                          <table className="tbl"><tbody>
                            {r.error_rows.slice(0, 10).map((er, j) => (
                              <tr key={j}><td>{er.sheet}</td><td>row {er.row}</td><td>{er.message}</td></tr>
                            ))}
                          </tbody></table>
                        )}
                      </>
                    ) : r.outcome ? (
                      <>
                        <span className="pos">
                          {r.outcome.duplicate
                            ? "Already imported (identical file)."
                            : `Imported: ${r.outcome.nav_rows} NAV rows, ${r.outcome.positions} positions, ${r.outcome.dividends} dividends, ${r.outcome.operations} operations.` +
                              // Only a NAV Recap ever carries a dividends/operations
                              // journal (CACEIS CSV outcomes never set dividends or
                              // operations, so div_ops_replaced is always false for
                              // them too) — gate on kind so a CSV row is never
                              // mislabeled as "older file" when it never had a
                              // journal to replace in the first place.
                              (r.kind === "nav_recap" && !r.outcome.div_ops_replaced
                                ? " (older file: dividends/operations left untouched)"
                                : "")}
                        </span>
                        {r.outcome.warnings.map((w, j) => <p key={j} className="warn-badge">{w}</p>)}
                      </>
                    ) : null}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      <div className="card">
        <h3>Import history</h3>
        {imports.forbidden ? <Unavailable reason={imports.forbidden} /> : imports.error && <p className="neg">{imports.error}</p>}
        <table className="tbl">
          <thead><tr><th>File</th><th>NAV date</th><th>Imported at</th><th>Rows</th></tr></thead>
          <tbody>
            {(imports.data ?? []).map((r) => (
              <tr key={r.id}>
                <td>{r.filename}</td>
                <td>{r.nav_date}</td>
                <td>{new Date(r.imported_at).toLocaleString("fr-FR")}</td>
                <td>{Object.entries(r.row_counts).map(([k, v]) => `${k}: ${v}`).join(", ")}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <SettingsCard settings={settings.data} forbidden={settings.forbidden} onSaved={settings.reload} />

      <RefsCard rows={refs.data} forbidden={refs.forbidden} advMaxAgeDays={settings.data?.adv_max_age_days ?? 7} onSaved={refs.reload} />

      <ShareholderRegister />

      <FuturesContracts />

      <BloombergPanel />

      <div className="card">
        <h3>Portfolio snapshot</h3>
        <div className="controls">
          <label>Date:{" "}
            <select value={positions.data?.date ?? ""} onChange={(e) => setPosDate(e.target.value || undefined)}>
              {(positions.data?.dates ?? []).map((d) => <option key={d} value={d}>{d}</option>)}
            </select>
          </label>
        </div>
        <table className="tbl">
          <thead><tr><th>Type</th><th>ISIN</th><th>Name</th><th>Ccy</th><th>Qty</th><th>Price</th><th>Valuation €</th><th>Weight</th></tr></thead>
          <tbody>
            {(positions.data?.rows ?? []).map((p, i) => (
              <tr key={i}>
                <td>{p.asset_type}</td><td>{p.isin}</td><td>{p.name ?? ""}</td><td>{p.currency ?? ""}</td>
                <td>{num(p.quantity, 0)}</td><td>{num(p.price)}</td><td>{eur(p.valuation_eur)}</td>
                <td>{pct(p.weight)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function SettingsCard({ settings, forbidden, onSaved }: { settings: Settings | null; forbidden?: string; onSaved: () => void }) {
  const portfolio = usePortfolio();
  const [draft, setDraft] = useState<Settings | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const flows = useFetch(() => getFlows(portfolio.id), [portfolio.id]);
  const s = draft ?? settings;
  if (forbidden) return <div className="card"><h3>Settings</h3><Unavailable reason={forbidden} /></div>;
  if (!s) return <div className="card"><h3>Settings</h3><p>Loading…</p></div>;
  const set = (patch: Partial<Settings>) => setDraft({ ...s, ...patch });
  const worst20 = flows.data && flows.data.status !== "unavailable"
    ? flows.data.worst?.find((w) => w.window === 20)?.pct_of_nav ?? null
    : null;
  return (
    <div className="card">
      <h3>Settings</h3>
      <div className="controls">
        <label>Risk-free %/yr <input type="number" step="0.1" value={(s.risk_free_rate * 100).toFixed(1)}
          onChange={(e) => set({ risk_free_rate: Number(e.target.value) / 100 })} /></label>
        <label>VaR conf % <input type="number" step="0.5" value={(s.var_confidence * 100).toFixed(1)}
          onChange={(e) => set({ var_confidence: Number(e.target.value) / 100 })} /></label>
        <label>Horizon d <input type="number" value={s.var_horizon_days}
          onChange={(e) => set({ var_horizon_days: Number(e.target.value) })} /></label>
        <label>Window d <input type="number" value={s.var_window_days}
          onChange={(e) => set({ var_window_days: Number(e.target.value) })} /></label>
        <label>VaR limit % <input type="number" step="1" value={(s.var_limit * 100).toFixed(0)}
          onChange={(e) => set({ var_limit: Number(e.target.value) / 100 })} /></label>
        <label>Short-DD max days <input type="number" value={s.short_dd_max_days}
          onChange={(e) => set({ short_dd_max_days: Number(e.target.value) })} /></label>
        <label>Redemption stress % <input type="number" step="5" value={(s.redemption_shock * 100).toFixed(0)}
          onChange={(e) => set({ redemption_shock: Number(e.target.value) / 100 })} /></label>
        {worst20 != null && (
          <span className="kpi-sub">
            Observed worst 20-day outflow: <strong>{pct(worst20)}</strong>
            <button type="button" disabled={worst20 <= 0}
              onClick={() => set({ redemption_shock: worst20 })}>
              Adopt as fixed shock
            </button>
          </span>
        )}
        <label>Participation rate % of ADV <input type="number" step="5" value={(s.participation_rate * 100).toFixed(0)}
          onChange={(e) => set({ participation_rate: Number(e.target.value) / 100 })} /></label>
        <label>ADV stress factor % <input type="number" step="5" value={(s.adv_stress_factor * 100).toFixed(0)}
          onChange={(e) => set({ adv_stress_factor: Number(e.target.value) / 100 })} /></label>
        <label>Liquidity horizon days <input type="number" value={s.liquidity_horizon_days}
          onChange={(e) => set({ liquidity_horizon_days: Number(e.target.value) })} /></label>
        <label>Settlement deadline days <input type="number" value={s.settlement_deadline_days}
          onChange={(e) => set({ settlement_deadline_days: Number(e.target.value) })} /></label>
        <label>ADV max age days <input type="number" value={s.adv_max_age_days}
          onChange={(e) => set({ adv_max_age_days: Number(e.target.value) })} /></label>
        <label>Flow lookback days <input type="number" value={s.flow_lookback_days}
          onChange={(e) => set({ flow_lookback_days: Number(e.target.value) })} /></label>
        <button disabled={!draft} onClick={() => {
          putSettings(portfolio.id, s).then(() => { setDraft(null); setMsg("Saved."); onSaved(); },
            (e) => setMsg(`Error: ${e.detail ?? e.message}`));
        }}>Save</button>
        {msg && <span>{msg}</span>}
      </div>
      <h4>Liquidity default days by asset type</h4>
      <div className="controls">
        {Object.entries(s.liquidity_default_days).map(([atype, days]) => (
          <label key={atype}>{atype}{" "}
            <input type="number" min={0} step={0.5} value={days} onChange={(e) =>
              set({ liquidity_default_days: { ...s.liquidity_default_days, [atype]: Number(e.target.value) } })} />
          </label>
        ))}
      </div>
    </div>
  );
}

function RefsCard({ rows, forbidden, advMaxAgeDays, onSaved }: { rows: import("../api").RefRow[] | null; forbidden?: string; advMaxAgeDays: number; onSaved: () => void }) {
  const [msg, setMsg] = useState<string | null>(null);
  const [drafts, setDrafts] = useState<Record<string, Partial<import("../api").RefBody>>>({});
  if (forbidden) return <div className="card"><h3>Reference data</h3><Unavailable reason={forbidden} /></div>;
  if (!rows) return <div className="card"><h3>Reference data</h3><p>Loading…</p></div>;

  const draftFor = (code: string) => drafts[code] ?? {};
  const setDraft = (code: string, patch: Partial<import("../api").RefBody>) =>
    setDrafts((d) => ({ ...d, [code]: { ...draftFor(code), ...patch } }));

  async function save(r: import("../api").RefRow) {
    const d = draftFor(r.code);
    const body: import("../api").RefBody = {
      issuer_group: d.issuer_group !== undefined ? d.issuer_group : r.issuer_group_override,
      liquidity_days: d.liquidity_days !== undefined ? d.liquidity_days : r.days_override,
      adv_eligible: d.adv_eligible !== undefined ? d.adv_eligible : r.adv_eligible,
      bond_coupon_pct: d.bond_coupon_pct !== undefined ? d.bond_coupon_pct : r.bond_coupon_pct,
      bond_maturity: d.bond_maturity !== undefined ? d.bond_maturity : r.bond_maturity,
      bond_coupon_freq: d.bond_coupon_freq !== undefined ? d.bond_coupon_freq : r.bond_coupon_freq,
    };
    try {
      await putRef(r.code, body);
      setDrafts((prev) => { const rest = { ...prev }; delete rest[r.code]; return rest; });
      setMsg(`Saved ${r.code}.`);
      onSaved();
    } catch (e) {
      const ae = e as ApiError;
      setMsg(`Error: ${ae.detail ?? ae.message}`);
    }
  }

  async function reset(r: import("../api").RefRow) {
    try {
      await putRef(r.code, { issuer_group: null, liquidity_days: null, adv_eligible: null, bond_coupon_pct: null, bond_maturity: null, bond_coupon_freq: null });
      setDrafts((prev) => { const rest = { ...prev }; delete rest[r.code]; return rest; });
      setMsg(`Reset ${r.code} to defaults.`);
      onSaved();
    } catch (e) {
      const ae = e as ApiError;
      setMsg(`Error: ${ae.detail ?? ae.message}`);
    }
  }

  return (
    <div className="card">
      <h3>Reference data</h3>
      <p className="kpi-sub">Shared across all portfolios.</p>
      <p className="kpi-sub">
        Issuer groups drive the concentration checks (merge connected issuers by giving them the same group);
        bond fields drive YTM/duration. Blank override = default.
      </p>
      <p className="kpi-sub">
        Days-to-liquidate drives the liquidity view; bond fields drive YTM and duration.
        Blank days = the asset-type default. ADV, market place and the bond schedule are
        maintained by the depositary feed and Bloomberg and cannot be edited here.
      </p>
      <p className="kpi-sub">
        Because the depositary's feed overwrites coupon and maturity on every import, editing
        those fields for a CACEIS-sourced instrument is effectively futile — the next import
        restores the depositary's value.
      </p>
      {msg && <p>{msg}</p>}
      <table className="tbl">
        <thead>
          <tr>
            <th>Code</th><th>Name</th><th>Type</th><th>Issuer group</th><th>Days</th>
            <th>ADV 30d</th><th>ADV as-of</th><th>Market place</th><th>ADV eligible</th>
            <th>Coupon %</th><th>Maturity</th><th>Freq</th><th></th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => {
            const d = draftFor(r.code);
            const dirty = Object.keys(d).length > 0;
            const overridden = r.issuer_group_override != null || r.days_override != null || r.adv_eligible != null;
            const advStale = r.adv_asof != null &&
              (Date.now() - new Date(r.adv_asof).getTime()) / 86_400_000 > advMaxAgeDays;
            const eligibleValue = d.adv_eligible !== undefined ? d.adv_eligible : r.adv_eligible;
            return (
              <tr key={r.code}>
                <td>{r.code}</td>
                <td>{r.name}</td>
                <td>{r.asset_type}</td>
                <td>
                  <input
                    value={d.issuer_group !== undefined ? (d.issuer_group ?? "") : (r.issuer_group_override ?? "")}
                    placeholder={r.effective_issuer_group}
                    onChange={(e) => setDraft(r.code, { issuer_group: e.target.value || null })}
                  />
                </td>
                <td>
                  <input
                    type="number" min={0} step={0.5}
                    placeholder={`default (${r.effective_days})`}
                    value={d.liquidity_days !== undefined ? (d.liquidity_days ?? "") : (r.days_override ?? "")}
                    onChange={(e) => setDraft(r.code, { liquidity_days: e.target.value === "" ? null : Number(e.target.value) })}
                  />
                </td>
                <td>{num(r.adv_30d, 0)}</td>
                <td>
                  {r.adv_asof ?? "—"}
                  {advStale && <span className="warn-badge">stale</span>}
                </td>
                <td>{r.market_place_name ?? "—"}</td>
                <td>
                  <select
                    value={eligibleValue === null || eligibleValue === undefined ? "" : String(eligibleValue)}
                    onChange={(e) => setDraft(r.code, {
                      adv_eligible: e.target.value === "" ? null : e.target.value === "true",
                    })}
                  >
                    <option value="">derived</option>
                    <option value="true">always</option>
                    <option value="false">never</option>
                  </select>
                </td>
                {r.is_bond ? (
                  <>
                    <td><input type="number" step="0.001" style={{ width: 70 }}
                      value={d.bond_coupon_pct !== undefined ? (d.bond_coupon_pct ?? "") : (r.bond_coupon_pct ?? "")}
                      onChange={(e) => setDraft(r.code, { bond_coupon_pct: e.target.value === "" ? null : Number(e.target.value) })} /></td>
                    <td><input type="date"
                      value={d.bond_maturity !== undefined ? (d.bond_maturity ?? "") : (r.bond_maturity ?? "")}
                      onChange={(e) => setDraft(r.code, { bond_maturity: e.target.value || null })} /></td>
                    <td>
                      <select
                        value={d.bond_coupon_freq !== undefined ? (d.bond_coupon_freq ?? "") : (r.bond_coupon_freq ?? "")}
                        onChange={(e) => setDraft(r.code, { bond_coupon_freq: e.target.value === "" ? null : Number(e.target.value) })}
                      >
                        <option value="">—</option>
                        <option value="1">annual</option>
                        <option value="2">semi-annual</option>
                        <option value="4">quarterly</option>
                        <option value="12">monthly</option>
                      </select>
                    </td>
                  </>
                ) : (
                  <><td>—</td><td>—</td><td>—</td></>
                )}
                <td>
                  <button disabled={!dirty} onClick={() => void save(r)}>Save</button>
                  {(overridden || r.bond_coupon_pct != null) && (
                    <button onClick={() => void reset(r)}>Reset</button>
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
