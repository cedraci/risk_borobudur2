export interface NavPoint { date: string; value: number }
export interface NavRow { date: string; aum: number; shares: number; nav: number }
export interface VarEs { var: number; es: number }
export interface VarBlock {
  confidence: number; horizon_days: number; window_days: number;
  historical: VarEs | null; gaussian: VarEs | null; cornish_fisher: VarEs | null;
  limit: number; utilization: number | null; var_eur: number | null;
}
export interface Summary {
  empty: boolean; as_of: string | null; nav: number | null; aum: number | null;
  ytd: number | null; vol_1y: number | null; vol_inception: number | null;
  ann_return_1y: number | null; yield_vol_1y: number | null; sharpe_1y: number | null;
  max_drawdown: number | null; var_ucits: VarBlock | null; warnings: string[];
}
export interface Rolling { empty: boolean; window: number; vol: NavPoint[]; sharpe: NavPoint[]; yield_vol: NavPoint[] }
export interface Episode { peak_date: string; trough_date: string; depth: number; duration_days: number; recovery_date: string | null }
export interface Drawdowns { empty: boolean; underwater: NavPoint[]; yearly: { year: number; max_drawdown: number }[]; top_short: Episode[]; overall_max: number; max_days: number }
export interface PeriodReturn { year: number; period: number; value: number }
export interface Calendar { empty: boolean; monthly: PeriodReturn[]; quarterly: PeriodReturn[]; annual: PeriodReturn[] }
export interface VarResp {
  empty: boolean; confidence: number; horizon_days: number; window_days: number;
  methods: VarBlock | null; rolling: NavPoint[]; breaches: NavPoint[]; limit: number; warnings: string[];
}
export interface PositionRecord {
  nav_date: string; asset_type: string; isin: string; name: string | null; currency: string | null;
  quantity: number | null; avg_cost: number | null; price: number | null; valuation_ccy: number | null;
  accrued_interest: number | null; fx_rate: number | null; valuation_eur: number | null;
  weight: number | null; ticker: string | null;
}
export interface Positions { dates: string[]; date: string | null; rows: PositionRecord[] }
export interface ImportRec { id: number; filename: string; nav_date: string; imported_at: string; row_counts: Record<string, number> }
export interface ImportOutcome {
  import_id: number; duplicate: boolean; nav_rows: number; positions: number; dividends: number;
  operations: number; div_ops_replaced: boolean; warnings: string[];
}
export interface Settings {
  risk_free_rate: number; var_confidence: number; var_horizon_days: number;
  var_window_days: number; var_limit: number; short_dd_max_days: number;
  liquidity_default_days: Record<string, number>; redemption_shock: number;
  participation_rate: number; adv_stress_factor: number; liquidity_horizon_days: number;
  settlement_deadline_days: number; adv_max_age_days: number; flow_lookback_days: number;
}
export interface RowError { sheet: string; row: number; message: string }

export class ApiError extends Error {
  status: number;
  detail?: string;
  rows?: RowError[];
  constructor(status: number, message: string, detail?: string, rows?: RowError[]) {
    super(message);
    this.status = status;
    this.detail = detail;
    this.rows = rows;
  }
}

export interface ReqOptions {
  /** A 401 normally means "the session is gone" and fires `borobudur:unauthenticated` so
   * App.tsx drops back to the login screen. `POST /api/login` also 401s on ordinary bad
   * credentials — that's an expected outcome of the login form itself, not a sign a
   * previously-valid session died, so `auth.ts`'s `login()` passes this to opt out.
   * Declared explicitly by the one caller that needs it rather than inferred from the URL,
   * so the exemption can't silently drift out of sync with which endpoint it's for. */
  suppressUnauthenticatedEvent?: boolean;
}

/** Exported so `auth.ts` (login/logout/me) shares the same fetch, error-body-mapping and
 * 401 handling as every other endpoint here, instead of duplicating it. */
