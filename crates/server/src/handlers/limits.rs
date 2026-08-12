use crate::error::AppError;
use crate::handlers::refs::effective_days;
use crate::state::AppState;
use analytics::{concentration, default_issuer_group, ConPosition};
use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::NaiveDate;
use std::collections::HashMap;

#[derive(serde::Deserialize)]
pub struct DateQuery { date: Option<String> }

type Snapshot = (Vec<NaiveDate>, Option<NaiveDate>, Vec<db::repo::PositionRecord>, Vec<db::repo::InstrumentRef>);

async fn snapshot(st: &AppState, pid: i64, q: &DateQuery) -> Result<Snapshot, AppError> {
    let dates = db::repo::position_dates(&st.pool, pid).await?;
    let date = match &q.date {
        Some(s) => Some(s.parse::<NaiveDate>().map_err(|_| AppError::BadRequest(format!("bad date: {s}")))?),
        None => dates.first().copied(),
    };
    let rows = match date {
        Some(d) => db::repo::positions_for(&st.pool, pid, d).await?,
        None => Vec::new(),
    };
    let refs = db::repo::refs_all(&st.pool).await?;
    Ok((dates, date, rows, refs))
}

fn ref_map(refs: &[db::repo::InstrumentRef]) -> HashMap<&str, &db::repo::InstrumentRef> {
    refs.iter().map(|r| (r.code.as_str(), r)).collect()
}

