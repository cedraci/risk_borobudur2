//! The shared computation behind a recorded limit-check run.
//!
//! `compute` is what `server::recorder` records; `holdings_at` is the same
//! grouping applied to an earlier snapshot so a proposal can compare
//! quantities. Both call the one `subject_group` helper below — two copies of
//! the grouping rule drifting apart would silently change what a "subject"
//! means between the run and its proposals, breaking every active/passive
//! classification.
//!
//! Task 5 computes concentration only; Task 6 adds liquidity, VaR and EMIR to
//! `compute`.

use crate::error::AppError;
use crate::state::AppState;
use analytics::breach::{Finding, SubjectHolding};
use analytics::{concentration, effective_issuer_group, CheckStatus, ConPosition};
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::{Extension, Json};
use chrono::NaiveDate;
use db::auth::marker::{Configure, Export, Nav, Positions, Reference, Settings, Shareholders, View};
use db::auth::{Access, Action, AuthCtx, Domain};
use db::repo::CheckResultRow;
use db::scoped::Scoped;
use std::collections::HashMap;

/// One run's worth of computed checks, plus what the proposal step needs and
/// what the run row needs to say about its inputs.
pub struct ComputedRun {
    pub results: Vec<CheckResultRow>,
    /// One per breaching row.
    pub findings: Vec<Finding>,
    /// Subject (issuer group) -> its instruments at THIS date.
    pub holdings: HashMap<String, Vec<SubjectHolding>>,
    /// Subject -> its weight at THIS date.
    pub weights: HashMap<String, f64>,
    /// Inputs that were genuinely ABSENT — never a permission, since this
    /// runs under the system context. A check that could not be evaluated is
    /// omitted from `results` and named here; a check that could not run must
    /// never appear in the register as one that passed.
    pub input_notes: serde_json::Map<String, serde_json::Value>,
}

/// What a subject is, for one position row. The rule itself lives in
/// `analytics::concentration::effective_issuer_group` — the one definition,
/// shared with `handlers::refs` (what the Reference page displays) and
/// `handlers::pnl` (the attribution dimension), which each had their own copy
/// without the `Fonds` carve-out until finding I4.
fn subject_group(
    p: &db::repo::PositionRecord, by: &HashMap<&str, &db::repo::InstrumentRef>,
) -> String {
    effective_issuer_group(
        &p.asset_type,
        p.name.as_deref().unwrap_or_default(),
        by.get(p.isin.as_str()).and_then(|r| r.issuer_group.as_deref()),
    )
}

/// The `ConPosition` assembly lifted out of `limits::concentration_h`, which
/// now calls this too — the grouping applied to a check must be the grouping
/// recorded in the register.
pub(crate) fn con_positions(
    rows: &[db::repo::PositionRecord], by: &HashMap<&str, &db::repo::InstrumentRef>,
) -> Vec<ConPosition> {
    rows.iter().filter_map(|p| {
        let w = p.weight?;
        Some(ConPosition {
            asset_type: p.asset_type.clone(),
            group: subject_group(p, by),
            weight: w,
        })
    }).collect()
}

fn status_str(s: CheckStatus) -> &'static str {
    match s {
        CheckStatus::Ok => "ok",
        CheckStatus::Watch => "watch",
        CheckStatus::Breach => "breach",
    }
}

/// Groups one snapshot's rows by subject: every instrument (for quantity
/// comparison) and the summed weight where the snapshot reports one.
fn group_holdings(
    rows: &[db::repo::PositionRecord], by: &HashMap<&str, &db::repo::InstrumentRef>,
) -> (HashMap<String, Vec<SubjectHolding>>, HashMap<String, f64>) {
    let mut holdings: HashMap<String, Vec<SubjectHolding>> = HashMap::new();
    let mut weights: HashMap<String, f64> = HashMap::new();
    for p in rows {
        let subject = subject_group(p, by);
        holdings.entry(subject.clone()).or_default().push(SubjectHolding {
            isin: p.isin.clone(),
            quantity: p.quantity,
        });
        if let Some(w) = p.weight {
            *weights.entry(subject).or_default() += w;
        }
    }
    (holdings, weights)
}