export async function req<T>(url: string, init?: RequestInit, opts?: ReqOptions): Promise<T> {
  const res = await fetch(url, init);
  if (!res.ok) {
    let detail: string | undefined, rows: RowError[] | undefined;
    try {
      // A 403 body is `{title, status, detail, domain, action, portfolio_id}` (see
      // crates/server/src/error.rs) — `detail` is the human-readable denial reason,
      // which callers render via the same <Unavailable/> treatment as a component-level
      // `status: "unavailable"` marker.
      const body = await res.json();
      detail = body.detail; rows = body.rows;
    } catch { /* non-JSON error body */ }
    if (res.status === 401 && !opts?.suppressUnauthenticatedEvent) {
      // Global signal: the session is gone (never logged in, expired, or logged out
      // elsewhere). App.tsx listens for this to drop back to the login screen without
      // losing the current URL.
      window.dispatchEvent(new Event("borobudur:unauthenticated"));
    }
    throw new ApiError(res.status, `${res.status} ${res.statusText}`, detail, rows);
  }
  if (res.status === 204) return undefined as T;
  return res.json() as Promise<T>;
}

export interface Portfolio {
  id: number; name: string; kind: "ucits" | "mandate"; archived: boolean;
  latest_nav_date: string | null;
}
export const getPortfolios = () => req<Portfolio[]>("/api/portfolios");
export const createPortfolio = (name: string, kind: "ucits" | "mandate") =>
  req<Portfolio>("/api/portfolios", {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ name, kind }),
  });
export const updatePortfolio = (id: number, name: string, archived: boolean) =>
  req<Portfolio>(`/api/portfolios/${id}`, {
    method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ name, archived }),
  });

export const getSummary = (pid: number) => req<Summary>(`/api/portfolios/${pid}/metrics/summary`);
export const getNav = (pid: number) => req<NavRow[]>(`/api/portfolios/${pid}/nav`);
export const getRolling = (pid: number, window: number) =>
  req<Rolling>(`/api/portfolios/${pid}/metrics/rolling?window=${window}`);
export const getDrawdowns = (pid: number) => req<Drawdowns>(`/api/portfolios/${pid}/metrics/drawdowns`);
export const getCalendar = (pid: number) => req<Calendar>(`/api/portfolios/${pid}/metrics/calendar`);
export const getVar = (pid: number, p: { confidence: number; horizon: number; window: number }) =>
  req<VarResp>(`/api/portfolios/${pid}/metrics/var?confidence=${p.confidence}&horizon=${p.horizon}&window=${p.window}`);
export const getPositions = (pid: number, date?: string) =>
  req<Positions>(`/api/portfolios/${pid}/positions${date ? `?date=${date}` : ""}`);
export const getImports = (pid: number) => req<ImportRec[]>(`/api/portfolios/${pid}/imports`);
export const getSettings = (pid: number) => req<Settings>(`/api/portfolios/${pid}/settings`);
export const putSettings = (pid: number, s: Settings) =>
  req<Settings>(`/api/portfolios/${pid}/settings`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(s) });
export interface FileImportResult {
  filename: string;
  kind: string | null;
  portfolio_id: number | null;
  portfolio_name: string | null;
  outcome: ImportOutcome | null;
  error: string | null;
  error_rows: { sheet: string; row: number; message: string }[] | null;
}
export const uploadFiles = (pid: number, files: File[]) => {
  const fd = new FormData();
  for (const f of files) fd.append("file", f);
  return req<FileImportResult[]>(`/api/portfolios/${pid}/imports`, { method: "POST", body: fd });
};

export interface PortfolioCode { portfolio_id: number; source: string; code: string }
export const getCodes = (pid: number) => req<PortfolioCode[]>(`/api/portfolios/${pid}/codes`);
export const putCodes = (pid: number, codes: { source: string; code: string }[]) =>
  req<PortfolioCode[]>(`/api/portfolios/${pid}/codes`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(codes) });

export interface Shareholder { id?: number; label: string; pct_of_nav: number; as_of: string }
export const getShareholders = (pid: number) => req<Shareholder[]>(`/api/portfolios/${pid}/shareholders`);
export const putShareholders = (pid: number, rows: Shareholder[]) =>
  req<Shareholder[]>(`/api/portfolios/${pid}/shareholders`, {
    method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(rows),
  });

export const advRequestUrl = "/api/bloomberg/adv-request";
export const getAdvDue = () => req<{ due: number; held: number }>("/api/bloomberg/adv-due");

