use chrono::NaiveDate;
use sqlx::PgPool;

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct NavRow {
    pub date: NaiveDate,
    pub aum: f64,
    pub shares: f64,
    pub nav: f64,
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

/// AUM recorded for a NAV date, used as the denominator for exposure.
pub async fn aum_for(pool: &PgPool, portfolio_id: i64, date: NaiveDate) -> anyhow::Result<Option<f64>> {
    Ok(sqlx::query_scalar("SELECT aum::float8 FROM nav_history WHERE portfolio_id = $1 AND date = $2")
        .bind(portfolio_id)
        .bind(date)
        .fetch_optional(pool)
        .await?)
}
