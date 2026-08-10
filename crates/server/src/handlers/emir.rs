//! EMIR monitoring: clearing thresholds, OTC obligation monitors, margin
//! view, monthly KPIs, and the evidence export.

use crate::error::AppError;
use crate::state::AppState;
use analytics::emir;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use chrono::{Datelike, NaiveDate};
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
}

/// One month-end's positions as EMIR sees them: the exposure path computes
/// the EUR notional (aum is irrelevant here, pass 0.0 — pct_nav is unused),
/// then each row picks up its contract's OTC flag by root.
async fn emir_positions(
    st: &AppState,
    pid: i64,
    date: NaiveDate,
    specs: &[db::repo::FuturesContract],
) -> Result<Vec<emir::EmirPosition>, AppError> {
    let rows = db::repo::positions_for(&st.pool, pid, date).await?;
    let snap = super::limits::future_positions(&rows, specs);
    let rep = analytics::exposure(&snap.positions, 0.0);
    Ok(rep
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
        .collect())
}

pub async fn assemble(st: &AppState, pid: i64, q_date: &Option<String>) -> Result<Option<Assembly>, AppError> {
    let dates = db::repo::position_dates(&st.pool, pid).await?;
    let anchor = match q_date {
        Some(s) => Some(s.parse::<NaiveDate>().map_err(|_| AppError::BadRequest(format!("bad date: {s}")))?),
        None => dates.first().copied(),
    };
    let Some(anchor) = anchor else { return Ok(None) };

    let specs = db::repo::contracts_all(&st.pool).await?;
    let mut months = Vec::with_capacity(12);
    for (month, chosen) in emir::month_window(anchor, &dates) {
        let snapshot = match chosen {
            Some(d) => Some((d, emir_positions(st, pid, d, &specs).await?)),
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
            let rows = db::repo::positions_for(&st.pool, pid, d).await?;
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
    let kpis = db::repo::emir_kpis_all(&st.pool, pid).await?;
    Ok(Some(Assembly { dates, anchor, report, monitors, margin, futures_count, kpis, contracts: specs }))
}

pub async fn get(
    State(st): State<AppState>,
    Path(pid): Path<i64>,
    Query(q): Query<DateQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    super::portfolios::ensure(&st.pool, pid, false).await?;
    let Some(a) = assemble(&st, pid, &q.date).await? else {
        return Ok(Json(serde_json::json!({"empty": true, "warnings": ["No snapshots imported yet."]})));
    };
    Ok(Json(serde_json::json!({
        "dates": a.dates,
        "date": a.anchor,
        "months_present": a.report.months_present,
        "months_total": a.report.months_total,
        "classes": a.report.classes,
        "warnings": a.report.warnings,
        "monitors": a.monitors,
        "monitors_note": "Counterparty breakdown unavailable: the reconciliation tier and compression trigger assume all OTC contracts face a single counterparty (the strictest reading).",
        "margin": a.margin,
        "futures_count": a.futures_count,
        "kpis": a.kpis,
        "otc_note": "Only OTC positions count toward the clearing thresholds. Contracts on an EU regulated market or an equivalent third-country market are not OTC; flag any contract on a non-equivalent venue as OTC on the Data page.",
    })))
}

pub async fn export(
    State(st): State<AppState>,
    Path(pid): Path<i64>,
    Query(q): Query<DateQuery>,
) -> Result<impl IntoResponse, AppError> {
    let portfolio = super::portfolios::ensure(&st.pool, pid, false).await?;
    let Some(a) = assemble(&st, pid, &q.date).await? else {
        return Err(AppError::Unprocessable(
            "no snapshots imported yet; there is nothing to evidence".into(),
        ));
    };
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
    h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_str(
        &format!("attachment; filename=\"EMIR - seuils - {} - {}.xlsx\"", portfolio.name, a.anchor))?);
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
    State(st): State<AppState>,
    Path((pid, month)): Path<(i64, String)>,
    Json(b): Json<KpiBody>,
) -> Result<Json<db::repo::EmirKpi>, AppError> {
    super::portfolios::ensure(&st.pool, pid, true).await?;
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
    db::repo::emir_kpi_upsert(&st.pool, pid, &k).await?;
    Ok(Json(k))
}