export type Bucket = "d1" | "d2_7" | "d8_30" | "d30p";
// "unavailable": Reference (issuer-group/liquidity overrides) was denied,
// so the check/row was computed without them and its own status is not
// trustworthy — the server stamps every check and row unavailable in that
// case rather than leaving a possibly-wrong "ok"/"watch"/"breach" beside
// `issuer_overrides`. Never render this as a pass.
export type CheckStatus = "ok" | "watch" | "breach" | "unavailable";
export interface CheckRow { group: string; weight: number; status: CheckStatus }
export interface Check { check: string; scope_label: string; limit: number; rows: CheckRow[]; status: CheckStatus }
export interface AuthzMarker { status: "ok" | "unavailable"; reason?: string }
export interface Concentration {
  dates: string[]; date: string | null; checks: Check[]; excluded_note: string;
  issuer_overrides: AuthzMarker;
}

export interface BucketWeight { bucket: Bucket; weight: number }
export interface AssetProfile { buckets: BucketWeight[]; cumulative: BucketWeight[] }

export interface LiquidityParams {
  participation_rate: number; adv_stress_factor: number;
  liquidity_horizon_days: number; settlement_deadline_days: number;
  adv_max_age_days: number; redemption_shock: number; day_unit: string;
}

export interface LiquidityCoverage {
  adv_pct_of_nav: number;
  fallbacks: { code: string; reason: string }[];
  coupon_gaps: { code: string; reason: string }[];
  register: { count: number; as_of: string | null; stale: boolean };
}

export interface Scenario {
  key: "top5" | "fixed" | "hybrid_top5" | "hybrid_fixed";
  status: "ok" | "breach" | "unavailable";
  reason?: string;
  required_eur?: number;
  required_pct?: number;
  register_count?: number;
  waterfall?: { days: number | null; unmet_eur: number };
  slice_days?: number | null;
  residual?: { slow_share_before: number; slow_share_after: number };
  curve?: { day: number; available_eur: number }[];
}

export interface Liquidity {
  dates: string[]; date: string | null; nav: number | null;
  params: LiquidityParams;
  coverage: LiquidityCoverage | null;
  asset: { normal: AssetProfile; stressed: AssetProfile } | null;
  scenarios: Scenario[];
  negative_memo: number; negative_memo_eur: number;
  // Reference denied -> every scenario's own status/waterfall/slice_days/
  // residual is already stamped unavailable/null server-side; this marker
  // just names the reason.
  issuer_overrides: AuthzMarker;
  nav_status: AuthzMarker;
}

export interface FlowStats {
  status?: "unavailable"; reason?: string;
  n_observations: number; from?: string; to?: string;
  worst?: { window: number; pct_of_nav: number }[];
  dates_excluded_no_nav: number;
}
export interface BondRow {
  code: string; name: string | null; missing: boolean;
  // Reference denied -> this bond's own `missing: true` carries a reason,
  // distinguishing "could not check" from "genuinely lacks coupon/maturity
  // data" (which sets `missing` alone, with no `status`/`reason`).
  status?: "unavailable"; reason?: string;
  coupon_pct?: number; maturity?: string; freq?: number; price?: number;
  ytm?: number; mod_duration?: number; dv01_eur?: number; weight?: number;
}
export interface FutureRow {
  ticker: string; name: string; missing: boolean; qty: number | null; price: number | null;
  point_value: number | null; ctd_isin?: string; ctd_mod_duration?: number;
  conversion_factor?: number; dv01_eur?: number; curve?: string | null;
}
export interface Rates {
  dates: string[]; date: string | null; bonds: BondRow[]; futures: FutureRow[];
  // Signed: negative means a +100bp move costs NAV (a book long rates),
  // positive means it gains. Null when the snapshot's AUM is unknown, OR
  // (see `reference_status`) when Reference is denied — a denial must never
  // read as a confident "flat" zero.
  total_dv01_eur: number | null; nav_sensitivity_100bp: number | null;
  missing_any: boolean; futures_missing_any: boolean;
  // Tickers of futures held with no contract spec at all, so excluded from the DV01.
  futures_no_spec: string[];
  reference_status: AuthzMarker;
}
export interface MethodSummary {
  exceptions: number; n: number; zone: "green" | "yellow" | "red";
  kupiec_lr: number | null; kupiec_p: number | null; reject: boolean;
}
export interface BacktestPoint {
  date: string; ret: number; var_hist: number | null; var_gauss: number | null; var_cf: number | null;
  exc_hist: boolean; exc_gauss: boolean; exc_cf: boolean;
}
export interface Backtest {
  window: number; confidence: number; horizon_days: number; n_points: number; insufficient: boolean;
  methods: { historical: MethodSummary; gaussian: MethodSummary; cornish_fisher: MethodSummary };
  series: BacktestPoint[];
}
export interface RefRow {
  code: string; name: string; asset_type: string;
  effective_issuer_group: string; issuer_group_override: string | null;
  effective_days: number; days_override: number | null;
  adv_30d: number | null; adv_asof: string | null; adv_eligible: boolean | null;
  market_place_name: string | null;
  bond_coupon_pct: number | null; bond_maturity: string | null; bond_coupon_freq: number | null;
  is_bond: boolean;
}
export interface RefBody {
  issuer_group: string | null; liquidity_days: number | null; adv_eligible: boolean | null;
  bond_coupon_pct: number | null; bond_maturity: string | null; bond_coupon_freq: number | null;
}

