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
    let snap_rate_opt = |rows: &[db::repo::PositionRecord], ccy: &str| -> Option<f64> {
        if ccy == "EUR" { return Some(1.0); }
        rows.iter()
            .find(|p| p.currency.as_deref() == Some(ccy) && p.fx_rate.is_some_and(|f| f > 0.0))
            .and_then(|p| p.fx_rate)
            .or_else(|| rows.iter()
                .find(|p| p.currency.as_deref() == Some(ccy)
                       && p.valuation_ccy.is_some_and(|v| v.abs() > 1e-9))
                .and_then(|p| Some(p.valuation_eur? / p.valuation_ccy?)))
    };
    let snap_rate = |rows: &[db::repo::PositionRecord], ccy: &str| -> f64 {
        snap_rate_opt(rows, ccy).unwrap_or(1.0)
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
    // Raw balance-sheet deltas per reconciliation bucket. `cash_delta` and
    // `dividend_receivable_delta` are inputs to the netted reconciliation
    // lines assembled below, not lines themselves.
    let (mut cash_delta, mut accrued_fees, mut provisions, mut dividend_receivable_delta) =
        (0.0, 0.0, 0.0, 0.0);
    // Σ CF_eur: every trade flow the per-instrument decompositions added back,
    // translated exactly as they translated it.
    let mut trade_flows_eur = 0.0;

    for isin in isins {
        // `isin` came from these two maps, so one of them holds it.
        let Some(p) = idx1.get(isin).or_else(|| idx0.get(isin)).copied() else { continue };
        let class = asset_class_of(&p.asset_type);
        let ccy = p.currency.clone().unwrap_or_else(|| "EUR".into());

        let v0 = idx0.get(isin).and_then(|r| r.valuation_ccy).unwrap_or(0.0);
        let v1 = idx1.get(isin).and_then(|r| r.valuation_ccy).unwrap_or(0.0);
        let e0 = idx0.get(isin).and_then(|r| r.valuation_eur).unwrap_or(0.0);
        let e1 = idx1.get(isin).and_then(|r| r.valuation_eur).unwrap_or(0.0);

        // Balance-sheet classes feed reconciliation lines, not instrument
        // P&L. The `Dividendes` receivable is kept apart from `Provisions
        // ordres`: it is the balance-sheet side of the DIV-sheet accrual
        // (spec: "the receivable side of the same accrual") and must not be
        // recognised as P&L a second time - see the assembly below.
        match class {
            "Cash" => { cash_delta += e1 - e0; continue; }
            "Fees" => { accrued_fees += e1 - e0; continue; }
            "Provisions" => { provisions += e1 - e0; continue; }
            "Income" => { dividend_receivable_delta += e1 - e0; continue; }
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

        // Recover Σ CF_eur from the decomposition identity
        //   total = v1·f1 − v0·f0 + Σ CF·F(trade)
        // (grid-tested in analytics::pnl). Subtracting reuses exactly the
        // translation `decompose` applied - including its documented f0
        // fallback for trade dates with no FX rate - so the cash netting
        // below can never drift from what investment_pnl actually added
        // back. Futures contribute 0: their realized leg is passed as 0.0
        // and margin flows are not modelled, so total == v1·f1 − v0·f0.
        trade_flows_eur += decomp.total() - (v1 * fx.f1 - v0 * fx.f0);

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

    // Data-completeness badge for the Bloomberg classification round trip
    // (Task 12's toolbar), not a mirror of the requested `dimension`: it
    // always counts missing country+sector regardless of what the response
    // is grouped by, so "N instruments missing classification data" reads
    // the same whether grouping by asset_class, currency, or anything else.
    let unclassified = rows.iter().filter(|r| r.country.is_none() && r.sector.is_none()).count();
    let investment_pnl: f64 = rows.iter().map(|r| r.decomp.total()).sum();

    // DIV-sheet amounts are in the dividend's own currency (the fund holds
    // USD/GBP/CHF/DKK/SEK names). Convert at the provision-date rate from
    // fx_history - the accrual's own date, mirroring how trade flows are
    // translated - falling back to the end- then start-snapshot rate (the
    // rate the resulting receivable is carried at). A dividend whose
    // currency has no rate anywhere is excluded and warned, never silently
    // taken at par: the residual then shows the gap instead of hiding it.
    let mut dividend_income = 0.0;
    for d in divs.iter().filter(|d| d.provision_date > t0 && d.provision_date <= t1) {
        let rate = fx_by_ccy.get(&d.currency)
            .and_then(|m| m.get(&d.provision_date)).copied()
            .or_else(|| snap_rate_opt(&p1, &d.currency))
            .or_else(|| snap_rate_opt(&p0, &d.currency));
        match rate {
            Some(r) => dividend_income += d.amount * r,
            None => warnings.push(format!(
                "{}: no FX rate for the {} {} dividend provisioned {}; excluded from dividend income",
                d.issuer, d.amount, d.currency, d.provision_date)),
        }
    }

    let points: Vec<NavPoint> = navs.iter()
        .map(|n| NavPoint { date: n.date, aum: n.aum, shares: n.shares, nav: n.nav })
        .collect();
    let aum0 = points.iter().find(|p| p.date == t0).map(|p| p.aum).unwrap_or(0.0);
    let aum1 = points.iter().find(|p| p.date == t1).map(|p| p.aum).unwrap_or(0.0);
    let flows = net_flows(&points, t0, t1);

    // Reconciliation assembly.
    //
    // Target (the RHS `reconcile` compares against):  ΔAUM − F
    //   ΔAUM = AUM(t1) − AUM(t0), from nav_history
    //   F    = external net subscriptions/redemptions over (t0, t1],
    //          derived from share-count changes (`net_flows`)
    //
    // Balance sheet at a snapshot:  AUM = INV + CASH + FEES + PROV + DIVREC
    // (investments; cash & margin accounts; accrued-fee accruals; order
    // provisions; dividend receivables), so
    //
    //   ΔAUM = ΔINV + ΔCASH + ΔFEES + ΔPROV + ΔDIVREC.               (1)
    //
    // The explained lines are wired as
    //   investment_pnl  = Σ_i [Δvalue_eur(i) + ΣCF_eur(i)] = ΔINV + ΣCF
    //                     (per-instrument P&L adds trade flows back;
    //                      that is what makes it P&L)
    //   cash_and_margin = ΔCASH − ΣCF − F − D_cash                    (2)
    //   accrued_fees    = ΔFEES
    //   provisions      = ΔPROV            (Provisions ordres only)
    //   dividend_income = D_acc            (DIV-sheet accruals in (t0, t1],
    //                                       converted to EUR above)
    // with the dividend cash received, D_cash, given by the receivable
    // roll-forward  DIVREC(t1) = DIVREC(t0) + D_acc − D_cash:
    //
    //   D_cash = D_acc − ΔDIVREC.                                     (3)
    //
    // Each netting in (2) is double-entry, not judgement:
    //   ΣCF    - a trade settlement is an internal transfer between CASH
    //            and INV already counted inside investment_pnl; leaving it
    //            in the cash line books the settlement leg of every buy or
    //            sell as cash "P&L" a second time.
    //   F      - a subscription lands in CASH but is the target's own
    //            right-hand side, not performance.
    //   D_cash - a dividend payment moves the receivable into CASH; the
    //            income was recognised at accrual on the dividend_income
    //            line, so the receipt is a transfer.
    // And ΔDIVREC itself is not a line: the receivable is the balance-sheet
    // side of the very accrual dividend_income recognises (spec, Income),
    // so adding both would count every dividend twice in its provision
    // period. What remains on the cash line is FX revaluation of the cash,
    // margin and receivable balances, interest, and genuinely unexplained
    // cash movements - the spec's "(FX revaluation)" annotation.
    //
    // Summing the lines and substituting (3):
    //   total = (ΔINV + ΣCF) + (ΔCASH − ΣCF − F − D_acc + ΔDIVREC)
    //         + ΔFEES + ΔPROV + D_acc
    //         = ΔINV + ΔCASH + ΔFEES + ΔPROV + ΔDIVREC − F
    //         = ΔAUM − F                                       by (1).
    // So the residual is identically zero on internally consistent books,
    // and nonzero exactly when the data disagrees with itself: AUM not
    // tying to the position sheet, stored valuation_eur disagreeing with
    // valuation_ccy × fx, a flow or dividend with no FX rate, or trade
    // history missing against the snapshots.
    let dividend_cash_received = dividend_income - dividend_receivable_delta;
    let cash_and_margin = cash_delta - trade_flows_eur - flows - dividend_cash_received;

    let recon = reconcile(
        investment_pnl, cash_and_margin, accrued_fees, provisions, dividend_income,
        aum1 - aum0, flows,
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
