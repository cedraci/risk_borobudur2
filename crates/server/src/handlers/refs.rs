use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::Json;
use chrono::NaiveDate;
use std::collections::{HashMap, HashSet};

#[derive(serde::Serialize)]
pub struct RefRow {
    pub code: String,
    pub name: String,
    pub asset_type: String,
    pub effective_issuer_group: String,
    pub issuer_group_override: Option<String>,
    pub effective_days: f64,
    pub days_override: Option<f64>,
    pub adv_30d: Option<f64>,
    pub adv_asof: Option<NaiveDate>,
    pub adv_eligible: Option<bool>,
    pub market_place_name: Option<String>,
    pub bond_coupon_pct: Option<f64>,
    pub bond_maturity: Option<NaiveDate>,
    pub bond_coupon_freq: Option<i32>,
    pub is_bond: bool,
}

/// Effective days-to-liquidate: override, else asset-type default, else 1.
pub fn effective_days(defaults: &serde_json::Value, asset_type: &str, override_: Option<f64>) -> f64 {
    override_
        .or_else(|| defaults.get(asset_type).and_then(|v| v.as_f64()))
        .unwrap_or(1.0)
}

/// Every non-archived portfolio's latest-snapshot positions merged with
/// their instrument_refs rows, de-duplicated by code across the whole
/// fleet (e.g. an equity plus its dividend receivable, or the same
/// instrument held by more than one portfolio) — the editor is per
/// instrument, so where a code appears in several portfolios the first
/// portfolio walked (by id) wins for display context fields (name,
/// asset_type, effective_days via that portfolio's own liquidity
/// defaults).
pub async fn list(State(st): State<AppState>) -> Result<Json<Vec<RefRow>>, AppError> {
    let refs = db::repo::refs_all(&st.pool).await?;
    let by_code: HashMap<&str, &db::repo::InstrumentRef> =
        refs.iter().map(|r| (r.code.as_str(), r)).collect();

    let mut seen: HashSet<String> = HashSet::new();
    let mut rows = Vec::new();
    for pf in db::repo::portfolios_list(&st.pool).await?.iter().filter(|p| !p.archived) {
        let dates = db::repo::position_dates(&st.pool, pf.id).await?;
        let Some(latest) = dates.first().copied() else { continue };
        let positions = db::repo::positions_for(&st.pool, pf.id, latest).await?;
        let settings = db::settings::get_settings(&st.pool, pf.id).await?;
        for p in &positions {
            if !seen.insert(p.isin.clone()) { continue; }
            let name = p.name.clone().unwrap_or_default();
            let r = by_code.get(p.isin.as_str());
            let issuer_group_override = r.and_then(|r| r.issuer_group.clone());
            let days_override = r.and_then(|r| r.liquidity_days);
            rows.push(RefRow {
                code: p.isin.clone(),
                effective_issuer_group: issuer_group_override
                    .clone()
                    .unwrap_or_else(|| analytics::default_issuer_group(&p.asset_type, &name)),
                issuer_group_override,
                effective_days: effective_days(&settings.liquidity_default_days, &p.asset_type, days_override),
                days_override,
                adv_30d: r.and_then(|r| r.adv_30d),
                adv_asof: r.and_then(|r| r.adv_asof),
                adv_eligible: r.and_then(|r| r.adv_eligible),
                market_place_name: r.and_then(|r| r.market_place_name.clone()),
                bond_coupon_pct: r.and_then(|r| r.bond_coupon_pct),
                bond_maturity: r.and_then(|r| r.bond_maturity),
                bond_coupon_freq: r.and_then(|r| r.bond_coupon_freq),
                is_bond: p.asset_type == "Obligation",
                asset_type: p.asset_type.clone(),
                name,
            });
        }
    }
    Ok(Json(rows))
}

#[derive(serde::Deserialize)]
// The spec requires the depositary and Bloomberg columns to be *rejected* in
// the body, not silently dropped. Serde ignores unknown fields by default,
// which would let a client believe it had written adv_30d.
#[serde(deny_unknown_fields)]
pub struct RefBody {
    pub issuer_group: Option<String>,
    pub liquidity_days: Option<f64>,
    pub adv_eligible: Option<bool>,
    pub bond_coupon_pct: Option<f64>,
    pub bond_maturity: Option<NaiveDate>,
    pub bond_coupon_freq: Option<i32>,
}

pub async fn put(
    State(st): State<AppState>,
    Path(code): Path<String>,
    Json(b): Json<RefBody>,
) -> Result<Json<db::repo::InstrumentRef>, AppError> {
    if let Some(d) = b.liquidity_days {
        if !(0.0..=3650.0).contains(&d) || !d.is_finite() {
            return Err(AppError::Unprocessable("liquidity_days must be in [0, 3650]".into()));
        }
    }
    if let Some(c) = b.bond_coupon_pct {
        if !(0.0..=100.0).contains(&c) {
            return Err(AppError::Unprocessable("bond_coupon_pct must be in [0, 100]".into()));
        }
    }
    if let Some(f) = b.bond_coupon_freq {
        if ![1, 2, 4, 12].contains(&f) {
            return Err(AppError::Unprocessable("bond_coupon_freq must be 1, 2, 4 or 12".into()));
        }
    }
    if let Some(g) = &b.issuer_group {
        if g.trim().is_empty() {
            return Err(AppError::Unprocessable("issuer_group must not be blank (send null to revert)".into()));
        }
    }
    let r = db::repo::InstrumentRef {
        code,
        issuer_group: b.issuer_group.map(|g| g.trim().to_string()),
        liquidity_days: b.liquidity_days,
        adv_eligible: b.adv_eligible,
        bond_coupon_pct: b.bond_coupon_pct,
        bond_maturity: b.bond_maturity,
        bond_coupon_freq: b.bond_coupon_freq,
        bond_next_coupon: None,
        bond_nominal: None,
        market_place: None,
        market_place_name: None,
        adv_30d: None,
        adv_asof: None,
        country_of_risk: None,
        region: None,
        gics_sector: None,
        gics_industry: None,
        ticker: None,
    };
    db::repo::refs_upsert(&st.pool, &r).await?;
    Ok(Json(r))
}