export type Category = "equity" | "interest_rate" | "fx" | "credit" | "commodity" | "other";
export interface CategoryTotals { category: Category; long_pct: number; short_pct: number; gross_pct: number }
export interface ExposureRow {
  ticker: string; name: string; currency: string; category: Category;
  qty: number | null; price: number | null; point_value: number | null;
  notional_ccy: number | null; notional_eur: number | null;
  pct_nav: number | null; spec_missing: boolean;
  // The workbook row itself was incomplete (no quantity or no price), as
  // distinct from spec_missing, which is about the contract spec.
  inputs_missing: boolean;
  // The contract spec exists but is unconfirmed, so this row's figures are provisional.
  unconfirmed: boolean;
}
export interface Derivatives {
  dates: string[]; date: string | null;
  // Null when Nav is denied (see `nav_status`) — distinct from a fund that
  // genuinely has no NAV row yet, which reports a real `0`.
  aum: number | null;
  // Null when Reference is denied (see `reference_status`): every future's
  // `category` falls back to "other" in that case, so the breakdown would
  // otherwise read as a real "all other" result rather than "could not
  // classify".
  categories: CategoryTotals[] | null;
  total: CategoryTotals;
  rows: ExposureRow[]; excluded: string[]; unconfirmed: string[]; note: string;
  reference_status: AuthzMarker;
  nav_status: AuthzMarker;
}
export interface FuturesContract {
  contract_root: string; label: string; category: Category; point_value: number | null;
  currency: string; curve: string | null; price_convention: "decimal" | "th32"; confirmed: boolean;
  otc: boolean;
}
export interface CtdRecord {
  nav_date: string; ticker: string; ctd_isin: string; ctd_mod_duration: number;
  ctd_clean_price: number; ctd_accrued: number; conversion_factor: number;
}
export interface CtdUploadOutcome { nav_date: string; rows: number; replaced: boolean }

export const getDerivatives = (pid: number, date?: string) =>
  req<Derivatives>(`/api/portfolios/${pid}/metrics/derivatives${date ? `?date=${date}` : ""}`);
export const getFuturesContracts = () => req<FuturesContract[]>("/api/futures-contracts");
export const putFuturesContract = (root: string, body: Omit<FuturesContract, "contract_root">) =>
  req<FuturesContract>(`/api/futures-contracts/${root}`, {
    method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
  });
export const getCtd = (pid: number, date?: string) =>
  req<CtdRecord[]>(`/api/portfolios/${pid}/futures-analytics${date ? `?date=${date}` : ""}`);
export const uploadCtd = (pid: number, f: File) => {
  const fd = new FormData();
  fd.append("file", f);
  return req<CtdUploadOutcome>(`/api/portfolios/${pid}/futures-analytics`, { method: "POST", body: fd });
};

export type PnlDimension =
  | "asset_class" | "country" | "region" | "sector" | "industry" | "currency" | "issuer_group";