async fn snapshot_grouped(
    scoped: &Scoped<'_>, pid: i64, date: NaiveDate,
) -> anyhow::Result<(Access<Positions, View>, Vec<db::repo::PositionRecord>, Vec<db::repo::InstrumentRef>)> {
    let a = scoped.authorize::<Positions, View>(pid)
        .map_err(|d| anyhow::anyhow!("system context refused: {d}"))?;
    let rows = scoped.positions_for(&a, date).await?;
    let rv = scoped.authorize_global::<Reference, View>()
        .map_err(|d| anyhow::anyhow!("system context refused: {d}"))?;
    let refs = scoped.refs_all(&rv).await?;
    Ok((a, rows, refs))
}

/// `AppError` has no `Display`/`ToString` impl (it renders straight to an
/// HTTP response, see `handlers::imports::err_msg`), so a variant reached
/// from this system-context path — which should never see a `Forbidden` or
/// user-facing variant — is converted to `anyhow::Error` by hand.
fn app_error_to_anyhow(e: AppError) -> anyhow::Error {
    match e {
        AppError::Internal(e) => e,
        AppError::Forbidden(d) => anyhow::anyhow!("system context refused: {d}"),
        _ => anyhow::anyhow!("unexpected error while assembling EMIR classes for the register"),
    }
}

/// The scenario keys `liquidity_assembly` returns, paired with the human
/// label recorded as the row's `scope_label`. This is the ONE mapping from
/// scenario key to label; `limits::liquidity_h` has no equivalent because
/// its scenarios are addressed by `key` in JSON, never rendered as a
/// standalone label.
const LIQUIDITY_LABELS: [(&str, &str); 4] = [
    ("top5", "Top 5 holders"),
    ("fixed", "Fixed shock"),
    ("hybrid_top5", "Top 5 holders, stressed ADV"),
    ("hybrid_fixed", "Fixed shock, stressed ADV"),
];

