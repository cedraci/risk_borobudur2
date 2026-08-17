use chrono::NaiveDate;
use ingest::ParsedWorkbook;
use sqlx::PgPool;

use super::positions::derive_dividends;
use super::reference::{FuturesContract, SELECT_CONTRACTS};
use super::shareholders::flows_upsert;

#[derive(Debug, serde::Serialize)]
pub struct ImportOutcome {
    pub import_id: i64,
    pub duplicate: bool,
    pub nav_rows: usize,
    pub positions: usize,
    pub dividends: usize,
    pub operations: usize,
    pub div_ops_replaced: bool,
    /// Non-fatal futures spec problems. A new or mis-specified contract must
    /// never block the weekly NAV import.
    pub warnings: Vec<String>,
}

#[derive(Debug, sqlx::FromRow, serde::Serialize)]
pub struct ImportRecord {
    pub id: i64,
    pub filename: String,
    pub nav_date: NaiveDate,
    pub imported_at: chrono::DateTime<chrono::Utc>,
    pub row_counts: serde_json::Value,
}

pub async fn import_batch(pool: &PgPool, portfolio_id: i64, filename: &str, sha256: &str, b: &ingest::adapter::UniversalBatch) -> anyhow::Result<ImportOutcome> {
    let all_positions = || b.snapshots.iter().flat_map(|s| s.positions.iter());

    if let Some((id,)) = sqlx::query_as::<_, (i64,)>("SELECT id FROM imports WHERE portfolio_id = $1 AND sha256 = $2")
        .bind(portfolio_id).bind(sha256).fetch_optional(pool).await?
    {
        // Duplicate: nothing re-ingested, but futures spec seeding still runs
        // (same rationale as before — repair path for pre-futures databases).
        let mut tx = pool.begin().await?;
        let positions: Vec<ingest::PositionRow> = all_positions().cloned().collect();
        let warnings = seed_futures_contracts(&mut tx, &positions).await?;
        tx.commit().await?;
        return Ok(ImportOutcome {
            import_id: id, duplicate: true, nav_rows: 0, positions: 0,
            dividends: 0, operations: 0, div_ops_replaced: false, warnings,
        });
    }

    let mut tx = pool.begin().await?;

    // Only ever compare a journal-bearing batch's date against OTHER
    // journal-bearing imports — CACEIS CSV imports also create `imports`
    // rows now, and their nav_date (typically daily, so almost always newer
    // than a weekly recap's own date) must never poison this gate.
    let prev_latest: Option<NaiveDate> =
        sqlx::query_scalar("SELECT max(nav_date) FROM imports WHERE portfolio_id = $1 AND has_div_ops")
            .bind(portfolio_id).fetch_one(&mut *tx).await?;
    let has_div_ops = b.dividends.is_some() || b.operations.is_some();
    let replace_div_ops = has_div_ops && prev_latest.is_none_or(|d| b.primary_date >= d);

    let nav_rows = b.nav_points.len();
    let n_positions: usize = b.snapshots.iter().map(|s| s.positions.len()).sum();
    let n_div = b.dividends.as_ref().map_or(0, |d| d.len());
    let n_ops = b.operations.as_ref().map_or(0, |o| o.len());
    let mut row_counts = serde_json::json!({
        "nav_rows": nav_rows, "positions": n_positions,
        "dividends": if replace_div_ops { n_div } else { 0 },
        "operations": if replace_div_ops { n_ops } else { 0 },
    });
    if !b.warnings.is_empty() {
        row_counts["warnings"] = serde_json::json!(b.warnings);
    }
    if let Some(rows) = &b.flows {
        flows_upsert(&mut tx, portfolio_id, rows).await?;
        row_counts["flows"] = serde_json::json!(rows.len());
    }
    let (import_id,): (i64,) = sqlx::query_as(
        "INSERT INTO imports (portfolio_id, filename, sha256, nav_date, row_counts, has_div_ops) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(portfolio_id).bind(filename).bind(sha256).bind(b.primary_date).bind(&row_counts).bind(has_div_ops)
    .fetch_one(&mut *tx).await?;

    const UPSERT_NAV: &str = "INSERT INTO nav_history (portfolio_id, date, aum, shares, nav) VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (portfolio_id, date) DO UPDATE SET aum = EXCLUDED.aum, shares = EXCLUDED.shares, nav = EXCLUDED.nav";
    for r in &b.nav_points {
        sqlx::query(UPSERT_NAV).bind(portfolio_id).bind(r.date).bind(r.aum).bind(r.shares).bind(r.nav)
            .execute(&mut *tx).await?;
    }

    for snap in &b.snapshots {
        sqlx::query("DELETE FROM position_snapshots WHERE portfolio_id = $1 AND nav_date = $2")
            .bind(portfolio_id).bind(snap.nav_date).execute(&mut *tx).await?;
        for p in &snap.positions {
            sqlx::query(
                "INSERT INTO position_snapshots (portfolio_id, nav_date, import_id, asset_type, isin, name, currency, quantity, avg_cost, price, valuation_ccy, accrued_interest, fx_rate, valuation_eur, weight, ticker)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
            )
            .bind(portfolio_id).bind(snap.nav_date).bind(import_id).bind(&p.asset_type).bind(&p.isin).bind(&p.name)
            .bind(&p.currency).bind(p.quantity).bind(p.avg_cost).bind(p.price).bind(p.valuation_ccy)
            .bind(p.accrued_interest).bind(p.fx_rate).bind(p.valuation_eur).bind(p.weight).bind(&p.ticker)
            .execute(&mut *tx).await?;
        }
    }

    // Bond statics from names — COALESCE keeps existing values (unchanged logic,
    // now over every snapshot's positions).
    for p in all_positions() {
        if p.asset_type != "Obligation" { continue; }
        let Some(name) = &p.name else { continue };
        let Some(bs) = ingest::parse_bond_statics(name, p.currency.as_deref()) else { continue };
        sqlx::query(
            "INSERT INTO instrument_refs (code, bond_coupon_pct, bond_maturity, bond_coupon_freq)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (code) DO UPDATE SET
               bond_coupon_pct = COALESCE(instrument_refs.bond_coupon_pct, EXCLUDED.bond_coupon_pct),
               bond_maturity = COALESCE(instrument_refs.bond_maturity, EXCLUDED.bond_maturity),
               bond_coupon_freq = COALESCE(instrument_refs.bond_coupon_freq, EXCLUDED.bond_coupon_freq),
               updated_at = now()",
        )
        .bind(&p.isin).bind(bs.coupon_pct).bind(bs.maturity).bind(bs.coupon_freq)
        .execute(&mut *tx).await?;
    }

    // Reference hints: fill NULLs only — Bloomberg data is never overwritten.
    for h in &b.ref_hints {
        sqlx::query(
            "INSERT INTO instrument_refs (code, country_of_risk, region, ticker)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (code) DO UPDATE SET
               country_of_risk = COALESCE(instrument_refs.country_of_risk, EXCLUDED.country_of_risk),
               region          = COALESCE(instrument_refs.region,          EXCLUDED.region),
               ticker          = COALESCE(instrument_refs.ticker,          EXCLUDED.ticker),
               updated_at = now()",
        )
        .bind(&h.isin).bind(&h.country_of_risk).bind(&h.region).bind(&h.ticker)
        .execute(&mut *tx).await?;
    }

    // Authoritative depositary facts: overwrite where present, leave alone
    // where this file says nothing. COALESCE(EXCLUDED, existing) rather than
    // COALESCE(existing, EXCLUDED) — the inverse of the hint loop above.
    for f in &b.ref_facts {
        sqlx::query(
            "INSERT INTO instrument_refs
               (code, market_place, market_place_name, bond_maturity,
                bond_next_coupon, bond_coupon_pct, bond_nominal, bond_coupon_freq)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (code) DO UPDATE SET
               market_place      = COALESCE(EXCLUDED.market_place,      instrument_refs.market_place),
               market_place_name = COALESCE(EXCLUDED.market_place_name, instrument_refs.market_place_name),
               bond_maturity     = COALESCE(EXCLUDED.bond_maturity,     instrument_refs.bond_maturity),
               bond_next_coupon  = COALESCE(EXCLUDED.bond_next_coupon,  instrument_refs.bond_next_coupon),
               bond_coupon_pct   = COALESCE(EXCLUDED.bond_coupon_pct,   instrument_refs.bond_coupon_pct),
               bond_nominal      = COALESCE(EXCLUDED.bond_nominal,      instrument_refs.bond_nominal),
               bond_coupon_freq  = COALESCE(EXCLUDED.bond_coupon_freq,  instrument_refs.bond_coupon_freq),
               updated_at = now()",
        )
        .bind(&f.isin).bind(&f.market_place).bind(&f.market_place_name).bind(f.bond_maturity)
        .bind(f.bond_next_coupon).bind(f.bond_coupon_pct).bind(f.bond_nominal).bind(f.bond_coupon_freq)
        .execute(&mut *tx).await?;
    }

    let positions: Vec<ingest::PositionRow> = all_positions().cloned().collect();
    let mut warnings = b.warnings.clone();
    warnings.extend(seed_futures_contracts(&mut tx, &positions).await?);
    if let Some(ops) = &b.operations {
        // Per snapshot, not over the flattened `positions`: the walk's upper
        // bound must be that snapshot's own nav_date, so a position from an
        // earlier snapshot is never checked against a walk that can include
        // trades dated after it (see task-2 review fix round 1).
        for snap in &b.snapshots {
            warnings.extend(pam_warnings(snap.nav_date, &snap.positions, ops));
        }
    }

    if replace_div_ops {
        // Scoped to explicit (file-sourced) rows only: derived rows are
        // owned exclusively by `derive_dividends`'s own delete-and-rebuild,
        // and are re-derived below, not wiped and left unreplaced here.
        sqlx::query("DELETE FROM dividends WHERE portfolio_id = $1 AND NOT derived")
            .bind(portfolio_id).execute(&mut *tx).await?;
        for r in b.dividends.as_deref().unwrap_or(&[]) {
            sqlx::query("INSERT INTO dividends (portfolio_id, provision_date, payment_date, issuer, amount, currency) VALUES ($1, $2, $3, $4, $5, $6)")
                .bind(portfolio_id).bind(r.provision_date).bind(r.payment_date).bind(&r.issuer).bind(r.amount).bind(&r.currency)
                .execute(&mut *tx).await?;
        }
        sqlx::query("DELETE FROM operations WHERE portfolio_id = $1").bind(portfolio_id).execute(&mut *tx).await?;
        for r in b.operations.as_deref().unwrap_or(&[]) {
            sqlx::query(
                "INSERT INTO operations (portfolio_id, trade_date, side, ticker, isin, name, currency, quantity, price, gross_amount, fees, net_price, net_amount)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
            )
            .bind(portfolio_id).bind(r.trade_date).bind(&r.side).bind(&r.ticker).bind(&r.isin).bind(&r.name)
            .bind(&r.currency).bind(r.quantity).bind(r.price).bind(r.gross_amount).bind(r.fees)
            .bind(r.net_price).bind(r.net_amount)
            .execute(&mut *tx).await?;
        }
    }

    // TNA cross-check: for every date this batch touched where BOTH a
    // snapshot and a NAV point now exist, the position sum must match AUM
    // within 0.1% — catches truncated position files and stale NAVs.
    let mut check_dates: Vec<NaiveDate> = b.snapshots.iter().map(|s| s.nav_date)
        .chain(b.nav_points.iter().map(|n| n.date)).collect();
    check_dates.sort();
    check_dates.dedup();
    let drift: Vec<(NaiveDate, f64, f64)> = sqlx::query_as(
        "SELECT n.date, n.aum::float8, s.total
         FROM nav_history n
         JOIN (SELECT nav_date, SUM(valuation_eur)::float8 AS total
               FROM position_snapshots WHERE portfolio_id = $1 GROUP BY nav_date) s
           ON s.nav_date = n.date
         WHERE n.portfolio_id = $1 AND n.date = ANY($2)
           AND n.aum <> 0 AND abs(s.total - n.aum::float8) / abs(n.aum::float8) > 0.001",
    )
    .bind(portfolio_id).bind(&check_dates)
    .fetch_all(&mut *tx).await?;
    for (d, aum, total) in drift {
        warnings.push(format!(
            "TNA cross-check {d}: positions sum to {total:.2} EUR but the NAV file says {aum:.2} EUR ({:+.2}%)",
            (total - aum) / aum * 100.0
        ));
    }

    tx.commit().await?;

    // Re-derive whenever this batch's own snapshots might contain new CPON
    // deltas (the original trigger), OR whenever a journal-bearing batch
    // just ran (explicit rows may have been inserted/changed) and the
    // portfolio holds any derived rows at all — a mixed-feed portfolio's
    // NAV Recap import must re-run derivation so newly explicit dates
    // correctly suppress the same-date derived rows, and so a derived date
    // this batch did not touch is recomputed rather than left stale.
    let has_derived_rows = has_div_ops && sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM dividends WHERE portfolio_id = $1 AND derived)")
        .bind(portfolio_id).fetch_one(pool).await?;
    if (b.dividends.is_none() && !b.snapshots.is_empty()) || has_derived_rows {
        let n = derive_dividends(pool, portfolio_id).await?;
        if n > 0 {
            warnings.push(format!("{n} dividend event(s) derived from receivable deltas"));
        }
    }

    Ok(ImportOutcome {
        import_id,
        duplicate: false,
        nav_rows,
        positions: n_positions,
        dividends: if replace_div_ops { n_div } else { 0 },
        operations: if replace_div_ops { n_ops } else { 0 },
        div_ops_replaced: replace_div_ops,
        warnings,
    })
}

