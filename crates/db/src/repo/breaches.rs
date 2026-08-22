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
use analytics::breach::{LiveEpisode, Proposal, Transition};
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

/// The `input_notes` key under which a run records that it left the episode
/// lifecycle alone. Named as a constant so the recorder, the tests and any
/// future reader agree on it.
pub const TRANSITIONS_SKIPPED_NOTE: &str = "transitions";

/// High half of this module's advisory-lock keys, so a lock taken here cannot
/// collide with one a future feature takes on a bare portfolio id. The low
/// half is the portfolio id.
const REGISTER_LOCK_TAG: i64 = 0x0B12;

fn register_lock_key(portfolio_id: i64) -> i64 {
    (REGISTER_LOCK_TAG << 32) | (portfolio_id & 0xFFFF_FFFF)
}

/// What `record_run_and_transitions` did.
#[derive(Debug, Clone, Copy)]
pub struct RunOutcome {
    pub run_id: i64,
    /// `Some(latest)` when this run's `nav_date` was older than `latest`, the
    /// newest `nav_date` already recorded for the portfolio, and the episode
    /// lifecycle was therefore left untouched. See
    /// `record_run_and_transitions`.
    pub transitions_skipped_after: Option<NaiveDate>,
}

impl Scoped<'_> {
    /// Records one run AND applies its transitions, in a single transaction
    /// under a per-portfolio advisory lock. The production entry point;
    /// `record_run` and `apply_transitions` below are the same two halves,
    /// exposed separately for tests only.
    ///
    /// Three things are load-bearing here and none of them survives splitting
    /// this into two calls:
    ///
    /// 1. **One transaction** (I2). A run whose results say `status =
    ///    "breach"` while `limit_breaches` holds no episode is a register that
    ///    contradicts its own run history — which is what a crash, or an
    ///    `apply_transitions` error, between two separate commits produced.
    ///    It only partially self-heals: the next run opens the episode with a
    ///    later `opened_nav_date`, permanently understating how long the fund
    ///    was in breach.
    /// 2. **`live_episodes` read inside the lock** (I3). Two concurrent runs
    ///    on one portfolio both read "no live episode for ACME", both compute
    ///    `Open`, and the loser hits `idx_breaches_live` and rolls back
    ///    *every* transition it computed — not just the conflicting one.
    ///    `pg_advisory_xact_lock` serializes the read against the write; the
    ///    partial unique index stays as the last-resort integrity net it was
    ///    designed to be.
    /// 3. **The back-date guard** (C2). `analytics::breach::transitions`
    ///    emits `Close` for every live episode absent from a run's findings —
    ///    it cannot tell a back-dated run from a fund that has cleared. So a
    ///    late or corrected depositary file dated before the register's
    ///    current state would stamp `closed_nav_date` *earlier than*
    ///    `opened_nav_date` on every open episode, plus a falsified `cleared`
    ///    event. `rerun` refuses such a date outright; an import cannot, and
    ///    should not — a back-dated file is legitimate history and the
    ///    register is meant to be complete. So the run and its results are
    ///    recorded as honest history for that date, and only the transition
    ///    phase is skipped. The skip is written into `input_notes` under
    ///    `TRANSITIONS_SKIPPED_NOTE`, never left for a reader to infer.
    ///
    /// `proposals_for` is called with the transitions computed inside the
    /// transaction; it must be pure (it runs while the lock is held).
    pub async fn record_run_and_transitions(
        &self, a: &Access<Settings, Configure>, run: &NewRun,
        findings: &[analytics::breach::Finding],
        actor_label: &str,
        proposals_for: &(dyn Fn(&[Transition]) -> HashMap<String, Proposal> + Send + Sync),
    ) -> anyhow::Result<RunOutcome> {
        let portfolio_id = a.portfolio_id();
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(register_lock_key(portfolio_id))
            .execute(&mut *tx).await?;

        let latest: Option<NaiveDate> = sqlx::query_scalar(
            "SELECT MAX(nav_date) FROM limit_check_runs WHERE portfolio_id = $1")
            .bind(portfolio_id).fetch_one(&mut *tx).await?;
        let skipped_after = latest.filter(|l| run.nav_date < *l);

        let mut run = run.clone();
        if let Some(l) = skipped_after {
            anyhow::ensure!(run.input_notes.is_object(),
                "input_notes must be a JSON object, got {}", run.input_notes);
            let notes = run.input_notes.as_object_mut().expect("checked just above");
            notes.insert(TRANSITIONS_SKIPPED_NOTE.to_string(), serde_json::Value::String(format!(
                "this run is back-dated ({} is before {l}, the newest run already recorded for \
                 this portfolio), so its results were recorded but no breach episode was opened, \
                 raised or closed",
                run.nav_date)));
        }
        let run_id = record_run_in(&mut tx, portfolio_id, &run).await?;

        if skipped_after.is_none() {
            let live = live_episodes_in(&mut tx, portfolio_id).await?;
            let transitions = analytics::breach::transitions(&live, findings);
            let proposals = proposals_for(&transitions);
            apply_transitions_in(
                &mut tx, portfolio_id,
                &RunContext { run_id, nav_date: run.nav_date, actor_label, actor_user_id: run.actor_user_id },
                &transitions, &proposals,
            ).await?;
        }

        tx.commit().await?;
        Ok(RunOutcome { run_id, transitions_skipped_after: skipped_after })
    }

    /// Writes a run and its results in one transaction: a run with no results
    /// would read as "we checked and found nothing", which is not the same as
    /// "we checked".
    ///
    /// Test-only. Production writes a run through
    /// `record_run_and_transitions`, which does this and the transitions in
    /// ONE transaction — see finding I2. Compiled out of a release build so
    /// the split path cannot come back.
    #[cfg(any(test, feature = "test-util"))]
    pub async fn record_run(
        &self, a: &Access<Settings, Configure>, run: &NewRun,
    ) -> anyhow::Result<i64> {
        let mut tx = self.pool.begin().await?;
        let run_id = record_run_in(&mut tx, a.portfolio_id(), run).await?;
        tx.commit().await?;
        Ok(run_id)
    }
}

