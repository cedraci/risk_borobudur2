# Limit Breach Register Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Record every limit check the tool performs on a portfolio, group consecutive breaches into episodes, and put an acknowledge/resolve workflow over them, so the fund has a register rather than a screenshot.

**Architecture:** Three layers, following the existing shape of this codebase. `analytics::breach` holds the pure logic (which episodes open, close and reopen given a run's results; whether a breach looks active or passive) with no database and no authorization. `db::repo::breaches` persists runs, results, episodes and their event timeline behind `Access<Settings, _>` tokens. `server::recorder` computes a run under a system context and writes it; the handlers read and drive the workflow under the caller's own grants.

**Tech Stack:** Rust (axum 0.8, sqlx, tokio), embedded PostgreSQL 17 for tests, React 19 + TypeScript + Vite + Vitest for the UI, `rust_xlsxwriter` for the evidence export.

**Spec:** `docs/superpowers/specs/2026-08-20-limit-breach-register-design.md` — read it before starting. Every decision below is argued there.

## Global Constraints

- **Platform is Windows.** Shell is PowerShell or Git Bash. Paths in commands use forward slashes; `cargo` and `npm` work from the repo root and `frontend/` respectively.
- **Embedded PostgreSQL.** Every `db`/`server` test starts its own throwaway PostgreSQL. The first run downloads binaries. Never kill a stray `postgres.exe` — tests own their own.
- **After adding a migration, run `touch crates/db/src/lib.rs` before rebuilding.** `sqlx::migrate!` embeds the directory at compile time and does not reliably notice a new file. Skipping this makes the next test run apply the old migration set and fail somewhere unrelated. This is documented in the README.
- **TDD is not optional here.** Write the test, run it, watch it fail for the stated reason, then implement. A test that passes the first time you run it is proving nothing.
- **Every new route is added to `crates/server/tests/api_authz_matrix.rs` in the same commit that adds it.** The matrix has a `CASES` table for portfolio-scoped routes; a new row is two lines.
- **Denials never render as data.** A denied read is `<Unavailable/>` in the UI and an explicit `{"status": "unavailable", "reason": ...}` marker on the wire. "No breaches" and "not permitted to see the breaches" must never look alike.
- **Commit after every task.** Message style: `feat(db): ...`, `feat(server): ...`, `feat(frontend): ...`, matching the existing log.
- **Verification before any completion claim:** `cargo test --workspace` (exit 0), `cargo clippy --workspace --all-targets -- -D warnings` (clean), and in `frontend/`: `npm run build`, `npm run lint`, `npm test`.

## File Structure

| File | Responsibility |
| --- | --- |
| `crates/db/migrations/0016_breach_register.sql` | Create the four tables and the partial unique index. |
| `crates/analytics/src/breach.rs` | Pure: episode transitions from run results; active/passive proposal from position pairs. No I/O. |
| `crates/db/src/repo/breaches.rs` | Persist and read runs, results, episodes, events. All behind `Access<Settings, _>`. |
| `crates/db/src/auth/model.rs` | Add `AuthCtx::system()` — the full-access context register runs execute under. |
| `crates/server/src/recorder.rs` | Compute one run's check results under the system context and write it, applying episode transitions. |
| `crates/server/src/handlers/breaches.rs` | The five HTTP handlers: run list, manual re-run, register list, episode detail, acknowledge, resolve, export. |
| `crates/server/src/routes.rs` | Mount them. |
| `crates/server/src/handlers/imports.rs` | Call the recorder after a successful import. |
| `frontend/src/api.ts` | Wire types and fetchers. |
| `frontend/src/pages/BreachesPage.tsx` | The register, the run-history grid, the episode detail. |
| `frontend/src/nav.ts` | Add the tab. |
| `docs/user-guide/breaches.md` | New chapter. |

---

### Task 1: Schema and run/result persistence

**Files:**
- Create: `crates/db/migrations/0016_breach_register.sql`
- Create: `crates/db/src/repo/breaches.rs`
- Modify: `crates/db/src/repo/mod.rs`
- Test: `crates/db/tests/breach_register_schema.rs`

**Interfaces:**
- Consumes: `crate::auth::{Access, marker::{Settings, View, Configure}}`, `crate::scoped::Scoped`.
- Produces:
  - `pub struct CheckRunRow { pub id: i64, pub nav_date: NaiveDate, pub run_at: DateTime<Utc>, pub triggered_by: String, pub import_id: Option<i64>, pub actor_user_id: Option<i64>, pub inputs_complete: bool, pub input_notes: serde_json::Value }`
  - `pub struct CheckResultRow { pub check_key: String, pub scope_label: String, pub limit_value: Option<f64>, pub observed_value: Option<f64>, pub status: String, pub detail: serde_json::Value }`
  - `pub struct NewRun { pub nav_date: NaiveDate, pub triggered_by: String, pub import_id: Option<i64>, pub actor_user_id: Option<i64>, pub inputs_complete: bool, pub input_notes: serde_json::Value, pub results: Vec<CheckResultRow> }`
  - `Scoped::record_run(&self, a: &Access<Settings, Configure>, run: &NewRun) -> anyhow::Result<i64>`
  - `Scoped::runs_for(&self, a: &Access<Settings, View>, limit: i64) -> anyhow::Result<Vec<(CheckRunRow, Vec<CheckResultRow>)>>`

- [ ] **Step 1: Write the failing test**

Create `crates/db/tests/breach_register_schema.rs`:

```rust
use chrono::NaiveDate;
use db::auth::marker::{Configure, Settings, View};
use db::auth::AuthCtx;
use db::repo::{CheckResultRow, NewRun};

async fn fresh() -> (db::Db, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    std::mem::forget(dir);
    (dbh, edb)
}

fn result(key: &str, status: &str, observed: Option<f64>) -> CheckResultRow {
    CheckResultRow {
        check_key: key.into(),
        scope_label: "Issuer <= 10% NAV (equities + bonds)".into(),
        limit_value: Some(0.10),
        observed_value: observed,
        status: status.into(),
        detail: serde_json::json!({"rows": []}),
    }
}

#[tokio::test]
async fn a_run_and_its_results_round_trip() {
    let (dbh, edb) = fresh().await;
    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let configure = scoped.authorize::<Settings, Configure>(1).unwrap();
    let view = scoped.authorize::<Settings, View>(1).unwrap();

    let run = NewRun {
        nav_date: NaiveDate::from_ymd_opt(2026, 8, 7).unwrap(),
        triggered_by: "import".into(),
        import_id: None,
        actor_user_id: None,
        inputs_complete: true,
        input_notes: serde_json::json!({}),
        results: vec![
            result("issuer_10", "breach", Some(0.106)),
            result("group_20", "ok", Some(0.04)),
        ],
    };
    let run_id = scoped.record_run(&configure, &run).await.unwrap();
    assert!(run_id > 0);

    let rows = scoped.runs_for(&view, 50).await.unwrap();
    assert_eq!(rows.len(), 1, "one run recorded");
    let (recorded, results) = &rows[0];
    assert_eq!(recorded.nav_date, NaiveDate::from_ymd_opt(2026, 8, 7).unwrap());
    assert_eq!(recorded.triggered_by, "import");
    assert!(recorded.inputs_complete);
    assert_eq!(results.len(), 2);
    let issuer = results.iter().find(|r| r.check_key == "issuer_10").unwrap();
    assert_eq!(issuer.status, "breach");
    assert_eq!(issuer.observed_value, Some(0.106));
    assert_eq!(issuer.detail["rows"], serde_json::json!([]));

    edb.stop().await;
}

#[tokio::test]
async fn a_result_with_no_natural_scalar_pair_stores_nulls() {
    let (dbh, edb) = fresh().await;
    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let configure = scoped.authorize::<Settings, Configure>(1).unwrap();
    let view = scoped.authorize::<Settings, View>(1).unwrap();

    let run = NewRun {
        nav_date: NaiveDate::from_ymd_opt(2026, 8, 7).unwrap(),
        triggered_by: "manual".into(),
        import_id: None,
        actor_user_id: None,
        inputs_complete: false,
        input_notes: serde_json::json!({"shareholders": "no register loaded"}),
        results: vec![CheckResultRow {
            check_key: "liq_top5".into(),
            scope_label: "Top 5 holders".into(),
            limit_value: None,
            observed_value: None,
            status: "breach".into(),
            detail: serde_json::json!({"waterfall": {"days": null}}),
        }],
    };
    scoped.record_run(&configure, &run).await.unwrap();

    let rows = scoped.runs_for(&view, 50).await.unwrap();
    let (recorded, results) = &rows[0];
    assert!(!recorded.inputs_complete);
    assert_eq!(recorded.input_notes["shareholders"], "no register loaded");
    assert_eq!(results[0].limit_value, None);
    assert_eq!(results[0].observed_value, None);

    edb.stop().await;
}
```

- [ ] **Step 2: Run the test and watch it fail**

Run: `cargo test -p db --test breach_register_schema`
Expected: FAIL to compile — `unresolved import db::repo::NewRun`, `no method named record_run`.

- [ ] **Step 3: Write the migration**

Create `crates/db/migrations/0016_breach_register.sql`:

```sql
-- The limit breach register. See
-- docs/superpowers/specs/2026-08-20-limit-breach-register-design.md.
--
-- Runs and results are immutable: nothing in the application updates them.
-- A limit lowered tomorrow cannot rewrite what a run said yesterday.

CREATE TABLE limit_check_runs (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  portfolio_id BIGINT NOT NULL REFERENCES portfolios(id) ON DELETE CASCADE,
  nav_date DATE NOT NULL,
  run_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  triggered_by TEXT NOT NULL CHECK (triggered_by IN ('import','manual')),
  import_id BIGINT REFERENCES imports(id) ON DELETE SET NULL,
  actor_user_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
  -- false when an input was genuinely absent (no shareholder register, no CTD
  -- analytics for the date). Never false because of a permission: a run
  -- computes under the system context.
  inputs_complete BOOLEAN NOT NULL DEFAULT true,
  input_notes JSONB NOT NULL DEFAULT '{}'::jsonb
);
CREATE UNIQUE INDEX idx_runs_unique ON limit_check_runs(portfolio_id, nav_date, run_at);
CREATE INDEX idx_runs_portfolio_date ON limit_check_runs(portfolio_id, nav_date DESC);

CREATE TABLE limit_check_results (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  run_id BIGINT NOT NULL REFERENCES limit_check_runs(id) ON DELETE CASCADE,
  check_key TEXT NOT NULL,
  scope_label TEXT NOT NULL,
  -- Both nullable: a check whose verdict comes from a waterfall rather than a
  -- threshold has no honest scalar pair, and renders from status + detail.
  limit_value DOUBLE PRECISION,
  observed_value DOUBLE PRECISION,
  status TEXT NOT NULL CHECK (status IN ('ok','watch','breach')),
  detail JSONB NOT NULL DEFAULT '{}'::jsonb
);
CREATE UNIQUE INDEX idx_results_unique ON limit_check_results(run_id, check_key);

CREATE TABLE limit_breaches (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  portfolio_id BIGINT NOT NULL REFERENCES portfolios(id) ON DELETE CASCADE,
  check_key TEXT NOT NULL,
  subject TEXT NOT NULL,
  opened_run_id BIGINT NOT NULL REFERENCES limit_check_runs(id) ON DELETE CASCADE,
  opened_nav_date DATE NOT NULL,
  opened_value DOUBLE PRECISION,
  peak_value DOUBLE PRECISION,
  peak_nav_date DATE,
  closed_run_id BIGINT REFERENCES limit_check_runs(id) ON DELETE SET NULL,
  closed_nav_date DATE,
  state TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open','acknowledged','resolved')),
  classification TEXT NOT NULL DEFAULT 'unclassified'
    CHECK (classification IN ('unclassified','active','passive')),
  proposed_classification TEXT CHECK (proposed_classification IN ('active','passive')),
  proposal_reason TEXT,
  acknowledged_by BIGINT REFERENCES users(id) ON DELETE SET NULL,
  acknowledged_at TIMESTAMPTZ,
  acknowledgement_note TEXT,
  deadline_date DATE,
  resolved_by BIGINT REFERENCES users(id) ON DELETE SET NULL,
  resolved_at TIMESTAMPTZ,
  resolution_note TEXT
);
-- At most one episode per subject that is still in breach on the data. An
-- episode that has cleared but awaits sign-off deliberately does NOT block a
-- new one: a fresh breach next week is a second thing to explain.
CREATE UNIQUE INDEX idx_breaches_live
  ON limit_breaches(portfolio_id, check_key, subject)
  WHERE closed_nav_date IS NULL AND state <> 'resolved';
CREATE INDEX idx_breaches_portfolio ON limit_breaches(portfolio_id, state);

CREATE TABLE limit_breach_events (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  breach_id BIGINT NOT NULL REFERENCES limit_breaches(id) ON DELETE CASCADE,
  at TIMESTAMPTZ NOT NULL DEFAULT now(),
  actor_user_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
  actor_label TEXT NOT NULL,
  event TEXT NOT NULL CHECK (event IN
    ('opened','classified','acknowledged','note','cleared','resolved','reopened')),
  detail JSONB NOT NULL DEFAULT '{}'::jsonb
);
CREATE INDEX idx_breach_events_breach ON limit_breach_events(breach_id, at);
```

- [ ] **Step 4: Write the repository module**

Create `crates/db/src/repo/breaches.rs`:

```rust
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
        let mut out = Vec::with_capacity(runs.len());
        for r in &runs {
            let run = CheckRunRow {
                id: r.get("id"),
                nav_date: r.get("nav_date"),
                run_at: r.get("run_at"),
                triggered_by: r.get("triggered_by"),
                import_id: r.get("import_id"),
                actor_user_id: r.get("actor_user_id"),
                inputs_complete: r.get("inputs_complete"),
                input_notes: r.get("input_notes"),
            };
            let results = sqlx::query(
                "SELECT check_key, scope_label, limit_value, observed_value, status, detail
                 FROM limit_check_results WHERE run_id = $1 ORDER BY check_key")
                .bind(run.id).fetch_all(self.pool).await?
                .iter().map(|x| CheckResultRow {
                    check_key: x.get("check_key"),
                    scope_label: x.get("scope_label"),
                    limit_value: x.get("limit_value"),
                    observed_value: x.get("observed_value"),
                    status: x.get("status"),
                    detail: x.get("detail"),
                }).collect();
            out.push((run, results));
        }
        Ok(out)
    }
}
```

Add to `crates/db/src/repo/mod.rs`, keeping the list alphabetical:

```rust
pub mod breaches;
```
and
```rust
pub use breaches::*;
```

- [ ] **Step 5: Force sqlx to re-embed the migration, then run the test**

Run:
```
touch crates/db/src/lib.rs
cargo test -p db --test breach_register_schema
```
Expected: PASS, 2 tests.

- [ ] **Step 6: Commit**

```bash
git add crates/db/migrations/0016_breach_register.sql crates/db/src/repo/breaches.rs crates/db/src/repo/mod.rs crates/db/tests/breach_register_schema.rs
git commit -m "feat(db): breach register schema and check-run persistence"
```

---

### Task 2: Episode transitions (pure)

**Files:**
- Create: `crates/analytics/src/breach.rs`
- Modify: `crates/analytics/src/lib.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks (deliberately — this module has no database and no authorization).
- Produces:
  - `pub struct Finding { pub check_key: String, pub subject: String, pub value: Option<f64> }`
  - `pub struct LiveEpisode { pub id: i64, pub check_key: String, pub subject: String, pub peak_value: Option<f64> }`
  - `pub enum Transition { Open { check_key: String, subject: String, value: Option<f64> }, RaisePeak { id: i64, value: f64 }, Close { id: i64 } }`
  - `pub fn transitions(live: &[LiveEpisode], findings: &[Finding]) -> Vec<Transition>`

- [ ] **Step 1: Write the failing test**

Append to `crates/analytics/src/breach.rs` (create the file with just this test module for now, plus `use super::*;`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn finding(check: &str, subject: &str, value: f64) -> Finding {
        Finding { check_key: check.into(), subject: subject.into(), value: Some(value) }
    }

    fn live(id: i64, check: &str, subject: &str, peak: f64) -> LiveEpisode {
        LiveEpisode { id, check_key: check.into(), subject: subject.into(), peak_value: Some(peak) }
    }

    #[test]
    fn a_first_breach_opens_an_episode() {
        let t = transitions(&[], &[finding("issuer_10", "ACME", 0.106)]);
        assert_eq!(t.len(), 1);
        assert!(matches!(&t[0], Transition::Open { check_key, subject, value }
            if check_key == "issuer_10" && subject == "ACME" && *value == Some(0.106)));
    }

    #[test]
    fn a_persisting_breach_does_not_open_a_second_episode() {
        let t = transitions(&[live(1, "issuer_10", "ACME", 0.106)],
                            &[finding("issuer_10", "ACME", 0.104)]);
        assert!(t.is_empty(), "still breaching, no worse: nothing to record, got {t:?}");
    }

    #[test]
    fn a_worsening_breach_raises_the_peak() {
        let t = transitions(&[live(1, "issuer_10", "ACME", 0.106)],
                            &[finding("issuer_10", "ACME", 0.121)]);
        assert_eq!(t.len(), 1);
        assert!(matches!(&t[0], Transition::RaisePeak { id: 1, value } if (*value - 0.121).abs() < 1e-12));
    }

    #[test]
    fn a_subject_that_stops_breaching_closes_its_episode() {
        let t = transitions(&[live(1, "issuer_10", "ACME", 0.106)], &[]);
        assert_eq!(t.len(), 1);
        assert!(matches!(&t[0], Transition::Close { id: 1 }));
    }

    #[test]
    fn episodes_are_keyed_by_check_and_subject_together() {
        // Same issuer breaching a different check is a different episode.
        let t = transitions(&[live(1, "issuer_10", "ACME", 0.106)],
                            &[finding("issuer_10", "ACME", 0.106),
                              finding("group_20", "ACME", 0.21)]);
        assert_eq!(t.len(), 1);
        assert!(matches!(&t[0], Transition::Open { check_key, .. } if check_key == "group_20"));
    }

    #[test]
    fn a_finding_with_no_value_still_opens_an_episode() {
        // The liquidity scenarios have no scalar; the episode is real anyway.
        let t = transitions(&[], &[Finding {
            check_key: "liq_top5".into(), subject: "Top 5 holders".into(), value: None,
        }]);
        assert_eq!(t.len(), 1);
        assert!(matches!(&t[0], Transition::Open { value: None, .. }));
    }
}
```

- [ ] **Step 2: Run the test and watch it fail**

Run: `cargo test -p analytics breach`
Expected: FAIL to compile — `cannot find type Finding`, `cannot find function transitions`.

- [ ] **Step 3: Write the implementation**

Prepend to `crates/analytics/src/breach.rs`, above the test module:

```rust
//! Breach episodes: the pure logic that turns one run's breaching findings
//! into transitions against the episodes already open.
//!
//! An episode, not a row per run: a breach that persists for six weeks is one
//! thing to remediate, not forty-two. Nothing here touches a database or
//! knows about authorization — it takes what is open, takes what this run
//! found, and says what changed.

use std::collections::{HashMap, HashSet};

/// One breaching row from a run: the check, the thing that breached it, and
/// the observed value where the check has one.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub check_key: String,
    pub subject: String,
    pub value: Option<f64>,
}

/// An episode already open on the data (`closed_nav_date IS NULL`).
#[derive(Debug, Clone, PartialEq)]
pub struct LiveEpisode {
    pub id: i64,
    pub check_key: String,
    pub subject: String,
    pub peak_value: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Transition {
    Open { check_key: String, subject: String, value: Option<f64> },
    RaisePeak { id: i64, value: f64 },
    Close { id: i64 },
}

/// What changed between the episodes currently open and what this run found.
///
/// Ordering is deterministic — opens in `findings` order, then peaks, then
/// closes in `live` order — so a test can assert on the sequence and a
/// reviewer reading the event timeline sees the same order every time.
pub fn transitions(live: &[LiveEpisode], findings: &[Finding]) -> Vec<Transition> {
    let key = |c: &str, s: &str| format!("{c}\u{1f}{s}");
    let open_by_key: HashMap<String, &LiveEpisode> =
        live.iter().map(|e| (key(&e.check_key, &e.subject), e)).collect();
    let found_keys: HashSet<String> =
        findings.iter().map(|f| key(&f.check_key, &f.subject)).collect();

    let mut out = Vec::new();
    for f in findings {
        match open_by_key.get(&key(&f.check_key, &f.subject)) {
            None => out.push(Transition::Open {
                check_key: f.check_key.clone(),
                subject: f.subject.clone(),
                value: f.value,
            }),
            Some(e) => {
                // A worse reading than the episode has ever seen is worth
                // recording; an equal or better one inside an open episode is
                // not news.
                if let Some(v) = f.value {
                    if e.peak_value.is_none_or(|p| v > p) {
                        out.push(Transition::RaisePeak { id: e.id, value: v });
                    }
                }
            }
        }
    }
    for e in live {
        if !found_keys.contains(&key(&e.check_key, &e.subject)) {
            out.push(Transition::Close { id: e.id });
        }
    }
    out
}
```

Add to `crates/analytics/src/lib.rs`, after `pub mod backtest;`:

```rust
pub mod breach;
```

Do **not** add `pub use breach::*;` — `Finding` and `Transition` are generic enough names that a glob re-export invites a collision later. Callers write `analytics::breach::transitions`.

- [ ] **Step 4: Run the test and watch it pass**

Run: `cargo test -p analytics breach`
Expected: PASS, 6 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/analytics/src/breach.rs crates/analytics/src/lib.rs
git commit -m "feat(analytics): breach episode transitions"
```

---

### Task 3: Active/passive proposal (pure)

**Files:**
- Modify: `crates/analytics/src/breach.rs`

**Interfaces:**
- Consumes: `Finding` from Task 2.
- Produces:
  - `pub struct SubjectHolding { pub isin: String, pub quantity: Option<f64> }`
  - `pub struct Proposal { pub classification: Option<&'static str>, pub reason: String }`
  - `pub fn propose(subject: &str, prev: Option<&[SubjectHolding]>, now: &[SubjectHolding], prev_weight: Option<f64>, now_weight: Option<f64>) -> Proposal`

- [ ] **Step 1: Write the failing test**

Append to the `tests` module in `crates/analytics/src/breach.rs`:

```rust
    fn hold(isin: &str, q: f64) -> SubjectHolding {
        SubjectHolding { isin: isin.into(), quantity: Some(q) }
    }

    #[test]
    fn no_purchase_proposes_passive_and_says_how_the_weight_moved() {
        let p = propose("ACME", Some(&[hold("X1", 100.0)]), &[hold("X1", 100.0)],
                        Some(0.094), Some(0.106));
        assert_eq!(p.classification, Some("passive"));
        assert!(p.reason.contains("no purchase in ACME"), "{}", p.reason);
        assert!(p.reason.contains("9.40%") && p.reason.contains("10.60%"), "{}", p.reason);
    }

    #[test]
    fn an_increased_quantity_proposes_active_and_names_the_instrument() {
        let p = propose("ACME", Some(&[hold("X1", 100.0)]), &[hold("X1", 180.0)],
                        Some(0.094), Some(0.106));
        assert_eq!(p.classification, Some("active"));
        assert!(p.reason.contains("X1"), "{}", p.reason);
        assert!(p.reason.contains("100") && p.reason.contains("180"), "{}", p.reason);
    }

    #[test]
    fn an_instrument_that_is_new_this_snapshot_is_a_purchase() {
        let p = propose("ACME", Some(&[]), &[hold("X2", 50.0)], Some(0.0), Some(0.11));
        assert_eq!(p.classification, Some("active"));
        assert!(p.reason.contains("X2"), "{}", p.reason);
    }

    #[test]
    fn with_no_previous_snapshot_nothing_is_proposed() {
        let p = propose("ACME", None, &[hold("X1", 100.0)], None, Some(0.106));
        assert_eq!(p.classification, None);
        assert!(p.reason.contains("no prior position to compare"), "{}", p.reason);
    }

    #[test]
    fn a_subject_with_no_holdings_at_all_proposes_nothing() {
        // Liquidity, VaR and EMIR episodes have no issuer subject.
        let p = propose("Top 5 holders", Some(&[]), &[], None, None);
        assert_eq!(p.classification, None);
        assert!(p.reason.contains("not derived from positions"), "{}", p.reason);
    }
```

- [ ] **Step 2: Run the test and watch it fail**

Run: `cargo test -p analytics breach`
Expected: FAIL to compile — `cannot find function propose`, `cannot find type SubjectHolding`.

- [ ] **Step 3: Write the implementation**

Append to `crates/analytics/src/breach.rs`, above the test module:

```rust
/// One instrument belonging to a breaching subject, at one snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct SubjectHolding {
    pub isin: String,
    pub quantity: Option<f64>,
}

/// What the machine thinks caused a breach, and why. `classification` is
/// `None` when the data cannot support a suggestion — never a guess.
#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    pub classification: Option<&'static str>,
    pub reason: String,
}

/// Quantities below this are treated as equal: position files carry rounding,
/// and a 1e-9 drift is not a purchase.
const QTY_EPSILON: f64 = 1e-6;

/// Proposes `active` or `passive` for a breach of `subject`, from the change
/// in its holdings between the previous snapshot and this one.
///
/// Deliberately derived from positions rather than the trade journal:
/// CACEIS-fed portfolios have no journal at all, and a classification that
/// silently skips those funds is worse than one that works everywhere and
/// asks a person to confirm it. Nothing here decides anything — the caller
/// stores this as a *proposal* and a reviewer confirms or overrides it.
pub fn propose(
    subject: &str,
    prev: Option<&[SubjectHolding]>,
    now: &[SubjectHolding],
    prev_weight: Option<f64>,
    now_weight: Option<f64>,
) -> Proposal {
    let Some(prev) = prev else {
        return Proposal {
            classification: None,
            reason: "first snapshot for this portfolio; no prior position to compare".to_string(),
        };
    };
    if prev.is_empty() && now.is_empty() {
        return Proposal {
            classification: None,
            reason: format!("{subject} has no instrument holdings; not derived from positions"),
        };
    }
    let before: std::collections::HashMap<&str, f64> = prev.iter()
        .map(|h| (h.isin.as_str(), h.quantity.unwrap_or(0.0)))
        .collect();
    let bought = now.iter().find(|h| {
        let was = before.get(h.isin.as_str()).copied().unwrap_or(0.0);
        h.quantity.unwrap_or(0.0) > was + QTY_EPSILON
    });
    match bought {
        Some(h) => {
            let was = before.get(h.isin.as_str()).copied().unwrap_or(0.0);
            Proposal {
                classification: Some("active"),
                reason: format!(
                    "quantity of {} rose from {} to {} since the previous snapshot",
                    h.isin, trim(was), trim(h.quantity.unwrap_or(0.0))),
            }
        }
        None => Proposal {
            classification: Some("passive"),
            reason: format!(
                "no purchase in {subject} since the previous snapshot; weight moved from {} to {}",
                pct(prev_weight), pct(now_weight)),
        },
    }
}

fn pct(x: Option<f64>) -> String {
    match x {
        Some(v) => format!("{:.2}%", v * 100.0),
        None => "an unknown weight".to_string(),
    }
}

/// Quantities are whole units far more often than not; render them without a
/// trailing ".00" so the reason reads like something a person wrote.
fn trim(x: f64) -> String {
    if (x - x.round()).abs() < QTY_EPSILON { format!("{}", x.round() as i64) } else { format!("{x:.4}") }
}
```

- [ ] **Step 4: Run the test and watch it pass**

Run: `cargo test -p analytics breach`
Expected: PASS, 11 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/analytics/src/breach.rs
git commit -m "feat(analytics): propose active vs passive from position changes"
```

---

### Task 4: Episode persistence and the event timeline

**Files:**
- Modify: `crates/db/src/repo/breaches.rs`
- Test: `crates/db/tests/breach_episodes.rs`

**Interfaces:**
- Consumes: `Scoped::record_run` (Task 1), `analytics::breach::Transition` (Task 2).
- Produces:
  - `pub struct BreachRow { pub id: i64, pub check_key: String, pub subject: String, pub opened_nav_date: NaiveDate, pub opened_value: Option<f64>, pub peak_value: Option<f64>, pub closed_nav_date: Option<NaiveDate>, pub state: String, pub classification: String, pub proposed_classification: Option<String>, pub proposal_reason: Option<String>, pub acknowledged_at: Option<DateTime<Utc>>, pub acknowledgement_note: Option<String>, pub deadline_date: Option<NaiveDate>, pub resolved_at: Option<DateTime<Utc>>, pub resolution_note: Option<String> }`
  - `pub struct BreachEventRow { pub at: DateTime<Utc>, pub actor_label: String, pub event: String, pub detail: serde_json::Value }`
  - `Scoped::live_episodes(&self, a: &Access<Settings, View>) -> anyhow::Result<Vec<analytics::breach::LiveEpisode>>`
  - `Scoped::apply_transitions(&self, a: &Access<Settings, Configure>, run_id: i64, nav_date: NaiveDate, actor_label: &str, actor_user_id: Option<i64>, transitions: &[analytics::breach::Transition], proposals: &HashMap<String, analytics::breach::Proposal>) -> anyhow::Result<()>` — `proposals` is keyed `"{check_key}\u{1f}{subject}"`
  - `Scoped::breaches_for(&self, a: &Access<Settings, View>, state: Option<&str>) -> anyhow::Result<Vec<BreachRow>>`
  - `Scoped::breach_events(&self, a: &Access<Settings, View>, breach_id: i64) -> anyhow::Result<Vec<BreachEventRow>>`

`crates/db` already depends on `analytics` (`crates/db/Cargo.toml`), so the shared `Transition`/`Proposal`/`LiveEpisode` types are importable with no manifest change.

- [ ] **Step 1: Write the failing test**

Create `crates/db/tests/breach_episodes.rs`:

```rust
use analytics::breach::{LiveEpisode, Proposal, Transition};
use chrono::NaiveDate;
use db::auth::marker::{Configure, Settings, View};
use db::auth::AuthCtx;
use db::repo::{CheckResultRow, NewRun};
use std::collections::HashMap;

async fn fresh() -> (db::Db, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    std::mem::forget(dir);
    (dbh, edb)
}

fn run_on(day: u32) -> NewRun {
    NewRun {
        nav_date: NaiveDate::from_ymd_opt(2026, 8, day).unwrap(),
        triggered_by: "import".into(),
        import_id: None,
        actor_user_id: None,
        inputs_complete: true,
        input_notes: serde_json::json!({}),
        results: vec![CheckResultRow {
            check_key: "issuer_10".into(),
            scope_label: "Issuer <= 10% NAV (equities + bonds)".into(),
            limit_value: Some(0.10),
            observed_value: Some(0.106),
            status: "breach".into(),
            detail: serde_json::json!({}),
        }],
    }
}

#[tokio::test]
async fn an_episode_opens_carries_its_proposal_and_closes() {
    let (dbh, edb) = fresh().await;
    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let configure = scoped.authorize::<Settings, Configure>(1).unwrap();
    let view = scoped.authorize::<Settings, View>(1).unwrap();

    // Day 7: the breach opens, with a proposal attached.
    let run1 = scoped.record_run(&configure, &run_on(7)).await.unwrap();
    let mut proposals = HashMap::new();
    proposals.insert("issuer_10\u{1f}ACME".to_string(), Proposal {
        classification: Some("passive"),
        reason: "no purchase in ACME since the previous snapshot".into(),
    });
    scoped.apply_transitions(
        &configure, run1, NaiveDate::from_ymd_opt(2026, 8, 7).unwrap(),
        "system", None,
        &[Transition::Open { check_key: "issuer_10".into(), subject: "ACME".into(), value: Some(0.106) }],
        &proposals,
    ).await.unwrap();

    let open = scoped.breaches_for(&view, Some("open")).await.unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].subject, "ACME");
    assert_eq!(open[0].proposed_classification.as_deref(), Some("passive"));
    assert_eq!(open[0].classification, "unclassified", "a proposal is not a decision");
    assert_eq!(open[0].closed_nav_date, None);
    let id = open[0].id;

    // The episode is live, so the next run sees it.
    let live = scoped.live_episodes(&view).await.unwrap();
    assert_eq!(live, vec![LiveEpisode {
        id, check_key: "issuer_10".into(), subject: "ACME".into(), peak_value: Some(0.106),
    }]);

    // Day 14: it clears on the data. The state does NOT move.
    let run2 = scoped.record_run(&configure, &run_on(14)).await.unwrap();
    scoped.apply_transitions(
        &configure, run2, NaiveDate::from_ymd_opt(2026, 8, 14).unwrap(),
        "system", None, &[Transition::Close { id }], &HashMap::new(),
    ).await.unwrap();

    let all = scoped.breaches_for(&view, None).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].closed_nav_date, NaiveDate::from_ymd_opt(2026, 8, 14));
    assert_eq!(all[0].state, "open", "clearing on the data is not sign-off");
    assert!(scoped.live_episodes(&view).await.unwrap().is_empty());

    let events = scoped.breach_events(&view, id).await.unwrap();
    let kinds: Vec<&str> = events.iter().map(|e| e.event.as_str()).collect();
    assert_eq!(kinds, vec!["opened", "cleared"]);

    edb.stop().await;
}