pub async fn import_workbook(pool: &PgPool, portfolio_id: i64, filename: &str, sha256: &str, wb: &ParsedWorkbook) -> anyhow::Result<ImportOutcome> {
    // Clone-into-batch: ParsedWorkbook fields are all Clone.
    let b = ingest::adapter::to_batch(ParsedWorkbook {
        nav_date: wb.nav_date, aum: wb.aum, shares: wb.shares, nav: wb.nav,
        positions: wb.positions.clone(), nav_history: wb.nav_history.clone(),
        dividends: wb.dividends.clone(), operations: wb.operations.clone(),
    });
    import_batch(pool, portfolio_id, filename, sha256, &b).await
}

pub async fn imports_list(pool: &PgPool, portfolio_id: i64) -> anyhow::Result<Vec<ImportRecord>> {
    Ok(sqlx::query_as(
        "SELECT id, filename, nav_date, imported_at, row_counts FROM imports
         WHERE portfolio_id = $1 ORDER BY imported_at DESC",
    )
    .bind(portfolio_id)
    .fetch_all(pool)
    .await?)
}

/// Seed a contract spec for every futures root the database does not know yet,
/// and cross-check the point value implied by the workbook against the spec
/// already stored for a root it does know. Returns the non-fatal warnings.
///
/// **Idempotent by construction, and that is load-bearing:** an unknown root is
/// inserted with `ON CONFLICT (contract_root) DO NOTHING`, and a known root is
/// only ever read - no branch here issues an `UPDATE` - so re-running this over
/// the same positions can never overwrite a value the user has edited. That is
/// what makes it safe to call on a duplicate import as well as a fresh one.
async fn seed_futures_contracts(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    positions: &[ingest::PositionRow],
) -> anyhow::Result<Vec<String>> {
    let mut warnings: Vec<String> = Vec::new();
    let known: Vec<FuturesContract> = sqlx::query_as(SELECT_CONTRACTS).fetch_all(&mut **tx).await?;
    let by_root: std::collections::HashMap<String, FuturesContract> =
        known.into_iter().map(|c| (c.contract_root.clone(), c)).collect();

    for p in positions.iter().filter(|p| p.asset_type == "Future") {
        let Some(ticker) = p.ticker.as_deref() else {
            warnings.push(format!("{}: futures row has no ticker; contract not identified", p.isin));
            continue;
        };
        let Some(root) = analytics::contract_root(ticker) else {
            warnings.push(format!("{ticker}: cannot derive a contract root"));
            continue;
        };
        let (Some(raw_price), Some(raw_pam), Some(qty), Some(val)) =
            (p.price, p.avg_cost, p.quantity, p.valuation_ccy)
        else {
            warnings.push(format!("{ticker}: incomplete row; point value not verified"));
            continue;
        };

        match by_root.get(&root) {
            None => {
                // Guess only what the ticker suffix states unambiguously.
                // "Comdty" covers bond and commodity futures alike, so it is
                // never guessed - the user confirms it.
                let category = match ticker.split_whitespace().last() {
                    Some("Index") => "equity",
                    Some("Curncy") => "fx",
                    _ => "other",
                };
                let pv = analytics::implied_point_value(raw_price, raw_pam, qty, val);
                sqlx::query(
                    "INSERT INTO futures_contracts
                       (contract_root, label, category, point_value, currency, price_convention, confirmed)
                     VALUES ($1, $2, $3, $4, $5, 'decimal', false)
                     ON CONFLICT (contract_root) DO NOTHING",
                )
                .bind(&root)
                .bind(p.name.clone().unwrap_or_else(|| root.clone()))
                .bind(category)
                .bind(pv)
                .bind(p.currency.clone().unwrap_or_else(|| "EUR".into()))
                .execute(&mut **tx)
                .await?;
                warnings.push(format!("{root}: new contract seeded from {ticker}; confirm its spec on the Data page"));
            }
            Some(spec) => {
                let Some(stored) = spec.point_value else { continue };
                let conv = analytics::PriceConvention::parse(&spec.price_convention)
                    .unwrap_or(analytics::PriceConvention::Decimal);
                let implied = analytics::implied_point_value(
                    analytics::decode_price(raw_price, conv),
                    analytics::decode_price(raw_pam, conv),
                    qty,
                    val,
                );
                let Some(implied) = implied else { continue }; // marked at cost: undeterminable
                if (implied - stored).abs() <= 0.005 * stored {
                    continue;
                }
                // Mismatch. If the opposite convention reconciles, say so.
                let other = match conv {
                    analytics::PriceConvention::Decimal => analytics::PriceConvention::Th32,
                    analytics::PriceConvention::Th32 => analytics::PriceConvention::Decimal,
                };
                let alt = analytics::implied_point_value(
                    analytics::decode_price(raw_price, other),
                    analytics::decode_price(raw_pam, other),
                    qty,
                    val,
                );
                match alt {
                    Some(a) if (a - stored).abs() <= 0.005 * stored => warnings.push(format!(
                        "{root}: point value implies convention {}, stored {}",
                        other.as_str(),
                        conv.as_str()
                    )),
                    _ => warnings.push(format!(
                        "{root}: point value mismatch - stored {stored}, implied {implied:.1}"
                    )),
                }
            }
        }
    }
    Ok(warnings)
}

