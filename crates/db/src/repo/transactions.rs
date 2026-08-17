use chrono::NaiveDate;
use sqlx::PgPool;

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