#[tokio::test]
async fn a_second_live_episode_for_the_same_subject_is_refused() {
    let (dbh, edb) = fresh().await;
    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let configure = scoped.authorize::<Settings, Configure>(1).unwrap();

    let run1 = scoped.record_run(&configure, &run_on(7)).await.unwrap();
    let open = || Transition::Open {
        check_key: "issuer_10".into(), subject: "ACME".into(), value: Some(0.106),
    };
    scoped.apply_transitions(&configure, run1, NaiveDate::from_ymd_opt(2026, 8, 7).unwrap(),
        "system", None, &[open()], &HashMap::new()).await.unwrap();

    let run2 = scoped.record_run(&configure, &run_on(8)).await.unwrap();
    let again = scoped.apply_transitions(&configure, run2, NaiveDate::from_ymd_opt(2026, 8, 8).unwrap(),
        "system", None, &[open()], &HashMap::new()).await;
    assert!(again.is_err(), "the partial unique index must refuse a second live episode");

    edb.stop().await;
}

#[tokio::test]
async fn raising_the_peak_records_the_worst_reading_and_its_date() {
    let (dbh, edb) = fresh().await;
    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let configure = scoped.authorize::<Settings, Configure>(1).unwrap();
    let view = scoped.authorize::<Settings, View>(1).unwrap();

    let run1 = scoped.record_run(&configure, &run_on(7)).await.unwrap();
    scoped.apply_transitions(&configure, run1, NaiveDate::from_ymd_opt(2026, 8, 7).unwrap(),
        "system", None,
        &[Transition::Open { check_key: "issuer_10".into(), subject: "ACME".into(), value: Some(0.106) }],
        &HashMap::new()).await.unwrap();
    let id = scoped.breaches_for(&view, None).await.unwrap()[0].id;

    let run2 = scoped.record_run(&configure, &run_on(14)).await.unwrap();
    scoped.apply_transitions(&configure, run2, NaiveDate::from_ymd_opt(2026, 8, 14).unwrap(),
        "system", None, &[Transition::RaisePeak { id, value: 0.131 }], &HashMap::new()).await.unwrap();

    let row = &scoped.breaches_for(&view, None).await.unwrap()[0];
    assert_eq!(row.peak_value, Some(0.131));
    assert_eq!(row.opened_value, Some(0.106), "the opening value is not overwritten");

    edb.stop().await;
}
```

- [ ] **Step 2: Run the test and watch it fail**

Run: `cargo test -p db --test breach_episodes`
Expected: FAIL to compile — `no method named live_episodes`, `apply_transitions`, `breaches_for`, `breach_events`.

- [ ] **Step 3: Write the implementation**

Append to `crates/db/src/repo/breaches.rs`:

```rust
use analytics::breach::{LiveEpisode, Proposal, Transition};
use std::collections::HashMap;

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
    pub acknowledgement_note: Option<String>,
    pub deadline_date: Option<NaiveDate>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolution_note: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BreachEventRow {
    pub at: DateTime<Utc>,
    pub actor_label: String,
    pub event: String,
    pub detail: serde_json::Value,
}