/// Cross-check the engine's weighted-average cost against the administrator's
/// PAM column for every cash position in the snapshot.
///
/// Two distinct problems are reported, and they are kept separate because
/// they have different causes: the walked quantity disagreeing with the
/// snapshot means OPERATIONS does not hold this instrument's complete
/// history, so no cost-basis comparison is meaningful (an "incomplete trade
/// history" warning); only once the quantity agrees is the cost basis itself
/// compared against PAM (a "PAM drift" warning), which reports a genuine
/// mismatch between the administrator's file and the trade walk. Non-fatal:
/// it warns, it never blocks the weekly import.
fn pam_warnings(nav_date: NaiveDate, positions: &[ingest::PositionRow], operations: &[ingest::OperationRow]) -> Vec<String> {
    use analytics::pnl::{Trade, is_buy, walk_instrument};

    let mut trades: Vec<Trade> = Vec::new();
    for o in operations {
        let (Some(isin), Some(qty), Some(px)) = (o.isin.as_deref(), o.quantity, o.net_price) else { continue };
        let Some(buy) = is_buy(&o.side) else { continue };
        trades.push(Trade {
            trade_date: o.trade_date,
            isin: isin.to_string(),
            is_buy: buy,
            quantity: qty.abs(),
            net_price: px,
            net_amount: o.net_amount.unwrap_or(0.0),
            currency: o.currency.clone().unwrap_or_default(),
        });
    }
    trades.sort_by_key(|t| t.trade_date);

    let mut warnings = Vec::new();
    for p in positions {
        // Futures have no cost basis; cash rows carry no PAM.
        if !matches!(p.asset_type.as_str(), "Action" | "Fonds" | "Obligation") { continue; }
        let (Some(pam), Some(qty)) = (p.avg_cost, p.quantity) else { continue };
        if qty.abs() < 1e-9 { continue; }
        let mine: Vec<Trade> = trades.iter().filter(|t| t.isin == p.isin).cloned().collect();

        // `walk_instrument` on an empty slice returns a zero `basis_end`, so an
        // ISIN entirely absent from OPERATIONS - the most severe form of
        // incomplete history - falls out of this call naturally rather than
        // needing its own branch. The walk is bounded above by this
        // position's own snapshot date: a trade dated after `nav_date` must
        // not feed the cost-basis comparison for a position taken as of
        // `nav_date` (restores the pre-UniversalBatch single-snapshot
        // behavior exactly; see task-2 review fix round 1).
        let w = walk_instrument(&mine, chrono::NaiveDate::MIN, nav_date);
        if w.oversold {
            // A sell exceeding the running quantity is itself unambiguous
            // evidence of a broken history; it gets its own warning and is not
            // also run through the quantity gate below (oversold subsumes it -
            // both report "history problem", and double-tagging the same
            // instrument would just be noise).
            warnings.push(format!("{}: sells exceed recorded buys; cost basis incomplete", p.isin));
            continue;
        }

        // The walked quantity is the evidence for whether OPERATIONS holds this
        // instrument's full history. Zero trades for the ISIN (mine.is_empty())
        // and a history that round-trips exactly back to flat (basis_end.qty <=
        // 0.0, the signature `analytics::pnl` documents for a truncated
        // history) both surface here as walked = 0.0, which the workbook's
        // already-confirmed non-zero holding will fail below - producing the
        // same "incomplete trade history" warning instead of silently
        // continuing.
        let walked = w.basis_end.qty;
        if (walked - qty).abs() > 1e-6 * qty.abs().max(1.0) {
            warnings.push(format!(
                "{}: incomplete trade history - OPERATIONS gives {:.4} units, workbook holds {:.4}; cost basis not compared",
                p.isin, walked, qty
            ));
            continue;
        }
        if (w.basis_end.avg_cost - pam).abs() > 0.01 {
            warnings.push(format!(
                "{}: PAM drift - workbook {:.6}, computed {:.6}",
                p.isin, pam, w.basis_end.avg_cost
            ));
        }
    }
    warnings
}

