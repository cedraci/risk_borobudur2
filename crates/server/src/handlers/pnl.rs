use crate::error::AppError;
use crate::state::AppState;
use analytics::pnl::{
    self, asset_class_of, decompose, futures_pnl, group_by, is_buy, net_flows, reconcile,
    Dimension, FxLookup, InstrumentPnl, NavPoint, Trade,
};
use axum::extract::{Query, State};
use axum::Json;
use chrono::NaiveDate;
use std::collections::{BTreeMap, HashMap};

#[derive(serde::Deserialize)]
pub struct PnlQuery {
    from: Option<String>,
    to: Option<String>,
    dimension: Option<String>,
}

fn parse_date(s: &str) -> Result<NaiveDate, AppError> {
    s.parse::<NaiveDate>().map_err(|_| AppError::BadRequest(format!("bad date: {s}")))
}

/// Nearest snapshot on or before `want`, falling back to the earliest
/// available. `dates` is descending, as `position_dates` returns it.
fn snap(dates: &[NaiveDate], want: NaiveDate) -> Option<NaiveDate> {
    dates.iter().copied().find(|d| *d <= want).or_else(|| dates.last().copied())
}

pub async fn get(State(st): State<AppState>, Query(q): Query<PnlQuery>) -> Result<Json<serde_json::Value>, AppError> {
    let dim = match q.dimension.as_deref() {
        None | Some("") => None,
        Some(s) => Some(Dimension::parse(s)
            .ok_or_else(|| AppError::BadRequest(format!("unknown dimension: {s}")))?),
    };

    let dates = db::repo::position_dates(&st.pool).await?;
    if dates.len() < 2 {
        return Ok(Json(serde_json::json!({
            "empty": true,
            "warnings": ["at least two imported NAV dates are needed to strike a P&L period"],
        })));
    }

    let requested_to = match &q.to { Some(s) => parse_date(s)?, None => dates[0] };
    let requested_from = match &q.from { Some(s) => parse_date(s)?, None => dates[dates.len() - 1] };
    if requested_from > requested_to {
        return Err(AppError::BadRequest("from is after to".into()));
    }
    // `dates` is non-empty (guarded above), so `snap` always yields a date;
    // the explicit error keeps that guarantee local instead of an unwrap.
    let nope = || AppError::Internal(anyhow::anyhow!("no position snapshot dates"));
    let t1 = snap(&dates, requested_to).ok_or_else(nope)?;
    let t0 = snap(&dates, requested_from).ok_or_else(nope)?;
    if t0 == t1 {
        return Ok(Json(serde_json::json!({
            "empty": true,
            "warnings": [format!("the requested range resolves to a single snapshot ({t0})")],
        })));
    }
    let snapshots = dates.iter().filter(|d| **d >= t0 && **d <= t1).count();

    let p0 = db::repo::positions_for(&st.pool, t0).await?;
    let p1 = db::repo::positions_for(&st.pool, t1).await?;
    let ops = db::repo::operations_all(&st.pool).await?;
    let divs = db::repo::dividends_all(&st.pool).await?;
    let refs = db::repo::refs_all(&st.pool).await?;
    let fx_rows = db::repo::fx_all(&st.pool).await?;
    let navs = db::repo::nav_rows(&st.pool).await?;

    let by_ref: HashMap<&str, &db::repo::InstrumentRef> =
        refs.iter().map(|r| (r.code.as_str(), r)).collect();

    // FX: daily history keyed by currency, plus snapshot rates from the file.
    let mut fx_by_ccy: BTreeMap<String, BTreeMap<NaiveDate, f64>> = BTreeMap::new();
    for r in &fx_rows {
        fx_by_ccy.entry(r.currency.clone()).or_default().insert(r.date, r.rate_to_eur);
    }
    let snap_rate = |rows: &[db::repo::PositionRecord], ccy: &str| -> f64 {
        if ccy == "EUR" { return 1.0; }
        rows.iter()
            .find(|p| p.currency.as_deref() == Some(ccy) && p.fx_rate.is_some_and(|f| f > 0.0))
            .and_then(|p| p.fx_rate)
            .or_else(|| rows.iter()
                .find(|p| p.currency.as_deref() == Some(ccy)
                       && p.valuation_ccy.is_some_and(|v| v.abs() > 1e-9))
                .and_then(|p| Some(p.valuation_eur? / p.valuation_ccy?)))
            .unwrap_or(1.0)
    };

    // Trades, keyed by ISIN.
    let mut trades_by_isin: HashMap<String, Vec<Trade>> = HashMap::new();
    let mut warnings: Vec<String> = Vec::new();
    for o in &ops {
        let (Some(isin), Some(qty), Some(px)) = (o.isin.as_deref(), o.quantity, o.net_price) else { continue };
        let Some(buy) = is_buy(&o.side) else {
            warnings.push(format!("{}: unrecognised side {:?}; trade ignored", isin, o.side));
            continue;
        };
        trades_by_isin.entry(isin.to_string()).or_default().push(Trade {
            trade_date: o.trade_date,
            isin: isin.to_string(),
            is_buy: buy,
            quantity: qty.abs(),
            net_price: px,
            net_amount: o.net_amount.unwrap_or(0.0),
            currency: o.currency.clone().unwrap_or_else(|| "EUR".into()),
        });
    }
    for v in trades_by_isin.values_mut() { v.sort_by_key(|t| t.trade_date); }

    let idx0: HashMap<&str, &db::repo::PositionRecord> = p0.iter().map(|p| (p.isin.as_str(), p)).collect();
    let idx1: HashMap<&str, &db::repo::PositionRecord> = p1.iter().map(|p| (p.isin.as_str(), p)).collect();
    let mut isins: Vec<&str> = idx0.keys().chain(idx1.keys()).copied().collect();
    isins.sort_unstable();
    isins.dedup();

    let mut rows: Vec<InstrumentPnl> = Vec::new();
    let (mut cash_and_margin, mut accrued_fees, mut provisions) = (0.0, 0.0, 0.0);

    for isin in isins {
        // `isin` came from these two maps, so one of them holds it.
        let Some(p) = idx1.get(isin).or_else(|| idx0.get(isin)).copied() else { continue };
        let class = asset_class_of(&p.asset_type);
        let ccy = p.currency.clone().unwrap_or_else(|| "EUR".into());

        let v0 = idx0.get(isin).and_then(|r| r.valuation_ccy).unwrap_or(0.0);
        let v1 = idx1.get(isin).and_then(|r| r.valuation_ccy).unwrap_or(0.0);
        let e0 = idx0.get(isin).and_then(|r| r.valuation_eur).unwrap_or(0.0);
        let e1 = idx1.get(isin).and_then(|r| r.valuation_eur).unwrap_or(0.0);

        // Balance-sheet classes are reconciliation lines, not instrument P&L.
        match class {
            "Cash" => { cash_and_margin += e1 - e0; continue; }
            "Fees" => { accrued_fees += e1 - e0; continue; }
            "Provisions" | "Income" => { provisions += e1 - e0; continue; }
            _ => {}
        }

        let fx = FxLookup {
            f0: snap_rate(&p0, &ccy),
            f1: snap_rate(&p1, &ccy),
            at_trade: fx_by_ccy.get(&ccy).cloned().unwrap_or_default(),
        };

        let decomp = if class == "Futures" {
            futures_pnl(v0, v1, 0.0, &fx)
        } else {
            let empty: Vec<Trade> = Vec::new();
            let t = trades_by_isin.get(isin).unwrap_or(&empty);
            let w = pnl::walk_instrument(t, t0, t1);
            if w.oversold {
                warnings.push(format!("{isin}: sells exceed recorded buys; figures incomplete"));
            }
            decompose(&w, v0, v1, &fx)
        };
        for d in &decomp.fx_missing {
            warnings.push(format!("{isin}: no FX rate for {ccy} on {d}; that flow is excluded"));
        }

        let r = by_ref.get(isin);
        rows.push(InstrumentPnl {
            isin: isin.to_string(),
            name: p.name.clone().unwrap_or_else(|| isin.to_string()),
            asset_class: class.to_string(),
            country: r.and_then(|r| r.country_of_risk.clone()),
            region: r.and_then(|r| r.region.clone()),
            sector: r.and_then(|r| r.gics_sector.clone()),
            industry: r.and_then(|r| r.gics_industry.clone()),
            currency: ccy,
            issuer_group: r.and_then(|r| r.issuer_group.clone()),
            decomp,
        });
    }

    let unclassified = rows.iter().filter(|r| r.country.is_none() && r.sector.is_none()).count();
    let investment_pnl: f64 = rows.iter().map(|r| r.decomp.total()).sum();
    let dividend_income: f64 = divs.iter()
        .filter(|d| d.provision_date > t0 && d.provision_date <= t1)
        .map(|d| d.amount)
        .sum();

    let points: Vec<NavPoint> = navs.iter()
        .map(|n| NavPoint { date: n.date, aum: n.aum, shares: n.shares, nav: n.nav })
        .collect();
    let aum0 = points.iter().find(|p| p.date == t0).map(|p| p.aum).unwrap_or(0.0);
    let aum1 = points.iter().find(|p| p.date == t1).map(|p| p.aum).unwrap_or(0.0);
    let recon = reconcile(
        investment_pnl, cash_and_margin, accrued_fees, provisions, dividend_income,
        aum1 - aum0, net_flows(&points, t0, t1),
    );

    let groups = match dim {
        Some(d) => group_by(rows, d),
        None => group_by(rows, Dimension::AssetClass),
    };

    Ok(Json(serde_json::json!({
        "empty": false,
        "period": {
            "requested_from": requested_from, "requested_to": requested_to,
            "actual_from": t0, "actual_to": t1, "snapshots": snapshots,
        },
        "groups": groups,
        "reconciliation": recon,
        "unclassified": unclassified,
        "warnings": warnings,
    })))
}