/// Computes the checks for one portfolio at one date, as rows to record.
pub async fn compute(
    scoped: &Scoped<'_>, pid: i64, nav_date: NaiveDate,
) -> anyhow::Result<ComputedRun> {
    let (pos_access, rows, refs) = snapshot_grouped(scoped, pid, nav_date).await?;
    let by = super::limits::ref_map(&refs);
    let cons = con_positions(&rows, &by);
    let checks = concentration(&cons);

    let mut results = Vec::with_capacity(checks.len());
    let mut findings = Vec::new();
    let mut input_notes = serde_json::Map::new();
    for c in &checks {
        // Rows come sorted descending by weight, but take the max explicitly:
        // the register must not depend on a presentation ordering.
        let observed = c.rows.iter().map(|r| r.weight)
            .max_by(|x, y| x.total_cmp(y));
        results.push(CheckResultRow {
            check_key: c.check.clone(),
            scope_label: c.scope_label.clone(),
            limit_value: Some(c.limit),
            observed_value: observed,
            status: status_str(c.status).to_string(),
            detail: serde_json::to_value(c)?,
        });
        for r in &c.rows {
            if r.status == CheckStatus::Breach {
                findings.push(Finding {
                    check_key: c.check.clone(),
                    subject: r.group.clone(),
                    value: Some(r.weight),
                });
            }
        }
    }

    let settings = scoped.get_settings(pid).await?;

    // ---- Liquidity: the same scenario assembly `limits::liquidity_h` uses.
    // Shareholders and Nav are read under the system context, so a missing
    // register or NAV row here is always a genuinely absent input, never a
    // permission gap (see `ComputedRun::input_notes`).
    let sh_access = scoped.authorize::<Shareholders, View>(pid)
        .map_err(|d| anyhow::anyhow!("system context refused: {d}"))?;
    let register = scoped.shareholders_for(&sh_access).await?;

    let nav_access = scoped.authorize::<Nav, View>(pid)
        .map_err(|d| anyhow::anyhow!("system context refused: {d}"))?;
    let nav_rows = scoped.nav_rows(&nav_access).await?;
    // The position snapshot's own date does not always have a matching NAV
    // row (the depositary's NAV history can lag a snapshot by a business
    // day) — the register uses the latest NAV known at or before the
    // snapshot date rather than requiring an exact match, so a routine
    // one-day lag does not blank out every liquidity scenario on every run.
    let liquidity_nav = nav_rows.iter().rev().find(|r| r.date <= nav_date).map(|r| (r.date, r.aum));

    match liquidity_nav {
        Some((nav_basis_date, nav)) => {
            // An empty register is an ABSENT input, not a 0% redemption
            // requirement: "if the five largest investors all redeemed at
            // once, could the fund meet it?" has no honest answer when
            // nobody has loaded who the largest investors are. Recording
            // `status: "ok"` here would state that a redemption stress test
            // passed when it was never run — the design doc names exactly
            // this ("no shareholder register loaded") as the worked example
            // of a genuinely absent input, distinct from a denial. The rule
            // lives in `liquidity_assembly`; it marks `top5`/`hybrid_top5`
            // "unavailable" and the skip+note path below records why.
            let assembly = super::limits::liquidity_assembly(&super::limits::LiquidityScenarioInputs {
                rows: &rows, by: &by, settings: &settings, asof: nav_date, nav,
                register: &register,
                top5_unavailable_reason: "no shareholder register",
            });
            for v in &assembly.scenarios {
                let key = v["key"].as_str().unwrap_or_default();
                let label = LIQUIDITY_LABELS.iter().find(|(k, _)| *k == key).map(|(_, l)| *l).unwrap_or(key);
                let check_key = format!("liq_{key}");
                // The DB's `limit_check_results.status` CHECK constraint only
                // allows ok/watch/breach — "unavailable" (a scenario the
                // register could not evaluate, e.g. an empty shareholder
                // register) cannot be stored as a row at all. Recording it
                // with a fabricated "ok" would be exactly the false pass this
                // whole feature exists to prevent, so it is omitted from
                // `results` and named in `input_notes` instead.
                if v["status"].as_str() == Some("unavailable") {
                    let reason = v["reason"].as_str().unwrap_or("unavailable").to_string();
                    input_notes.insert(check_key, reason.into());
                    continue;
                }
                let status = v["status"].as_str().unwrap_or("ok").to_string();
                if status == "breach" {
                    findings.push(Finding { check_key: check_key.clone(), subject: label.to_string(), value: None });
                }
                // The register's NAV lookup is at-or-before `nav_date`, not
                // an exact match (see `liquidity_nav` above) — `nav_basis_date`
                // records which NAV date the waterfall actually used, so a
                // later reader is never left to infer it.
                let mut detail = v.clone();
                detail["nav_basis_date"] = serde_json::json!(nav_basis_date);
                results.push(CheckResultRow {
                    check_key,
                    scope_label: label.to_string(),
                    // No honest scalar pair: a liquidity verdict comes from a
                    // waterfall over several days, not a single limit/observed
                    // pair (see `detail` instead).
                    limit_value: None,
                    observed_value: None,
                    status,
                    detail,
                });
            }
        }
        None => {
            // Keyed per scenario, like every other note: an auditor asking
            // "why is liq_fixed missing from this run?" must get an answer
            // under that check's own key, not under a category heading.
            for (key, _) in LIQUIDITY_LABELS {
                input_notes.insert(format!("liq_{key}"), "no NAV data at or before this date".into());
            }
        }
    }

    // ---- VaR: the same NAV-returns math `metrics::var` uses. ----------
    let nav_points = super::metrics::to_points(&nav_rows);
    let rets: Vec<f64> = analytics::daily_returns(&nav_points).iter().map(|p| p.value).collect();
    let aum = nav_rows.last().map(|r| r.aum);
    let mut var_warnings = Vec::new();
    let var_result = super::metrics::var_block(
        &rets, settings.var_confidence, settings.var_horizon_days,
        settings.var_window_days, settings.var_limit, aum, &mut var_warnings,
    );
    match var_result.and_then(|b| b.historical.map(|h| (b, h))) {
        Some((block, hist)) => {
            let observed = hist.var;
            let limit = settings.var_limit;
            let status = if observed > limit { "breach" }
                else if observed >= 0.8 * limit { "watch" }
                else { "ok" };
            if status == "breach" {
                findings.push(Finding { check_key: "var_limit".into(), subject: "Historical VaR".into(), value: Some(observed) });
            }
            results.push(CheckResultRow {
                check_key: "var_limit".into(),
                scope_label: "Historical VaR".into(),
                limit_value: Some(limit),
                observed_value: Some(observed),
                status: status.to_string(),
                detail: serde_json::to_value(&block)?,
            });
        }
        None => {
            // Too little NAV history to compute a VaR at all — never
            // recorded as a passing "ok".
            input_notes.insert("var_limit".into(), "too little price history for VaR".into());
        }
    }

    // ---- EMIR: the same 12-month threshold assembly `emir::get` uses. -
    let emir_assembly = super::emir::assemble(scoped, &pos_access, pid, &Some(nav_date.to_string()))
        .await
        .map_err(app_error_to_anyhow)?;
    match emir_assembly {
        Some(a) => {
            // `emir::assemble` soft-degrades for the live route, where
            // Reference is a secondary domain worth losing rather than
            // failing the whole page. The register cannot accept that
            // trade: without contract specs every position defaults to
            // `otc: false`, so every clearing-obligation verdict would be
            // computed on wrong data and recorded as a pass. Under the
            // system context neither can be denied — so if one ever is,
            // that is a bug in the context, not a degraded run to persist.
            if a.contracts_denied.is_some() || a.kpis_denied.is_some() {
                anyhow::bail!("system context refused: EMIR reference data");
            }
            for c in &a.report.classes {
                let class_key = serde_json::to_value(c.class)?.as_str().unwrap_or("other").to_string();
                let check_key = format!("emir_{class_key}");
                let status = c.verdict.as_str().to_string();
                if status == "breach" {
                    findings.push(Finding {
                        check_key: check_key.clone(),
                        subject: c.label.to_string(),
                        value: Some(c.avg_otc_eur),
                    });
                }
                results.push(CheckResultRow {
                    check_key,
                    scope_label: c.label.to_string(),
                    limit_value: Some(c.threshold_eur),
                    observed_value: Some(c.avg_otc_eur),
                    status,
                    detail: serde_json::to_value(c)?,
                });
            }
        }
        None => {
            input_notes.insert("emir".into(), "no position snapshot to compute EMIR thresholds against".into());
        }
    }

    let (holdings, weights) = group_holdings(&rows, &by);
    Ok(ComputedRun {
        results,
        findings,
        holdings,
        weights,
        input_notes,
    })
}

