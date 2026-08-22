//! EMIR monitoring: clearing thresholds, OTC obligation monitors, margin
//! view, monthly KPIs, and the evidence export.

use crate::error::AppError;
use crate::state::AppState;
use analytics::emir;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::{Extension, Json};
use chrono::{Datelike, NaiveDate};
use db::auth::marker::{Configure, Export, Positions, Reference, Settings, View};
use db::auth::{Access, AuthCtx, Denied, Domain};
use db::scoped::Scoped;
use ingest::emir_file;

#[derive(serde::Deserialize)]
pub struct DateQuery {
    date: Option<String>,
}

#[derive(serde::Serialize)]
pub struct MarginLine {
    pub name: Option<String>,
    pub currency: Option<String>,
    pub valuation_ccy: Option<f64>,
    pub valuation_eur: Option<f64>,
}

pub struct Assembly {
    pub dates: Vec<NaiveDate>,
    pub anchor: NaiveDate,
    pub report: emir::ThresholdReport,
    pub monitors: emir::Monitors,
    pub margin: Vec<MarginLine>,
    pub futures_count: usize,
    pub kpis: Vec<db::repo::EmirKpi>,
    pub contracts: Vec<db::repo::FuturesContract>,
    /// Set when the Reference read behind `contracts` (`contracts_all`) was
    /// denied rather than genuinely empty. `report`/`monitors` above are
    /// still computed from whatever positions data is available, but every
    /// contract in that computation was treated as non-OTC by default (see
    /// `emir_positions`) — every clearing-obligation verdict built on it
    /// reads "ok" regardless of the fund's real OTC exposure. Callers must
    /// not present that verdict as a pass; `get` surfaces it as
    /// `clearing_obligation: unavailable` and `export` refuses outright.
    pub contracts_denied: Option<Denied>,
    /// Set when the Reference read behind `kpis` (`emir_kpis_all`) was
    /// denied rather than genuinely having no recorded KPIs.
    pub kpis_denied: Option<Denied>,
}

/// One month-end's positions as EMIR sees them: the exposure path computes
/// the EUR notional (aum is irrelevant here, pass 0.0 — pct_nav is unused),
/// then each row picks up its contract's OTC flag by root.
fn emir_positions(
    rows: Vec<db::repo::PositionRecord>,
    specs: &[db::repo::FuturesContract],
) -> Vec<emir::EmirPosition> {
    let snap = super::limits::future_positions(&rows, specs);
    let rep = analytics::exposure(&snap.positions, 0.0);
    rep
        .rows
        .into_iter()
        .map(|r| {
            let otc = analytics::contract_root(&r.ticker)
                .and_then(|root| specs.iter().find(|s| s.contract_root == root).map(|s| s.otc))
                .unwrap_or(false);
            emir::EmirPosition {
                ticker: r.ticker,
                category: r.category,
                notional_eur: r.notional_eur,
                otc,
                unconfirmed: r.unconfirmed,
            }
        })
        .collect()
}

pub async fn assemble(
    scoped: &Scoped<'_>, a: &Access<Positions, View>, pid: i64, q_date: &Option<String>,
) -> Result<Option<Assembly>, AppError> {
    let dates = scoped.position_dates(a).await?;
    let anchor = match q_date {
        Some(s) => Some(s.parse::<NaiveDate>().map_err(|_| AppError::BadRequest(format!("bad date: {s}")))?),
        None => dates.first().copied(),
    };
    let Some(anchor) = anchor else { return Ok(None) };

    // Reference is a secondary domain here (this route is gated on
    // Positions) — see routes.rs's comment on this route: `contracts_all`
    // and `emir_kpis_all` still degrade to empty rather than gating the
    // endpoint (Positions data alone is worth returning), but the denial is
    // now carried through in `contracts_denied`/`kpis_denied` rather than
    // discarded. Every contract-spec-dependent computation below (OTC
    // flagging, hence every clearing-obligation verdict) is only trustworthy
    // when `contracts_denied` is `None`; `get`/`export` below both check it.
    let (specs, contracts_denied) = match scoped.authorize_global::<Reference, View>() {
        Ok(rv) => (scoped.contracts_all(&rv).await?, None),
        Err(denied) => (Vec::new(), Some(denied)),
    };
    let mut months = Vec::with_capacity(12);
    for (month, chosen) in emir::month_window(anchor, &dates) {
        let snapshot = match chosen {
            Some(d) => Some((d, emir_positions(scoped.positions_for(a, d).await?, &specs))),
            None => None,
        };
        months.push(emir::MonthSnapshot { month, snapshot });
    }

    // The anchor month's cell doubles as "the state at the anchor": monitors,
    // margin and the futures count are all struck there.
    let anchor_cell = months.last().and_then(|m| m.snapshot.clone());
    let monitors = emir::monitors(anchor_cell.as_ref().map(|(_, p)| p.as_slice()).unwrap_or(&[]));
    let (margin, futures_count) = match anchor_cell.as_ref().map(|(d, _)| *d) {
        Some(d) => {
            let rows = scoped.positions_for(a, d).await?;
            let margin = rows
                .iter()
                .filter(|r| r.asset_type == "Margin Acc")
                .map(|r| MarginLine {
                    name: r.name.clone(),
                    currency: r.currency.clone(),
                    valuation_ccy: r.valuation_ccy,
                    valuation_eur: r.valuation_eur,
                })
                .collect();
            let n = rows.iter().filter(|r| r.asset_type == "Future").count();
            (margin, n)
        }
        None => (Vec::new(), 0),
    };

    let report = emir::thresholds(&months);
    let (kpis, kpis_denied) = match scoped.authorize::<Settings, View>(pid) {
        Ok(rv) => (scoped.emir_kpis_all(&rv).await?, None),
        Err(denied) => (Vec::new(), Some(denied)),
    };
    Ok(Some(Assembly {
        dates, anchor, report, monitors, margin, futures_count, kpis, contracts: specs,
        contracts_denied, kpis_denied,
    }))
}

