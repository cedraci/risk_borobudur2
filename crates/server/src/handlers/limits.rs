use crate::error::AppError;
use crate::handlers::refs::effective_bucket;
use crate::state::AppState;
use analytics::{concentration, default_issuer_group, liquidity, ConPosition, LiqPosition};
use axum::extract::{Query, State};
use axum::Json;
use chrono::NaiveDate;
use std::collections::HashMap;

#[derive(serde::Deserialize)]
pub struct DateQuery { date: Option<String> }

type Snapshot = (Vec<NaiveDate>, Option<NaiveDate>, Vec<db::repo::PositionRecord>, Vec<db::repo::InstrumentRef>);

async fn snapshot(st: &AppState, q: &DateQuery) -> Result<Snapshot, AppError> {
    let dates = db::repo::position_dates(&st.pool).await?;
    let date = match &q.date {
        Some(s) => Some(s.parse::<NaiveDate>().map_err(|_| AppError::BadRequest(format!("bad date: {s}")))?),
        None => dates.first().copied(),
    };
    let rows = match date {
        Some(d) => db::repo::positions_for(&st.pool, d).await?,
        None => Vec::new(),
    };
    let refs = db::repo::refs_all(&st.pool).await?;
    Ok((dates, date, rows, refs))
}

fn ref_map(refs: &[db::repo::InstrumentRef]) -> HashMap<&str, &db::repo::InstrumentRef> {
    refs.iter().map(|r| (r.code.as_str(), r)).collect()
}

pub async fn concentration_h(State(st): State<AppState>, Query(q): Query<DateQuery>) -> Result<Json<serde_json::Value>, AppError> {
    let (dates, date, rows, refs) = snapshot(&st, &q).await?;
    let by = ref_map(&refs);
    let cons: Vec<ConPosition> = rows.iter().filter_map(|p| {
        let w = p.weight?;
        let name = p.name.clone().unwrap_or_default();
        // fund_20 is per target fund: overrides don't regroup Fonds rows
        let group = if p.asset_type == "Fonds" {
            default_issuer_group(&p.asset_type, &name)
        } else {
            by.get(p.isin.as_str())
                .and_then(|r| r.issuer_group.clone())
                .unwrap_or_else(|| default_issuer_group(&p.asset_type, &name))
        };
        Some(ConPosition { asset_type: p.asset_type.clone(), group, weight: w })
    }).collect();
    Ok(Json(serde_json::json!({
        "dates": dates,
        "date": date,
        "checks": concentration(&cons),
        "excluded_note": "Futures are excluded from issuer limits (not issuer exposure under 5/10/40); fee and order provisions are excluded.",
    })))
}

pub async fn liquidity_h(State(st): State<AppState>, Query(q): Query<DateQuery>) -> Result<Json<serde_json::Value>, AppError> {
    let (dates, date, rows, refs) = snapshot(&st, &q).await?;
    let settings = db::settings::get_settings(&st.pool).await?;
    let by = ref_map(&refs);
    let liq: Vec<LiqPosition> = rows.iter().filter_map(|p| {
        let w = p.weight?;
        let override_ = by.get(p.isin.as_str()).and_then(|r| r.liquidity_bucket.as_deref());
        Some(LiqPosition {
            weight: w,
            bucket: effective_bucket(&settings.liquidity_defaults, &p.asset_type, override_),
        })
    }).collect();
    let report = liquidity(&liq, settings.redemption_shock);
    Ok(Json(serde_json::json!({
        "dates": dates,
        "date": date,
        "buckets": report.buckets,
        "cumulative": report.cumulative,
        "negative_memo": report.negative_memo,
        "shock": settings.redemption_shock,
        "stress_status": if report.stress_ok { "ok" } else { "breach" },
    })))
}