/// Just the groupings for one date — no checks. Used for the previous
/// snapshot, where only the quantities and weights matter.
pub async fn holdings_at(
    scoped: &Scoped<'_>, pid: i64, date: NaiveDate,
) -> anyhow::Result<(
    HashMap<String, Vec<SubjectHolding>>,
    HashMap<String, f64>,
)> {
    let (_pos_access, rows, refs) = snapshot_grouped(scoped, pid, date).await?;
    let by = super::limits::ref_map(&refs);
    Ok(group_holdings(&rows, &by))
}

// ---- Read endpoints: the register's HTTP surface. ------------------------
//
// All three are gated on `Domain::Settings`/`Action::View` — the same domain
// the write side (Task 5/6's recorder) uses under the system context, and the
// same domain `imports::list` already uses for the import ledger. See the
// design doc's "Known limitation" section for why the register doesn't get
// its own dedicated domain.

// ---- The payload gate ----------------------------------------------------
//
// A run is COMPUTED under the system context, so every `detail` blob it
// records is the verbatim analytics of some OTHER domain: the issuer-by-issuer
// weights behind the concentration checks (Positions), `var_eur` and the
// liquidity `*_eur` figures (Nav), `avg_otc_eur` (Positions, via EMIR), and —
// for `liq_top5`/`liq_hybrid_top5` — `required_pct`, the combined share of NAV
// held by the fund's five largest investors (Shareholders). A `BreachRow`'s
// `proposal_reason` likewise names ISINs and quantities.
//
// `handlers::limits::liquidity_h` deliberately withholds that last figure from
// a principal without `Shareholders/View`, marking the scenario
// `{"status":"unavailable","reason":…}` so a denial stays distinguishable from
// an empty register. Serving the recorded run back on `Settings/View` alone
// would make the register a side channel around exactly that non-disclosure —
// and the shipped `Operations` and `RiskAnalyst` roles both hold `Settings`
// with no `Shareholders` grant at all.
//
// So the payload gets a SECOND gate, here on the read path: each field is
// re-authorized against the domain it came from and replaced with the same
// marker, carrying the same `Denied::reason()` string, where the reader lacks
// it. `check_key`, `scope_label`, `status` and the whole episode lifecycle
// stay on `Settings/View` — knowing that a check breached is what the register
// is for; the numbers behind it are the other domain's business.

