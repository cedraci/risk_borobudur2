//! The limit breach register: recorded check runs, their per-check results,
//! breach episodes and each episode's event timeline.
//!
//! Everything here is per portfolio and gated on `Domain::Settings` — see the
//! "Known limitation" section of the design for why that domain and not a
//! dedicated one. Runs and results are write-once: there is deliberately no
//! update method for either.

use crate::auth::marker::{Configure, Settings, View};
use crate::auth::Access;
use crate::scoped::Scoped;
use chrono::{DateTime, NaiveDate, Utc};
use sqlx::Row;
use std::collections::HashMap;

#[derive(Debug, Clone, serde::Serialize)]
pub struct CheckRunRow {
    pub id: i64,
    pub nav_date: NaiveDate,
    pub run_at: DateTime<Utc>,
    pub triggered_by: String,
    pub import_id: Option<i64>,
    pub actor_user_id: Option<i64>,
    pub inputs_complete: bool,
    pub input_notes: serde_json::Value,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CheckResultRow {
    pub check_key: String,
    pub scope_label: String,
    pub limit_value: Option<f64>,
    pub observed_value: Option<f64>,
    pub status: String,
    pub detail: serde_json::Value,
}

/// One run about to be written, with everything it found.
#[derive(Debug, Clone)]
pub struct NewRun {
    pub nav_date: NaiveDate,
    pub triggered_by: String,
    pub import_id: Option<i64>,
    pub actor_user_id: Option<i64>,
    pub inputs_complete: bool,
    pub input_notes: serde_json::Value,
    pub results: Vec<CheckResultRow>,
}

impl Scoped<'_> {
    /// Writes a run and its results in one transaction: a run with no results
    /// would read as "we checked and found nothing", which is not the same as
    /// "we checked".
    pub async fn record_run(
        &self, a: &Access<Settings, Configure>, run: &NewRun,
    ) -> anyhow::Result<i64> {
        let mut tx = self.pool.begin().await?;
        let run_id: i64 = sqlx::query_scalar(
            "INSERT INTO limit_check_runs
                 (portfolio_id, nav_date, triggered_by, import_id, actor_user_id,
                  inputs_complete, input_notes)
             VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING id")
            .bind(a.portfolio_id()).bind(run.nav_date).bind(&run.triggered_by)
            .bind(run.import_id).bind(run.actor_user_id)
            .bind(run.inputs_complete).bind(&run.input_notes)
            .fetch_one(&mut *tx).await?;
        for r in &run.results {
            sqlx::query(
                "INSERT INTO limit_check_results
                     (run_id, check_key, scope_label, limit_value, observed_value, status, detail)
                 VALUES ($1,$2,$3,$4,$5,$6,$7)")
                .bind(run_id).bind(&r.check_key).bind(&r.scope_label)
                .bind(r.limit_value).bind(r.observed_value).bind(&r.status).bind(&r.detail)
                .execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(run_id)
    }

    /// Newest run first, each with its results. `limit` is clamped by the
    /// caller; the register page asks for a page at a time.
    pub async fn runs_for(
        &self, a: &Access<Settings, View>, limit: i64,
    ) -> anyhow::Result<Vec<(CheckRunRow, Vec<CheckResultRow>)>> {
        let runs = sqlx::query(
            "SELECT id, nav_date, run_at, triggered_by, import_id, actor_user_id,
                    inputs_complete, input_notes
             FROM limit_check_runs WHERE portfolio_id = $1
             ORDER BY nav_date DESC, run_at DESC LIMIT $2")
            .bind(a.portfolio_id()).bind(limit)
            .fetch_all(self.pool).await?;
        let run_rows: Vec<CheckRunRow> = runs
            .iter()
            .map(|r| CheckRunRow {
                id: r.get("id"),
                nav_date: r.get("nav_date"),
                run_at: r.get("run_at"),
                triggered_by: r.get("triggered_by"),
                import_id: r.get("import_id"),
                actor_user_id: r.get("actor_user_id"),
                inputs_complete: r.get("inputs_complete"),
                input_notes: r.get("input_notes"),
            })
            .collect();

        // One query for every run's results rather than one per run: the page
        // asks for up to 500 runs at a time, and a loop here would be up to
        // 500 extra round-trips. `run_id` groups the single result set back
        // onto each run in Rust instead.
        let ids: Vec<i64> = run_rows.iter().map(|r| r.id).collect();
        let mut by_run: HashMap<i64, Vec<CheckResultRow>> = HashMap::new();
        if !ids.is_empty() {
            let result_rows = sqlx::query(
                "SELECT run_id, check_key, scope_label, limit_value, observed_value, status, detail
                 FROM limit_check_results WHERE run_id = ANY($1) ORDER BY run_id, check_key")
                .bind(&ids[..])
                .fetch_all(self.pool).await?;
            for x in &result_rows {
                let run_id: i64 = x.get("run_id");
                by_run.entry(run_id).or_default().push(CheckResultRow {
                    check_key: x.get("check_key"),
                    scope_label: x.get("scope_label"),
                    limit_value: x.get("limit_value"),
                    observed_value: x.get("observed_value"),
                    status: x.get("status"),
                    detail: x.get("detail"),
                });
            }
        }

        let mut out = Vec::with_capacity(run_rows.len());
        for run in run_rows {
            let results = by_run.remove(&run.id).unwrap_or_default();
            out.push((run, results));
        }
        Ok(out)
    }
}