export interface PnlInstrument {
  isin: string; name: string; asset_class: string;
  country: string | null; region: string | null;
  sector: string | null; industry: string | null;
  currency: string; issuer_group: string | null;
  realized_price: number; unrealized_price: number;
  realized_fx: number; unrealized_fx: number;
  fx_split_imprecise: boolean; fx_missing: string[];
}
export interface PnlGroup {
  key: string;
  realized_price: number; unrealized_price: number;
  realized_fx: number; unrealized_fx: number;
  realized: number; unrealized: number; fx: number; total: number;
  instruments: PnlInstrument[];
}
export interface PnlPeriod {
  requested_from: string; requested_to: string;
  actual_from: string; actual_to: string; snapshots: number;
}
export interface PnlReconciliation {
  investment_pnl: number; cash_and_margin: number; accrued_fees: number;
  provisions: number; dividend_income: number; total_pnl: number;
  aum_change: number; net_flows: number;
  residual: number; gross: number; within_tolerance: boolean;
}
export interface Pnl {
  empty: boolean;
  period?: PnlPeriod;
  groups?: PnlGroup[];
  reconciliation?: PnlReconciliation;
  unclassified?: number;
  warnings: string[];
}

export const getPnl = (pid: number, p: { from: string; to: string; dimension: PnlDimension }) =>
  req<Pnl>(`/api/portfolios/${pid}/pnl?from=${p.from}&to=${p.to}&dimension=${p.dimension}`);

export const bloombergRequestUrl = "/api/bloomberg/request";
export interface BloombergUpload {
  classified: number; fx_rows: number; adv_rows: number; skipped: RowError[];
  fx_check: { currency: string; date: string; workbook: number; bloomberg: number; drift: number }[];
  // Portfolios the fx-drift cross-check could not walk because the uploader
  // lacks Positions/View there — distinct from "checked, no drift found".
  fx_check_skipped: { portfolio_id: number; portfolio_name: string; reason: string }[];
  // Reference/Configure denied -> `classified` is stuck at 0 for a reason
  // that has nothing to do with the workbook; this marker says why, so 0
  // classified never silently reads as "nothing to classify".
  classification_status: AuthzMarker;
}
export const uploadBloomberg = (f: File) => {
  const fd = new FormData();
  fd.append("file", f);
  return req<BloombergUpload>("/api/bloomberg/upload", { method: "POST", body: fd });
};

export const getConcentration = (pid: number, date?: string) =>
  req<Concentration>(`/api/portfolios/${pid}/metrics/concentration${date ? `?date=${date}` : ""}`);
export const getLiquidity = (pid: number, date?: string) =>
  req<Liquidity>(`/api/portfolios/${pid}/metrics/liquidity${date ? `?date=${date}` : ""}`);
export const getFlows = (pid: number) => req<FlowStats>(`/api/portfolios/${pid}/flows`);
export const getRates = (pid: number, date?: string) =>
  req<Rates>(`/api/portfolios/${pid}/metrics/rates${date ? `?date=${date}` : ""}`);
export const getBacktest = (pid: number) => req<Backtest>(`/api/portfolios/${pid}/metrics/backtest`);
export const getRefs = () => req<RefRow[]>("/api/refs");
export const putRef = (code: string, body: RefBody) =>
  req<unknown>(`/api/refs/${code}`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });

export type EmirVerdict = "ok" | "watch" | "breach";
export interface EmirMonthCell {
  month: string;
  snapshot_date: string | null;
  total_eur: number | null;
  otc_eur: number | null;
}
export interface EmirClass {
  class: string;
  label: string;
  threshold_eur: number;
  months: EmirMonthCell[];
  avg_total_eur: number;
  avg_otc_eur: number;
  pct_of_threshold: number;
  verdict: EmirVerdict;
}
export interface EmirMonitors {
  otc_open_contracts: number;
  reconciliation: "not_triggered" | "quarterly" | "weekly" | "daily";
  compression_required: boolean;
}
export interface EmirMarginLine {
  name: string | null;
  currency: string | null;
  valuation_ccy: number | null;
  valuation_eur: number | null;
}
export interface EmirKpi {
  month: string;
  unconfirmed_over_5d: number;
  reconciliation: "done" | "not_done" | "not_applicable";
  disputes: number;
  note: string | null;
}
export interface EmirResponse {
  empty?: boolean;
  dates?: string[];
  date?: string;
  months_present?: number;
  months_total?: number;
  classes?: EmirClass[];
  warnings: string[];
  monitors?: EmirMonitors;
  monitors_note?: string;
  margin?: EmirMarginLine[];
  futures_count?: number;
  kpis?: EmirKpi[];
  otc_note?: string;
}
export const getEmir = (pid: number, date?: string) =>
  req<EmirResponse>(`/api/portfolios/${pid}/emir${date ? `?date=${date}` : ""}`);