pub async fn concentration_h(State(st): State<AppState>, Path(pid): Path<i64>, Query(q): Query<DateQuery>) -> Result<Json<serde_json::Value>, AppError> {
    super::portfolios::ensure(&st.pool, pid, false).await?;
    let (dates, date, rows, refs) = snapshot(&st, pid, &q).await?;
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

pub async fn liquidity_h(State(st): State<AppState>, Path(pid): Path<i64>, Query(q): Query<DateQuery>) -> Result<Json<serde_json::Value>, AppError> {
    super::portfolios::ensure(&st.pool, pid, false).await?;
    let (dates, date, rows, refs) = snapshot(&st, pid, &q).await?;
    let settings = db::settings::get_settings(&st.pool, pid).await?;
    let by = ref_map(&refs);
    let nav = db::repo::aum_for(&st.pool, pid, date.unwrap_or_default()).await?.unwrap_or(0.0);
    let caps: Vec<analytics::Capacity> = rows.iter().filter_map(|p| {
        let v = p.valuation_eur?;
        if v <= 0.0 { return None; }
        let r = by.get(p.isin.as_str());
        Some(analytics::capacity(&analytics::LiqPosition {
            code: p.isin.clone(), asset_type: p.asset_type.clone(), valuation_eur: v,
            quantity: p.quantity, adv_30d: None, adv_stale: false,
            adv_eligible: r.and_then(|r| r.adv_eligible), market_place: None,
            liquidity_days: r.and_then(|r| r.liquidity_days),
            default_days: effective_days(&settings.liquidity_default_days, &p.asset_type, None),
        }, settings.participation_rate, 1.0))
    }).collect();
    let profile = analytics::asset_profile(&caps, nav);
    let negative_memo: f64 = rows.iter().filter_map(|p| p.weight).filter(|w| *w < 0.0).sum();
    Ok(Json(serde_json::json!({
        "dates": dates,
        "date": date,
        "buckets": profile.buckets,
        "cumulative": profile.cumulative,
        "negative_memo": negative_memo,
        "shock": settings.redemption_shock,
        "stress_status": if profile.cumulative[1].weight >= settings.redemption_shock { "ok" } else { "breach" },
    })))
}

pub async fn rates_h(State(st): State<AppState>, Path(pid): Path<i64>, Query(q): Query<DateQuery>) -> Result<Json<serde_json::Value>, AppError> {
    super::portfolios::ensure(&st.pool, pid, false).await?;
    let (dates, date, rows, refs) = snapshot(&st, pid, &q).await?;
    let by = ref_map(&refs);
    let mut bonds = Vec::new();
    let mut total_dv01 = 0.0f64;
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
    // Bond futures: only contracts classified interest_rate, and only where
    // CTD analytics exist for this exact NAV date. No carry-forward.
    let specs = db::repo::contracts_all(&st.pool).await?;
    let snap = future_positions(&rows, &specs);
    let unconfirmed: std::collections::HashSet<&str> =
        snap.unconfirmed.iter().map(String::as_str).collect();
    let ctd = match date {
        Some(d) => db::repo::ctd_for(&st.pool, pid, d).await?,
        None => Vec::new(),
    };
    let mut futures = Vec::new();
    // A `Future` row with no resolvable spec cannot pass the candidate filter
    // below, so it never gets a chance to set this flag from inside the loop -
    // yet its DV01 is just as absent from the total. `futures_missing_any:
    // false` is a positive assertion of completeness and must not be made while
    // a futures row in the snapshot could not be evaluated at all.
    let mut futures_missing_any = !snap.no_spec.is_empty();
    // Candidate bond futures: contracts confirmed `interest_rate`, plus
    // contracts still sitting at the import-time seed's default `other` -
    // for any `Comdty`-suffixed ticker, since a bare ticker can't distinguish
    // a bond future from a commodity future (see db::repo::import_workbook)
    // - AND whose root the user has not yet confirmed. `other` is only a
    // placeholder while unconfirmed; once the user confirms a spec, that
    // confirmation is authoritative. A root confirmed `interest_rate` stays
    // (or joins) here; a root confirmed to any other category (including a
    // deliberate, terminal `other`) drops out for good, instead of sitting
    // forever with `missing: true` and pinning `futures_missing_any`.
    for f in snap.positions.iter().filter(|f| {
        f.category == analytics::Category::InterestRate
            || (f.category == analytics::Category::Other
                && analytics::contract_root(&f.ticker).is_some_and(|r| unconfirmed.contains(r.as_str())))
    }) {
        let a = ctd.iter().find(|c| c.ticker == f.ticker);
        let dv01 = match (a, f.point_value, f.fx_rate, f.qty) {
            (Some(a), Some(pv), Some(fx), Some(qty)) => analytics::dv01_position(
                &analytics::CtdAnalytics {
                    mod_duration: a.ctd_mod_duration,
                    clean_price: a.ctd_clean_price,
                    accrued: a.ctd_accrued,
                    conversion_factor: a.conversion_factor,
                },
                pv,
                qty,
                fx,
            ),
            _ => None,
        };
        match dv01 {
            Some(d) => {
                total_dv01 += d;
                let a = a.unwrap();
                futures.push(serde_json::json!({
                    "ticker": f.ticker, "name": f.name, "missing": false,
                    "qty": f.qty, "price": f.price, "point_value": f.point_value,
                    "ctd_isin": a.ctd_isin, "ctd_mod_duration": a.ctd_mod_duration,
                    "conversion_factor": a.conversion_factor, "dv01_eur": d,
                    "curve": specs.iter()
                        .find(|s| Some(&s.contract_root) == analytics::contract_root(&f.ticker).as_ref())
                        .and_then(|s| s.curve.clone()),
                }));
            }
            None => {
                futures_missing_any = true;
                futures.push(serde_json::json!({
                    "ticker": f.ticker, "name": f.name, "missing": true,
                    "qty": f.qty, "price": f.price, "point_value": f.point_value,
                }));
            }
        }
    }
    let aum = match date {
        Some(d) => db::repo::aum_for(&st.pool, pid, d).await?,
        None => None,
    };
    // Signed P&L, not a magnitude. `dv01 = modified x mv x 1e-4` is positive
    // for a long bond, and the price relation is `dP = -D_mod x P x dy`, so a
    // +100bp move changes net assets by `-100 x total_dv01`. A long bond book
    // therefore reports a NEGATIVE sensitivity (it loses on a rate rise) and a
    // book that is net short rates once futures are counted reports a positive
    // one. Without the minus sign the figure read as the exact opposite of its
    // own label the moment futures could push the total negative.
    //
    // The denominator is AUM at the same NAV date. An unknown AUM yields null,
    // not 0.00% - a confident zero next to a large non-zero DV01 is a lie, and
    // the derivatives handler already routes the same missing input to null.
    let nav_sensitivity_100bp = match aum {
        Some(a) if a > 0.0 => serde_json::json!(-100.0 * total_dv01 / a),
        _ => serde_json::Value::Null,
    };
    Ok(Json(serde_json::json!({
        "dates": dates,
        "date": date,
        "bonds": bonds,
        "futures": futures,
        "total_dv01_eur": total_dv01,
        // 100bp in EUR as a fraction of net assets, i.e. total DV01 scaled
        // from 1bp to 100bp and divided by AUM. This replaces the old
        // sum(modified x weight) x 0.01, which inherited a unit mismatch
        // from the source workbook's own `Poids` column (it adds unconverted
        // accrued interest to a EUR valuation); the DV01-based figure is the
        // more defensible of the two, and it extends naturally to futures,
        // which carry no market-value weight at all.
        "nav_sensitivity_100bp": nav_sensitivity_100bp,
        "missing_any": missing_any,
        "futures_missing_any": futures_missing_any,
        // Futures held in this snapshot that carry no contract spec at all.
        // They are absent from the DV01 above for a different reason than a
        // missing CTD row, and the fix is a different one, so they are named
        // separately rather than folded into the CTD prompt.
        "futures_no_spec": snap.no_spec,
    })))
}

/// Futures positions for a snapshot, joined to their contract specs, with
/// prices decoded.
pub(crate) struct FuturesSnapshot {
    pub(crate) positions: Vec<analytics::FuturePosition>,
    /// Roots whose spec exists but the user has not confirmed yet.
    pub(crate) unconfirmed: Vec<String>,
    /// Tickers of `Future` rows with no resolvable spec at all: the root is
    /// absent from `futures_contracts`, or the ticker would not parse into one.
    /// Such a row is neither `interest_rate` nor `unconfirmed`, so the rates
    /// candidate filter below drops it; any completeness claim made by the
    /// rates section has to consult this list first.
    no_spec: Vec<String>,
}

pub(crate) fn future_positions(
    rows: &[db::repo::PositionRecord],
    specs: &[db::repo::FuturesContract],
) -> FuturesSnapshot {
    let by_root: HashMap<&str, &db::repo::FuturesContract> =
        specs.iter().map(|c| (c.contract_root.as_str(), c)).collect();
    let mut out = Vec::new();
    let mut unconfirmed = Vec::new();
    let mut no_spec = Vec::new();
    for p in rows.iter().filter(|p| p.asset_type == "Future") {
        let ticker = p.ticker.clone().unwrap_or_else(|| p.isin.clone());
        let spec = analytics::contract_root(&ticker).and_then(|r| by_root.get(r.as_str()).copied());
        match spec {
            Some(s) if !s.confirmed => unconfirmed.push(s.contract_root.clone()),
            Some(_) => {}
            None => no_spec.push(ticker.clone()),
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
            // Absent, not zero: a blank or `#VALUE!` price cell is a state the
            // ingest layer deliberately tolerates without erroring, and
            // `unwrap_or(0.0)` here turned it into a fully populated row
            // reporting exactly zero exposure and zero DV01.
            qty: p.quantity,
            price: p.price.map(|x| analytics::decode_price(x, conv)),
            point_value: spec.and_then(|s| s.point_value),
            fx_rate: p.fx_rate,
            unconfirmed: spec.is_some_and(|s| !s.confirmed),
        });
    }
    unconfirmed.sort();
    unconfirmed.dedup();
    FuturesSnapshot { positions: out, unconfirmed, no_spec }
}

pub async fn derivatives_h(
    State(st): State<AppState>,
    Path(pid): Path<i64>,
    Query(q): Query<DateQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    super::portfolios::ensure(&st.pool, pid, false).await?;
    let (dates, date, rows, _refs) = snapshot(&st, pid, &q).await?;
    let specs = db::repo::contracts_all(&st.pool).await?;
    let aum = match date {
        Some(d) => db::repo::aum_for(&st.pool, pid, d).await?.unwrap_or(0.0),
        None => 0.0,
    };
    let snap = future_positions(&rows, &specs);
    let rep = analytics::exposure(&snap.positions, aum);
    Ok(Json(serde_json::json!({
        "dates": dates,
        "date": date,
        "aum": aum,
        "categories": rep.categories,
        "total": rep.total,
        "rows": rep.rows,
        "excluded": rep.excluded,
        "unconfirmed": snap.unconfirmed,
        "note": "Notional by reference to the underlying; long and short each in absolute value as a percentage of net assets. No netting.",
    })))
}
