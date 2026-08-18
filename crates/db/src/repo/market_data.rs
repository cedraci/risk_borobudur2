use crate::auth::marker::{Import, MarketData, View};
use crate::auth::{Access, GlobalAccess};
use crate::scoped::Scoped;
use chrono::NaiveDate;

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct FxRow {
    pub date: NaiveDate,
    pub currency: String,
    pub rate_to_eur: f64,
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

impl<'a> Scoped<'a> {
    pub async fn fx_all(&self, _a: &GlobalAccess<MarketData, View>) -> anyhow::Result<Vec<FxRow>> {
        Ok(sqlx::query_as(
            "SELECT date, currency, rate_to_eur::float8 AS rate_to_eur
             FROM fx_history ORDER BY currency, date",
        )
        .fetch_all(self.pool)
        .await?)
    }

    /// Replace-by-key: an FX rate is market data, so a fresh pull always wins.
    pub async fn fx_upsert_many(&self, _a: &GlobalAccess<MarketData, Import>, rows: &[FxRow]) -> anyhow::Result<u64> {
        let mut tx = self.pool.begin().await?;
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

    /// Replace every analytics row for `date` in one transaction. Unlike the
    /// workbook import there is no content dedupe: the expected reason to
    /// re-upload is a corrected pull, which must win.
    pub async fn ctd_replace(
        &self,
        a: &Access<MarketData, Import>,
        date: NaiveDate,
        filename: &str,
        rows: &[ingest::CtdRow],
    ) -> anyhow::Result<usize> {
        let portfolio_id = a.portfolio_id();
        let mut tx = self.pool.begin().await?;
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

    pub async fn ctd_for(&self, a: &Access<MarketData, View>, date: NaiveDate) -> anyhow::Result<Vec<CtdRecord>> {
        Ok(sqlx::query_as(
            "SELECT nav_date, ticker, ctd_isin,
                    ctd_mod_duration::float8 AS ctd_mod_duration,
                    ctd_clean_price::float8 AS ctd_clean_price,
                    ctd_accrued::float8 AS ctd_accrued,
                    conversion_factor::float8 AS conversion_factor
             FROM futures_analytics WHERE portfolio_id = $1 AND nav_date = $2 ORDER BY ticker",
        )
        .bind(a.portfolio_id())
        .bind(date)
        .fetch_all(self.pool)
        .await?)
    }

    /// Store the Bloomberg ADV response: `adv_30d` and `adv_asof` only, touching
    /// no other column. `refs_upsert` deliberately never writes this pair — it is
    /// owned exclusively by this upload path, where `asof` is the upload date. A
    /// fresh pull always wins, matching `fx_upsert_many`'s replace discipline
    /// rather than `classify_upsert_many`'s COALESCE-preserve one: an ADV value
    /// is a snapshot in time, not a fact that only needs discovering once. No
    /// `portfolio_id` in the row shape — an ADV pull is keyed by instrument code
    /// alone — so this is instance-wide like the other Bloomberg-upload writes.
    pub async fn adv_upsert_many(
        &self,
        _a: &GlobalAccess<MarketData, Import>,
        rows: &[(String, f64)],
        asof: NaiveDate,
    ) -> anyhow::Result<u64> {
        let mut tx = self.pool.begin().await?;
        let mut n = 0u64;
        for (isin, adv_30d) in rows {
            n += sqlx::query(
                "INSERT INTO instrument_refs (code, adv_30d, adv_asof)
                 VALUES ($1, $2, $3)
                 ON CONFLICT (code) DO UPDATE SET
                   adv_30d = EXCLUDED.adv_30d,
                   adv_asof = EXCLUDED.adv_asof,
                   updated_at = now()",
            )
            .bind(isin).bind(adv_30d).bind(asof)
            .execute(&mut *tx).await?
            .rows_affected();
        }
        tx.commit().await?;
        Ok(n)
    }
}
