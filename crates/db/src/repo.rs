use chrono::NaiveDate;
use ingest::ParsedWorkbook;
use sqlx::PgPool;

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

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct NavRow {
    pub date: NaiveDate,
    pub aum: f64,
    pub shares: f64,
    pub nav: f64,
}

#[derive(Debug, sqlx::FromRow, serde::Serialize)]
pub struct PositionRecord {
    pub nav_date: NaiveDate,
    pub asset_type: String,
    pub isin: String,
    pub name: Option<String>,
    pub currency: Option<String>,
    pub quantity: Option<f64>,
    pub avg_cost: Option<f64>,
    pub price: Option<f64>,
    pub valuation_ccy: Option<f64>,
    pub accrued_interest: Option<f64>,
    pub fx_rate: Option<f64>,
    pub valuation_eur: Option<f64>,
    pub weight: Option<f64>,
    pub ticker: Option<String>,
}

#[derive(Debug, sqlx::FromRow, serde::Serialize)]
pub struct ImportRecord {
    pub id: i64,
    pub filename: String,
    pub nav_date: NaiveDate,
    pub imported_at: chrono::DateTime<chrono::Utc>,
    pub row_counts: serde_json::Value,
}

pub async fn import_workbook(pool: &PgPool, portfolio_id: i64, filename: &str, sha256: &str, wb: &ParsedWorkbook) -> anyhow::Result<ImportOutcome> {
    if let Some((id,)) = sqlx::query_as::<_, (i64,)>("SELECT id FROM imports WHERE portfolio_id = $1 AND sha256 = $2")
        .bind(portfolio_id)
        .bind(sha256)
        .fetch_optional(pool)
        .await?
    {
        // Nothing is re-ingested for a duplicate - but the futures spec seeding
        // is deliberately NOT skipped. A database populated before futures
        // support shipped holds the workbook and no contract specs at all, and
        // re-dropping the same file is the user's only repair path; skipping it
        // here left the whole feature inert on every existing installation.
        // `seed_futures_contracts` only ever INSERTs roots it does not already
        // know, so running it again is a no-op once the specs are in place.
        let mut tx = pool.begin().await?;
        let warnings = seed_futures_contracts(&mut tx, &wb.positions).await?;
        tx.commit().await?;
        return Ok(ImportOutcome {
            import_id: id, duplicate: true, nav_rows: 0, positions: 0,
            dividends: 0, operations: 0, div_ops_replaced: false,
            warnings,
        });
    }

    let mut tx = pool.begin().await?;

    let prev_latest: Option<NaiveDate> =
        sqlx::query_scalar("SELECT max(nav_date) FROM imports WHERE portfolio_id = $1")
            .bind(portfolio_id)
            .fetch_one(&mut *tx).await?;
    let replace_div_ops = prev_latest.is_none_or(|d| wb.nav_date >= d);

    let nav_rows = wb.nav_history.len() + 1;
    let row_counts = serde_json::json!({
        "nav_rows": nav_rows, "positions": wb.positions.len(),
        "dividends": if replace_div_ops { wb.dividends.len() } else { 0 },
        "operations": if replace_div_ops { wb.operations.len() } else { 0 },
    });
    let (import_id,): (i64,) = sqlx::query_as(
        "INSERT INTO imports (portfolio_id, filename, sha256, nav_date, row_counts) VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(portfolio_id).bind(filename).bind(sha256).bind(wb.nav_date).bind(&row_counts)
    .fetch_one(&mut *tx)
    .await?;

    const UPSERT_NAV: &str = "INSERT INTO nav_history (portfolio_id, date, aum, shares, nav) VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (portfolio_id, date) DO UPDATE SET aum = EXCLUDED.aum, shares = EXCLUDED.shares, nav = EXCLUDED.nav";
    for r in &wb.nav_history {
        sqlx::query(UPSERT_NAV).bind(portfolio_id).bind(r.date).bind(r.aum).bind(r.shares).bind(r.nav)
            .execute(&mut *tx).await?;
    }
    // the recap's own NAV row (not yet in HISTO_NAV)
    sqlx::query(UPSERT_NAV).bind(portfolio_id).bind(wb.nav_date).bind(wb.aum).bind(wb.shares).bind(wb.nav)
        .execute(&mut *tx).await?;

    sqlx::query("DELETE FROM position_snapshots WHERE portfolio_id = $1 AND nav_date = $2")
        .bind(portfolio_id).bind(wb.nav_date).execute(&mut *tx).await?;
    for p in &wb.positions {
        sqlx::query(
            "INSERT INTO position_snapshots (portfolio_id, nav_date, import_id, asset_type, isin, name, currency, quantity, avg_cost, price, valuation_ccy, accrued_interest, fx_rate, valuation_eur, weight, ticker)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
        )
        .bind(portfolio_id).bind(wb.nav_date).bind(import_id).bind(&p.asset_type).bind(&p.isin).bind(&p.name)
        .bind(&p.currency).bind(p.quantity).bind(p.avg_cost).bind(p.price).bind(p.valuation_ccy)
        .bind(p.accrued_interest).bind(p.fx_rate).bind(p.valuation_eur).bind(p.weight).bind(&p.ticker)
        .execute(&mut *tx)
        .await?;
    }

    // Seed bond reference data parsed from names; never overwrite user
    // values (COALESCE keeps existing non-NULL columns).
    for p in &wb.positions {
        if p.asset_type != "Obligation" { continue; }
        let Some(name) = &p.name else { continue };
        let Some(b) = ingest::parse_bond_statics(name, p.currency.as_deref()) else { continue };
        sqlx::query(
            "INSERT INTO instrument_refs (code, bond_coupon_pct, bond_maturity, bond_coupon_freq)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (code) DO UPDATE SET
               bond_coupon_pct = COALESCE(instrument_refs.bond_coupon_pct, EXCLUDED.bond_coupon_pct),
               bond_maturity = COALESCE(instrument_refs.bond_maturity, EXCLUDED.bond_maturity),
               bond_coupon_freq = COALESCE(instrument_refs.bond_coupon_freq, EXCLUDED.bond_coupon_freq),
               updated_at = now()",
        )
        .bind(&p.isin).bind(b.coupon_pct).bind(b.maturity).bind(b.coupon_freq)
        .execute(&mut *tx)
        .await?;
    }

    let mut warnings = seed_futures_contracts(&mut tx, &wb.positions).await?;
    warnings.extend(pam_warnings(wb));

    if replace_div_ops {
        sqlx::query("DELETE FROM dividends WHERE portfolio_id = $1").bind(portfolio_id).execute(&mut *tx).await?;
        for r in &wb.dividends {
            sqlx::query("INSERT INTO dividends (portfolio_id, provision_date, payment_date, issuer, amount, currency) VALUES ($1, $2, $3, $4, $5, $6)")
                .bind(portfolio_id).bind(r.provision_date).bind(r.payment_date).bind(&r.issuer).bind(r.amount).bind(&r.currency)
                .execute(&mut *tx).await?;
        }
        sqlx::query("DELETE FROM operations WHERE portfolio_id = $1").bind(portfolio_id).execute(&mut *tx).await?;
        for r in &wb.operations {
            sqlx::query(
                "INSERT INTO operations (portfolio_id, trade_date, side, ticker, isin, name, currency, quantity, price, gross_amount, fees, net_price, net_amount)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
            )
            .bind(portfolio_id).bind(r.trade_date).bind(&r.side).bind(&r.ticker).bind(&r.isin).bind(&r.name)
            .bind(&r.currency).bind(r.quantity).bind(r.price).bind(r.gross_amount).bind(r.fees)
            .bind(r.net_price).bind(r.net_amount)
            .execute(&mut *tx)
            .await?;
        }
    }

    tx.commit().await?;
    Ok(ImportOutcome {
        import_id,
        duplicate: false,
        nav_rows,
        positions: wb.positions.len(),
        dividends: if replace_div_ops { wb.dividends.len() } else { 0 },
        operations: if replace_div_ops { wb.operations.len() } else { 0 },
        div_ops_replaced: replace_div_ops,
        warnings,
    })
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
fn pam_warnings(wb: &ingest::ParsedWorkbook) -> Vec<String> {
    use analytics::pnl::{Trade, is_buy, walk_instrument};

    let mut trades: Vec<Trade> = Vec::new();
    for o in &wb.operations {
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
    for p in &wb.positions {
        // Futures have no cost basis; cash rows carry no PAM.
        if !matches!(p.asset_type.as_str(), "Action" | "Fonds" | "Obligation") { continue; }
        let (Some(pam), Some(qty)) = (p.avg_cost, p.quantity) else { continue };
        if qty.abs() < 1e-9 { continue; }
        let mine: Vec<Trade> = trades.iter().filter(|t| t.isin == p.isin).cloned().collect();

        // `walk_instrument` on an empty slice returns a zero `basis_end`, so an
        // ISIN entirely absent from OPERATIONS - the most severe form of
        // incomplete history - falls out of this call naturally rather than
        // needing its own branch.
        let w = walk_instrument(&mine, chrono::NaiveDate::MIN, wb.nav_date);
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

pub async fn nav_rows(pool: &PgPool, portfolio_id: i64) -> anyhow::Result<Vec<NavRow>> {
    Ok(sqlx::query_as(
        "SELECT date, aum::float8 AS aum, shares::float8 AS shares, nav::float8 AS nav
         FROM nav_history WHERE portfolio_id = $1 ORDER BY date",
    )
    .bind(portfolio_id)
    .fetch_all(pool)
    .await?)
}

pub async fn position_dates(pool: &PgPool, portfolio_id: i64) -> anyhow::Result<Vec<NaiveDate>> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT nav_date FROM position_snapshots WHERE portfolio_id = $1 ORDER BY nav_date DESC",
    )
    .bind(portfolio_id)
    .fetch_all(pool)
    .await?)
}

pub async fn positions_for(pool: &PgPool, portfolio_id: i64, date: NaiveDate) -> anyhow::Result<Vec<PositionRecord>> {
    Ok(sqlx::query_as(
        "SELECT nav_date, asset_type, isin, name, currency,
                quantity::float8 AS quantity, avg_cost::float8 AS avg_cost, price::float8 AS price,
                valuation_ccy::float8 AS valuation_ccy, accrued_interest::float8 AS accrued_interest,
                fx_rate::float8 AS fx_rate, valuation_eur::float8 AS valuation_eur,
                weight::float8 AS weight, ticker
         FROM position_snapshots WHERE portfolio_id = $1 AND nav_date = $2 ORDER BY id",
    )
    .bind(portfolio_id)
    .bind(date)
    .fetch_all(pool)
    .await?)
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

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct InstrumentRef {
    pub code: String,
    pub issuer_group: Option<String>,
    pub liquidity_bucket: Option<String>,
    pub bond_coupon_pct: Option<f64>,
    pub bond_maturity: Option<NaiveDate>,
    pub bond_coupon_freq: Option<i32>,
    pub country_of_risk: Option<String>,
    pub region: Option<String>,
    pub gics_sector: Option<String>,
    pub gics_industry: Option<String>,
    pub ticker: Option<String>,
}

pub async fn refs_all(pool: &PgPool) -> anyhow::Result<Vec<InstrumentRef>> {
    Ok(sqlx::query_as(
        "SELECT code, issuer_group, liquidity_bucket,
                bond_coupon_pct::float8 AS bond_coupon_pct, bond_maturity, bond_coupon_freq,
                country_of_risk, region, gics_sector, gics_industry, ticker
         FROM instrument_refs ORDER BY code",
    )
    .fetch_all(pool)
    .await?)
}

/// Full-row replace: every field is written as given; None stores NULL,
/// which means "use the derived default".
pub async fn refs_upsert(pool: &PgPool, r: &InstrumentRef) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO instrument_refs (code, issuer_group, liquidity_bucket, bond_coupon_pct, bond_maturity, bond_coupon_freq, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, now())
         ON CONFLICT (code) DO UPDATE SET
           issuer_group = EXCLUDED.issuer_group,
           liquidity_bucket = EXCLUDED.liquidity_bucket,
           bond_coupon_pct = EXCLUDED.bond_coupon_pct,
           bond_maturity = EXCLUDED.bond_maturity,
           bond_coupon_freq = EXCLUDED.bond_coupon_freq,
           updated_at = now()",
    )
    .bind(&r.code).bind(&r.issuer_group).bind(&r.liquidity_bucket)
    .bind(r.bond_coupon_pct).bind(r.bond_maturity).bind(r.bond_coupon_freq)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct FuturesContract {
    pub contract_root: String,
    pub label: String,
    pub category: String,
    pub point_value: Option<f64>,
    pub currency: String,
    pub curve: Option<String>,
    pub price_convention: String,
    pub confirmed: bool,
    pub otc: bool,
}

const SELECT_CONTRACTS: &str = "SELECT contract_root, label, category,
        point_value::float8 AS point_value, currency, curve, price_convention, confirmed, otc
     FROM futures_contracts ORDER BY contract_root";

pub async fn contracts_all(pool: &PgPool) -> anyhow::Result<Vec<FuturesContract>> {
    Ok(sqlx::query_as(SELECT_CONTRACTS).fetch_all(pool).await?)
}

/// Full-row replace, like `refs_upsert`: every field is written as given.
pub async fn contracts_upsert(pool: &PgPool, c: &FuturesContract) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO futures_contracts
           (contract_root, label, category, point_value, currency, curve, price_convention, confirmed, otc, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now())
         ON CONFLICT (contract_root) DO UPDATE SET
           label = EXCLUDED.label,
           category = EXCLUDED.category,
           point_value = EXCLUDED.point_value,
           currency = EXCLUDED.currency,
           curve = EXCLUDED.curve,
           price_convention = EXCLUDED.price_convention,
           confirmed = EXCLUDED.confirmed,
           otc = EXCLUDED.otc,
           updated_at = now()",
    )
    .bind(&c.contract_root).bind(&c.label).bind(&c.category).bind(c.point_value)
    .bind(&c.currency).bind(&c.curve).bind(&c.price_convention).bind(c.confirmed)
    .bind(c.otc)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct CtdRecord {
    pub nav_date: NaiveDate,
    pub ticker: String,
    pub ctd_isin: String,
    pub ctd_mod_duration: f64,
    pub ctd_clean_price: f64,
    pub ctd_accrued: f64,
    pub conversion_factor: f64,
}

/// Replace every analytics row for `date` in one transaction. Unlike the
/// workbook import there is no content dedupe: the expected reason to
/// re-upload is a corrected pull, which must win.
pub async fn ctd_replace(
    pool: &PgPool,
    portfolio_id: i64,
    date: NaiveDate,
    filename: &str,
    rows: &[ingest::CtdRow],
) -> anyhow::Result<usize> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM futures_analytics WHERE portfolio_id = $1 AND nav_date = $2")
        .bind(portfolio_id)
        .bind(date)
        .execute(&mut *tx)
        .await?;
    for r in rows {
        sqlx::query(
            "INSERT INTO futures_analytics
               (portfolio_id, nav_date, ticker, ctd_isin, ctd_mod_duration, ctd_clean_price,
                ctd_accrued, conversion_factor, source_file)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(portfolio_id).bind(date).bind(&r.ticker).bind(&r.ctd_isin).bind(r.ctd_mod_duration)
        .bind(r.ctd_clean_price).bind(r.ctd_accrued).bind(r.conversion_factor).bind(filename)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(rows.len())
}

pub async fn ctd_for(pool: &PgPool, portfolio_id: i64, date: NaiveDate) -> anyhow::Result<Vec<CtdRecord>> {
    Ok(sqlx::query_as(
        "SELECT nav_date, ticker, ctd_isin,
                ctd_mod_duration::float8 AS ctd_mod_duration,
                ctd_clean_price::float8 AS ctd_clean_price,
                ctd_accrued::float8 AS ctd_accrued,
                conversion_factor::float8 AS conversion_factor
         FROM futures_analytics WHERE portfolio_id = $1 AND nav_date = $2 ORDER BY ticker",
    )
    .bind(portfolio_id)
    .bind(date)
    .fetch_all(pool)
    .await?)
}

/// AUM recorded for a NAV date, used as the denominator for exposure.
pub async fn aum_for(pool: &PgPool, portfolio_id: i64, date: NaiveDate) -> anyhow::Result<Option<f64>> {
    Ok(sqlx::query_scalar("SELECT aum::float8 FROM nav_history WHERE portfolio_id = $1 AND date = $2")
        .bind(portfolio_id)
        .bind(date)
        .fetch_optional(pool)
        .await?)
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct OperationRecord {
    pub trade_date: NaiveDate,
    pub side: String,
    pub isin: Option<String>,
    pub ticker: Option<String>,
    pub name: Option<String>,
    pub currency: Option<String>,
    pub quantity: Option<f64>,
    pub net_price: Option<f64>,
    pub net_amount: Option<f64>,
    pub fees: Option<f64>,
}

pub async fn operations_all(pool: &PgPool, portfolio_id: i64) -> anyhow::Result<Vec<OperationRecord>> {
    Ok(sqlx::query_as(
        "SELECT trade_date, side, isin, ticker, name, currency,
                quantity::float8 AS quantity, net_price::float8 AS net_price,
                net_amount::float8 AS net_amount, fees::float8 AS fees
         FROM operations WHERE portfolio_id = $1 ORDER BY trade_date, id",
    )
    .bind(portfolio_id)
    .fetch_all(pool)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct DividendRecord {
    pub provision_date: NaiveDate,
    pub issuer: String,
    pub amount: f64,
    pub currency: String,
}

pub async fn dividends_all(pool: &PgPool, portfolio_id: i64) -> anyhow::Result<Vec<DividendRecord>> {
    Ok(sqlx::query_as(
        "SELECT provision_date, issuer, amount::float8 AS amount, currency
         FROM dividends WHERE portfolio_id = $1 ORDER BY provision_date",
    )
    .bind(portfolio_id)
    .fetch_all(pool)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct FxRow {
    pub date: NaiveDate,
    pub currency: String,
    pub rate_to_eur: f64,
}

pub async fn fx_all(pool: &PgPool) -> anyhow::Result<Vec<FxRow>> {
    Ok(sqlx::query_as(
        "SELECT date, currency, rate_to_eur::float8 AS rate_to_eur
         FROM fx_history ORDER BY currency, date",
    )
    .fetch_all(pool)
    .await?)
}

/// Replace-by-key: an FX rate is market data, so a fresh pull always wins.
pub async fn fx_upsert_many(pool: &PgPool, rows: &[FxRow]) -> anyhow::Result<u64> {
    let mut tx = pool.begin().await?;
    let mut n = 0u64;
    for r in rows {
        n += sqlx::query(
            "INSERT INTO fx_history (date, currency, rate_to_eur) VALUES ($1, $2, $3)
             ON CONFLICT (date, currency) DO UPDATE SET rate_to_eur = EXCLUDED.rate_to_eur",
        )
        .bind(r.date).bind(&r.currency).bind(r.rate_to_eur)
        .execute(&mut *tx).await?
        .rows_affected();
    }
    tx.commit().await?;
    Ok(n)
}

/// Seed classifications without ever overwriting a value already present,
/// matching the bond-statics discipline at :126-137. A user correction, or an
/// earlier good pull, always wins over a later one.
#[allow(clippy::type_complexity)]
pub async fn classify_upsert_many(
    pool: &PgPool,
    rows: &[(String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>)],
) -> anyhow::Result<u64> {
    let mut tx = pool.begin().await?;
    let mut n = 0u64;
    for (code, ticker, country, region, sector, industry) in rows {
        n += sqlx::query(
            "INSERT INTO instrument_refs
               (code, ticker, country_of_risk, region, gics_sector, gics_industry, classified_at)
             VALUES ($1, $2, $3, $4, $5, $6, now())
             ON CONFLICT (code) DO UPDATE SET
               ticker          = COALESCE(instrument_refs.ticker,          EXCLUDED.ticker),
               country_of_risk = COALESCE(instrument_refs.country_of_risk, EXCLUDED.country_of_risk),
               region          = COALESCE(instrument_refs.region,          EXCLUDED.region),
               gics_sector     = COALESCE(instrument_refs.gics_sector,     EXCLUDED.gics_sector),
               gics_industry   = COALESCE(instrument_refs.gics_industry,   EXCLUDED.gics_industry),
               classified_at   = now(),
               updated_at      = now()",
        )
        .bind(code).bind(ticker).bind(country).bind(region).bind(sector).bind(industry)
        .execute(&mut *tx).await?
        .rows_affected();
    }
    tx.commit().await?;
    Ok(n)
}

// ---- EMIR monthly KPIs ----

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct EmirKpi {
    /// First day of the calendar month the record describes.
    pub month: NaiveDate,
    pub unconfirmed_over_5d: i32,
    pub reconciliation: String,
    pub disputes: i32,
    pub note: Option<String>,
}

pub async fn emir_kpis_all(pool: &PgPool, portfolio_id: i64) -> anyhow::Result<Vec<EmirKpi>> {
    Ok(sqlx::query_as::<_, EmirKpi>(
        "SELECT month, unconfirmed_over_5d, reconciliation, disputes, note
         FROM emir_kpis WHERE portfolio_id = $1 ORDER BY month DESC",
    )
    .bind(portfolio_id)
    .fetch_all(pool)
    .await?)
}

/// Full-row replace, like `contracts_upsert`: every field is written as given.
pub async fn emir_kpi_upsert(pool: &PgPool, portfolio_id: i64, k: &EmirKpi) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO emir_kpis (portfolio_id, month, unconfirmed_over_5d, reconciliation, disputes, note)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (portfolio_id, month) DO UPDATE SET
           unconfirmed_over_5d = EXCLUDED.unconfirmed_over_5d,
           reconciliation = EXCLUDED.reconciliation,
           disputes = EXCLUDED.disputes,
           note = EXCLUDED.note,
           updated_at = now()",
    )
    .bind(portfolio_id)
    .bind(k.month)
    .bind(k.unconfirmed_over_5d)
    .bind(&k.reconciliation)
    .bind(k.disputes)
    .bind(&k.note)
    .execute(pool)
    .await?;
    Ok(())
}

// ---- portfolios ----

#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct Portfolio {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub archived: bool,
    /// Latest imported NAV date, the freshness signal for selector/overview.
    pub latest_nav_date: Option<chrono::NaiveDate>,
}

const SELECT_PORTFOLIO: &str = "SELECT p.id, p.name, p.kind, p.archived,
    (SELECT max(nav_date) FROM imports i WHERE i.portfolio_id = p.id) AS latest_nav_date
 FROM portfolios p";

pub async fn portfolios_list(pool: &PgPool) -> anyhow::Result<Vec<Portfolio>> {
    Ok(sqlx::query_as(&format!("{SELECT_PORTFOLIO} ORDER BY p.id")).fetch_all(pool).await?)
}

pub async fn portfolio_get(pool: &PgPool, id: i64) -> anyhow::Result<Option<Portfolio>> {
    Ok(sqlx::query_as(&format!("{SELECT_PORTFOLIO} WHERE p.id = $1"))
        .bind(id).fetch_optional(pool).await?)
}

pub async fn portfolio_create(pool: &PgPool, name: &str, kind: &str) -> anyhow::Result<Portfolio> {
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO portfolios (name, kind) VALUES ($1, $2) RETURNING id")
        .bind(name).bind(kind).fetch_one(pool).await?;
    Ok(portfolio_get(pool, id).await?.expect("just inserted"))
}

pub async fn portfolio_update(pool: &PgPool, id: i64, name: &str, archived: bool) -> anyhow::Result<Option<Portfolio>> {
    let n = sqlx::query("UPDATE portfolios SET name = $2, archived = $3 WHERE id = $1")
        .bind(id).bind(name).bind(archived).execute(pool).await?.rows_affected();
    if n == 0 { return Ok(None); }
    portfolio_get(pool, id).await
}

// ---- portfolio codes (external identifiers for upload auto-routing) ----

#[derive(Debug, sqlx::FromRow, serde::Serialize)]
pub struct PortfolioCode {
    pub portfolio_id: i64,
    pub source: String,
    pub code: String,
}

pub async fn portfolio_codes_for(pool: &PgPool, portfolio_id: i64) -> anyhow::Result<Vec<PortfolioCode>> {
    Ok(sqlx::query_as("SELECT portfolio_id, source, code FROM portfolio_codes WHERE portfolio_id = $1 ORDER BY source, code")
        .bind(portfolio_id).fetch_all(pool).await?)
}

/// Replace the full code set for one portfolio. A `(source, code)` already
/// claimed by ANOTHER portfolio surfaces as a unique violation the caller
/// maps to 422.
pub async fn portfolio_codes_replace(pool: &PgPool, portfolio_id: i64, codes: &[(String, String)]) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM portfolio_codes WHERE portfolio_id = $1")
        .bind(portfolio_id).execute(&mut *tx).await?;
    for (source, code) in codes {
        sqlx::query("INSERT INTO portfolio_codes (portfolio_id, source, code) VALUES ($1, $2, $3)")
            .bind(portfolio_id).bind(source).bind(code).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn portfolio_by_code(pool: &PgPool, source: &str, code: &str) -> anyhow::Result<Option<i64>> {
    Ok(sqlx::query_scalar("SELECT portfolio_id FROM portfolio_codes WHERE source = $1 AND code = $2")
        .bind(source).bind(code).fetch_optional(pool).await?)
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
        let warnings = pam_warnings(&wb);
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
        let warnings = pam_warnings(&wb);
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
        let warnings = pam_warnings(&wb);
        assert!(
            warnings.iter().any(|w| w.starts_with("T3_OVERSOLD") && w.contains("sells exceed recorded buys")),
            "an oversold position must warn 'sells exceed recorded buys', got: {warnings:?}"
        );
        assert!(
            !warnings.iter().any(|w| w.starts_with("T3_OVERSOLD") && w.contains("incomplete trade history")),
            "an oversold position must not also be tagged 'incomplete trade history', got: {warnings:?}"
        );
    }
}
