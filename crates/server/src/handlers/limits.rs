use crate::error::AppError;
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

/// A register whose as-of date is older than this is flagged stale. It is not
/// a setting: the register is a compliance artefact with no per-portfolio
/// cadence to calibrate against, and a quarter is the interval at which one
/// would normally be refreshed.
const REGISTER_MAX_AGE_DAYS: i64 = 90;

fn build_positions(
    rows: &[db::repo::PositionRecord],
    by: &HashMap<&str, &db::repo::InstrumentRef>,
    settings: &db::settings::AppSettings,
    asof: chrono::NaiveDate,
) -> Vec<analytics::LiqPosition> {
    rows.iter().filter_map(|p| {
        let v = p.valuation_eur?;
        if v <= 0.0 { return None; }  // negatives are a cash need, not a sale
        let r = by.get(p.isin.as_str());
        Some(analytics::LiqPosition {
            code: p.isin.clone(),
            asset_type: p.asset_type.clone(),
            valuation_eur: v,
            quantity: p.quantity,
            adv_30d: r.and_then(|r| r.adv_30d),
            adv_stale: r.and_then(|r| r.adv_asof)
                .map(|d| (asof - d).num_days() > settings.adv_max_age_days as i64)
                // No as-of at all is "no adv", reported by its own reason.
                .unwrap_or(false),
            adv_eligible: r.and_then(|r| r.adv_eligible),
            market_place: r.and_then(|r| r.market_place.clone()),
            liquidity_days: r.and_then(|r| r.liquidity_days),
            default_days: super::refs::effective_days(
                &settings.liquidity_default_days, &p.asset_type, None),
        })
    }).collect()
}