/// The marker `handlers::limits` uses for a component a principal may not
/// see. Never a fabricated value, never a silent omission.
fn unavailable(reason: &str) -> serde_json::Value {
    serde_json::json!({"status": "unavailable", "reason": reason})
}

fn available() -> serde_json::Value {
    serde_json::json!({"status": "ok"})
}

/// One reader's denials on the three secondary domains a recorded run's
/// payload can carry. Built once per request from `Scoped::denial`, which
/// asks without taking a token — nothing here reads those domains, it only
/// decides what may be served back.
pub(crate) struct DetailGate {
    positions: Option<String>,
    nav: Option<String>,
    shareholders: Option<String>,
}

impl DetailGate {
    fn for_reader(scoped: &Scoped<'_>, pid: i64) -> Self {
        let reason = |d: Domain| scoped.denial(d, Action::View, pid).map(|x| x.reason());
        DetailGate {
            positions: reason(Domain::Positions),
            nav: reason(Domain::Nav),
            shareholders: reason(Domain::Shareholders),
        }
    }

    /// Which domains a check's payload draws from, as
    /// `(positions, nav, shareholders)`.
    ///
    /// An unrecognised key is treated as drawing on ALL THREE. A check added
    /// later must fail closed: the cost of that is a marker where a number
    /// could have been served, and the cost of the other default is another
    /// C1.
    fn domains_for(check_key: &str) -> (bool, bool, bool) {
        match check_key {
            // Concentration: `Check::rows` is the fund's issuer-by-issuer
            // weights, and `observed_value` is the largest of them.
            "issuer_10" | "group_20" | "fund_20" | "deposit_20" | "forty" => (true, false, false),
            // `VarBlock::var_eur` is VaR x AUM.
            "var_limit" => (false, true, false),
            // The liquidity waterfall is built from positions and scaled by
            // NAV; the two top-5 scenarios additionally carry `required_pct`,
            // which IS the top five investors' combined share of NAV.
            k if k.starts_with("liq_") => (true, true, k.ends_with("top5")),
            // `avg_otc_eur` over the fund's own derivative positions.
            k if k.starts_with("emir_") => (true, false, false),
            _ => (true, true, true),
        }
    }

    /// The reason to show for a check whose payload this reader may not have,
    /// or `None` when they may. Shareholders first, then Nav, then Positions:
    /// where several apply, the narrowest grant is the informative one.
    fn reason_for(&self, check_key: &str) -> Option<&str> {
        let (pos, nav, sh) = Self::domains_for(check_key);
        None.or_else(|| if sh { self.shareholders.as_deref() } else { None })
            .or_else(|| if nav { self.nav.as_deref() } else { None })
            .or_else(|| if pos { self.positions.as_deref() } else { None })
    }

    /// One recorded check result. `limit_value` survives a denial — it is the
    /// fund's own configured limit (or a regulatory threshold), which is
    /// `Settings` data the reader already holds. `observed_value` does not:
    /// for `emir_*` it IS `avg_otc_eur`, and for the concentration checks it
    /// is the largest issuer weight, so it belongs with `detail` rather than
    /// surviving as a summary of what was withheld.
    fn result_json(&self, r: &CheckResultRow) -> serde_json::Value {
        match self.reason_for(&r.check_key) {
            None => serde_json::json!({
                "check_key": r.check_key, "scope_label": r.scope_label,
                "limit_value": r.limit_value, "observed_value": r.observed_value,
                "status": r.status, "detail": r.detail,
                "payload_status": available(),
            }),
            Some(reason) => serde_json::json!({
                "check_key": r.check_key, "scope_label": r.scope_label,
                "limit_value": r.limit_value, "observed_value": serde_json::Value::Null,
                "status": r.status, "detail": unavailable(reason),
                "payload_status": unavailable(reason),
            }),
        }
    }