pub async fn rates_h(State(st): State<AppState>, Query(q): Query<DateQuery>) -> Result<Json<serde_json::Value>, AppError> {
    let (dates, date, rows, refs) = snapshot(&st, &q).await?;
    let by = ref_map(&refs);
    let mut bonds = Vec::new();
    let mut total_dv01 = 0.0f64;
    let mut md_weight_sum = 0.0f64;
    let mut missing_any = false;
    for p in rows.iter().filter(|p| p.asset_type == "Obligation") {
        let r = by.get(p.isin.as_str());
        let complete = r.map(|r| r.bond_coupon_pct.is_some() && r.bond_maturity.is_some() && r.bond_coupon_freq.is_some()).unwrap_or(false);
        let metrics = match (complete, p.price, p.valuation_eur, p.weight, date) {
            (true, Some(price), Some(mv), Some(w), Some(d)) => {
                let r = r.unwrap();
                analytics::bond_metrics(price, r.bond_coupon_pct.unwrap(), r.bond_coupon_freq.unwrap() as u32, d, r.bond_maturity.unwrap())
                    .map(|m| (m, price, mv, w, r))
            }
            _ => None,
        };
        match metrics {
            Some((m, price, mv, w, r)) => {
                let dv01 = m.modified * mv * 1e-4;
                total_dv01 += dv01;
                md_weight_sum += m.modified * w;
                bonds.push(serde_json::json!({
                    "code": p.isin, "name": p.name, "missing": false,
                    "coupon_pct": r.bond_coupon_pct, "maturity": r.bond_maturity, "freq": r.bond_coupon_freq,
                    "price": price, "ytm": m.ytm, "mod_duration": m.modified, "dv01_eur": dv01, "weight": w,
                }));
            }
            None => {
                missing_any = true;
                bonds.push(serde_json::json!({ "code": p.isin, "name": p.name, "missing": true }));
            }
        }
    }
    let futures_note: Vec<String> = rows.iter()
        .filter(|p| p.asset_type == "Future")
        .map(|p| p.name.clone().unwrap_or_else(|| p.isin.clone()))
        .collect();
    Ok(Json(serde_json::json!({
        "dates": dates,
        "date": date,
        "bonds": bonds,
        "total_dv01_eur": total_dv01,
        "nav_sensitivity_100bp": md_weight_sum * 0.01,
        "futures_note": futures_note,
        "missing_any": missing_any,
    })))
}

/// Futures positions for a snapshot, joined to their contract specs, with
/// prices decoded. Also reports roots whose spec is still unconfirmed.
fn future_positions(
    rows: &[db::repo::PositionRecord],
    specs: &[db::repo::FuturesContract],
) -> (Vec<analytics::FuturePosition>, Vec<String>) {
    let by_root: HashMap<&str, &db::repo::FuturesContract> =
        specs.iter().map(|c| (c.contract_root.as_str(), c)).collect();
    let mut out = Vec::new();
    let mut unconfirmed = Vec::new();
    for p in rows.iter().filter(|p| p.asset_type == "Future") {
        let ticker = p.ticker.clone().unwrap_or_else(|| p.isin.clone());
        let spec = analytics::contract_root(&ticker).and_then(|r| by_root.get(r.as_str()).copied());
        if let Some(s) = spec {
            if !s.confirmed {
                unconfirmed.push(s.contract_root.clone());
            }
        }
        let conv = spec
            .and_then(|s| analytics::PriceConvention::parse(&s.price_convention))
            .unwrap_or(analytics::PriceConvention::Decimal);
        out.push(analytics::FuturePosition {
            ticker,
            name: p.name.clone().unwrap_or_default(),
            currency: p.currency.clone().unwrap_or_default(),
            category: spec
                .and_then(|s| analytics::Category::parse(&s.category))
                .unwrap_or(analytics::Category::Other),
            qty: p.quantity.unwrap_or(0.0),
            price: analytics::decode_price(p.price.unwrap_or(0.0), conv),
            point_value: spec.and_then(|s| s.point_value),
            fx_rate: p.fx_rate,
        });
    }
    unconfirmed.sort();
    unconfirmed.dedup();
    (out, unconfirmed)
}

pub async fn derivatives_h(
    State(st): State<AppState>,
    Query(q): Query<DateQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (dates, date, rows, _refs) = snapshot(&st, &q).await?;
    let specs = db::repo::contracts_all(&st.pool).await?;
    let aum = match date {
        Some(d) => db::repo::aum_for(&st.pool, d).await?.unwrap_or(0.0),
        None => 0.0,
    };
    let (positions, unconfirmed) = future_positions(&rows, &specs);
    let rep = analytics::exposure(&positions, aum);
    Ok(Json(serde_json::json!({
        "dates": dates,
        "date": date,
        "aum": aum,
        "categories": rep.categories,
        "total": rep.total,
        "rows": rep.rows,
        "excluded": rep.excluded,
        "unconfirmed": unconfirmed,
        "note": "Notional by reference to the underlying; long and short each in absolute value as a percentage of net assets. No netting.",
    })))
}