#[cfg(test)]
mod pam_warnings_tests {
    //! Unit-level pin for the two silent-skip paths in `pam_warnings`, using
    //! synthetic `ParsedWorkbook` data rather than the `sample.xlsx` fixture:
    //! no real position in that fixture is both non-oversold and walks to a
    //! zero/empty history (see task-9-fix1-report.md, Round 1), so the
    //! integration test in `pam_check.rs` cannot exercise the exact defect
    //! class this fix addresses. These tests construct the minimal shape that
    //! does, and would fail if either removed `continue` were reinstated.
    use super::pam_warnings;
    use chrono::NaiveDate;
    use ingest::{OperationRow, ParsedWorkbook, PositionRow};

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn position(isin: &str, quantity: f64, avg_cost: f64) -> PositionRow {
        PositionRow {
            asset_type: "Action".to_string(),
            isin: isin.to_string(),
            name: None,
            currency: None,
            quantity: Some(quantity),
            avg_cost: Some(avg_cost),
            price: None,
            valuation_ccy: None,
            accrued_interest: None,
            fx_rate: None,
            valuation_eur: None,
            weight: None,
            ticker: None,
        }
    }

    fn op(isin: &str, side: &str, trade_date: NaiveDate, quantity: f64, net_price: f64) -> OperationRow {
        OperationRow {
            trade_date,
            side: side.to_string(),
            ticker: None,
            isin: Some(isin.to_string()),
            name: None,
            currency: Some("EUR".to_string()),
            quantity: Some(quantity),
            price: Some(net_price),
            gross_amount: None,
            fees: None,
            net_price: Some(net_price),
            net_amount: Some(-quantity * net_price),
        }
    }

