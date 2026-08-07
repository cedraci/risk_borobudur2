//! EMIR monitoring: clearing thresholds, OTC obligation monitors, margin
//! view, monthly KPIs, and the evidence export.

use crate::error::AppError;
use crate::state::AppState;
use analytics::emir;
use axum::extract::{Query, State};
use axum::Json;
use chrono::NaiveDate;

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
    date: NaiveDate,
    specs: &[db::repo::FuturesContract],
) -> Result<Vec<emir::EmirPosition>, AppError> {
    let rows = db::repo::positions_for(&st.pool, date).await?;
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

pub async fn assemble(st: &AppState, q_date: &Option<String>) -> Result<Option<Assembly>, AppError> {
    let dates = db::repo::position_dates(&st.pool).await?;
    let anchor = match q_date {
        Some(s) => Some(s.parse::<NaiveDate>().map_err(|_| AppError::BadRequest(format!("bad date: {s}")))?),
        None => dates.first().copied(),
    };
    let Some(anchor) = anchor else { return Ok(None) };

    let specs = db::repo::contracts_all(&st.pool).await?;
    let mut months = Vec::with_capacity(12);
    for (month, chosen) in emir::month_window(anchor, &dates) {
        let snapshot = match chosen {
            Some(d) => Some((d, emir_positions(st, d, &specs).await?)),
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
            let rows = db::repo::positions_for(&st.pool, d).await?;
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
    let kpis = db::repo::emir_kpis_all(&st.pool).await?;
    Ok(Some(Assembly { dates, anchor, report, monitors, margin, futures_count, kpis, contracts: specs }))
}

pub async fn get(
    State(st): State<AppState>,
    Query(q): Query<DateQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let Some(a) = assemble(&st, &q.date).await? else {
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