pub async fn get(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(pid): Path<i64>, Query(q): Query<DateQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Positions, View>(pid)?;
    super::portfolios::ensure(&scoped, pid, false).await?;
    let Some(a) = assemble(&scoped, &a, pid, &q.date).await? else {
        return Ok(Json(serde_json::json!({"empty": true, "warnings": ["No snapshots imported yet."]})));
    };
    // Ruling 1 (Task 9 review, Task 11): a denied Reference read must not
    // silently render every clearing-obligation verdict "ok". Every future
    // with no resolvable spec falls back to `Category::Other` (see
    // `future_positions`), so with `contracts_all` denied EVERY position in
    // the fleet misclassifies into `CommodityOther` as well as defaulting
    // `otc: false` — the misclassification taints `avg_total_eur` and
    // `pct_of_threshold`, not only `avg_otc_eur`. Round 1 review (Important
    // 2): a computed "ok" verdict or a computed number beside the
    // `clearing_obligation` marker is still a pass-adjacent value one field
    // away, so every class's verdict AND every one of its computed numbers
    // are stamped unavailable/null here — only the static per-class shape
    // (class/label/threshold_eur/month labels) survives.
    let clearing_obligation = match &a.contracts_denied {
        Some(denied) => serde_json::json!({"status": "unavailable", "reason": denied.reason()}),
        None => serde_json::json!({"status": "ok"}),
    };
    let classes: Vec<serde_json::Value> = a.report.classes.iter().map(|c| {
        let mut v = serde_json::to_value(c).expect("ClassReport always serializes");
        if a.contracts_denied.is_some() {
            v["verdict"] = serde_json::json!("unavailable");
            v["avg_total_eur"] = serde_json::Value::Null;
            v["avg_otc_eur"] = serde_json::Value::Null;
            v["pct_of_threshold"] = serde_json::Value::Null;
            if let Some(months) = v["months"].as_array_mut() {
                for m in months.iter_mut() {
                    m["total_eur"] = serde_json::Value::Null;
                    m["otc_eur"] = serde_json::Value::Null;
                }
            }
        }
        v
    }).collect();
    let kpis_status = match &a.kpis_denied {
        Some(denied) => serde_json::json!({"status": "unavailable", "reason": denied.reason()}),
        None => serde_json::json!({"status": "ok"}),
    };
    Ok(Json(serde_json::json!({
        "dates": a.dates,
        "date": a.anchor,
        "months_present": a.report.months_present,
        "months_total": a.report.months_total,
        "classes": classes,
        "clearing_obligation": clearing_obligation,
        "warnings": a.report.warnings,
        "monitors": a.monitors,
        "monitors_note": "Counterparty breakdown unavailable: the reconciliation tier and compression trigger assume all OTC contracts face a single counterparty (the strictest reading).",
        "margin": a.margin,
        "futures_count": a.futures_count,
        "kpis": a.kpis,
        "kpis_status": kpis_status,
        "otc_note": "Only OTC positions count toward the clearing thresholds. Contracts on an EU regulated market or an equivalent third-country market are not OTC; flag any contract on a non-equivalent venue as OTC on the Data page.",
    })))
}