    fn workbook(positions: Vec<PositionRow>, operations: Vec<OperationRow>) -> ParsedWorkbook {
        ParsedWorkbook {
            nav_date: d(2026, 1, 31),
            aum: 0.0,
            shares: 0.0,
            nav: 0.0,
            positions,
            nav_history: Vec::new(),
            dividends: Vec::new(),
            operations,
        }
    }

    /// Point 1: OPERATIONS has zero rows at all for an ISIN the workbook
    /// currently holds a non-zero, non-oversold position in. This is
    /// `mine.is_empty()` in `pam_warnings`. Before the fix this `continue`d
    /// with no warning at all.
    #[test]
    fn zero_operations_rows_warns_incomplete_history() {
        let wb = workbook(
            vec![position("T1_NO_TRADES", 50.0, 10.0)],
            vec![
                // An operation for a *different* ISIN, so `mine` for
                // T1_NO_TRADES is empty but `wb.operations` is not.
                op("SOME_OTHER_ISIN", "achat", d(2026, 1, 5), 10.0, 5.0),
            ],
        );
        let warnings = pam_warnings(wb.nav_date, &wb.positions, &wb.operations);
        assert!(
            warnings.iter().any(|w| w.starts_with("T1_NO_TRADES") && w.contains("incomplete trade history")),
            "an ISIN with zero OPERATIONS rows but a non-zero workbook holding must warn \
             'incomplete trade history', got: {warnings:?}"
        );
    }

