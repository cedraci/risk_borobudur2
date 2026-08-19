use crate::auth::marker::{Import, Positions, View};
use crate::auth::Access;
use crate::scoped::Scoped;
use chrono::NaiveDate;

/// One `Dividendes` snapshot row, as fetched from `position_snapshots` before
/// `derive_dividends` groups it by `(isin, currency)`.
type DividendSnapshotRow = (NaiveDate, String, Option<String>, Option<String>, Option<f64>);
/// `derive_dividends`'s working accumulator: instrument -> date -> (summed
/// local value, instrument name).
type DividendsByKey = std::collections::BTreeMap<(String, String), std::collections::BTreeMap<NaiveDate, (f64, Option<String>)>>;

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

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct DividendRecord {
    pub provision_date: NaiveDate,
    pub issuer: String,
    pub amount: f64,
    pub currency: String,
}

impl<'a> Scoped<'a> {
    pub async fn position_dates(&self, a: &Access<Positions, View>) -> anyhow::Result<Vec<NaiveDate>> {
        Ok(sqlx::query_scalar(
            "SELECT DISTINCT nav_date FROM position_snapshots WHERE portfolio_id = $1 ORDER BY nav_date DESC",
        )
        .bind(a.portfolio_id())
        .fetch_all(self.pool)
        .await?)
    }

    pub async fn positions_for(
        &self, a: &Access<Positions, View>, date: NaiveDate,
    ) -> anyhow::Result<Vec<PositionRecord>> {
        Ok(sqlx::query_as(
            "SELECT nav_date, asset_type, isin, name, currency,
                    quantity::float8 AS quantity, avg_cost::float8 AS avg_cost, price::float8 AS price,
                    valuation_ccy::float8 AS valuation_ccy, accrued_interest::float8 AS accrued_interest,
                    fx_rate::float8 AS fx_rate, valuation_eur::float8 AS valuation_eur,
                    weight::float8 AS weight, ticker
             FROM position_snapshots WHERE portfolio_id = $1 AND nav_date = $2 ORDER BY id",
        )
        .bind(a.portfolio_id())
        .bind(date)
        .fetch_all(self.pool)
        .await?)
    }

    pub async fn dividends_all(&self, a: &Access<Positions, View>) -> anyhow::Result<Vec<DividendRecord>> {
        Ok(sqlx::query_as(
            "SELECT provision_date, issuer, amount::float8 AS amount, currency
             FROM dividends WHERE portfolio_id = $1 ORDER BY provision_date",
        )
        .bind(a.portfolio_id())
        .fetch_all(self.pool)
        .await?)
    }

    /// Recompute the derived dividend set for a portfolio from its `Dividendes`
    /// snapshot rows (CACEIS CPON receivables). A pure function of the
    /// snapshots — delete-and-rebuild — so backlog uploads converge in any
    /// order. Change detection runs on the LOCAL-currency value: FX moves on a
    /// foreign receivable emit nothing. The first snapshot is baseline only
    /// (its receivables existed before monitoring started). Dates carrying an
    /// explicit (derived = false) dividend are skipped entirely.
    pub async fn derive_dividends(&self, a: &Access<Positions, Import>) -> anyhow::Result<usize> {
        use std::collections::BTreeMap;
        let portfolio_id = a.portfolio_id();

        let rows: Vec<DividendSnapshotRow> = sqlx::query_as(
            "SELECT nav_date, isin, name, currency, valuation_ccy::float8 FROM position_snapshots
             WHERE portfolio_id = $1 AND asset_type = 'Dividendes' ORDER BY nav_date")
            .bind(portfolio_id).fetch_all(self.pool).await?;
        let dates: Vec<NaiveDate> = sqlx::query_scalar(
            "SELECT DISTINCT nav_date FROM position_snapshots WHERE portfolio_id = $1 ORDER BY nav_date")
            .bind(portfolio_id).fetch_all(self.pool).await?;
        let explicit: Vec<NaiveDate> = sqlx::query_scalar(
            "SELECT DISTINCT provision_date FROM dividends WHERE portfolio_id = $1 AND NOT derived")
            .bind(portfolio_id).fetch_all(self.pool).await?;

        // (isin, currency) -> date -> summed local value (a code may appear twice).
        let mut by_key: DividendsByKey = BTreeMap::new();
        for (date, isin, name, currency, local) in rows {
            let Some(local) = local else { continue };
            let key = (isin, currency.unwrap_or_else(|| "EUR".into()));
            let e = by_key.entry(key).or_default().entry(date).or_insert((0.0, name));
            e.0 += local;
        }

        // Events: growth or appearance between consecutive snapshot dates.
        let mut events: Vec<(NaiveDate, String, f64, String)> = Vec::new();
        for ((isin, currency), series) in &by_key {
            for pair in dates.windows(2) {
                let (d_prev, d_cur) = (pair[0], pair[1]);
                let Some((cur, name)) = series.get(&d_cur) else { continue }; // absent = paid, no event
                let prev = series.get(&d_prev).map(|(v, _)| *v).unwrap_or(0.0);
                let delta = cur - prev;
                if delta > 0.005 && !explicit.contains(&d_cur) {
                    let issuer = name.clone().unwrap_or_else(|| isin.clone());
                    events.push((d_cur, issuer, delta, currency.clone()));
                }
            }
        }

        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM dividends WHERE portfolio_id = $1 AND derived")
            .bind(portfolio_id).execute(&mut *tx).await?;
        for (date, issuer, amount, currency) in &events {
            sqlx::query(
                "INSERT INTO dividends (portfolio_id, provision_date, issuer, amount, currency, derived)
                 VALUES ($1, $2, $3, $4, $5, true)")
                .bind(portfolio_id).bind(date).bind(issuer).bind(amount).bind(currency)
                .execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(events.len())
    }
}