pub async fn export(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(pid): Path<i64>, Query(q): Query<DateQuery>,
) -> Result<impl IntoResponse, AppError> {
    let scoped = st.db.scope(&ctx);
    // Export needs its own action authorized (routes.rs gates this route on
    // Action::Export, not View); the reads inside `assemble` still take a
    // View token, which the Export grant already implies (`GrantSet` adds
    // the View entry alongside any non-View action).
    scoped.authorize::<Positions, Export>(pid)?;
    let a = scoped.authorize::<Positions, View>(pid)?;
    let portfolio = super::portfolios::ensure(&scoped, pid, false).await?;
    let Some(a) = assemble(&scoped, &a, pid, &q.date).await? else {
        return Err(AppError::Unprocessable(
            "no snapshots imported yet; there is nothing to evidence".into(),
        ));
    };
    // Ruling 1 (Task 9 review, Task 11): the export is an evidence document
    // whose whole purpose is the clearing-obligation verdict and the
    // contract/KPI listings behind it. Refuse outright rather than produce a
    // file that would read as a clean pass while the read it was built on
    // was denied — an explicit error, never a silently degraded document.
    if let Some(denied) = a.contracts_denied {
        return Err(AppError::from(denied));
    }
    if let Some(denied) = a.kpis_denied {
        return Err(AppError::from(denied));
    }
    let summary = a.report.classes.iter().map(|c| emir_file::SummaryRow {
        label: c.label.to_string(),
        threshold_eur: c.threshold_eur,
        avg_otc_eur: c.avg_otc_eur,
        pct_of_threshold: c.pct_of_threshold,
        verdict: c.verdict.as_str().to_string(),
        avg_total_eur: c.avg_total_eur,
    }).collect();
    let months = a.report.classes.iter().flat_map(|c| {
        c.months.iter().map(|m| emir_file::MonthRow {
            label: c.label.to_string(),
            month: m.month.format("%Y-%m").to_string(),
            snapshot_date: m.snapshot_date.map(|d| d.to_string()),
            total_eur: m.total_eur,
            otc_eur: m.otc_eur,
        })
    }).collect();
    let contracts = a.contracts.iter().map(|c| emir_file::ContractRow {
        root: c.contract_root.clone(),
        label: c.label.clone(),
        category: c.category.clone(),
        otc: c.otc,
        confirmed: c.confirmed,
        point_value: c.point_value,
        currency: c.currency.clone(),
    }).collect();
    let kpis = a.kpis.iter().map(|k| emir_file::KpiRow {
        month: k.month.format("%Y-%m").to_string(),
        unconfirmed_over_5d: k.unconfirmed_over_5d,
        reconciliation: k.reconciliation.clone(),
        disputes: k.disputes,
        note: k.note.clone().unwrap_or_default(),
    }).collect();
    let bytes = emir_file::build_evidence(&emir_file::EmirEvidence {
        anchor: a.anchor,
        months_present: a.report.months_present,
        months_total: a.report.months_total,
        summary,
        months,
        contracts,
        kpis,
        warnings: a.report.warnings.clone(),
    })?;

    let mut h = HeaderMap::new();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"));
    h.insert(header::CONTENT_DISPOSITION, super::download::attachment(
        &format!("EMIR - seuils - {} - {}.xlsx", portfolio.name, a.anchor)));
    crate::audit::record(&st, &ctx, "export", Some(Domain::Positions), Some(pid),
        serde_json::json!({"kind": "emir_evidence", "anchor": a.anchor, "portfolio": portfolio.name})).await;
    Ok((StatusCode::OK, h, bytes))
}

#[derive(serde::Deserialize)]
pub struct KpiBody {
    pub unconfirmed_over_5d: i32,
    pub reconciliation: String,
    pub disputes: i32,
    pub note: Option<String>,
}

pub async fn put_kpi(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path((pid, month)): Path<(i64, String)>,
    Json(b): Json<KpiBody>,
) -> Result<Json<db::repo::EmirKpi>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Settings, Configure>(pid)?;
    super::portfolios::ensure(&scoped, pid, true).await?;
    let month = month
        .parse::<NaiveDate>()
        .map_err(|_| AppError::BadRequest(format!("bad month: {month}")))?;
    if month.day() != 1 {
        return Err(AppError::Unprocessable("month must be a first-of-month date (YYYY-MM-01)".into()));
    }
    if !["done", "not_done", "not_applicable"].contains(&b.reconciliation.as_str()) {
        return Err(AppError::Unprocessable(
            "reconciliation must be one of done, not_done, not_applicable".into(),
        ));
    }
    if b.unconfirmed_over_5d < 0 || b.disputes < 0 {
        return Err(AppError::Unprocessable("counts must be >= 0".into()));
    }
    let k = db::repo::EmirKpi {
        month,
        unconfirmed_over_5d: b.unconfirmed_over_5d,
        reconciliation: b.reconciliation,
        disputes: b.disputes,
        note: b.note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()),
    };
    scoped.emir_kpi_upsert(&a, &k).await?;
    crate::audit::record(&st, &ctx, "configure", Some(Domain::Settings), Some(pid),
        serde_json::json!({"kind": "emir_kpi", "after": k})).await;
    Ok(Json(k))
}
