use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::Json;
use chrono::NaiveDate;
use std::collections::{HashMap, HashSet};

pub const BUCKETS: [&str; 4] = ["d1", "d2_7", "d8_30", "d30p"];

#[derive(serde::Serialize)]
pub struct RefRow {
    pub code: String,
    pub name: String,
    pub asset_type: String,
    pub effective_issuer_group: String,
    pub issuer_group_override: Option<String>,
    pub effective_bucket: String,
    pub bucket_override: Option<String>,
    pub bond_coupon_pct: Option<f64>,
    pub bond_maturity: Option<NaiveDate>,
    pub bond_coupon_freq: Option<i32>,
    pub is_bond: bool,
}

/// Effective liquidity bucket: override, else asset-type default, else d1.
pub fn effective_bucket(defaults: &serde_json::Value, asset_type: &str, override_: Option<&str>) -> String {
    override_
        .map(str::to_string)
        .or_else(|| defaults.get(asset_type).and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_else(|| "d1".into())
}

/// Latest-snapshot positions merged with their instrument_refs rows,
/// de-duplicated by code (e.g. an equity plus its dividend receivable).
pub async fn list(State(st): State<AppState>) -> Result<Json<Vec<RefRow>>, AppError> {
    let dates = db::repo::position_dates(&st.pool).await?;
    let Some(latest) = dates.first().copied() else { return Ok(Json(Vec::new())); };
    let positions = db::repo::positions_for(&st.pool, latest).await?;
    let refs = db::repo::refs_all(&st.pool).await?;
    let settings = db::settings::get_settings(&st.pool).await?;
    let by_code: HashMap<&str, &db::repo::InstrumentRef> =
        refs.iter().map(|r| (r.code.as_str(), r)).collect();

    let mut seen: HashSet<&str> = HashSet::new();
    let mut rows = Vec::new();
    for p in &positions {
        if !seen.insert(p.isin.as_str()) { continue; }
        let name = p.name.clone().unwrap_or_default();
        let r = by_code.get(p.isin.as_str());
        let issuer_group_override = r.and_then(|r| r.issuer_group.clone());
        let bucket_override = r.and_then(|r| r.liquidity_bucket.clone());
        rows.push(RefRow {
            code: p.isin.clone(),
            effective_issuer_group: issuer_group_override
                .clone()
                .unwrap_or_else(|| analytics::default_issuer_group(&p.asset_type, &name)),
            issuer_group_override,
            effective_bucket: effective_bucket(&settings.liquidity_defaults, &p.asset_type, bucket_override.as_deref()),
            bucket_override,
            bond_coupon_pct: r.and_then(|r| r.bond_coupon_pct),
            bond_maturity: r.and_then(|r| r.bond_maturity),
            bond_coupon_freq: r.and_then(|r| r.bond_coupon_freq),
            is_bond: p.asset_type == "Obligation",
            asset_type: p.asset_type.clone(),
            name,
        });
    }
    Ok(Json(rows))
}

#[derive(serde::Deserialize)]
pub struct RefBody {
    pub issuer_group: Option<String>,
    pub liquidity_bucket: Option<String>,
    pub bond_coupon_pct: Option<f64>,
    pub bond_maturity: Option<NaiveDate>,
    pub bond_coupon_freq: Option<i32>,
}

pub async fn put(
    State(st): State<AppState>,
    Path(code): Path<String>,
    Json(b): Json<RefBody>,
) -> Result<Json<db::repo::InstrumentRef>, AppError> {
    if let Some(bkt) = &b.liquidity_bucket {
        if !BUCKETS.contains(&bkt.as_str()) {
            return Err(AppError::Unprocessable(format!("liquidity_bucket must be one of {BUCKETS:?}")));
        }
    }
    if let Some(c) = b.bond_coupon_pct {
        if !(0.0..=100.0).contains(&c) {
            return Err(AppError::Unprocessable("bond_coupon_pct must be in [0, 100]".into()));
        }
    }
    if let Some(f) = b.bond_coupon_freq {
        if f != 1 && f != 2 {
            return Err(AppError::Unprocessable("bond_coupon_freq must be 1 or 2".into()));
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
        liquidity_bucket: b.liquidity_bucket,
        bond_coupon_pct: b.bond_coupon_pct,
        bond_maturity: b.bond_maturity,
        bond_coupon_freq: b.bond_coupon_freq,
    };
    db::repo::refs_upsert(&st.pool, &r).await?;
    Ok(Json(r))
}