    /// One register episode. `opened_value`/`peak_value` are the observed
    /// readings — the same disclosure as `observed_value` above — and go
    /// behind the check's own domains. `proposal_reason` is gated separately
    /// on Positions, because `analytics::breach::propose` builds it from
    /// instrument quantities whatever the check is.
    fn breach_json(&self, b: &db::repo::BreachRow) -> serde_json::Value {
        let mut v = serde_json::to_value(b).expect("BreachRow always serializes");
        let values_reason = self.reason_for(&b.check_key);
        v["values_status"] = match values_reason {
            None => available(),
            Some(reason) => {
                v["opened_value"] = serde_json::Value::Null;
                v["peak_value"] = serde_json::Value::Null;
                v["peak_nav_date"] = serde_json::Value::Null;
                unavailable(reason)
            }
        };
        v["proposal_status"] = match self.positions.as_deref() {
            None => available(),
            Some(reason) => {
                v["proposal_reason"] = serde_json::Value::Null;
                unavailable(reason)
            }
        };
        v
    }

    /// One timeline event. The `opened` and `note` events restate the
    /// observed reading (`value`, `peak_value`) and the proposal's `reason`
    /// in their own `detail`, so gating only `breach_json` would leave the
    /// same numbers readable one route over.
    fn event_json(
        &self, check_key: &str, e: &db::repo::BreachEventRow,
    ) -> serde_json::Value {
        let mut v = serde_json::to_value(e).expect("BreachEventRow always serializes");
        v["values_status"] = match self.reason_for(check_key) {
            None => available(),
            Some(reason) => {
                for k in ["value", "peak_value"] {
                    if v["detail"].get(k).is_some() {
                        v["detail"][k] = serde_json::Value::Null;
                    }
                }
                unavailable(reason)
            }
        };
        v["proposal_status"] = match self.positions.as_deref() {
            None => available(),
            Some(reason) => {
                if v["detail"].get("reason").is_some() {
                    v["detail"]["reason"] = serde_json::Value::Null;
                }
                unavailable(reason)
            }
        };
        v
    }
}

#[derive(serde::Deserialize)]
pub struct RunsQuery {
    pub limit: Option<i64>,
}

/// The recorded check runs as JSON, newest first, each with its per-check
/// results — the one construction shared by `runs_list` and the evidence
/// export, so the two never drift into serving different shapes of the same
/// data, and so the `DetailGate` cannot be applied to one and forgotten on the
/// other. Takes the already-fetched rows rather than fetching them itself: the
/// two callers fetch differently (`runs_list` pages with a clamped `limit`
/// via `runs_for`, the export reads everything via `runs_all` — see Ruling 1
/// in the task-9 review, an export must never silently cap the history it
/// claims to be complete).
fn runs_json(
    rows: Vec<(db::repo::CheckRunRow, Vec<CheckResultRow>)>, gate: &DetailGate,
) -> Vec<serde_json::Value> {
    rows.into_iter()
        .map(|(run, results)| serde_json::json!({
            "id": run.id, "nav_date": run.nav_date, "run_at": run.run_at,
            "triggered_by": run.triggered_by, "import_id": run.import_id,
            "inputs_complete": run.inputs_complete, "input_notes": run.input_notes,
            "results": results.iter().map(|r| gate.result_json(r)).collect::<Vec<_>>(),
        }))
        .collect()
}

/// The recorded check runs, newest first, each with its per-check results.
pub async fn runs_list(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>,
    Path(pid): Path<i64>, Query(q): Query<RunsQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Settings, View>(pid)?;
    super::portfolios::ensure(&scoped, pid, false).await?;
    let limit = q.limit.unwrap_or(52).clamp(1, 500);
    let gate = DetailGate::for_reader(&scoped, pid);
    let runs = runs_json(scoped.runs_for(&a, limit).await?, &gate);
    Ok(Json(serde_json::json!({ "runs": runs })))
}

#[derive(serde::Deserialize)]
pub struct RegisterQuery {
    pub state: Option<String>,
}

/// The breach register as JSON, optionally filtered to one `state` — the one
/// construction shared by `register_list` and the evidence export.
async fn register_json(
    scoped: &Scoped<'_>, a: &Access<Settings, View>, state: Option<&str>, gate: &DetailGate,
) -> Result<Vec<serde_json::Value>, AppError> {
    if let Some(s) = state
        && !matches!(s, "open" | "acknowledged" | "resolved") {
        return Err(AppError::BadRequest(format!("unknown state: {s}")));
    }
    Ok(scoped.breaches_for(a, state).await?
        .iter()
        .map(|b| gate.breach_json(b))
        .collect())
}