export const putEmirKpi = (pid: number, month: string, body: Omit<EmirKpi, "month">) =>
  req<EmirKpi>(`/api/portfolios/${pid}/emir/kpis/${month}`, {
    method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
  });
export const emirExportUrl = (pid: number) => `/api/portfolios/${pid}/emir/export`;

// ---- Administration (crates/server/src/handlers/admin.rs, `/api/admin/*`, administrator-only) ----

export interface AdminUser {
  id: number; email: string; display_name: string; is_administrator: boolean; disabled: boolean;
}
export const getUsers = () => req<AdminUser[]>("/api/admin/users");
export const createUser = (body: { email: string; display_name: string; password: string; is_administrator: boolean }) =>
  req<AdminUser>("/api/admin/users", {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
  });
export const adminSetPassword = (id: number, password: string) =>
  req<void>(`/api/admin/users/${id}/password`, {
    method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ password }),
  });
export const adminSetDisabled = (id: number, disabled: boolean) =>
  req<void>(`/api/admin/users/${id}/disabled`, {
    method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ disabled }),
  });

/** Mirrors `db::auth::Grant` (`crates/db/src/auth/model.rs`) exactly — the wire
 * field is `portfolio`, not `portfolio_id` (that spelling belongs to `Capability`
 * in `auth.ts`, a different struct on a different endpoint). `portfolio: null`
 * means "every portfolio", same as elsewhere in this app. */
export interface AdminGrant { domain: string; action: string; portfolio: number | null }
export const getGrants = (id: number) => req<AdminGrant[]>(`/api/admin/users/${id}/grants`);
export const addGrant = (id: number, g: AdminGrant) =>
  req<void>(`/api/admin/users/${id}/grants`, {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(g),
  });
export const removeGrant = (id: number, g: AdminGrant) =>
  req<void>(`/api/admin/users/${id}/grants`, {
    method: "DELETE", headers: { "content-type": "application/json" }, body: JSON.stringify(g),
  });

export const assignRole = (id: number, role: string, scope: number | null) =>
  req<void>(`/api/admin/users/${id}/roles`, {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ role, scope }),
  });

/** The six domains and four actions, in the exact serde spellings from
 * `crates/db/src/auth/model.rs::{Domain,Action}::as_str`. */
export const ADMIN_DOMAINS = ["positions", "nav", "transactions", "shareholders", "market_data", "reference"] as const;
export const ADMIN_ACTIONS = ["view", "export", "import", "configure"] as const;
export function domainLabel(d: string): string {
  switch (d) {
    case "positions": return "Positions";
    case "nav": return "NAV history";
    case "transactions": return "Transactions";
    case "shareholders": return "Shareholder register";
    case "market_data": return "Market data";
    case "reference": return "Reference data";
    default: return d;
  }
}

/** The four roles, in the exact serde spellings from `crates/db/src/auth/roles.rs::Role::as_str`. */
export const ADMIN_ROLES: { value: string; label: string }[] = [
  { value: "risk_analyst", label: "Risk Analyst" },
  { value: "head_of_risk", label: "Head of Risk" },
  { value: "operations", label: "Operations" },
  { value: "auditor", label: "Auditor" },
];

export interface AuditRow {
  id: number; at: string; actor_label: string; action: string;
  domain: string | null; portfolio_id: number | null; detail: unknown; source_addr: string | null;
}
export const getAudit = (limit = 200) => req<AuditRow[]>(`/api/admin/audit?limit=${limit}`);
