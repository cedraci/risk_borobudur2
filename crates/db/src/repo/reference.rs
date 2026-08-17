use chrono::NaiveDate;
use sqlx::PgPool;

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct InstrumentRef {
    pub code: String,
    pub issuer_group: Option<String>,
    /// Per-instrument days-to-liquidate override. NULL = asset-type default.
    pub liquidity_days: Option<f64>,
    /// User override of the derived venue rule. NULL = derive.
    pub adv_eligible: Option<bool>,
    pub bond_coupon_pct: Option<f64>,
    pub bond_maturity: Option<NaiveDate>,
    pub bond_coupon_freq: Option<i32>,
    // Depositary-maintained (HISINVLUX / INVJCPLUX), overwritten on import.
    pub bond_next_coupon: Option<NaiveDate>,
    pub bond_nominal: Option<f64>,
    pub market_place: Option<String>,
    pub market_place_name: Option<String>,
    // Bloomberg-maintained, written only by the ADV response upload.
    pub adv_30d: Option<f64>,
    pub adv_asof: Option<NaiveDate>,
    pub country_of_risk: Option<String>,
    pub region: Option<String>,
    pub gics_sector: Option<String>,
    pub gics_industry: Option<String>,
    pub ticker: Option<String>,
}

pub async fn refs_all(pool: &PgPool) -> anyhow::Result<Vec<InstrumentRef>> {
    Ok(sqlx::query_as(
        "SELECT code, issuer_group,
                liquidity_days::float8 AS liquidity_days, adv_eligible,
                bond_coupon_pct::float8 AS bond_coupon_pct, bond_maturity, bond_coupon_freq,
                bond_next_coupon, bond_nominal::float8 AS bond_nominal,
                market_place, market_place_name,
                adv_30d::float8 AS adv_30d, adv_asof,
                country_of_risk, region, gics_sector, gics_industry, ticker
         FROM instrument_refs ORDER BY code",
    )
    .fetch_all(pool)
    .await?)
}

/// User-owned fields only. The depositary columns (`market_place`,
/// `bond_next_coupon`, `bond_nominal`) and the Bloomberg columns (`adv_30d`,
/// `adv_asof`) are deliberately absent: an editor save must never blank data
/// the import or the terminal owns.
pub async fn refs_upsert(pool: &PgPool, r: &InstrumentRef) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO instrument_refs
           (code, issuer_group, liquidity_days, adv_eligible,
            bond_coupon_pct, bond_maturity, bond_coupon_freq, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, now())
         ON CONFLICT (code) DO UPDATE SET
           issuer_group     = EXCLUDED.issuer_group,
           liquidity_days   = EXCLUDED.liquidity_days,
           adv_eligible     = EXCLUDED.adv_eligible,
           bond_coupon_pct  = EXCLUDED.bond_coupon_pct,
           bond_maturity    = EXCLUDED.bond_maturity,
           bond_coupon_freq = EXCLUDED.bond_coupon_freq,
           updated_at = now()",
    )
    .bind(&r.code).bind(&r.issuer_group).bind(r.liquidity_days).bind(r.adv_eligible)
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

/// Shared with `imports::seed_futures_contracts`, which needs the same
/// projection when checking a workbook's positions against known contracts.
pub(crate) const SELECT_CONTRACTS: &str = "SELECT contract_root, label, category,
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