/// The breach register itself: every episode, optionally filtered to one
/// `state` (open/acknowledged/resolved).
pub async fn register_list(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>,
    Path(pid): Path<i64>, Query(q): Query<RegisterQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Settings, View>(pid)?;
    super::portfolios::ensure(&scoped, pid, false).await?;
    let gate = DetailGate::for_reader(&scoped, pid);
    let breaches = register_json(&scoped, &a, q.state.as_deref(), &gate).await?;
    Ok(Json(serde_json::json!({ "breaches": breaches })))
}

/// The xlsx evidence export — the artefact a fund hands a regulator or
/// auditor: every recorded run plus the full breach register, one file. See
/// `ingest::breach_evidence::build`.
pub async fn export(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(pid): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let scoped = st.db.scope(&ctx);
    // `Export` is the gate on this route (routes.rs); the reads below still
    // need their own `View` token, same as `handlers::emir::export`.
    let _export = scoped.authorize::<Settings, Export>(pid)?;
    let a = scoped.authorize::<Settings, View>(pid)?;
    let portfolio = super::portfolios::ensure(&scoped, pid, false).await?;
    // Every run, uncapped: an evidence artefact must not silently omit
    // history the way `runs_list`'s paged UI read is allowed to.
    //
    // Through the SAME `runs_json`/`register_json` the read endpoints use, so
    // the payload gate cannot be applied to the JSON routes and forgotten
    // here — an export is a read, and a downloadable one at that.
    let gate = DetailGate::for_reader(&scoped, pid);
    let runs = runs_json(scoped.runs_all(&a).await?, &gate);
    let breaches = register_json(&scoped, &a, None, &gate).await?;
    let bytes = ingest::breach_evidence::build(&portfolio.name, &runs, &breaches)
        .map_err(AppError::Internal)?;
    let filename = format!("Breach register - {} - {}.xlsx", portfolio.name, chrono::Utc::now().date_naive());
    let mut h = HeaderMap::new();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"));
    h.insert(header::CONTENT_DISPOSITION, super::download::attachment(&filename));
    // Audited AFTER the response is fully built (M8). It used to run before
    // the header construction that could fail, so a 500 that delivered nothing
    // was recorded in the audit log as a completed export.
    crate::audit::record(&st, &ctx, "export", Some(Domain::Settings), Some(pid),
        serde_json::json!({"kind": "breach_register"})).await;
    Ok((StatusCode::OK, h, bytes))
}

/// One breach episode with its full event timeline.
pub async fn episode_get(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>,
    Path((pid, bid)): Path<(i64, i64)>,
) -> Result<Json<serde_json::Value>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Settings, View>(pid)?;
    super::portfolios::ensure(&scoped, pid, false).await?;
    let breach = scoped.breach_get(&a, bid).await?
        .ok_or_else(|| AppError::NotFound(format!("no breach {bid}")))?;
    let events = scoped.breach_events(&a, bid).await?;
    let gate = DetailGate::for_reader(&scoped, pid);
    // The timeline's `opened`/`note` events carry the same observed readings
    // in their `detail` — the third door into the same numbers.
    let events: Vec<serde_json::Value> = events.iter()
        .map(|e| gate.event_json(&breach.check_key, e)).collect();
    Ok(Json(serde_json::json!({ "breach": gate.breach_json(&breach), "events": events })))
}

// ---- The workflow: manual re-run, acknowledge, resolve. ------------------

#[derive(serde::Deserialize)]
pub struct AcknowledgeBody {
    pub classification: String,
    pub note: String,
    #[serde(default)]
    pub deadline_date: Option<NaiveDate>,
}

