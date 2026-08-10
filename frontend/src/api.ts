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
  liquidity_defaults: Record<string, Bucket>; redemption_shock: number;
}
export interface RowError { sheet: string; row: number; message: string }

export class ApiError extends Error {
  detail?: string;
  rows?: RowError[];
  constructor(message: string, detail?: string, rows?: RowError[]) {
    super(message);
    this.detail = detail;
    this.rows = rows;
  }
}

async function req<T>(url: string, init?: RequestInit): Promise<T> {
  const res = await fetch(url, init);
  if (!res.ok) {
    let detail: string | undefined, rows: RowError[] | undefined;
    try {
      const body = await res.json();
      detail = body.detail; rows = body.rows;
    } catch { /* non-JSON error body */ }
    throw new ApiError(`${res.status} ${res.statusText}`, detail, rows);
  }
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
export const uploadFile = (pid: number, f: File) => {
  const fd = new FormData();
  fd.append("file", f);
  return req<ImportOutcome>(`/api/portfolios/${pid}/imports`, { method: "POST", body: fd });
};

export type Bucket = "d1" | "d2_7" | "d8_30" | "d30p";
export type CheckStatus = "ok" | "watch" | "breach";
export interface CheckRow { group: string; weight: number; status: CheckStatus }
export interface Check { check: string; scope_label: string; limit: number; rows: CheckRow[]; status: CheckStatus }
export interface Concentration { dates: string[]; date: string | null; checks: Check[]; excluded_note: string }
export interface BucketWeight { bucket: Bucket; weight: number }
export interface Liquidity {
  dates: string[]; date: string | null; buckets: BucketWeight[]; cumulative: BucketWeight[];
  negative_memo: number; shock: number; stress_status: "ok" | "breach";
}
export interface BondRow {
  code: string; name: string | null; missing: boolean;
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
  // positive means it gains. Null when the snapshot's AUM is unknown.
  total_dv01_eur: number; nav_sensitivity_100bp: number | null;
  missing_any: boolean; futures_missing_any: boolean;
  // Tickers of futures held with no contract spec at all, so excluded from the DV01.
  futures_no_spec: string[];
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
  effective_bucket: Bucket; bucket_override: Bucket | null;
  bond_coupon_pct: number | null; bond_maturity: string | null; bond_coupon_freq: number | null;
  is_bond: boolean;
}
export interface RefBody {
  issuer_group: string | null; liquidity_bucket: Bucket | null;
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
  dates: string[]; date: string | null; aum: number;
  categories: CategoryTotals[]; total: CategoryTotals;
  rows: ExposureRow[]; excluded: string[]; unconfirmed: string[]; note: string;
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
  classified: number; fx_rows: number; skipped: RowError[];
  fx_check: { currency: string; date: string; workbook: number; bloomberg: number; drift: number }[];
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