pub async fn liquidity_h(
    State(st): State<AppState>, Path(pid): Path<i64>, Query(q): Query<DateQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    super::portfolios::ensure(&st.pool, pid, false).await?;
    let (dates, date, rows, refs) = snapshot(&st, pid, &q).await?;
    let settings = db::settings::get_settings(&st.pool, pid).await?;
    let by = ref_map(&refs);
    let horizon = settings.liquidity_horizon_days;

    let params = serde_json::json!({
        "participation_rate": settings.participation_rate,
        "adv_stress_factor": settings.adv_stress_factor,
        "liquidity_horizon_days": horizon,
        "settlement_deadline_days": settings.settlement_deadline_days,
        "adv_max_age_days": settings.adv_max_age_days,
        "redemption_shock": settings.redemption_shock,
        "day_unit": "business days (Mon-Fri, no holiday calendar)",
    });

    // An absent snapshot or NAV returns the established empty shape rather
    // than an error, matching every other metrics endpoint.
    let (Some(asof), Some(nav)) = (date, match date {
        Some(d) => db::repo::aum_for(&st.pool, pid, d).await?,
        None => None,
    }) else {
        return Ok(Json(serde_json::json!({
            "dates": dates, "date": date, "nav": null, "params": params,
            "coverage": serde_json::Value::Null, "asset": serde_json::Value::Null,
            "scenarios": [], "negative_memo": 0.0, "negative_memo_eur": 0.0,
        })));
    };

    let positions = build_positions(&rows, &by, &settings, asof);
    let cap_at = |stress: f64| -> Vec<analytics::Capacity> {
        positions.iter().map(|p| analytics::capacity(p, settings.participation_rate, stress)).collect()
    };
    let normal = cap_at(1.0);
    let stressed = cap_at(settings.adv_stress_factor);

    let negative_eur: f64 = rows.iter().filter_map(|p| p.valuation_eur).filter(|v| *v < 0.0).sum();
    let negative_memo: f64 = rows.iter().filter_map(|p| p.weight).filter(|w| *w < 0.0).sum();

    // Coupon and redemption inflows, from the depositary's own schedule.
    // CACEIS derives fx_rate from market-value-EUR / market-value-local, so
    // it is NULL when the local market value is missing. A missing rate is
    // never defaulted to parity (that would silently convert a non-EUR
    // coupon at 1.0, a unit assumption on a cash inflow); the bond is
    // skipped and surfaced in the coverage block instead.
    let mut coupon_inputs: Vec<analytics::CouponInput> = Vec::new();
    let mut fx_gaps: Vec<analytics::CouponGap> = Vec::new();
    for p in rows.iter().filter(|p| p.asset_type == "Obligation") {
        let Some(r) = by.get(p.isin.as_str()) else { continue };
        let Some(fx_rate) = p.fx_rate else {
            fx_gaps.push(analytics::CouponGap { code: p.isin.clone(), reason: "no fx rate" });
            continue;
        };
        coupon_inputs.push(analytics::CouponInput {
            code: p.isin.clone(),
            quantity: p.quantity.unwrap_or(0.0),
            coupon_pct: r.bond_coupon_pct,
            // Only a fixed coupon reaches instrument_refs at all, so its
            // presence is the FIX gate the parser already applied.
            coupon_type: r.bond_coupon_pct.map(|_| "FIX".to_string()),
            next_coupon: r.bond_next_coupon,
            maturity: r.bond_maturity,
            freq: r.bond_coupon_freq,
            accrued_eur: p.accrued_interest,
            fx_rate,
        });
    }
    let mut coupons = analytics::bond_inflows(&coupon_inputs, asof, horizon);
    coupons.gaps.extend(fx_gaps);

    let register = db::repo::shareholders_for(&st.pool, pid).await?;
    let top5_pct: f64 = register.iter().take(5).map(|s| s.pct_of_nav).sum::<f64>() / 100.0;

    let scenario = |key: &str, required_pct: Option<f64>, caps: &[analytics::Capacity]| -> serde_json::Value {
        let Some(pct) = required_pct else {
            return serde_json::json!({
                "key": key, "status": "unavailable", "reason": "no shareholder register",
            });
        };
        let required = pct * nav;
        let w = analytics::waterfall(caps, &coupons.inflows, negative_eur, required, horizon);
        let status = match w.days {
            Some(d) if d <= settings.settlement_deadline_days => "ok",
            _ => "breach",
        };
        let curve: Vec<serde_json::Value> = (1..=horizon).map(|d| serde_json::json!({
            "day": d,
            "available_eur": analytics::available(caps, &coupons.inflows, negative_eur, d),
        })).collect();
        serde_json::json!({
            "key": key,
            "required_eur": required,
            "required_pct": pct,
            "register_count": register.len().min(5),
            "status": status,
            "waterfall": w,
            "slice_days": analytics::slice_days(caps, required, nav),
            "residual": analytics::residual(caps, required, nav, w.days.unwrap_or(horizon)),
            "curve": curve,
        })
    };

    let top5 = (!register.is_empty()).then_some(top5_pct);
    let fixed = Some(settings.redemption_shock);
    let scenarios = vec![
        scenario("top5", top5, &normal),
        scenario("fixed", fixed, &normal),
        scenario("hybrid_top5", top5, &stressed),
        scenario("hybrid_fixed", fixed, &stressed),
    ];

    let measured_eur: f64 = normal.iter().filter(|c| c.measured).map(|c| c.valuation_eur).sum();
    let fallbacks: Vec<serde_json::Value> = normal.iter()
        .filter_map(|c| c.reason.map(|r| serde_json::json!({"code": c.code, "reason": r})))
        .collect();

    Ok(Json(serde_json::json!({
        "dates": dates,
        "date": date,
        "nav": nav,
        "params": params,
        "coverage": {
            "adv_pct_of_nav": if nav > 0.0 { measured_eur / nav } else { 0.0 },
            "fallbacks": fallbacks,
            "coupon_gaps": coupons.gaps,
            "register": {
                "count": register.len(),
                "as_of": register.iter().map(|s| s.as_of).min(),
                "stale": register.iter().any(|s| (asof - s.as_of).num_days() > REGISTER_MAX_AGE_DAYS),
            },
        },
        "asset": {
            "normal": analytics::asset_profile(&normal, nav),
            "stressed": analytics::asset_profile(&stressed, nav),
        },
        "scenarios": scenarios,
        "negative_memo": negative_memo,
        "negative_memo_eur": negative_eur,
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