/// Where a proposal becomes a decision. Requires a real classification and a
/// non-blank note — acknowledging is the compliance act this whole register
/// exists to force, not a rubber stamp.
pub async fn acknowledge(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>,
    Path((pid, bid)): Path<(i64, i64)>, Json(b): Json<AcknowledgeBody>,
) -> Result<StatusCode, AppError> {
    if !matches!(b.classification.as_str(), "active" | "passive") {
        return Err(AppError::Unprocessable(
            "classification must be 'active' or 'passive' — acknowledging is where the proposal becomes a decision".into()));
    }
    let note = b.note.trim();
    if note.is_empty() {
        return Err(AppError::Unprocessable("a note is required to acknowledge a breach".into()));
    }
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Settings, Configure>(pid)?;
    super::portfolios::ensure(&scoped, pid, true).await?;
    let actor = (ctx.principal_id != 0).then_some(ctx.principal_id);
    let ok = scoped.breach_acknowledge(
        &a, bid, &b.classification, note, b.deadline_date, actor, &ctx.display_name).await?;
    if !ok {
        return Err(AppError::Unprocessable(
            "no open breach with that id — it may already be acknowledged or resolved".into()));
    }
    crate::audit::record(&st, &ctx, "configure", Some(Domain::Settings), Some(pid),
        serde_json::json!({"kind": "breach_acknowledged", "breach_id": bid,
                           "classification": b.classification})).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Deserialize)]
pub struct ResolveBody { pub note: String }

/// Only from `acknowledged`. Resolving something nobody classified is the
/// gap this whole feature exists to close.
pub async fn resolve(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>,
    Path((pid, bid)): Path<(i64, i64)>, Json(b): Json<ResolveBody>,
) -> Result<StatusCode, AppError> {
    let note = b.note.trim();
    if note.is_empty() {
        return Err(AppError::Unprocessable("a note is required to resolve a breach".into()));
    }
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Settings, Configure>(pid)?;
    super::portfolios::ensure(&scoped, pid, true).await?;
    let actor = (ctx.principal_id != 0).then_some(ctx.principal_id);
    let ok = scoped.breach_resolve(&a, bid, note, actor, &ctx.display_name).await?;
    if !ok {
        return Err(AppError::Unprocessable(
            "only an acknowledged breach can be resolved — classify it first".into()));
    }
    crate::audit::record(&st, &ctx, "configure", Some(Domain::Settings), Some(pid),
        serde_json::json!({"kind": "breach_resolved", "breach_id": bid})).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Deserialize)]
pub struct RerunBody {
    /// Optional, and when given it must name the latest snapshot date — the
    /// caller stating which date it believes it is re-checking, not choosing
    /// one. Defaults to that same date.
    #[serde(default)]
    pub date: Option<NaiveDate>,
}

/// A manual re-run of the recorder, for a fund whose data or settings
/// changed without a fresh import — e.g. a limit was tightened and someone
/// wants to know today's verdict without waiting for tomorrow's file.
pub async fn rerun(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>,
    Path(pid): Path<i64>, Json(b): Json<RerunBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let scoped = st.db.scope(&ctx);
    // The Configure token is the authorization gate for a manual re-run,
    // even though the run itself then executes under the system context
    // below (`recorder::record` authorizes for itself via `AuthCtx::system`).
    let _configure = scoped.authorize::<Settings, Configure>(pid)?;
    super::portfolios::ensure(&scoped, pid, true).await?;
    let view = scoped.authorize::<Settings, View>(pid)?;
    // Only ever the latest snapshot. A run for an earlier date would compute
    // findings from that day's holdings and then close every live episode
    // absent from them — `analytics::breach::transitions` cannot tell a
    // back-dated run from a fund that has cleared — stamping `closed_nav_date`
    // and a `cleared` event on episodes that never cleared. The design puts
    // retrospective backfill out of scope for exactly this reason, so a date
    // that is not the latest is refused rather than honoured.
    let latest = scoped.latest_position_date(&view, pid).await?
        .ok_or_else(|| AppError::Unprocessable("no snapshot imported yet".into()))?;
    if let Some(d) = b.date
        && d != latest
    {
        return Err(AppError::Unprocessable(format!(
            "a re-run re-checks the latest snapshot ({latest}); {d} would back-date the register")));
    }
    let date = latest;
    let run_id = crate::recorder::record(&st, pid, date, crate::recorder::Trigger::Manual {
        actor_user_id: (ctx.principal_id != 0).then_some(ctx.principal_id),
        actor_label: ctx.display_name.clone(),
    }).await?;
    crate::audit::record(&st, &ctx, "configure", Some(Domain::Settings), Some(pid),
        serde_json::json!({"kind": "limit_check_rerun", "run_id": run_id, "nav_date": date})).await;
    Ok(Json(serde_json::json!({"run_id": run_id, "nav_date": date})))
}