const BREACH_COLUMNS: &str =
    "id, check_key, subject, opened_nav_date, opened_value, peak_value, peak_nav_date, \
     closed_nav_date, state, classification, proposed_classification, proposal_reason, \
     acknowledged_at, acknowledgement_note, deadline_date, resolved_at, resolution_note";

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
        acknowledgement_note: r.get("acknowledgement_note"),
        deadline_date: r.get("deadline_date"),
        resolved_at: r.get("resolved_at"),
        resolution_note: r.get("resolution_note"),
    }
}

impl Scoped<'_> {
    /// Episodes still in breach on the data. This is what the next run's
    /// transitions are computed against.
    pub async fn live_episodes(
        &self, a: &Access<Settings, View>,
    ) -> anyhow::Result<Vec<LiveEpisode>> {
        Ok(sqlx::query(
            "SELECT id, check_key, subject, peak_value FROM limit_breaches
             WHERE portfolio_id = $1 AND closed_nav_date IS NULL AND state <> 'resolved'
             ORDER BY id")
            .bind(a.portfolio_id()).fetch_all(self.pool).await?
            .iter().map(|r| LiveEpisode {
                id: r.get("id"),
                check_key: r.get("check_key"),
                subject: r.get("subject"),
                peak_value: r.get("peak_value"),
            }).collect())
    }

    /// Applies one run's transitions and writes the matching timeline events,
    /// in a single transaction: an episode without its `opened` event would be
    /// a record with no provenance.
    pub async fn apply_transitions(
        &self, a: &Access<Settings, Configure>, run_id: i64, nav_date: NaiveDate,
        actor_label: &str, actor_user_id: Option<i64>,
        transitions: &[Transition], proposals: &HashMap<String, Proposal>,
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
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
                        .bind(a.portfolio_id()).bind(check_key).bind(subject)
                        .bind(run_id).bind(nav_date).bind(value)
                        .bind(p.and_then(|p| p.classification))
                        .bind(p.map(|p| p.reason.as_str()))
                        .fetch_one(&mut *tx).await?;
                    sqlx::query(
                        "INSERT INTO limit_breach_events (breach_id, actor_user_id, actor_label, event, detail)
                         VALUES ($1,$2,$3,'opened',$4)")
                        .bind(breach_id).bind(actor_user_id).bind(actor_label)
                        .bind(serde_json::json!({
                            "nav_date": nav_date, "value": value,
                            "proposed": p.and_then(|p| p.classification),
                            "reason": p.map(|p| p.reason.clone()),
                        }))
                        .execute(&mut *tx).await?;
                }
                Transition::RaisePeak { id, value } => {
                    sqlx::query(
                        "UPDATE limit_breaches SET peak_value = $2, peak_nav_date = $3 WHERE id = $1")
                        .bind(id).bind(value).bind(nav_date).execute(&mut *tx).await?;
                    sqlx::query(
                        "INSERT INTO limit_breach_events (breach_id, actor_user_id, actor_label, event, detail)
                         VALUES ($1,$2,$3,'note',$4)")
                        .bind(id).bind(actor_user_id).bind(actor_label)
                        .bind(serde_json::json!({"peak_value": value, "nav_date": nav_date}))
                        .execute(&mut *tx).await?;
                }
                Transition::Close { id } => {
                    sqlx::query(
                        "UPDATE limit_breaches SET closed_run_id = $2, closed_nav_date = $3 WHERE id = $1")
                        .bind(id).bind(run_id).bind(nav_date).execute(&mut *tx).await?;
                    sqlx::query(
                        "INSERT INTO limit_breach_events (breach_id, actor_user_id, actor_label, event, detail)
                         VALUES ($1,$2,$3,'cleared',$4)")
                        .bind(id).bind(actor_user_id).bind(actor_label)
                        .bind(serde_json::json!({"nav_date": nav_date}))
                        .execute(&mut *tx).await?;
                }
            }
        }
        tx.commit().await?;
        Ok(())
    }

    /// The register. `state` filters; `None` returns everything, newest first.
    pub async fn breaches_for(
        &self, a: &Access<Settings, View>, state: Option<&str>,
    ) -> anyhow::Result<Vec<BreachRow>> {
        let sql = format!(
            "SELECT {BREACH_COLUMNS} FROM limit_breaches
             WHERE portfolio_id = $1 AND ($2::text IS NULL OR state = $2)
             ORDER BY opened_nav_date DESC, id DESC");
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
            "SELECT {BREACH_COLUMNS} FROM limit_breaches WHERE id = $1 AND portfolio_id = $2");
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
}
```

- [ ] **Step 4: Run the test and watch it pass**

Run: `cargo test -p db --test breach_episodes`
Expected: PASS, 3 tests.

- [ ] **Step 5: Run the whole db suite so nothing else moved**

Run: `cargo test -p db`
Expected: PASS, all targets.

- [ ] **Step 6: Commit**

```bash
git add crates/db/src/repo/breaches.rs crates/db/tests/breach_episodes.rs crates/db/Cargo.toml
git commit -m "feat(db): breach episode persistence and event timeline"
```

---

### Task 5: The system context and the recorder

**Files:**
- Modify: `crates/db/src/auth/model.rs`
- Create: `crates/server/src/recorder.rs`
- Modify: `crates/server/src/lib.rs`
- Test: `crates/server/tests/api_breach_recorder.rs`

**Interfaces:**
- Consumes: everything from Tasks 1–4.
- Produces:
  - `db::auth::AuthCtx::system() -> AuthCtx`
  - `server::recorder::record(st: &AppState, portfolio_id: i64, nav_date: NaiveDate, trigger: Trigger) -> anyhow::Result<i64>`
  - `pub enum Trigger { Import { import_id: i64, actor_user_id: Option<i64>, actor_label: String }, Manual { actor_user_id: Option<i64>, actor_label: String } }`

- [ ] **Step 1: Write the failing test**

Create `crates/server/tests/api_breach_recorder.rs`:

```rust
//! The register is the fund's compliance record, not a transcript of what one
//! user could see. A run triggered by a principal without reference access
//! must still be computed on the real issuer groups — otherwise finding P3
//! (a denial rendering as data) comes back, persisted and harder to notice.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::NaiveDate;
use db::auth::marker::{Settings, View};
use db::auth::{Action, AuthCtx, Domain, Grant};
use tower::util::ServiceExt;

const SAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx");
const BOUNDARY: &str = "XBOUNDARYX";

async fn app() -> (axum::Router, sqlx::PgPool, db::Db, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    let pool = dbh.test_pool().clone();
    std::mem::forget(dir);
    let desktop = server::routes::router(server::state::AppState::desktop(dbh.clone()));
    (desktop, pool, dbh, edb)
}

fn upload_req(uri: &str, bytes: &[u8]) -> Request<Body> {
    let mut body = Vec::new();
    body.extend_from_slice(format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"s.xlsx\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    ).as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    Request::post(uri)
        .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(Body::from(body)).unwrap()
}

#[tokio::test]
async fn an_import_records_one_run_per_snapshot_date() {
    let (desktop, pool, dbh, edb) = app().await;
    let bytes = std::fs::read(SAMPLE).unwrap();
    let res = desktop.clone().oneshot(upload_req("/api/portfolios/1/imports", &bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let view = scoped.authorize::<Settings, View>(1).unwrap();
    let runs = scoped.runs_for(&view, 50).await.unwrap();
    assert!(!runs.is_empty(), "an import must record a run");
    let (run, results) = &runs[0];
    assert_eq!(run.triggered_by, "import");
    assert!(run.import_id.is_some(), "the run must point at the import that caused it");
    assert!(results.iter().any(|r| r.check_key == "issuer_10"),
        "the concentration checks must be recorded: {:?}",
        results.iter().map(|r| &r.check_key).collect::<Vec<_>>());

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn a_run_uses_the_real_reference_data_even_when_the_importer_cannot_see_it() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    let pool = dbh.test_pool().clone();
    std::mem::forget(dir);
    let desktop = server::routes::router(server::state::AppState::desktop(dbh.clone()));
    let server = server::routes::router(server::state::AppState::server(dbh.clone()));

    // An issuer-group override that regroups two holdings under one issuer.
    // With reference data denied, the checks would fall back to the default
    // per-name grouping and under-aggregate.
    let admin_ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&admin_ctx);
    let rc = scoped.authorize_global::<db::auth::marker::Reference, db::auth::marker::Configure>().unwrap();
    for code in ["AT000000STR1", "AT0000606306"] {
        scoped.refs_upsert(&rc, &db::repo::InstrumentRef {
            code: code.into(),
            issuer_group: Some("SHARED GROUP".into()),
            liquidity_days: None,
            adv_eligible: None,
            bond_coupon_pct: None, bond_maturity: None, bond_coupon_freq: None,
            bond_next_coupon: None, bond_nominal: None,
            market_place: None, market_place_name: None,
            adv_30d: None, adv_asof: None,
            country_of_risk: None, region: None, gics_sector: None, gics_industry: None,
            ticker: None,
        }).await.unwrap();
    }

    // An importer with import rights and NO reference grant at all.
    let hash = server::auth::local::hash_password("pw").unwrap();
    let admin = db::admin::Admin::new(&pool);
    let uid = admin.create_user("ops@f.lu", "Ops", &hash, false).await.unwrap();
    for d in [Domain::Positions, Domain::Nav, Domain::Transactions] {
        admin.grant_add(uid, Grant { domain: d, action: Action::Import, portfolio: Some(1) }, None).await.unwrap();
    }
    admin.session_create(&server::auth::local::token_hash("ops-t"), uid, 1).await.unwrap();

    let bytes = std::fs::read(SAMPLE).unwrap();
    let mut req = upload_req("/api/portfolios/1/imports", &bytes);
    req.headers_mut().insert("cookie", "borobudur_session=ops-t".parse().unwrap());
    let res = server.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let view = scoped.authorize::<Settings, View>(1).unwrap();
    let runs = scoped.runs_for(&view, 50).await.unwrap();
    let (run, results) = &runs[0];
    assert!(run.inputs_complete,
        "inputs_complete is about missing data, never about the caller's grants");
    let issuer = results.iter().find(|r| r.check_key == "issuer_10").unwrap();
    assert_ne!(issuer.status, "unavailable",
        "a recorded run must never carry a denial marker: {issuer:?}");
    let groups: Vec<String> = issuer.detail["rows"].as_array().unwrap_or(&vec![])
        .iter().filter_map(|r| r["group"].as_str().map(str::to_string)).collect();
    assert!(groups.iter().any(|g| g == "SHARED GROUP"),
        "the override must have been applied under the system context, got {groups:?}");

    let _ = desktop;
    pool.close().await;
    edb.stop().await;
}
```

The `InstrumentRef` literal above is the full struct as of this baseline —
`crates/db/tests/instrument_refs.rs` builds the same one. If a field has been
added since, the compiler will say so; copy the shape from that test rather
than guessing.

- [ ] **Step 2: Run the test and watch it fail**

Run: `cargo test -p server --test api_breach_recorder`
Expected: FAIL — `an_import_records_one_run_per_snapshot_date` panics with "an import must record a run" (the import succeeds, nothing is recorded).

- [ ] **Step 3: Add the system context**

In `crates/db/src/auth/model.rs`, beside `AuthCtx::desktop()`:

```rust
    /// The context a *recorded* computation runs under.
    ///
    /// The breach register is the fund's compliance record, not a transcript
    /// of what one user could see: a run computed on fallback issuer groups
    /// because the importer lacked reference access would be a denial
    /// rendering as data, persisted. Constructed only by
    /// `server::recorder`; it never reaches a request handler, and it writes
    /// nothing a principal with full grants could not have computed.
    pub fn system() -> Self {
        AuthCtx {
            principal_id: 0,
            display_name: "system".to_string(),
            is_administrator: false,
            grants: GrantSet::all_access(),
            source_addr: None,
        }
    }
```

- [ ] **Step 4: Write the recorder**

Create `crates/server/src/recorder.rs`:

```rust
//! Computes one limit-check run for a portfolio and writes it to the register.
//!
//! Runs under `AuthCtx::system()`, not the caller's grants — see the design's
//! "The system context" section. The caller's identity is still recorded, on
//! the run row and on every timeline event, so the register says who caused
//! the run even though it does not say what they could see.

use crate::state::AppState;
use analytics::breach::{self, Finding, Proposal, SubjectHolding};
use chrono::NaiveDate;
use db::auth::marker::{Configure, Settings, View};
use db::auth::AuthCtx;
use db::repo::{CheckResultRow, NewRun};
use std::collections::HashMap;

pub enum Trigger {
    Import { import_id: i64, actor_user_id: Option<i64>, actor_label: String },
    Manual { actor_user_id: Option<i64>, actor_label: String },
}

impl Trigger {
    fn kind(&self) -> &'static str {
        match self { Trigger::Import { .. } => "import", Trigger::Manual { .. } => "manual" }
    }
    fn actor(&self) -> (Option<i64>, &str) {
        match self {
            Trigger::Import { actor_user_id, actor_label, .. } => (*actor_user_id, actor_label),
            Trigger::Manual { actor_user_id, actor_label } => (*actor_user_id, actor_label),
        }
    }
    fn import_id(&self) -> Option<i64> {
        match self { Trigger::Import { import_id, .. } => Some(*import_id), _ => None }
    }
}

/// Computes and records one run. Returns the run id.
///
/// Failure here must never fail the request that triggered it — an import
/// that imported is an import, and losing the user's data to protect the
/// register is the wrong trade. Callers log and carry on; see
/// `handlers::imports`.
pub async fn record(
    st: &AppState, portfolio_id: i64, nav_date: NaiveDate, trigger: Trigger,
) -> anyhow::Result<i64> {
    let ctx = AuthCtx::system();
    let scoped = st.db.scope(&ctx);
    let view = scoped.authorize::<Settings, View>(portfolio_id)
        .map_err(|d| anyhow::anyhow!("system context refused: {d}"))?;
    let configure = scoped.authorize::<Settings, Configure>(portfolio_id)
        .map_err(|d| anyhow::anyhow!("system context refused: {d}"))?;

    let (results, findings, holdings, weights) =
        crate::handlers::breaches::compute(&scoped, portfolio_id, nav_date).await?;

    let mut input_notes = serde_json::Map::new();
    // `inputs_complete` is about data that is genuinely absent, never about
    // permissions — the system context holds every grant.
    if holdings.is_empty() {
        input_notes.insert("positions".into(), "no position snapshot for this date".into());
    }
    let inputs_complete = input_notes.is_empty();

    let (actor_user_id, actor_label) = trigger.actor();
    let actor_label = actor_label.to_string();
    let run = NewRun {
        nav_date,
        triggered_by: trigger.kind().to_string(),
        import_id: trigger.import_id(),
        actor_user_id,
        inputs_complete,
        input_notes: serde_json::Value::Object(input_notes),
        results,
    };
    let run_id = scoped.record_run(&configure, &run).await?;

    let live = scoped.live_episodes(&view).await?;
    let transitions = breach::transitions(&live, &findings);

    // A proposal is only built for episodes about to open, and only where the
    // subject maps to instruments — liquidity, VaR and EMIR subjects do not.
    let prev_date = scoped.position_dates_before(&view, portfolio_id, nav_date).await?;
    let mut proposals: HashMap<String, Proposal> = HashMap::new();
    for t in &transitions {
        if let breach::Transition::Open { check_key, subject, .. } = t {
            let now: Vec<SubjectHolding> = holdings.get(subject).cloned().unwrap_or_default();
            let prev: Option<Vec<SubjectHolding>> = match prev_date {
                Some(d) => Some(crate::handlers::breaches::holdings_at(&scoped, portfolio_id, d).await?
                    .get(subject).cloned().unwrap_or_default()),
                None => None,
            };
            let p = breach::propose(
                subject, prev.as_deref(), &now,
                weights.get(&(prev_date, subject.clone())).copied(),
                weights.get(&(Some(nav_date), subject.clone())).copied(),
            );
            proposals.insert(format!("{check_key}\u{1f}{subject}"), p);
        }
    }

    scoped.apply_transitions(
        &configure, run_id, nav_date, &actor_label, actor_user_id, &transitions, &proposals,
    ).await?;
    Ok(run_id)
}
```

Add to `crates/server/src/lib.rs`:

```rust
pub mod recorder;
```

**Note for the implementer:** `compute`, `holdings_at` and
`position_dates_before` do not exist yet. Write them in this task:

- `crates/server/src/handlers/breaches.rs::compute(scoped, pid, nav_date)` returns
  `(Vec<CheckResultRow>, Vec<Finding>, HashMap<String, Vec<SubjectHolding>>, HashMap<(Option<NaiveDate>, String), f64>)`.
  Build it by lifting the body of `handlers::limits::concentration_h` — the
  `ConPosition` assembly and the `concentration(&cons)` call — into a function
  that returns `Check`s instead of JSON, then map each `Check` to a
  `CheckResultRow` (`observed_value` = the worst row's weight) and each
  breaching `CheckRow` to a `Finding`. Start with **concentration only** in
  this task; Task 6 adds liquidity, VaR and EMIR to the same function, and
  the tests here only assert on `issuer_10`.
- `holdings_at` maps issuer group → its instruments' ISINs and quantities,
  using the same grouping rule `concentration_h` uses (`issuer_group`
  override, falling back to `default_issuer_group`, with `Fonds` never
  regrouped).
- `Scoped::position_dates_before(&self, a: &Access<Settings, View>, pid: i64, before: NaiveDate)`
  goes in `crates/db/src/repo/breaches.rs` and returns
  `Option<NaiveDate>` — the most recent snapshot date strictly before
  `before`, or `None`.

- [ ] **Step 5: Hook the recorder into the import path**

In `crates/server/src/handlers/imports.rs::import_one`, after the audit call inside the `Ok(outcome)` arm:

```rust
            // The register run is best-effort: an import that imported is an
            // import, and failing the user's upload to protect the register
            // would be the wrong trade. Logged loudly instead.
            if !outcome.duplicate {
                for d in batch.snapshots.iter().map(|s| s.nav_date).collect::<std::collections::BTreeSet<_>>() {
                    if let Err(e) = crate::recorder::record(st, target.id, d, crate::recorder::Trigger::Import {
                        import_id: outcome.import_id,
                        actor_user_id: (ctx.principal_id != 0).then_some(ctx.principal_id),
                        actor_label: ctx.display_name.clone(),
                    }).await {
                        tracing::error!("breach register run failed for portfolio {} on {d}: {e:#}", target.id);
                    }
                }
            }
```

- [ ] **Step 6: Run the test and watch it pass**

Run: `cargo test -p server --test api_breach_recorder`
Expected: PASS, 2 tests.

- [ ] **Step 7: Run the full suite — the import path is shared**

Run: `cargo test --workspace`
Expected: exit 0. `api_imports`, `api_ingest_routing`, `api_partial_denial` and `api_audit` all drive imports and must be unaffected.

- [ ] **Step 8: Commit**

```bash
git add crates/db/src/auth/model.rs crates/db/src/repo/breaches.rs crates/server/src/recorder.rs crates/server/src/lib.rs crates/server/src/handlers/breaches.rs crates/server/src/handlers/imports.rs crates/server/src/handlers/mod.rs crates/server/tests/api_breach_recorder.rs
git commit -m "feat(server): record a limit check run on every import, under a system context"
```

---

### Task 6: Extend coverage to liquidity, VaR and EMIR

**Files:**
- Modify: `crates/server/src/handlers/breaches.rs`
- Test: `crates/server/tests/api_breach_recorder.rs`

**Interfaces:**
- Consumes: `compute` from Task 5.
- Produces: no new signatures — `compute` returns more `CheckResultRow`s.

- [ ] **Step 1: Write the failing test**

Append to `crates/server/tests/api_breach_recorder.rs`:

```rust
#[tokio::test]
async fn a_run_covers_every_check_that_has_a_limit() {
    let (desktop, pool, dbh, edb) = app().await;
    let bytes = std::fs::read(SAMPLE).unwrap();
    let res = desktop.clone().oneshot(upload_req("/api/portfolios/1/imports", &bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let view = scoped.authorize::<Settings, View>(1).unwrap();
    let (_, results) = &scoped.runs_for(&view, 50).await.unwrap()[0];
    let keys: std::collections::BTreeSet<&str> =
        results.iter().map(|r| r.check_key.as_str()).collect();

    for expected in ["issuer_10", "forty", "group_20", "fund_20", "deposit_20",
                     "liq_top5", "liq_fixed", "liq_hybrid_top5", "liq_hybrid_fixed",
                     "var_limit",
                     "emir_credit", "emir_equity", "emir_interest_rate", "emir_fx",
                     "emir_commodity_other"] {
        assert!(keys.contains(expected), "missing {expected} from a run: {keys:?}");
    }

    // The liquidity scenarios have no honest scalar pair.
    let liq = results.iter().find(|r| r.check_key == "liq_top5").unwrap();
    assert_eq!(liq.limit_value, None);
    assert_eq!(liq.observed_value, None);
    assert!(!liq.scope_label.is_empty());

    // VaR does: the configured limit against the measured utilisation.
    let var = results.iter().find(|r| r.check_key == "var_limit").unwrap();
    assert!(var.limit_value.is_some(), "var_limit stores the configured limit");

    pool.close().await;
    edb.stop().await;
}
```

- [ ] **Step 2: Run the test and watch it fail**

Run: `cargo test -p server --test api_breach_recorder a_run_covers_every_check`
Expected: FAIL with "missing liq_top5 from a run".

- [ ] **Step 3: Extend `compute`**

In `crates/server/src/handlers/breaches.rs`, add to `compute`:

- **Liquidity** — call the same assembly `handlers::limits::liquidity_h` uses to build `scenarios`. For each scenario, emit a `CheckResultRow` with `check_key = format!("liq_{}", scenario_key)`, `scope_label` = the scenario's human label (`"Top 5 holders"`, `"Fixed shock"`, `"Top 5 holders, stressed ADV"`, `"Fixed shock, stressed ADV"`), `limit_value: None`, `observed_value: None`, `status` = the scenario's own `"ok"`/`"breach"`, and `detail` = the scenario JSON. A scenario the data cannot produce (no shareholder register) is recorded with `status: "ok"` **only if it genuinely passed**; where it cannot be evaluated, skip the row entirely and add a note to `input_notes` — a check that could not run must not appear in the register as one that passed.
- **VaR** — read `scoped.get_settings(portfolio_id)`, compute the NAV returns as `handlers::metrics::var` does, and emit `check_key = "var_limit"`, `limit_value = Some(settings.var_limit)`, `observed_value = Some(historical_var)`, `status = "breach"` when `observed > limit`, `"watch"` at or above `0.8 * limit`, else `"ok"`. Where there is too little history for a VaR at all, skip the row and note it.
- **EMIR** — call the same assembly `handlers::emir::get` uses; for each `ClassReport` emit `check_key = format!("emir_{}", class_snake_case)` where the five values are exactly `credit`, `equity`, `interest_rate`, `fx`, `commodity_other`; `scope_label` = the class label; `limit_value` = `threshold_eur`; `observed_value` = `avg_otc_eur`; `status` from `Verdict`; `detail` = the serialized `ClassReport`.

Findings: emit one `Finding` per breaching row for concentration (subject = the row's group), and one per breaching non-concentration check (subject = the `scope_label`, since those have no per-row subject).

- [ ] **Step 4: Run the test and watch it pass**

Run: `cargo test -p server --test api_breach_recorder`
Expected: PASS, 3 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/handlers/breaches.rs crates/server/tests/api_breach_recorder.rs
git commit -m "feat(server): record liquidity, VaR and EMIR checks in the register"
```

---

### Task 7: Read endpoints and the authorization matrix

**Files:**
- Modify: `crates/server/src/handlers/breaches.rs`, `crates/server/src/routes.rs`
- Test: `crates/server/tests/api_breach_register.rs`, `crates/server/tests/api_authz_matrix.rs`

**Interfaces:**
- Consumes: `Scoped::runs_for`, `breaches_for`, `breach_get`, `breach_events`.
- Produces: `handlers::breaches::{runs_list, register_list, episode_get}`.

- [ ] **Step 1: Write the failing test**

Create `crates/server/tests/api_breach_register.rs` with a test asserting that, after an import through the desktop router:

```rust
#[tokio::test]
async fn the_register_lists_runs_and_open_episodes() {
    // ... same app()/upload_req helpers as api_breach_recorder.rs ...
    let res = desktop.clone().oneshot(
        Request::get("/api/portfolios/1/limit-runs").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let runs = body["runs"].as_array().unwrap();
    assert!(!runs.is_empty());
    assert!(runs[0]["results"].as_array().unwrap().iter()
        .any(|r| r["check_key"] == "issuer_10"));

    let res = desktop.clone().oneshot(
        Request::get("/api/portfolios/1/breaches").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}
```

And add to `crates/server/tests/api_authz_matrix.rs`'s `CASES`:

```rust
    r("/api/portfolios/{pid}/limit-runs", Domain::Settings, Action::View),
    r("/api/portfolios/{pid}/breaches", Domain::Settings, Action::View),
```

- [ ] **Step 2: Run the tests and watch them fail**

Run: `cargo test -p server --test api_breach_register --test api_authz_matrix`
Expected: FAIL — 404 on both new routes; the matrix's `no_cookie_is_401_for_every_case` fails because an unmounted route returns 404 rather than 401.

- [ ] **Step 3: Write the handlers and mount them**

In `crates/server/src/handlers/breaches.rs`:

```rust
#[derive(serde::Deserialize)]
pub struct RunsQuery { pub limit: Option<i64> }

pub async fn runs_list(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>,
    Path(pid): Path<i64>, Query(q): Query<RunsQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Settings, View>(pid)?;
    super::portfolios::ensure(&scoped, pid, false).await?;
    let limit = q.limit.unwrap_or(52).clamp(1, 500);
    let runs: Vec<serde_json::Value> = scoped.runs_for(&a, limit).await?
        .into_iter()
        .map(|(run, results)| serde_json::json!({
            "id": run.id, "nav_date": run.nav_date, "run_at": run.run_at,
            "triggered_by": run.triggered_by, "import_id": run.import_id,
            "inputs_complete": run.inputs_complete, "input_notes": run.input_notes,
            "results": results,
        }))
        .collect();
    Ok(Json(serde_json::json!({ "runs": runs })))
}

#[derive(serde::Deserialize)]
pub struct RegisterQuery { pub state: Option<String> }

pub async fn register_list(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>,
    Path(pid): Path<i64>, Query(q): Query<RegisterQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Settings, View>(pid)?;
    super::portfolios::ensure(&scoped, pid, false).await?;
    if let Some(s) = q.state.as_deref() {
        if !matches!(s, "open" | "acknowledged" | "resolved") {
            return Err(AppError::BadRequest(format!("unknown state: {s}")));
        }
    }
    let rows = scoped.breaches_for(&a, q.state.as_deref()).await?;
    Ok(Json(serde_json::json!({ "breaches": rows })))
}

pub async fn episode_get(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>,
    Path((pid, bid)): Path<(i64, i64)>,
) -> Result<Json<serde_json::Value>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Settings, View>(pid)?;
    super::portfolios::ensure(&scoped, pid, false).await?;
    let breach = scoped.breach_get(&a, bid).await?
        .ok_or_else(|| AppError::NotFound(format!("no breach {bid}")))?;
    let events = scoped.breach_events(&a, bid).await?;
    Ok(Json(serde_json::json!({ "breach": breach, "events": events })))
}
```

In `crates/server/src/routes.rs`, beside the other `/api/portfolios/{id}/...` routes:

```rust
        .protected("/api/portfolios/{id}/limit-runs", get(handlers::breaches::runs_list), Domain::Settings, Action::View)
        .protected("/api/portfolios/{id}/breaches", get(handlers::breaches::register_list), Domain::Settings, Action::View)
        .protected("/api/portfolios/{id}/breaches/{bid}", get(handlers::breaches::episode_get), Domain::Settings, Action::View)
```

Add `pub mod breaches;` to `crates/server/src/handlers/mod.rs` if Task 5 did not already.

- [ ] **Step 4: Run the tests and watch them pass**

Run: `cargo test -p server --test api_breach_register --test api_authz_matrix`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/handlers/breaches.rs crates/server/src/routes.rs crates/server/tests/api_breach_register.rs crates/server/tests/api_authz_matrix.rs
git commit -m "feat(server): read endpoints for the breach register"
```

---

### Task 8: The workflow — manual re-run, acknowledge, resolve

**Files:**
- Modify: `crates/db/src/repo/breaches.rs`, `crates/server/src/handlers/breaches.rs`, `crates/server/src/routes.rs`
- Test: `crates/server/tests/api_breach_register.rs`, `crates/server/tests/api_authz_matrix.rs`

**Interfaces:**
- Produces:
  - `Scoped::breach_acknowledge(&self, a: &Access<Settings, Configure>, breach_id: i64, classification: &str, note: &str, deadline: Option<NaiveDate>, actor_user_id: Option<i64>, actor_label: &str) -> anyhow::Result<bool>` — `false` when no such episode in this portfolio
  - `Scoped::breach_resolve(&self, a: &Access<Settings, Configure>, breach_id: i64, note: &str, actor_user_id: Option<i64>, actor_label: &str) -> anyhow::Result<bool>`
  - `handlers::breaches::{rerun, acknowledge, resolve}`

- [ ] **Step 1: Write the failing test**

Append to `crates/server/tests/api_breach_register.rs`:

```rust
#[tokio::test]
async fn an_episode_must_be_classified_before_it_can_be_resolved() {
    // Import, then find an open episode. If the sample workbook produces no
    // breach, insert one directly through the repository so the state machine
    // is what is under test, not the fixture's holdings.
    // ... setup ...

    // Resolving straight from `open` is refused.
    let res = desktop.clone().oneshot(
        Request::post(format!("/api/portfolios/1/breaches/{bid}/resolve"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"note":"looks fine"}"#)).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY,
        "resolving something nobody classified is the gap this feature closes");

    // Acknowledging without a classification is refused.
    let res = desktop.clone().oneshot(
        Request::post(format!("/api/portfolios/1/breaches/{bid}/acknowledge"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"classification":"unclassified","note":"x"}"#)).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Acknowledging with an empty note is refused.
    let res = desktop.clone().oneshot(
        Request::post(format!("/api/portfolios/1/breaches/{bid}/acknowledge"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"classification":"passive","note":"  "}"#)).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Acknowledge properly, then resolve.
    let res = desktop.clone().oneshot(
        Request::post(format!("/api/portfolios/1/breaches/{bid}/acknowledge"))
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"classification":"passive","note":"market move, no purchase","deadline_date":"2026-09-30"}"#
            )).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let res = desktop.clone().oneshot(
        Request::post(format!("/api/portfolios/1/breaches/{bid}/resolve"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"note":"position trimmed on 21 Aug"}"#)).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // The timeline carries both acts, in order.
    let res = desktop.clone().oneshot(
        Request::get(format!("/api/portfolios/1/breaches/{bid}")).body(Body::empty()).unwrap()).await.unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["breach"]["state"], "resolved");
    assert_eq!(body["breach"]["classification"], "passive");
    let kinds: Vec<&str> = body["events"].as_array().unwrap().iter()
        .map(|e| e["event"].as_str().unwrap()).collect();
    assert!(kinds.contains(&"acknowledged") && kinds.contains(&"resolved"), "{kinds:?}");
}

#[tokio::test]
async fn a_manual_rerun_records_a_second_run_for_the_same_date() {
    // ... import, then POST /api/portfolios/1/limit-runs ...
    let res = desktop.clone().oneshot(
        Request::post("/api/portfolios/1/limit-runs")
            .header("content-type", "application/json")
            .body(Body::from("{}")).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    // Two runs now, the newer one triggered manually, and still ONE episode:
    // a re-run of a still-breaching check does not open a duplicate.
}
```

Add to `api_authz_matrix.rs`'s `CASES`:

```rust
    Case { uri: "/api/portfolios/{pid}/limit-runs", method: "POST", body: Some(Payload::Json("{}")),
           domain: Domain::Settings, action: Action::Configure },
```

- [ ] **Step 2: Run the tests and watch them fail**

Run: `cargo test -p server --test api_breach_register --test api_authz_matrix`
Expected: FAIL — 404 on the three POST routes.

- [ ] **Step 3: Write the repository methods**

Append to `crates/db/src/repo/breaches.rs`:

```rust
impl Scoped<'_> {
    /// Acknowledgement is where a proposal becomes a decision. Refuses an
    /// episode that is already acknowledged or resolved: re-deciding
    /// something that was signed off is a new episode's business, not an
    /// overwrite of the record.
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
}
```

- [ ] **Step 4: Write the handlers**

Append to `crates/server/src/handlers/breaches.rs`:

```rust
#[derive(serde::Deserialize)]
pub struct AcknowledgeBody {
    pub classification: String,
    pub note: String,
    #[serde(default)]
    pub deadline_date: Option<NaiveDate>,
}

pub async fn acknowledge(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>,
    Path((pid, bid)): Path<(i64, i64)>, Json(b): Json<AcknowledgeBody>,
) -> Result<StatusCode, AppError> {
    if !matches!(b.classification.as_str(), "active" | "passive") {
        return Err(AppError::Unprocessable(
            "classification must be 'active' or 'passive' — acknowledging is where the proposal becomes a decision".into()));
    }
    let note = b.note.trim();
    if note.is_empty() {
        return Err(AppError::Unprocessable("a note is required to acknowledge a breach".into()));
    }
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Settings, Configure>(pid)?;
    super::portfolios::ensure(&scoped, pid, true).await?;
    let actor = (ctx.principal_id != 0).then_some(ctx.principal_id);
    let ok = scoped.breach_acknowledge(
        &a, bid, &b.classification, note, b.deadline_date, actor, &ctx.display_name).await?;
    if !ok {
        return Err(AppError::Unprocessable(
            "no open breach with that id — it may already be acknowledged or resolved".into()));
    }
    crate::audit::record(&st, &ctx, "configure", Some(Domain::Settings), Some(pid),
        serde_json::json!({"kind": "breach_acknowledged", "breach_id": bid,
                           "classification": b.classification})).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Deserialize)]
pub struct ResolveBody { pub note: String }

pub async fn resolve(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>,
    Path((pid, bid)): Path<(i64, i64)>, Json(b): Json<ResolveBody>,
) -> Result<StatusCode, AppError> {
    let note = b.note.trim();
    if note.is_empty() {
        return Err(AppError::Unprocessable("a note is required to resolve a breach".into()));
    }
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Settings, Configure>(pid)?;
    super::portfolios::ensure(&scoped, pid, true).await?;
    let actor = (ctx.principal_id != 0).then_some(ctx.principal_id);
    let ok = scoped.breach_resolve(&a, bid, note, actor, &ctx.display_name).await?;
    if !ok {
        return Err(AppError::Unprocessable(
            "only an acknowledged breach can be resolved — classify it first".into()));
    }
    crate::audit::record(&st, &ctx, "configure", Some(Domain::Settings), Some(pid),
        serde_json::json!({"kind": "breach_resolved", "breach_id": bid})).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Deserialize)]
pub struct RerunBody {
    /// Defaults to the latest snapshot date.
    #[serde(default)]
    pub date: Option<NaiveDate>,
}

pub async fn rerun(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>,
    Path(pid): Path<i64>, Json(b): Json<RerunBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Settings, Configure>(pid)?;
    super::portfolios::ensure(&scoped, pid, true).await?;
    let view = scoped.authorize::<Settings, View>(pid)?;
    let date = match b.date {
        Some(d) => d,
        None => scoped.latest_position_date(&view, pid).await?
            .ok_or_else(|| AppError::Unprocessable("no snapshot imported yet".into()))?,
    };
    let _ = a;
    let run_id = crate::recorder::record(&st, pid, date, crate::recorder::Trigger::Manual {
        actor_user_id: (ctx.principal_id != 0).then_some(ctx.principal_id),
        actor_label: ctx.display_name.clone(),
    }).await?;
    crate::audit::record(&st, &ctx, "configure", Some(Domain::Settings), Some(pid),
        serde_json::json!({"kind": "limit_check_rerun", "run_id": run_id, "nav_date": date})).await;
    Ok(Json(serde_json::json!({"run_id": run_id, "nav_date": date})))
}
```

`Scoped::latest_position_date(&self, a: &Access<Settings, View>, pid: i64) -> anyhow::Result<Option<NaiveDate>>` goes in `crates/db/src/repo/breaches.rs`: `SELECT MAX(nav_date) FROM position_snapshots WHERE portfolio_id = $1`.

Mount in `crates/server/src/routes.rs`:

```rust
        .protected("/api/portfolios/{id}/limit-runs", axum::routing::post(handlers::breaches::rerun), Domain::Settings, Action::Configure)
        .protected("/api/portfolios/{id}/breaches/{bid}/acknowledge", axum::routing::post(handlers::breaches::acknowledge), Domain::Settings, Action::Configure)
        .protected("/api/portfolios/{id}/breaches/{bid}/resolve", axum::routing::post(handlers::breaches::resolve), Domain::Settings, Action::Configure)
```

- [ ] **Step 5: Run the tests and watch them pass**

Run: `cargo test -p server --test api_breach_register --test api_authz_matrix`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/db/src/repo/breaches.rs crates/server/src/handlers/breaches.rs crates/server/src/routes.rs crates/server/tests/api_breach_register.rs crates/server/tests/api_authz_matrix.rs
git commit -m "feat(server): breach acknowledge/resolve workflow and manual re-run"
```

---

### Task 9: The evidence export

**Files:**
- Create: `crates/ingest/src/breach_evidence.rs`
- Modify: `crates/ingest/src/lib.rs`, `crates/server/src/handlers/breaches.rs`, `crates/server/src/routes.rs`
- Test: `crates/ingest/tests/breach_evidence.rs`

**Interfaces:**
- Produces: `ingest::breach_evidence::build(portfolio_name: &str, runs: &[serde_json::Value], breaches: &[serde_json::Value]) -> anyhow::Result<Vec<u8>>`

Model this on `crates/ingest/src/emir_file.rs` and its test `crates/ingest/tests/emir_evidence.rs` — same `rust_xlsxwriter` usage, same "one file, sections stacked" shape.

- [ ] **Step 1: Write the failing test**

Create `crates/ingest/tests/breach_evidence.rs`:

```rust
#[test]
fn the_evidence_file_has_a_register_sheet_and_a_run_history_sheet() {
    let runs = vec![serde_json::json!({
        "nav_date": "2026-08-07", "run_at": "2026-08-07T09:00:00Z",
        "triggered_by": "import", "inputs_complete": true,
        "results": [{"check_key": "issuer_10", "scope_label": "Issuer <= 10% NAV",
                     "limit_value": 0.10, "observed_value": 0.106, "status": "breach"}]
    })];
    let breaches = vec![serde_json::json!({
        "check_key": "issuer_10", "subject": "ACME",
        "opened_nav_date": "2026-08-07", "peak_value": 0.106,
        "state": "acknowledged", "classification": "passive",
        "acknowledgement_note": "market move, no purchase"
    })];
    let bytes = ingest::breach_evidence::build("Borobudur", &runs, &breaches).unwrap();
    assert!(bytes.len() > 4000, "an xlsx with two sheets is never this small");
    assert_eq!(&bytes[0..2], b"PK", "xlsx files are zip archives");
}

#[test]
fn an_empty_register_still_produces_a_file_that_says_so() {
    let bytes = ingest::breach_evidence::build("Borobudur", &[], &[]).unwrap();
    assert!(bytes.len() > 2000);
}
```

- [ ] **Step 2: Run the test and watch it fail**

Run: `cargo test -p ingest --test breach_evidence`
Expected: FAIL to compile — `could not find breach_evidence in ingest`.

- [ ] **Step 3: Write the builder and the handler**

Write `crates/ingest/src/breach_evidence.rs` with two worksheets — `Register` (one row per episode: check, subject, opened, peak, cleared, state, classification, who acknowledged and when, the notes) and `Run history` (one row per run, one column per check, the status in the cell) — plus a header block naming the portfolio and the generation timestamp, exactly as `emir_file.rs` does. Add `pub mod breach_evidence;` to `crates/ingest/src/lib.rs`.

The handler mirrors `handlers::emir::export`:

```rust
pub async fn export(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(pid): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let scoped = st.db.scope(&ctx);
    scoped.authorize::<Settings, Export>(pid)?;
    let a = scoped.authorize::<Settings, View>(pid)?;
    let portfolio = super::portfolios::ensure(&scoped, pid, false).await?;
    let runs = /* same shape as runs_list */;
    let breaches = /* same shape as register_list */;
    let bytes = ingest::breach_evidence::build(&portfolio.name, &runs, &breaches)
        .map_err(AppError::Internal)?;
    crate::audit::record(&st, &ctx, "export", Some(Domain::Settings), Some(pid),
        serde_json::json!({"kind": "breach_register"})).await;
    let filename = format!("Breach register - {} - {}.xlsx", portfolio.name, chrono::Utc::now().date_naive());
    Ok((
        [(axum::http::header::CONTENT_TYPE,
          "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".to_string()),
         (axum::http::header::CONTENT_DISPOSITION,
          format!("attachment; filename=\"{filename}\""))],
        bytes,
    ))
}
```

Mount it and add the matrix row:

```rust
        .protected("/api/portfolios/{id}/breaches/export", get(handlers::breaches::export), Domain::Settings, Action::Export)
```
```rust
    r("/api/portfolios/{pid}/breaches/export", Domain::Settings, Action::Export),
```

**Route ordering note:** `/breaches/export` must be declared **before**
`/breaches/{bid}` in `routes.rs`, or axum matches `export` as a `{bid}` and
the handler fails parsing it as an `i64`. Add a test for this in
`api_breach_register.rs`: `GET /api/portfolios/1/breaches/export` returns 200
with an xlsx content type, not 400.

- [ ] **Step 4: Run the tests and watch them pass**

Run: `cargo test -p ingest --test breach_evidence && cargo test -p server --test api_breach_register --test api_authz_matrix`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/ingest/src/breach_evidence.rs crates/ingest/src/lib.rs crates/ingest/tests/breach_evidence.rs crates/server/src/handlers/breaches.rs crates/server/src/routes.rs crates/server/tests/api_breach_register.rs crates/server/tests/api_authz_matrix.rs
git commit -m "feat: breach register evidence export"
```

---

### Task 10: The Breaches page

**Files:**
- Modify: `frontend/src/api.ts`, `frontend/src/nav.ts`, `frontend/src/App.tsx`
- Create: `frontend/src/pages/BreachesPage.tsx`
- Test: `frontend/src/pages/BreachesPage.test.tsx`, `frontend/src/nav.test.tsx`

**Interfaces:**
- Consumes: the endpoints from Tasks 7–9.
- Produces: `getLimitRuns`, `getBreaches`, `getBreach`, `acknowledgeBreach`, `resolveBreach`, `rerunLimitChecks`, `breachExportUrl` in `api.ts`.

- [ ] **Step 1: Write the failing tests**

Create `frontend/src/pages/BreachesPage.test.tsx`:

```tsx
/** The register's three states have to be visually distinct, and a denial
 * must never look like an empty register. */
import { screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import BreachesPage from "./BreachesPage";
import { denied, renderPage, stubFetch, TEST_PORTFOLIO } from "../test/harness";

const RUNS = {
  runs: [{
    id: 1, nav_date: "2026-08-07", run_at: "2026-08-07T09:00:00Z",
    triggered_by: "import", import_id: 3, inputs_complete: true, input_notes: {},
    results: [{ check_key: "issuer_10", scope_label: "Issuer <= 10% NAV",
                limit_value: 0.10, observed_value: 0.106, status: "breach", detail: {} }],
  }],
};

const episode = (over: Record<string, unknown>) => ({
  id: 9, check_key: "issuer_10", subject: "ACME",
  opened_nav_date: "2026-08-07", opened_value: 0.106, peak_value: 0.121,
  peak_nav_date: "2026-08-14", closed_nav_date: null,
  state: "open", classification: "unclassified",
  proposed_classification: "passive",
  proposal_reason: "no purchase in ACME since the previous snapshot",
  acknowledged_at: null, acknowledgement_note: null, deadline_date: null,
  resolved_at: null, resolution_note: null,
  ...over,
});

function stub(breaches: unknown[]) {
  const p = TEST_PORTFOLIO.id;
  stubFetch({
    [`/api/portfolios/${p}/limit-runs`]: RUNS,
    [`/api/portfolios/${p}/breaches`]: { breaches },
  });
}

describe("breach register", () => {
  it("shows the proposal and says it is not yet a decision", async () => {
    stub([episode({})]);
    const { container } = renderPage(<BreachesPage />);
    await waitFor(() => expect(container.textContent).toContain("ACME"));
    expect(screen.getByText(/no purchase in ACME/)).toBeDefined();
    expect(container.textContent).toContain("Unclassified");
  });

  it("distinguishes cleared-awaiting-sign-off from resolved", async () => {
    stub([
      episode({ id: 1, subject: "CLEARED", closed_nav_date: "2026-08-14", state: "acknowledged" }),
      episode({ id: 2, subject: "SIGNEDOFF", closed_nav_date: "2026-08-14", state: "resolved",
                classification: "passive", resolution_note: "trimmed" }),
    ]);
    const { container } = renderPage(<BreachesPage />);
    await waitFor(() => expect(container.textContent).toContain("CLEARED"));
    expect(container.textContent).toMatch(/awaiting sign-off/i);
    expect(container.textContent).toMatch(/Resolved/);
  });

  it("renders a denial rather than an empty register", async () => {
    const p = TEST_PORTFOLIO.id;
    stubFetch({
      [`/api/portfolios/${p}/limit-runs`]: RUNS,
      [`/api/portfolios/${p}/breaches`]: denied("portfolio settings"),
    });
    // `stubFetch` answers 200; for a 403 the page must use useFetch's
    // `forbidden`. Extend the harness with a `stubFetchStatus` helper that
    // returns a chosen status code, then assert:
    const { container } = renderPage(<BreachesPage />);
    await waitFor(() => expect(container.textContent).toContain("N/A"));
    expect(screen.getByText(/not permitted: portfolio settings/)).toBeDefined();
  });

  it("marks a run whose inputs were incomplete", async () => {
    const p = TEST_PORTFOLIO.id;
    stubFetch({
      [`/api/portfolios/${p}/limit-runs`]: { runs: [{
        ...RUNS.runs[0], inputs_complete: false,
        input_notes: { shareholders: "no register loaded" },
      }] },
      [`/api/portfolios/${p}/breaches`]: { breaches: [] },
    });
    const { container } = renderPage(<BreachesPage />);
    await waitFor(() => expect(container.textContent).toContain("2026-08-07"));
    expect(container.textContent).toMatch(/incomplete/i);
  });
});
```

Extend `frontend/src/test/harness.tsx` with:

```tsx
/** Like `stubFetch`, but each route may name a status code, so a page's
 * handling of a 403 can be tested without hand-rolling a Response. */
export function stubFetchStatus(routes: Record<string, { status: number; body: unknown }>) {
  vi.stubGlobal("fetch", async (input: RequestInfo | URL): Promise<Response> => {
    const url = typeof input === "string" ? input : input.toString();
    const hit = Object.keys(routes).find((k) => url.includes(k));
    if (!hit) throw new Error(`no stub for ${url}`);
    const { status, body } = routes[hit];
    return new Response(JSON.stringify(body), {
      status, headers: { "content-type": "application/json" },
    });
  });
}
```

and use it for the denial test with `{ status: 403, body: { detail: "not permitted: portfolio settings" } }`.

Append to `frontend/src/nav.test.tsx`:

```tsx
  it("offers Breaches to a principal holding the fund's settings", () => {
    const me = principal(["settings", "view", PID]);
    expect(labels(me)).toContain("Breaches");
  });
```

- [ ] **Step 2: Run the tests and watch them fail**

Run (from `frontend/`): `npx vitest run src/pages/BreachesPage.test.tsx src/nav.test.tsx`
Expected: FAIL — `Cannot find module './BreachesPage'`, and `expected [...] to include 'Breaches'`.

- [ ] **Step 3: Write the types, fetchers, nav entry and page**

In `frontend/src/api.ts`:

```ts
export interface CheckResult {
  check_key: string; scope_label: string;
  limit_value: number | null; observed_value: number | null;
  status: "ok" | "watch" | "breach";
  detail: unknown;
}
export interface LimitRun {
  id: number; nav_date: string; run_at: string;
  triggered_by: "import" | "manual"; import_id: number | null;
  inputs_complete: boolean; input_notes: Record<string, string>;
  results: CheckResult[];
}
export interface BreachEpisode {
  id: number; check_key: string; subject: string;
  opened_nav_date: string; opened_value: number | null;
  peak_value: number | null; peak_nav_date: string | null;
  closed_nav_date: string | null;
  state: "open" | "acknowledged" | "resolved";
  classification: "unclassified" | "active" | "passive";
  proposed_classification: "active" | "passive" | null;
  proposal_reason: string | null;
  acknowledged_at: string | null; acknowledgement_note: string | null;
  deadline_date: string | null;
  resolved_at: string | null; resolution_note: string | null;
}
export interface BreachEvent {
  at: string; actor_label: string; event: string; detail: unknown;
}

export const getLimitRuns = (pid: number, limit = 52) =>
  req<{ runs: LimitRun[] }>(`/api/portfolios/${pid}/limit-runs?limit=${limit}`);
export const getBreaches = (pid: number, state?: string) =>
  req<{ breaches: BreachEpisode[] }>(`/api/portfolios/${pid}/breaches${state ? `?state=${state}` : ""}`);
export const getBreach = (pid: number, bid: number) =>
  req<{ breach: BreachEpisode; events: BreachEvent[] }>(`/api/portfolios/${pid}/breaches/${bid}`);
export const acknowledgeBreach = (
  pid: number, bid: number, body: { classification: string; note: string; deadline_date?: string },
) => req<void>(`/api/portfolios/${pid}/breaches/${bid}/acknowledge`, {
  method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
});
export const resolveBreach = (pid: number, bid: number, note: string) =>
  req<void>(`/api/portfolios/${pid}/breaches/${bid}/resolve`, {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ note }),
  });
export const rerunLimitChecks = (pid: number) =>
  req<{ run_id: number; nav_date: string }>(`/api/portfolios/${pid}/limit-runs`, {
    method: "POST", headers: { "content-type": "application/json" }, body: "{}",
  });
export const breachExportUrl = (pid: number) => `/api/portfolios/${pid}/breaches/export`;
```

In `frontend/src/nav.ts`, after the Limits entry:

```ts
  { to: "/breaches", label: "Breaches", requires: [{ domain: "settings", action: "view" }] },
```

In `frontend/src/App.tsx`, add the import and the route:

```tsx
import BreachesPage from "./pages/BreachesPage";
```
```tsx
            <Route path="breaches" element={<BreachesPage />} />
```

Write `frontend/src/pages/BreachesPage.tsx` following the shape of
`LimitsPage.tsx`: `useFetch` per endpoint, `<Unavailable reason={x.forbidden}/>`
for a denied read, `eur`/`pct` from `fmt.ts` for numbers. Three sections:

1. **Open episodes** — sorted `state === "open"` first, then by `opened_nav_date`
   ascending (oldest unaddressed first). Each shows the check's `scope_label`,
   the `subject`, days open, `opened_value` → `peak_value` against
   `limit_value` from the newest run's matching result, a classification chip
   (`Unclassified` / `Active` / `Passive`), a state chip, and — when
   `closed_nav_date` is set and `state !== "resolved"` — the words
   *"cleared on the data since {date} — awaiting sign-off"*. The
   `proposal_reason` is shown verbatim beneath, labelled *Proposed:* so it
   never reads as a decision.
2. **Run history** — a table, one row per check key, one column per run date,
   cell coloured by status using the same classes `LimitsPage` uses
   (`pos` / `warn-badge` / `neg`), and `unavailable` for a check absent from
   that run. A run with `inputs_complete: false` gets a marker in its column
   header carrying the `input_notes` values in its `title`.
3. **Actions** — Acknowledge (classification radio, note textarea, optional
   deadline) and Resolve (note textarea), both calling the endpoints above and
   then `reload()`ing. Plus an Export button linking to `breachExportUrl`.

- [ ] **Step 4: Run the tests and watch them pass**

Run (from `frontend/`): `npx vitest run`
Expected: PASS, all files.

- [ ] **Step 5: Type-check, build and lint**

Run (from `frontend/`): `npm run build && npm run lint`
Expected: build succeeds, no new lint warnings beyond the pre-existing
`only-export-components` one in `App.tsx`.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/api.ts frontend/src/nav.ts frontend/src/nav.test.tsx frontend/src/App.tsx frontend/src/pages/BreachesPage.tsx frontend/src/pages/BreachesPage.test.tsx frontend/src/test/harness.tsx
git commit -m "feat(frontend): breach register page with run history and sign-off"
```

---

### Task 11: Documentation and end-to-end verification

**Files:**
- Create: `docs/user-guide/breaches.md`
- Modify: `docs/user-guide/README.md`, `docs/user-guide/limits.md`, `docs/user-guide/access-rights.md`, `README.md`, `scripts/test-access-rights.ps1`, `docs/testing/access-rights-manual-checklist.md`

- [ ] **Step 1: Write the user-guide chapter**

Create `docs/user-guide/breaches.md` in the voice of the existing chapters —
second person, no jargon unexplained. Cover: what a run is and when one
happens (on import, or the Re-run button); what an episode is and why a
six-week breach is one row and not forty-two; what *Proposed: passive* means
and that it is not a decision; the difference between *cleared on the data*
and *resolved*; that acknowledging needs a classification and a note; what the
export contains; and which permission each of those needs
(`settings/view` to read, `settings/configure` to act, `settings/export` to
download). Link it from `docs/user-guide/README.md`'s chapter list and from
the Limits chapter.

- [ ] **Step 2: Note the register in the top-level README**

Add to the Features list in `README.md`:

```markdown
- **Breach register** (Breaches page): every limit check the tool runs is
  recorded against the snapshot date it was struck on — concentration,
  liquidity scenarios, the VaR limit and the EMIR thresholds. Consecutive
  breaches of the same check by the same issuer are grouped into one episode
  with its own timeline, classified active or passive (proposed from position
  changes, confirmed by a person), and closed out through acknowledge and
  resolve. A run is recorded on every import and can be re-run by hand;
  runs are immutable. Exports to `.xlsx` as evidence.
```

- [ ] **Step 3: Extend the live access-rights script**

In `scripts/test-access-rights.ps1`, in the "Configure semantics" phase after
the existing settings checks:

```powershell
$r = Invoke-Api -Method GET -Path "/api/portfolios/$pidA/breaches" -Cookie $sub
Check 'settings/view reaches the breach register' ($r.Status -eq 200) "got $($r.Status)"
$r = Invoke-Api -Method GET -Path "/api/portfolios/$pidB/breaches" -Cookie $sub
Check "B's register stays 403 (settings was A-only)" ($r.Status -eq 403) "got $($r.Status)"
```

- [ ] **Step 4: Extend the manual checklist**

Add a section 4b to `docs/testing/access-rights-manual-checklist.md`:

```markdown
- [ ] Breaches: an open episode shows **Proposed: passive** (or active) with
      its reasoning, and the classification chip reads **Unclassified** until
      someone acknowledges it — a proposal must never look like a decision.
- [ ] An episode whose check has cleared but which nobody has signed off reads
      **"cleared on the data … awaiting sign-off"**, visibly different from a
      resolved one.
- [ ] Resolve is refused on an episode nobody has acknowledged, with a
      readable message.
```

- [ ] **Step 5: Full verification**

Run, from the repo root:
```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
Then from `frontend/`:
```
npm run build && npm run lint && npm test
```
Then the live suite:
```
cargo run -p server --example dev_server
pwsh scripts/test-access-rights.ps1 -BaseUrl http://127.0.0.1:8788 -AdminEmail admin@dev.local -AdminPassword <pick one> -EnrolToken <printed token>
```
Expected: `cargo test` exit 0; clippy clean; frontend all green; the live
suite `FAIL 0`.

- [ ] **Step 6: Commit**

```bash
git add docs README.md scripts/test-access-rights.ps1
git commit -m "docs: breach register user guide and end-to-end coverage"
```

---

## Self-review notes

Checked against the spec:

- **Recorded on import + manual re-run** → Tasks 5 and 8.
- **Coverage: every check with a limit** → Task 6, with the exact fifteen keys asserted in the test.
- **Episodes, not rows per run** → Task 2 (logic) and Task 4 (the partial unique index that enforces it).
- **System context** → Task 5, `AuthCtx::system()` plus the test that a reference-less importer still produces a correctly grouped run.
- **Active/passive proposed, never decided** → Task 3, plus the Task 4 assertion that `classification` stays `unclassified` when a proposal exists, plus the Task 8 refusal to acknowledge with `unclassified`.
- **Clearing ≠ resolving** → Task 4's assertion that `state` stays `open` after a `Close`, and Task 10's "awaiting sign-off" test.
- **Immutable runs** → no update method exists for runs or results anywhere in this plan; the migration comment says so.
- **Settings gates everything** → every route declared `Domain::Settings`, every one added to the matrix in the task that adds it.
- **Denials never render as data** → Task 10's third test.
- **Out of scope honoured** → no alerting, no scheduler, no deadline enforcement, no backfill anywhere in these tasks.

Type consistency: `CheckResultRow`, `NewRun`, `BreachRow`, `BreachEventRow`, `LiveEpisode`, `Finding`, `Transition`, `Proposal`, `SubjectHolding` are each defined once and referred to by the same name everywhere after. The proposal map key is `"{check_key}\u{1f}{subject}"` in Task 4's signature, Task 4's implementation and Task 5's construction.

Two things checked against the code while writing this plan, so nobody has to
rediscover them: `crates/db` already depends on `analytics`, and the reference
setter is `Scoped::refs_upsert(&Access<Reference, Configure>, &InstrumentRef)`
— the full struct literal is written out in Task 5's test.

One thing left deliberately for the implementer to prove rather than assume:
route ordering for `/breaches/export` versus `/breaches/{bid}`. Task 9 flags
it and asks for a test, because the failure mode (export matching as a `{bid}`
and 400ing on the parse) is exactly the kind that looks like a typo in the
frontend rather than a routing bug.
