use crate::auth::marker::{Import, Shareholders, View};
use crate::auth::Access;
use crate::scoped::Scoped;
use chrono::NaiveDate;

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct FlowRecord {
    pub flow_date: NaiveDate,
    pub share_class: String,
    pub outstanding_shares: Option<f64>,
    pub nav_per_share: Option<f64>,
    pub subscription_amount: f64,
    pub redemption_amount: f64,
}

/// Idempotent: re-loading the same day overwrites rather than duplicating.
///
/// Takes a connection rather than a pool so `import_batch` can call it inside
/// its own transaction, following `seed_futures_contracts`. `pub(crate)`
/// rather than a gated `Scoped` method directly: `import_batch` writes this
/// table as part of its own (positions/nav/transactions) authorization, not a
/// fresh shareholders grant, so the type-safe entry point is
/// `Scoped::flows_upsert` below, which this function backs.
pub(crate) async fn flows_upsert_conn(
    conn: &mut sqlx::PgConnection, portfolio_id: i64, rows: &[ingest::ShareClassFlowRow],
) -> anyhow::Result<u64> {
    let mut n = 0;
    for r in rows {
        n += sqlx::query(
            "INSERT INTO share_class_flows
               (portfolio_id, flow_date, share_class, outstanding_shares,
                nav_per_share, subscription_amount, redemption_amount)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (portfolio_id, flow_date, share_class) DO UPDATE SET
               outstanding_shares  = EXCLUDED.outstanding_shares,
               nav_per_share       = EXCLUDED.nav_per_share,
               subscription_amount = EXCLUDED.subscription_amount,
               redemption_amount   = EXCLUDED.redemption_amount",
        )
        .bind(portfolio_id).bind(r.flow_date).bind(&r.share_class)
        .bind(r.outstanding_shares).bind(r.nav_per_share)
        .bind(r.subscription_amount).bind(r.redemption_amount)
        .execute(&mut *conn).await?.rows_affected();
    }
    Ok(n)
}

// ---- shareholder register (manually maintained top holders, as % of NAV) ----

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct Shareholder {
    pub id: i64,
    pub label: String,
    pub pct_of_nav: f64,
    pub as_of: NaiveDate,
}

impl<'a> Scoped<'a> {
    /// Standalone entry point for a shareholders-flows load outside an
    /// import transaction; acquires its own connection and delegates to
    /// `flows_upsert_conn`.
    pub async fn flows_upsert(
        &self, a: &Access<Shareholders, Import>, rows: &[ingest::ShareClassFlowRow],
    ) -> anyhow::Result<u64> {
        let mut conn = self.pool.acquire().await?;
        flows_upsert_conn(&mut conn, a.portfolio_id(), rows).await
    }

    /// The most recent `lookback` distinct dates, oldest first.
    pub async fn flows_for(&self, a: &Access<Shareholders, View>, lookback: u32) -> anyhow::Result<Vec<FlowRecord>> {
        Ok(sqlx::query_as(
            "SELECT flow_date, share_class,
                    outstanding_shares::float8 AS outstanding_shares,
                    nav_per_share::float8 AS nav_per_share,
                    subscription_amount::float8 AS subscription_amount,
                    redemption_amount::float8 AS redemption_amount
             FROM share_class_flows
             WHERE portfolio_id = $1 AND flow_date IN (
                 SELECT DISTINCT flow_date FROM share_class_flows
                 WHERE portfolio_id = $1 ORDER BY flow_date DESC LIMIT $2)
             ORDER BY flow_date, share_class",
        )
        .bind(a.portfolio_id()).bind(lookback as i64)
        .fetch_all(self.pool).await?)
    }

    /// Largest first: the top-five scenario reads straight off this order.
    pub async fn shareholders_for(&self, a: &Access<Shareholders, View>) -> anyhow::Result<Vec<Shareholder>> {
        Ok(sqlx::query_as(
            "SELECT id, label, pct_of_nav::float8 AS pct_of_nav, as_of
             FROM shareholders WHERE portfolio_id = $1 ORDER BY pct_of_nav DESC, id",
        )
        .bind(a.portfolio_id()).fetch_all(self.pool).await?)
    }

    /// Replace the full register for one portfolio in a single transaction, so a
    /// mid-list failure cannot leave a half-replaced register.
    pub async fn shareholders_replace(
        &self, a: &Access<Shareholders, Import>, rows: &[(String, f64, NaiveDate)],
    ) -> anyhow::Result<()> {
        let portfolio_id = a.portfolio_id();
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM shareholders WHERE portfolio_id = $1")
            .bind(portfolio_id).execute(&mut *tx).await?;
        for (label, pct, as_of) in rows {
            sqlx::query("INSERT INTO shareholders (portfolio_id, label, pct_of_nav, as_of) VALUES ($1, $2, $3, $4)")
                .bind(portfolio_id).bind(label).bind(pct).bind(as_of)
                .execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