    /// Point 2: OPERATIONS has trades for the ISIN, but they round-trip
    /// exactly back to flat (buy 100, sell 100) while the workbook still
    /// shows a non-zero holding. This is `basis_end.qty <= 0.0` in
    /// `pam_warnings`. Before the fix this `continue`d with no warning.
    #[test]
    fn flat_round_trip_warns_incomplete_history() {
        let wb = workbook(
            vec![position("T2_FLAT", 50.0, 10.0)],
            vec![
                op("T2_FLAT", "achat", d(2026, 1, 5), 100.0, 9.0),
                op("T2_FLAT", "vente", d(2026, 1, 10), 100.0, 11.0),
            ],
        );
        let warnings = pam_warnings(wb.nav_date, &wb.positions, &wb.operations);
        assert!(
            warnings.iter().any(|w| w.starts_with("T2_FLAT") && w.contains("incomplete trade history")),
            "a history that round-trips to exactly flat against a non-zero workbook holding must \
             warn 'incomplete trade history', got: {warnings:?}"
        );
    }

    /// Point 3: an oversold position (a sell exceeding the running quantity)
    /// must still warn "sells exceed recorded buys" only - not also get
    /// double-tagged as "incomplete trade history" by the quantity gate.
    /// Pins the oversold-subsumes-gate ordering decision at the unit level,
    /// matching the ES0113900J37 / FR0010599399 fixture-level pin in
    /// `pam_check.rs`.
    #[test]
    fn oversold_warns_only_oversold() {
        let wb = workbook(
            vec![position("T3_OVERSOLD", 50.0, 10.0)],
            vec![
                op("T3_OVERSOLD", "achat", d(2026, 1, 5), 50.0, 9.0),
                op("T3_OVERSOLD", "vente", d(2026, 1, 10), 100.0, 11.0),
            ],
        );
        let warnings = pam_warnings(wb.nav_date, &wb.positions, &wb.operations);
        assert!(
            warnings.iter().any(|w| w.starts_with("T3_OVERSOLD") && w.contains("sells exceed recorded buys")),
            "an oversold position must warn 'sells exceed recorded buys', got: {warnings:?}"
        );
        assert!(
            !warnings.iter().any(|w| w.starts_with("T3_OVERSOLD") && w.contains("incomplete trade history")),
            "an oversold position must not also be tagged 'incomplete trade history', got: {warnings:?}"
        );
    }