async fn record_run_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, portfolio_id: i64, run: &NewRun,
) -> anyhow::Result<i64> {
    {
        let run_id: i64 = sqlx::query_scalar(
            "INSERT INTO limit_check_runs
                 (portfolio_id, nav_date, triggered_by, import_id, actor_user_id,
                  inputs_complete, input_notes)
             VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING id")
            .bind(portfolio_id).bind(run.nav_date).bind(&run.triggered_by)
            .bind(run.import_id).bind(run.actor_user_id)
            .bind(run.inputs_complete).bind(&run.input_notes)
            .fetch_one(&mut **tx).await?;
        for r in &run.results {
            sqlx::query(
                "INSERT INTO limit_check_results
                     (run_id, check_key, scope_label, limit_value, observed_value, status, detail)
                 VALUES ($1,$2,$3,$4,$5,$6,$7)")
                .bind(run_id).bind(&r.check_key).bind(&r.scope_label)
                .bind(r.limit_value).bind(r.observed_value).bind(&r.status).bind(&r.detail)
                .execute(&mut **tx).await?;
        }
        Ok(run_id)
    }
}

impl Scoped<'_> {
    /// The most recent snapshot date strictly before `before`, or `None` —
    /// what a proposal compares this run's holdings against. Mirrors
    /// `import_batch`'s token-mismatch guard: `pid` travelling separately
    /// from the token must not silently name a different portfolio.
    pub async fn position_dates_before(
        &self, a: &Access<Settings, View>, pid: i64, before: NaiveDate,
    ) -> anyhow::Result<Option<NaiveDate>> {
        anyhow::ensure!(pid == a.portfolio_id(),
            "position_dates_before: pid {pid} does not match the token's portfolio {}",
            a.portfolio_id());
        Ok(sqlx::query_scalar(
            "SELECT MAX(nav_date) FROM position_snapshots
             WHERE portfolio_id = $1 AND nav_date < $2")
            .bind(a.portfolio_id()).bind(before)
            .fetch_one(self.pool).await?)
    }

    /// Newest run first, each with its results. `limit` is clamped by the
    /// caller; the register page asks for a page at a time.
    pub async fn runs_for(
        &self, a: &Access<Settings, View>, limit: i64,
    ) -> anyhow::Result<Vec<(CheckRunRow, Vec<CheckResultRow>)>> {
        self.runs_with_results(a.portfolio_id(), Some(limit)).await
    }

    /// Every recorded run, no cap — the evidence export's need, distinct from
    /// `runs_for`'s paged UI read. An export that silently dropped runs past
    /// some limit would misstate the very history it exists to attest to.
    pub async fn runs_all(
        &self, a: &Access<Settings, View>,
    ) -> anyhow::Result<Vec<(CheckRunRow, Vec<CheckResultRow>)>> {
        self.runs_with_results(a.portfolio_id(), None).await
    }

    /// Shared by `runs_for` and `runs_all`: `limit = None` binds SQL `NULL`,
    /// which PostgreSQL's own `LIMIT` treats as "no limit" (the same as
    /// omitting the clause) — so the two callers share one query rather than
    /// one duplicating the other's text with the clause stripped out.
    async fn runs_with_results(
        &self, portfolio_id: i64, limit: Option<i64>,
    ) -> anyhow::Result<Vec<(CheckRunRow, Vec<CheckResultRow>)>> {
        let runs = sqlx::query(
            "SELECT id, nav_date, run_at, triggered_by, import_id, actor_user_id,
                    inputs_complete, input_notes
             FROM limit_check_runs WHERE portfolio_id = $1
             ORDER BY nav_date DESC, run_at DESC LIMIT $2")
            .bind(portfolio_id).bind(limit)
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

#[derive(Debug, Clone, serde::Serialize)]
pub struct BreachRow {
    pub id: i64,
    pub check_key: String,
    pub subject: String,
    pub opened_nav_date: NaiveDate,
    pub opened_value: Option<f64>,
    pub peak_value: Option<f64>,
    pub peak_nav_date: Option<NaiveDate>,
    pub closed_nav_date: Option<NaiveDate>,
    pub state: String,
    pub classification: String,
    pub proposed_classification: Option<String>,
    pub proposal_reason: Option<String>,
    pub acknowledged_at: Option<DateTime<Utc>>,
    /// `limit_breach_events.actor_label` of the most recent `acknowledged`
    /// event, not a join to `users` on `limit_breaches.acknowledged_by`: a
    /// deleted user (`ON DELETE SET NULL`) must not erase who acted from an
    /// audit artefact whose whole point is recording that. The event's label
    /// is captured at the moment of the act and is immutable.
    pub acknowledged_by_label: Option<String>,
    pub acknowledgement_note: Option<String>,
    pub deadline_date: Option<NaiveDate>,
    pub resolved_at: Option<DateTime<Utc>>,
    /// Same reasoning as `acknowledged_by_label`, sourced from the most
    /// recent `resolved` event.
    pub resolved_by_label: Option<String>,
    pub resolution_note: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BreachEventRow {
    pub at: DateTime<Utc>,
    pub actor_label: String,
    pub event: String,
    pub detail: serde_json::Value,
}

/// Who recorded a run, when, and against which run row. These four travel
/// together through every transition an `apply_transitions` call writes, so
/// they are one value rather than four positional arguments.
#[derive(Debug, Clone, Copy)]
pub struct RunContext<'a> {
    pub run_id: i64,
    pub nav_date: NaiveDate,
    pub actor_label: &'a str,
    pub actor_user_id: Option<i64>,
}

const BREACH_COLUMNS: &str =
    "b.id, b.check_key, b.subject, b.opened_nav_date, b.opened_value, b.peak_value, b.peak_nav_date, \
     b.closed_nav_date, b.state, b.classification, b.proposed_classification, b.proposal_reason, \
     b.acknowledged_at, b.acknowledgement_note, b.deadline_date, b.resolved_at, b.resolution_note";

/// The two "who" columns, each a correlated subquery onto its event's own
/// `actor_label` — see `BreachRow::acknowledged_by_label`'s doc for why this
/// reads `limit_breach_events` rather than joining `users`.
const BREACH_ACTOR_COLUMNS: &str =
    "(SELECT e.actor_label FROM limit_breach_events e \
      WHERE e.breach_id = b.id AND e.event = 'acknowledged' \
      ORDER BY e.at DESC LIMIT 1) AS acknowledged_by_label, \
     (SELECT e.actor_label FROM limit_breach_events e \
      WHERE e.breach_id = b.id AND e.event = 'resolved' \
      ORDER BY e.at DESC LIMIT 1) AS resolved_by_label";

fn breach_from_row(r: &sqlx::postgres::PgRow) -> BreachRow {
    BreachRow {
        id: r.get("id"),
        check_key: r.get("check_key"),
        subject: r.get("subject"),
        opened_nav_date: r.get("opened_nav_date"),
        opened_value: r.get("opened_value"),
        peak_value: r.get("peak_value"),
        peak_nav_date: r.get("peak_nav_date"),
        closed_nav_date: r.get("closed_nav_date"),
        state: r.get("state"),
        classification: r.get("classification"),
        proposed_classification: r.get("proposed_classification"),
        proposal_reason: r.get("proposal_reason"),
        acknowledged_at: r.get("acknowledged_at"),
        acknowledged_by_label: r.get("acknowledged_by_label"),
        acknowledgement_note: r.get("acknowledgement_note"),
        deadline_date: r.get("deadline_date"),
        resolved_at: r.get("resolved_at"),
        resolved_by_label: r.get("resolved_by_label"),
        resolution_note: r.get("resolution_note"),
    }
}

impl Scoped<'_> {
    /// Episodes still in breach on the data. This is what the next run's
    /// transitions are computed against.
    pub async fn live_episodes(
        &self, a: &Access<Settings, View>,
    ) -> anyhow::Result<Vec<LiveEpisode>> {
        Ok(sqlx::query(LIVE_EPISODES_SQL)
            .bind(a.portfolio_id()).fetch_all(self.pool).await?
            .iter().map(live_from_row).collect())
    }

    /// Applies one run's transitions and writes the matching timeline events,
    /// in a single transaction: an episode without its `opened` event would be
    /// a record with no provenance.
    ///
    /// Test-only, for the same reason as `record_run` above: production goes
    /// through `record_run_and_transitions`, which reads `live_episodes` under
    /// the same lock and in the same transaction as this write (I2, I3).
    #[cfg(any(test, feature = "test-util"))]
    pub async fn apply_transitions(
        &self, a: &Access<Settings, Configure>, ctx: &RunContext<'_>,
        transitions: &[Transition], proposals: &HashMap<String, Proposal>,
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        apply_transitions_in(&mut tx, a.portfolio_id(), ctx, transitions, proposals).await?;
        tx.commit().await?;
        Ok(())
    }
}

const LIVE_EPISODES_SQL: &str =
    "SELECT id, check_key, subject, peak_value FROM limit_breaches
     WHERE portfolio_id = $1 AND closed_nav_date IS NULL AND state <> 'resolved'
     ORDER BY id";

fn live_from_row(r: &sqlx::postgres::PgRow) -> LiveEpisode {
    LiveEpisode {
        id: r.get("id"),
        check_key: r.get("check_key"),
        subject: r.get("subject"),
        peak_value: r.get("peak_value"),
    }
}

async fn live_episodes_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, portfolio_id: i64,
) -> anyhow::Result<Vec<LiveEpisode>> {
    Ok(sqlx::query(LIVE_EPISODES_SQL)
        .bind(portfolio_id).fetch_all(&mut **tx).await?
        .iter().map(live_from_row).collect())
}

async fn apply_transitions_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, portfolio_id: i64,
    ctx: &RunContext<'_>, transitions: &[Transition], proposals: &HashMap<String, Proposal>,
) -> anyhow::Result<()> {
    {
        for t in transitions {
            match t {
                Transition::Open { check_key, subject, value } => {
                    let p = proposals.get(&format!("{check_key}\u{1f}{subject}"));
                    let breach_id: i64 = sqlx::query_scalar(
                        "INSERT INTO limit_breaches
                             (portfolio_id, check_key, subject, opened_run_id, opened_nav_date,
                              opened_value, peak_value, peak_nav_date,
                              proposed_classification, proposal_reason)
                         VALUES ($1,$2,$3,$4,$5,$6,$6,$5,$7,$8) RETURNING id")
                        .bind(portfolio_id).bind(check_key).bind(subject)
                        .bind(ctx.run_id).bind(ctx.nav_date).bind(value)
                        .bind(p.and_then(|p| p.classification))
                        .bind(p.map(|p| p.reason.as_str()))
                        .fetch_one(&mut **tx).await?;
                    sqlx::query(
                        "INSERT INTO limit_breach_events (breach_id, actor_user_id, actor_label, event, detail)
                         VALUES ($1,$2,$3,'opened',$4)")
                        .bind(breach_id).bind(ctx.actor_user_id).bind(ctx.actor_label)
                        .bind(serde_json::json!({
                            "nav_date": ctx.nav_date, "value": value,
                            "proposed": p.and_then(|p| p.classification),
                            "reason": p.map(|p| p.reason.clone()),
                        }))
                        .execute(&mut **tx).await?;
                }
                Transition::RaisePeak { id, value } => {
                    // The id in a transition is data, not a grant: without this
                    // portfolio check a caller authorized for one fund could name
                    // another fund's episode and falsify its record. `bail!` here
                    // runs before `tx.commit()`, so sqlx rolls the whole batch back
                    // on drop and nothing partial is persisted.
                    let hit: Option<i64> = sqlx::query_scalar(
                        "UPDATE limit_breaches SET peak_value = $2, peak_nav_date = $3
                         WHERE id = $1 AND portfolio_id = $4 RETURNING id")
                        .bind(id).bind(value).bind(ctx.nav_date).bind(portfolio_id)
                        .fetch_optional(&mut **tx).await?;
                    if hit.is_none() {
                        anyhow::bail!(
                            "transition names breach {id}, which is not in portfolio {}",
                            portfolio_id);
                    }
                    sqlx::query(
                        "INSERT INTO limit_breach_events (breach_id, actor_user_id, actor_label, event, detail)
                         VALUES ($1,$2,$3,'note',$4)")
                        .bind(id).bind(ctx.actor_user_id).bind(ctx.actor_label)
                        .bind(serde_json::json!({"peak_value": value, "nav_date": ctx.nav_date}))
                        .execute(&mut **tx).await?;
                }
                Transition::Close { id } => {
                    // `AND closed_nav_date IS NULL` (M5) makes this the
                    // conditional-update idiom every other state transition in
                    // this file uses. Without it a second application of the
                    // same `Close` silently overwrote `closed_run_id` /
                    // `closed_nav_date` and appended a SECOND `cleared` event
                    // to a timeline whose whole value is being exact. With it,
                    // a re-application is a no-op rather than a falsified
                    // record — so `bail!` below must not fire for an episode
                    // that is simply already closed, only for one that is not
                    // this portfolio's.
                    let hit: Option<i64> = sqlx::query_scalar(
                        "UPDATE limit_breaches SET closed_run_id = $2, closed_nav_date = $3
                         WHERE id = $1 AND portfolio_id = $4 AND closed_nav_date IS NULL
                         RETURNING id")
                        .bind(id).bind(ctx.run_id).bind(ctx.nav_date).bind(portfolio_id)
                        .fetch_optional(&mut **tx).await?;
                    if hit.is_none() {
                        let mine: Option<i64> = sqlx::query_scalar(
                            "SELECT id FROM limit_breaches WHERE id = $1 AND portfolio_id = $2")
                            .bind(id).bind(portfolio_id)
                            .fetch_optional(&mut **tx).await?;
                        anyhow::ensure!(mine.is_some(),
                            "transition names breach {id}, which is not in portfolio {portfolio_id}");
                        // Already closed: nothing to write, and nothing to
                        // report — the register is already in the state this
                        // transition asks for.
                        continue;
                    }
                    sqlx::query(
                        "INSERT INTO limit_breach_events (breach_id, actor_user_id, actor_label, event, detail)
                         VALUES ($1,$2,$3,'cleared',$4)")
                        .bind(id).bind(ctx.actor_user_id).bind(ctx.actor_label)
                        .bind(serde_json::json!({"nav_date": ctx.nav_date}))
                        .execute(&mut **tx).await?;
                }
            }
        }
        Ok(())
    }
}

impl Scoped<'_> {
    /// The register. `state` filters; `None` returns everything, newest first.
    pub async fn breaches_for(
        &self, a: &Access<Settings, View>, state: Option<&str>,
    ) -> anyhow::Result<Vec<BreachRow>> {
        let sql = format!(
            "SELECT {BREACH_COLUMNS}, {BREACH_ACTOR_COLUMNS} FROM limit_breaches b
             WHERE b.portfolio_id = $1 AND ($2::text IS NULL OR b.state = $2)
             ORDER BY b.opened_nav_date DESC, b.id DESC");
        Ok(sqlx::query(&sql).bind(a.portfolio_id()).bind(state)
            .fetch_all(self.pool).await?
            .iter().map(breach_from_row).collect())
    }

    /// One episode, or `None` when it belongs to another portfolio — the
    /// `portfolio_id` predicate is what stops an id from one fund being read
    /// through another fund's grant.
    pub async fn breach_get(
        &self, a: &Access<Settings, View>, breach_id: i64,
    ) -> anyhow::Result<Option<BreachRow>> {
        let sql = format!(
            "SELECT {BREACH_COLUMNS}, {BREACH_ACTOR_COLUMNS} FROM limit_breaches b
             WHERE b.id = $1 AND b.portfolio_id = $2");
        Ok(sqlx::query(&sql).bind(breach_id).bind(a.portfolio_id())
            .fetch_optional(self.pool).await?
            .as_ref().map(breach_from_row))
    }

    pub async fn breach_events(
        &self, a: &Access<Settings, View>, breach_id: i64,
    ) -> anyhow::Result<Vec<BreachEventRow>> {
        Ok(sqlx::query(
            "SELECT e.at, e.actor_label, e.event, e.detail
             FROM limit_breach_events e
             JOIN limit_breaches b ON b.id = e.breach_id
             WHERE e.breach_id = $1 AND b.portfolio_id = $2
             ORDER BY e.at, e.id")
            .bind(breach_id).bind(a.portfolio_id())
            .fetch_all(self.pool).await?
            .iter().map(|r| BreachEventRow {
                at: r.get("at"),
                actor_label: r.get("actor_label"),
                event: r.get("event"),
                detail: r.get("detail"),
            }).collect())
    }

    /// Acknowledgement is where a proposal becomes a decision. Refuses an
    /// episode that is already acknowledged or resolved: re-deciding
    /// something that was signed off is a new episode's business, not an
    /// overwrite of the record.
    #[allow(clippy::too_many_arguments)] // the task-8 brief's interface, verbatim
    pub async fn breach_acknowledge(
        &self, a: &Access<Settings, Configure>, breach_id: i64,
        classification: &str, note: &str, deadline: Option<NaiveDate>,
        actor_user_id: Option<i64>, actor_label: &str,
    ) -> anyhow::Result<bool> {
        let mut tx = self.pool.begin().await?;
        let n = sqlx::query(
            "UPDATE limit_breaches
             SET state = 'acknowledged', classification = $3, acknowledgement_note = $4,
                 deadline_date = $5, acknowledged_by = $6, acknowledged_at = now()
             WHERE id = $1 AND portfolio_id = $2 AND state = 'open'")
            .bind(breach_id).bind(a.portfolio_id()).bind(classification).bind(note)
            .bind(deadline).bind(actor_user_id)
            .execute(&mut *tx).await?.rows_affected();
        if n == 0 { tx.rollback().await?; return Ok(false); }
        sqlx::query(
            "INSERT INTO limit_breach_events (breach_id, actor_user_id, actor_label, event, detail)
             VALUES ($1,$2,$3,'acknowledged',$4)")
            .bind(breach_id).bind(actor_user_id).bind(actor_label)
            .bind(serde_json::json!({
                "classification": classification, "note": note, "deadline_date": deadline,
            }))
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Only from `acknowledged`. Resolving something nobody classified is the
    /// gap this whole feature exists to close.
    pub async fn breach_resolve(
        &self, a: &Access<Settings, Configure>, breach_id: i64, note: &str,
        actor_user_id: Option<i64>, actor_label: &str,
    ) -> anyhow::Result<bool> {
        let mut tx = self.pool.begin().await?;
        let n = sqlx::query(
            "UPDATE limit_breaches
             SET state = 'resolved', resolution_note = $3, resolved_by = $4, resolved_at = now()
             WHERE id = $1 AND portfolio_id = $2 AND state = 'acknowledged'")
            .bind(breach_id).bind(a.portfolio_id()).bind(note).bind(actor_user_id)
            .execute(&mut *tx).await?.rows_affected();
        if n == 0 { tx.rollback().await?; return Ok(false); }
        sqlx::query(
            "INSERT INTO limit_breach_events (breach_id, actor_user_id, actor_label, event, detail)
             VALUES ($1,$2,$3,'resolved',$4)")
            .bind(breach_id).bind(actor_user_id).bind(actor_label)
            .bind(serde_json::json!({"note": note}))
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }

    /// The latest snapshot date on file — what a manual re-run defaults to
    /// when no date is given. Mirrors `position_dates_before`'s
    /// token-mismatch guard: `pid` travelling separately from the token must
    /// not silently name a different portfolio.
    pub async fn latest_position_date(
        &self, a: &Access<Settings, View>, pid: i64,
    ) -> anyhow::Result<Option<NaiveDate>> {
        anyhow::ensure!(pid == a.portfolio_id(),
            "latest_position_date: pid {pid} does not match the token's portfolio {}",
            a.portfolio_id());
        Ok(sqlx::query_scalar(
            "SELECT MAX(nav_date) FROM position_snapshots WHERE portfolio_id = $1")
            .bind(a.portfolio_id())
            .fetch_one(self.pool).await?)
    }
}