    /// A trade dated after the position's own snapshot date must not affect
    /// its PAM walk. Pins the review fix for a regression introduced while
    /// generalizing `pam_warnings` to `UniversalBatch`: the walk's upper
    /// bound had transiently become unbounded (`NaiveDate::MAX`), so a trade
    /// dated after `nav_date` would still feed `basis_end`. Without the date
    /// bound, the second buy here (at 20.0) would move the walked avg_cost
    /// off the PAM-matching 10.0 and both change the walked quantity (100 ->
    /// 150, vs. the workbook's 100) and warn "PAM drift" / "incomplete trade
    /// history". The date-bounded walk must exclude it entirely, leaving the
    /// outcome identical to the same batch without that trade.
    #[test]
    fn trade_after_nav_date_excluded_from_walk() {
        let with_future_trade = workbook(
            vec![position("T4_AFTER_NAV", 100.0, 10.0)],
            vec![
                op("T4_AFTER_NAV", "achat", d(2026, 1, 5), 100.0, 10.0),
                // Dated after nav_date (2026-01-31): must not enter the walk.
                op("T4_AFTER_NAV", "achat", d(2026, 2, 15), 50.0, 20.0),
            ],
        );
        let without_future_trade = workbook(
            vec![position("T4_AFTER_NAV", 100.0, 10.0)],
            vec![op("T4_AFTER_NAV", "achat", d(2026, 1, 5), 100.0, 10.0)],
        );

        let warnings_with = pam_warnings(
            with_future_trade.nav_date,
            &with_future_trade.positions,
            &with_future_trade.operations,
        );
        let warnings_without = pam_warnings(
            without_future_trade.nav_date,
            &without_future_trade.positions,
            &without_future_trade.operations,
        );

        assert!(
            !warnings_with.iter().any(|w| w.starts_with("T4_AFTER_NAV")),
            "a trade dated after nav_date must not produce a warning (PAM matches the pre-future-trade walk), got: {warnings_with:?}"
        );
        assert_eq!(
            warnings_with, warnings_without,
            "including a trade dated after nav_date must not change the warning outcome"
        );
    }
}
