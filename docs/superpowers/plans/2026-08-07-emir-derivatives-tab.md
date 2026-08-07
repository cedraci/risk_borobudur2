# Derivatives / EMIR Tab Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A new Derivatives tab that monitors the EMIR clearing thresholds (12-month average of month-end gross notional per asset class, OTC-only fed to the thresholds), derives the OTC reconciliation/compression obligations, shows margin balances, records monthly KPIs, and exports the calculation as an `.xlsx` evidence file.

**Architecture:** Pure analytics in a new `analytics::emir` module (mirrors `analytics::futures`); one migration pair (`otc` flag on `futures_contracts`, new `emir_kpis` table); three server endpoints (`GET /api/emir`, `PUT /api/emir/kpis/{month}`, `GET /api/emir/export`); the evidence workbook built in `ingest` (which owns `rust_xlsxwriter`); a new React page that also absorbs the `DerivativesExposure` component from the Limits page.

**Tech Stack:** Rust (axum 0.8, sqlx, chrono, rust_xlsxwriter 0.97, calamine 0.26), React + TypeScript (no new deps), embedded PostgreSQL 17.

Spec: `docs/superpowers/specs/2026-08-07-emir-derivatives-tab-design.md`

## Global Constraints

- Work on branch `feat/emir-derivatives` off `main` (create it in Task 1, Step 1).
- Windows / PowerShell. The Bash tool is also available; each has its own syntax.
- Embedded-PG tests spin up PostgreSQL under `%LOCALAPPDATA%\borobudur-risk`. If a db/server test fails to START PG (stale postmaster from a killed run), stop it cleanly with:
  `& "$env:LOCALAPPDATA\borobudur-risk\pg-install\17.10.0\bin\pg_ctl.exe" -D "$env:LOCALAPPDATA\borobudur-risk\pg-data" -m fast stop`
- Migrations are embedded at compile time by `sqlx::migrate!("./migrations")` in `crates/db/src/lib.rs` — a new `.sql` file needs **no registration**, but if the compiler seems not to see it, run `cargo clean -p db` first.
- axum 0.8 path-parameter syntax is `{name}` (braces), not `:name`.
- Never edit migrations `0001`–`0005`, and never edit `0006`/`0007` after the task that creates them is committed (sqlx checksums applied migrations).
- Frontend: no new npm dependencies; use only existing `index.css` classes (`.card`, `.tbl`, `.controls`, `.warn-badge`, `.pos`, `.neg`, `.kpi-sub`, `.drop`). There is no `.badge`/`.btn`/`.panel` class. UI copy is English (house convention).
- Error responses use the existing `AppError` variants; body keys are `title`/`status`/`detail` (or `rows`).
- Every commit message ends with the trailer:
  `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`
- Test counts given in steps are indicative; trust the actual green run.
- Regulatory constants (copy verbatim): thresholds EUR 1e9 credit, 1e9 equity, 3e9 interest-rate, 3e9 FX, 4e9 commodity-and-other; WATCH at ≥ 80% of threshold, BREACH at ≥ 100%; reconciliation tiers 0 → not triggered, 1–50 → quarterly, 51–499 → weekly, ≥ 500 → daily; compression analysis required at ≥ 500 OTC contracts.

---

### Task 1: `otc` flag on futures contracts, end-to-end on the backend

**Files:**
- Create: `crates/db/migrations/0006_futures_otc.sql`
- Modify: `crates/db/src/repo.rs` (FuturesContract struct, SELECT_CONTRACTS, contracts_upsert)
- Modify: `crates/server/src/handlers/futures.rs` (ContractBody + put_contract)
- Modify: `crates/server/tests/api_futures.rs` (existing PUT payloads + new assertion)
- Possibly modify: any other `FuturesContract` construction site the compiler flags (seeding code in `repo.rs`, db tests) — set `otc: false` there.

**Interfaces:**
- Consumes: existing `db::repo::FuturesContract`, `contracts_all`, `contracts_upsert`; `PUT /api/futures-contracts/{root}` handler.
- Produces: `FuturesContract` gains `pub otc: bool`; `ContractBody` gains `pub otc: bool` (required field — a PUT without it is a 422, which is intentional: the upsert is a full-row replace and a silent default would erase the flag). Later tasks read `spec.otc`.

- [ ] **Step 1: Create the branch**

```powershell
git checkout -b feat/emir-derivatives
```

- [ ] **Step 2: Write the failing test**

In `crates/server/tests/api_futures.rs`, add to the existing test (or a new `#[tokio::test] async fn otc_flag_round_trips()` using the same inline setup as `contracts_and_ctd_upload`): PUT a contract with `"otc": true` and assert the response and a subsequent GET carry it.

```rust
    // OTC flag round-trips through PUT and GET (EMIR threshold feed).
    let (status, body) = put_json(&app, "/api/futures-contracts/RX", serde_json::json!({
        "label": "Euro-Bund", "category": "interest_rate", "point_value": 1000.0,
        "currency": "EUR", "curve": null, "price_convention": "decimal",
        "confirmed": true, "otc": true,
    })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["otc"], true, "{body}");
    let (status, list) = get_json(&app, "/api/futures-contracts").await;
    assert_eq!(status, StatusCode::OK);
    let rx = list.as_array().unwrap().iter().find(|c| c["contract_root"] == "RX").unwrap();
    assert_eq!(rx["otc"], true, "{rx}");
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo test -p server --test api_futures`
Expected: FAIL — the PUT ignores the unknown `otc` field and the response has no `otc` key (`body["otc"]` is `null`).

- [ ] **Step 4: Migration**

Create `crates/db/migrations/0006_futures_otc.sql`:

```sql
-- EMIR clearing thresholds count OTC positions only. A contract executed on
-- an EU regulated market or an equivalent third-country market is not OTC;
-- one on a non-equivalent venue is, even if exchange-listed. Default false:
-- every contract currently on record is listed on an equivalent venue.
ALTER TABLE futures_contracts
  ADD COLUMN otc BOOLEAN NOT NULL DEFAULT false;
```

- [ ] **Step 5: Plumb the field through db and server**

In `crates/db/src/repo.rs`:
- Add `pub otc: bool,` to `struct FuturesContract` (after `confirmed`).
- Add `, otc` to the column list in `SELECT_CONTRACTS`.
- In `contracts_upsert`: add `otc` to the INSERT column list, a `$9`-style bind for `c.otc`, and `otc = EXCLUDED.otc` to the `DO UPDATE SET` list.

In `crates/server/src/handlers/futures.rs`:
- Add `pub otc: bool,` to `ContractBody`.
- In `put_contract`, add `otc: b.otc,` to the `db::repo::FuturesContract` construction.

Then `cargo build --workspace` and fix every other `FuturesContract { .. }` construction site the compiler flags with `otc: false` (expected: the import-seeding path in `repo.rs` if it constructs the struct, and db tests). Update the pre-existing PUT payloads in `api_futures.rs` (and any other server test that PUTs a contract — grep `futures-contracts` under `crates/server/tests/`) to include `"otc": false`.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p db -p server`
Expected: PASS, including the new round-trip assertion.

- [ ] **Step 7: Commit**

```bash
git add crates/db/migrations/0006_futures_otc.sql crates/db/src/repo.rs crates/server/src/handlers/futures.rs crates/server/tests/
git commit -m "feat(db+server): OTC flag on futures contracts for EMIR thresholds"
```

---

### Task 2: `emir_kpis` table and repo functions

**Files:**
- Create: `crates/db/migrations/0007_emir_kpis.sql`
- Modify: `crates/db/src/repo.rs` (EmirKpi struct + two functions, at the end of the file)
- Create: `crates/db/tests/emir_kpis.rs`

**Interfaces:**
- Produces:
  - `pub struct EmirKpi { pub month: NaiveDate, pub unconfirmed_over_5d: i32, pub reconciliation: String, pub disputes: i32, pub note: Option<String> }` (derives `Debug, Clone, sqlx::FromRow, serde::Serialize`)
  - `pub async fn emir_kpis_all(pool: &PgPool) -> anyhow::Result<Vec<EmirKpi>>` — ordered `month DESC`
  - `pub async fn emir_kpi_upsert(pool: &PgPool, k: &EmirKpi) -> anyhow::Result<()>`

- [ ] **Step 1: Migration**

Create `crates/db/migrations/0007_emir_kpis.sql`:

```sql
-- Monthly EMIR KPIs for the risk committee: confirmation follow-up,
-- reconciliation status and dispute count are middle-office facts the tool
-- cannot derive, so they are entered by hand, one row per calendar month.
CREATE TABLE emir_kpis (
  month               DATE PRIMARY KEY
                      CHECK (month = date_trunc('month', month)::date),
  unconfirmed_over_5d INT NOT NULL CHECK (unconfirmed_over_5d >= 0),
  reconciliation      TEXT NOT NULL
                      CHECK (reconciliation IN ('done','not_done','not_applicable')),
  disputes            INT NOT NULL CHECK (disputes >= 0),
  note                TEXT,
  updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

- [ ] **Step 2: Write the failing test**

Create `crates/db/tests/emir_kpis.rs` (same embedded-db setup as the existing db tests, e.g. `futures_seeding.rs`):

```rust
use chrono::NaiveDate;

#[tokio::test]
async fn kpi_upsert_round_trip_and_constraints() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    let d = |s: &str| s.parse::<NaiveDate>().unwrap();
    let k = db::repo::EmirKpi {
        month: d("2026-07-01"),
        unconfirmed_over_5d: 2,
        reconciliation: "done".into(),
        disputes: 0,
        note: Some("one late FX forward confirmation".into()),
    };
    db::repo::emir_kpi_upsert(&pool, &k).await.unwrap();

    // Upsert on the same month replaces, not duplicates.
    let k2 = db::repo::EmirKpi { disputes: 1, note: None, ..k.clone() };
    db::repo::emir_kpi_upsert(&pool, &k2).await.unwrap();

    let earlier = db::repo::EmirKpi { month: d("2026-06-01"), ..k2.clone() };
    db::repo::emir_kpi_upsert(&pool, &earlier).await.unwrap();

    let all = db::repo::emir_kpis_all(&pool).await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].month, d("2026-07-01")); // DESC order
    assert_eq!(all[0].disputes, 1);
    assert_eq!(all[0].note, None);

    // Mid-month date violates the first-of-month CHECK.
    let bad = db::repo::EmirKpi { month: d("2026-07-15"), ..k2.clone() };
    assert!(db::repo::emir_kpi_upsert(&pool, &bad).await.is_err());
    // Unknown reconciliation value violates its CHECK.
    let bad = db::repo::EmirKpi { reconciliation: "maybe".into(), ..k2.clone() };
    assert!(db::repo::emir_kpi_upsert(&pool, &bad).await.is_err());

    pool.close().await;
    edb.stop().await;
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo test -p db --test emir_kpis`
Expected: FAIL to compile — `EmirKpi`, `emir_kpi_upsert`, `emir_kpis_all` not defined.

- [ ] **Step 4: Implement the repo functions**

Append to `crates/db/src/repo.rs`:

```rust
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

pub async fn emir_kpis_all(pool: &PgPool) -> anyhow::Result<Vec<EmirKpi>> {
    Ok(sqlx::query_as::<_, EmirKpi>(
        "SELECT month, unconfirmed_over_5d, reconciliation, disputes, note
         FROM emir_kpis ORDER BY month DESC",
    )
    .fetch_all(pool)
    .await?)
}

/// Full-row replace, like `contracts_upsert`: every field is written as given.
pub async fn emir_kpi_upsert(pool: &PgPool, k: &EmirKpi) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO emir_kpis (month, unconfirmed_over_5d, reconciliation, disputes, note)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (month) DO UPDATE SET
           unconfirmed_over_5d = EXCLUDED.unconfirmed_over_5d,
           reconciliation = EXCLUDED.reconciliation,
           disputes = EXCLUDED.disputes,
           note = EXCLUDED.note,
           updated_at = now()",
    )
    .bind(k.month)
    .bind(k.unconfirmed_over_5d)
    .bind(&k.reconciliation)
    .bind(k.disputes)
    .bind(&k.note)
    .execute(pool)
    .await?;
    Ok(())
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p db`
Expected: PASS (all db tests, including the new one).

- [ ] **Step 6: Commit**

```bash
git add crates/db/migrations/0007_emir_kpis.sql crates/db/src/repo.rs crates/db/tests/emir_kpis.rs
git commit -m "feat(db): emir_kpis monthly table with first-of-month and enum checks"
```

---

### Task 3: `analytics::emir` — month window, thresholds, monitors

**Files:**
- Create: `crates/analytics/src/emir.rs`
- Modify: `crates/analytics/src/lib.rs` (add `pub mod emir;` alongside the existing module list)

**Interfaces:**
- Consumes: `crate::futures::Category` (the six-variant enum defined in `futures.rs`).
- Produces (all `pub`, in `analytics::emir`):
  - `fn month_window(anchor: NaiveDate, available: &[NaiveDate]) -> Vec<(NaiveDate, Option<NaiveDate>)>` — 12 `(first_of_month, chosen_snapshot_date)` pairs, oldest first
  - `struct EmirPosition { ticker: String, category: Category, notional_eur: Option<f64>, otc: bool, unconfirmed: bool }`
  - `struct MonthSnapshot { month: NaiveDate, snapshot: Option<(NaiveDate, Vec<EmirPosition>)> }`
  - `enum ThresholdClass { Credit, Equity, InterestRate, Fx, CommodityOther }` with `ALL`, `of(Category)`, `threshold_eur()`, `label()`
  - `enum Verdict { Ok, Watch, Breach }` with `as_str()`
  - `struct MonthCell { month, snapshot_date: Option<NaiveDate>, total_eur: Option<f64>, otc_eur: Option<f64> }`
  - `struct ClassReport { class, label, threshold_eur, months: Vec<MonthCell>, avg_total_eur, avg_otc_eur, pct_of_threshold, verdict }`
  - `struct ThresholdReport { classes: Vec<ClassReport>, months_present: usize, months_total: usize, warnings: Vec<String> }`
  - `fn thresholds(months: &[MonthSnapshot]) -> ThresholdReport`
  - `enum ReconciliationTier { NotTriggered, Quarterly, Weekly, Daily }`
  - `struct Monitors { otc_open_contracts: usize, reconciliation: ReconciliationTier, compression_required: bool }`
  - `fn monitors(anchor_positions: &[EmirPosition]) -> Monitors`

- [ ] **Step 1: Write the failing tests**

Create `crates/analytics/src/emir.rs` with a `#[cfg(test)] mod tests` first (the module body comes in Step 3 — write the test module now at the bottom of the new file, with `use super::*;`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::futures::Category;
    use chrono::NaiveDate;

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn pos(ticker: &str, cat: Category, notional: Option<f64>, otc: bool, unconfirmed: bool) -> EmirPosition {
        EmirPosition { ticker: ticker.into(), category: cat, notional_eur: notional, otc, unconfirmed }
    }

    #[test]
    fn window_is_twelve_months_oldest_first_within_month_capped_at_anchor() {
        let available = [
            d("2026-07-31"), // after the anchor: must never be chosen
            d("2026-07-24"),
            d("2026-07-10"),
            d("2026-06-26"),
            d("2026-04-30"),
            d("2025-08-29"),
            d("2025-07-31"), // before the window: must not leak into 2025-08
        ];
        let w = month_window(d("2026-07-24"), &available);
        assert_eq!(w.len(), 12);
        assert_eq!(w[0].0, d("2025-08-01"));
        assert_eq!(w[11].0, d("2026-07-01"));
        assert_eq!(w[0].1, Some(d("2025-08-29")));
        assert_eq!(w[1].1, None); // 2025-09: no snapshot IN that month
        assert_eq!(w[8].1, Some(d("2026-04-30")));
        assert_eq!(w[9].1, None); // 2026-05
        assert_eq!(w[10].1, Some(d("2026-06-26")));
        // Anchor month: capped at the anchor itself, so 2026-07-31 is skipped.
        assert_eq!(w[11].1, Some(d("2026-07-24")));
    }

    #[test]
    fn window_handles_year_boundary() {
        let w = month_window(d("2026-01-15"), &[d("2026-01-15")]);
        assert_eq!(w[0].0, d("2025-02-01"));
        assert_eq!(w[11].0, d("2026-01-01"));
    }

    #[test]
    fn category_mapping_and_threshold_amounts() {
        assert_eq!(ThresholdClass::of(Category::Equity), ThresholdClass::Equity);
        assert_eq!(ThresholdClass::of(Category::Credit), ThresholdClass::Credit);
        assert_eq!(ThresholdClass::of(Category::InterestRate), ThresholdClass::InterestRate);
        assert_eq!(ThresholdClass::of(Category::Fx), ThresholdClass::Fx);
        assert_eq!(ThresholdClass::of(Category::Commodity), ThresholdClass::CommodityOther);
        assert_eq!(ThresholdClass::of(Category::Other), ThresholdClass::CommodityOther);
        assert_eq!(ThresholdClass::Credit.threshold_eur(), 1e9);
        assert_eq!(ThresholdClass::Equity.threshold_eur(), 1e9);
        assert_eq!(ThresholdClass::InterestRate.threshold_eur(), 3e9);
        assert_eq!(ThresholdClass::Fx.threshold_eur(), 3e9);
        assert_eq!(ThresholdClass::CommodityOther.threshold_eur(), 4e9);
    }

    #[test]
    fn averages_divide_by_months_present_and_shorts_count_absolute() {
        // Two present months out of a 3-slot window; shorts enter the gross
        // sum in absolute value; only OTC-flagged notional feeds the OTC line.
        let months = [
            MonthSnapshot {
                month: d("2026-05-01"),
                snapshot: Some((d("2026-05-29"), vec![
                    pos("A Index", Category::Equity, Some(100.0), false, false),
                    pos("B Index", Category::Equity, Some(-40.0), true, false), // short, OTC
                ])),
            },
            MonthSnapshot { month: d("2026-06-01"), snapshot: None },
            MonthSnapshot {
                month: d("2026-07-01"),
                snapshot: Some((d("2026-07-24"), vec![
                    pos("A Index", Category::Equity, Some(300.0), false, false),
                    pos("B Index", Category::Equity, Some(-60.0), true, false),
                ])),
            },
        ];
        let r = thresholds(&months);
        assert_eq!(r.months_present, 2);
        assert_eq!(r.months_total, 3);
        let eq = r.classes.iter().find(|c| c.class == ThresholdClass::Equity).unwrap();
        assert_eq!(eq.months[0].total_eur, Some(140.0));
        assert_eq!(eq.months[0].otc_eur, Some(40.0));
        assert_eq!(eq.months[1].total_eur, None);
        assert_eq!(eq.months[1].snapshot_date, None);
        assert!((eq.avg_total_eur - 250.0).abs() < 1e-9); // (140+360)/2
        assert!((eq.avg_otc_eur - 50.0).abs() < 1e-9); // (40+60)/2
        assert!(r.warnings.iter().any(|w| w.contains("2026-06") && w.contains("no snapshot")));
        // A class with no positions averages to zero, verdict OK.
        let fx = r.classes.iter().find(|c| c.class == ThresholdClass::Fx).unwrap();
        assert_eq!(fx.avg_otc_eur, 0.0);
        assert_eq!(fx.verdict, Verdict::Ok);
    }

    #[test]
    fn verdict_boundaries() {
        assert_eq!(verdict(0.799_999e9, 1e9), Verdict::Ok);
        assert_eq!(verdict(0.8e9, 1e9), Verdict::Watch);
        assert_eq!(verdict(0.999e9, 1e9), Verdict::Watch);
        assert_eq!(verdict(1.0e9, 1e9), Verdict::Breach);
        assert_eq!(verdict(2.5e9, 3e9), Verdict::Watch); // 83% of the 3bn tier
    }

    #[test]
    fn warnings_name_the_contract_and_date() {
        let months = [MonthSnapshot {
            month: d("2026-07-01"),
            snapshot: Some((d("2026-07-24"), vec![
                pos("TYU6 Comdty", Category::InterestRate, None, false, false),
                pos("RXU6 Comdty", Category::InterestRate, Some(1000.0), false, true),
            ])),
        }];
        let r = thresholds(&months);
        assert!(r.warnings.iter().any(|w| w.contains("TYU6 Comdty") && w.contains("2026-07-24") && w.contains("excluded")));
        assert!(r.warnings.iter().any(|w| w.contains("RXU6 Comdty") && w.contains("provisional")));
        // The missing notional is excluded, not zeroed: the sum still counts RX.
        let ir = r.classes.iter().find(|c| c.class == ThresholdClass::InterestRate).unwrap();
        assert_eq!(ir.months[0].total_eur, Some(1000.0));
    }

    #[test]
    fn monitor_tiers() {
        let mk = |n: usize| -> Vec<EmirPosition> {
            (0..n).map(|i| pos(&format!("C{i}"), Category::Fx, Some(1.0), true, false)).collect()
        };
        let m = monitors(&[]);
        assert_eq!(m.otc_open_contracts, 0);
        assert_eq!(m.reconciliation, ReconciliationTier::NotTriggered);
        assert!(!m.compression_required);
        assert_eq!(monitors(&mk(1)).reconciliation, ReconciliationTier::Quarterly);
        assert_eq!(monitors(&mk(50)).reconciliation, ReconciliationTier::Quarterly);
        assert_eq!(monitors(&mk(51)).reconciliation, ReconciliationTier::Weekly);
        assert_eq!(monitors(&mk(499)).reconciliation, ReconciliationTier::Weekly);
        let m = monitors(&mk(500));
        assert_eq!(m.reconciliation, ReconciliationTier::Daily);
        assert!(m.compression_required);
        // Non-OTC positions never count.
        let mut ps = mk(2);
        ps.push(pos("LISTED", Category::Fx, Some(1.0), false, false));
        assert_eq!(monitors(&ps).otc_open_contracts, 2);
    }
}
```

Add `pub mod emir;` to `crates/analytics/src/lib.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p analytics emir`
Expected: FAIL to compile — none of the items exist yet.

- [ ] **Step 3: Implement the module**

Above the test module in `crates/analytics/src/emir.rs`:

```rust
//! EMIR clearing-threshold monitoring (suivi des seuils de compensation).
//!
//! Average month-end position over the last 12 months per asset class,
//! compared to the clearing thresholds of Delegated Regulation (EU)
//! No 149/2013 as amended. Only OTC positions count toward a threshold —
//! a contract on an EU regulated market or an equivalent third-country
//! market is not OTC — but the total line is reported alongside so the
//! disclosure works under either reading. Gross notional, no netting.

use crate::futures::Category;
use chrono::{Datelike, NaiveDate};
use serde::Serialize;

/// WATCH once the OTC average reaches this fraction of the threshold.
pub const WATCH_FRACTION: f64 = 0.80;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThresholdClass {
    Credit,
    Equity,
    InterestRate,
    Fx,
    CommodityOther,
}

impl ThresholdClass {
    pub const ALL: [ThresholdClass; 5] = [
        ThresholdClass::Credit,
        ThresholdClass::Equity,
        ThresholdClass::InterestRate,
        ThresholdClass::Fx,
        ThresholdClass::CommodityOther,
    ];

    pub fn of(cat: Category) -> Self {
        match cat {
            Category::Credit => Self::Credit,
            Category::Equity => Self::Equity,
            Category::InterestRate => Self::InterestRate,
            Category::Fx => Self::Fx,
            // The regulation's fifth bucket is "commodity and other".
            Category::Commodity | Category::Other => Self::CommodityOther,
        }
    }

    /// EUR notional thresholds per RTS 149/2013 art. 11 as amended.
    pub fn threshold_eur(&self) -> f64 {
        match self {
            Self::Credit | Self::Equity => 1e9,
            Self::InterestRate | Self::Fx => 3e9,
            Self::CommodityOther => 4e9,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Credit => "Credit derivatives",
            Self::Equity => "Equity derivatives",
            Self::InterestRate => "Interest-rate derivatives",
            Self::Fx => "FX derivatives",
            Self::CommodityOther => "Commodity and other derivatives",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Ok,
    Watch,
    Breach,
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Watch => "watch",
            Self::Breach => "breach",
        }
    }
}

pub fn verdict(avg_otc_eur: f64, threshold_eur: f64) -> Verdict {
    let frac = avg_otc_eur / threshold_eur;
    if frac >= 1.0 {
        Verdict::Breach
    } else if frac >= WATCH_FRACTION {
        Verdict::Watch
    } else {
        Verdict::Ok
    }
}

fn month_start(d: NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(d.year(), d.month(), 1).unwrap()
}

fn prev_month_start(s: NaiveDate) -> NaiveDate {
    if s.month() == 1 {
        NaiveDate::from_ymd_opt(s.year() - 1, 12, 1).unwrap()
    } else {
        NaiveDate::from_ymd_opt(s.year(), s.month() - 1, 1).unwrap()
    }
}

/// Last day of the month that starts at `start` (a first-of-month).
fn month_end(start: NaiveDate) -> NaiveDate {
    let next = if start.month() == 12 {
        NaiveDate::from_ymd_opt(start.year() + 1, 1, 1).unwrap()
    } else {
        NaiveDate::from_ymd_opt(start.year(), start.month() + 1, 1).unwrap()
    };
    next.pred_opt().unwrap()
}

/// The 12 calendar months ending with `anchor`'s month, oldest first, each
/// paired with the snapshot date to use: the latest available date that
/// falls INSIDE the month and at or before `min(month end, anchor)`. A date
/// from an earlier month never stands in for a missing month — that would
/// double-count one position as two month-ends. Deterministic from data:
/// no wall clock.
pub fn month_window(anchor: NaiveDate, available: &[NaiveDate]) -> Vec<(NaiveDate, Option<NaiveDate>)> {
    let mut starts = Vec::with_capacity(12);
    let mut s = month_start(anchor);
    for _ in 0..12 {
        starts.push(s);
        s = prev_month_start(s);
    }
    starts.reverse();
    starts
        .into_iter()
        .map(|m| {
            let cutoff = month_end(m).min(anchor);
            let chosen = available.iter().copied().filter(|d| *d >= m && *d <= cutoff).max();
            (m, chosen)
        })
        .collect()
}

/// One derivative position at one month-end, with its EUR notional already
/// computed by the exposure path. `notional_eur` is `None` when the spec, an
/// input or the FX rate was missing — excluded from the sums and warned
/// about, never silently zeroed.
#[derive(Debug, Clone)]
pub struct EmirPosition {
    pub ticker: String,
    pub category: Category,
    pub notional_eur: Option<f64>,
    pub otc: bool,
    pub unconfirmed: bool,
}

#[derive(Debug, Clone)]
pub struct MonthSnapshot {
    /// First day of the calendar month.
    pub month: NaiveDate,
    /// `None` when no snapshot falls inside the month.
    pub snapshot: Option<(NaiveDate, Vec<EmirPosition>)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MonthCell {
    pub month: NaiveDate,
    pub snapshot_date: Option<NaiveDate>,
    pub total_eur: Option<f64>,
    pub otc_eur: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClassReport {
    pub class: ThresholdClass,
    pub label: &'static str,
    pub threshold_eur: f64,
    pub months: Vec<MonthCell>,
    pub avg_total_eur: f64,
    pub avg_otc_eur: f64,
    pub pct_of_threshold: f64,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, Serialize)]
pub struct ThresholdReport {
    pub classes: Vec<ClassReport>,
    pub months_present: usize,
    pub months_total: usize,
    pub warnings: Vec<String>,
}

/// Gross notional per threshold class per month, averaged over the months
/// present. Shorts count in absolute value; long and short are never netted.
pub fn thresholds(months: &[MonthSnapshot]) -> ThresholdReport {
    let months_present = months.iter().filter(|m| m.snapshot.is_some()).count();

    let mut warnings = Vec::new();
    for m in months {
        match &m.snapshot {
            None => warnings.push(format!(
                "{}: no snapshot in this month; excluded from the average",
                m.month.format("%Y-%m")
            )),
            Some((date, ps)) => {
                for p in ps {
                    if p.notional_eur.is_none() {
                        warnings.push(format!(
                            "{date}: {} notional unavailable (missing spec, quantity, price or FX rate); excluded from the sums",
                            p.ticker
                        ));
                    } else if p.unconfirmed {
                        warnings.push(format!(
                            "{date}: {} contract spec unconfirmed; its notional is provisional",
                            p.ticker
                        ));
                    }
                }
            }
        }
    }

    let classes = ThresholdClass::ALL
        .iter()
        .map(|cls| {
            let cells: Vec<MonthCell> = months
                .iter()
                .map(|m| match &m.snapshot {
                    None => MonthCell { month: m.month, snapshot_date: None, total_eur: None, otc_eur: None },
                    Some((date, ps)) => {
                        let mut total = 0.0;
                        let mut otc = 0.0;
                        for p in ps.iter().filter(|p| ThresholdClass::of(p.category) == *cls) {
                            if let Some(n) = p.notional_eur {
                                let n = n.abs();
                                total += n;
                                if p.otc {
                                    otc += n;
                                }
                            }
                        }
                        MonthCell { month: m.month, snapshot_date: Some(*date), total_eur: Some(total), otc_eur: Some(otc) }
                    }
                })
                .collect();
            // Average over the months that have data; max(1) only guards the
            // no-data case, where the sums are zero anyway.
            let n = months_present.max(1) as f64;
            let avg_total_eur = cells.iter().filter_map(|c| c.total_eur).sum::<f64>() / n;
            let avg_otc_eur = cells.iter().filter_map(|c| c.otc_eur).sum::<f64>() / n;
            let threshold_eur = cls.threshold_eur();
            ClassReport {
                class: *cls,
                label: cls.label(),
                threshold_eur,
                months: cells,
                avg_total_eur,
                avg_otc_eur,
                pct_of_threshold: avg_otc_eur / threshold_eur,
                verdict: verdict(avg_otc_eur, threshold_eur),
            }
        })
        .collect();

    ThresholdReport { classes, months_present, months_total: months.len(), warnings }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationTier {
    NotTriggered,
    Quarterly,
    Weekly,
    Daily,
}

#[derive(Debug, Clone, Serialize)]
pub struct Monitors {
    pub otc_open_contracts: usize,
    pub reconciliation: ReconciliationTier,
    /// Semiannual portfolio-compression analysis required (>= 500 OTC
    /// contracts outstanding with one counterparty, RTS 149/2013 art. 14).
    pub compression_required: bool,
}

/// Reconciliation tiers for a financial counterparty (RTS 149/2013 art. 13):
/// daily above 500 contracts, weekly 51-499, quarterly 50 or fewer. The tool
/// has no counterparty data, so the count assumes a single counterparty —
/// the strictest possible tier assignment.
pub fn monitors(anchor_positions: &[EmirPosition]) -> Monitors {
    let n = anchor_positions.iter().filter(|p| p.otc).count();
    let reconciliation = match n {
        0 => ReconciliationTier::NotTriggered,
        1..=50 => ReconciliationTier::Quarterly,
        51..=499 => ReconciliationTier::Weekly,
        _ => ReconciliationTier::Daily,
    };
    Monitors { otc_open_contracts: n, reconciliation, compression_required: n >= 500 }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p analytics`
Expected: PASS — all pre-existing analytics tests plus the 7 new emir tests.

- [ ] **Step 5: Commit**

```bash
git add crates/analytics/src/emir.rs crates/analytics/src/lib.rs
git commit -m "feat(analytics): EMIR clearing-threshold engine with OTC monitors"
```

---

### Task 4: Evidence workbook builder in `ingest`

**Files:**
- Create: `crates/ingest/src/emir_file.rs`
- Modify: `crates/ingest/src/lib.rs` (add `pub mod emir_file;`)
- Create: `crates/ingest/tests/emir_evidence.rs`

**Interfaces:**
- Consumes: `rust_xlsxwriter` (already a dependency of `ingest`). No dependency on `analytics` or `db` — the builder takes flat rows the server maps into (house pattern: `bloomberg.rs` defines its own `RequestItem`).
- Produces (in `ingest::emir_file`):
  - `pub struct SummaryRow { pub label: String, pub threshold_eur: f64, pub avg_otc_eur: f64, pub pct_of_threshold: f64, pub verdict: String, pub avg_total_eur: f64 }`
  - `pub struct MonthRow { pub label: String, pub month: String, pub snapshot_date: Option<String>, pub total_eur: Option<f64>, pub otc_eur: Option<f64> }`
  - `pub struct ContractRow { pub root: String, pub label: String, pub category: String, pub otc: bool, pub confirmed: bool, pub point_value: Option<f64>, pub currency: String }`
  - `pub struct KpiRow { pub month: String, pub unconfirmed_over_5d: i32, pub reconciliation: String, pub disputes: i32, pub note: String }`
  - `pub struct EmirEvidence { pub anchor: chrono::NaiveDate, pub months_present: usize, pub months_total: usize, pub summary: Vec<SummaryRow>, pub months: Vec<MonthRow>, pub contracts: Vec<ContractRow>, pub kpis: Vec<KpiRow>, pub warnings: Vec<String> }`
  - `pub fn build_evidence(e: &EmirEvidence) -> anyhow::Result<Vec<u8>>` — sheets `Seuils`, `Contrats`, `KPI`

- [ ] **Step 1: Write the failing round-trip test**

Create `crates/ingest/tests/emir_evidence.rs`:

```rust
use calamine::{Data, Reader, Xlsx};
use ingest::emir_file::{build_evidence, ContractRow, EmirEvidence, KpiRow, MonthRow, SummaryRow};
use std::io::Cursor;

fn cell(r: &calamine::Range<Data>, row: u32, col: u32) -> String {
    match r.get_value((row, col)) {
        Some(Data::String(s)) => s.clone(),
        Some(Data::Float(f)) => f.to_string(),
        Some(Data::Bool(b)) => b.to_string(),
        other => format!("{other:?}"),
    }
}

#[test]
fn evidence_round_trips_through_calamine() {
    let e = EmirEvidence {
        anchor: "2026-07-24".parse().unwrap(),
        months_present: 2,
        months_total: 12,
        summary: vec![SummaryRow {
            label: "Interest-rate derivatives".into(),
            threshold_eur: 3e9,
            avg_otc_eur: 0.0,
            pct_of_threshold: 0.0,
            verdict: "ok".into(),
            avg_total_eur: 1.25e7,
        }],
        months: vec![
            MonthRow { label: "Interest-rate derivatives".into(), month: "2026-06".into(), snapshot_date: Some("2026-06-26".into()), total_eur: Some(1.0e7), otc_eur: Some(0.0) },
            MonthRow { label: "Interest-rate derivatives".into(), month: "2026-05".into(), snapshot_date: None, total_eur: None, otc_eur: None },
        ],
        contracts: vec![ContractRow { root: "RX".into(), label: "Euro-Bund".into(), category: "interest_rate".into(), otc: false, confirmed: true, point_value: Some(1000.0), currency: "EUR".into() }],
        kpis: vec![KpiRow { month: "2026-07".into(), unconfirmed_over_5d: 0, reconciliation: "not_applicable".into(), disputes: 0, note: "".into() }],
        warnings: vec!["2026-05: no snapshot in this month; excluded from the average".into()],
    };
    let bytes = build_evidence(&e).unwrap();

    let mut wb: Xlsx<_> = Xlsx::new(Cursor::new(bytes)).expect("valid xlsx");
    let names = wb.sheet_names().to_vec();
    for n in ["Seuils", "Contrats", "KPI"] {
        assert!(names.iter().any(|x| x == n), "missing sheet {n} in {names:?}");
    }

    let s = wb.worksheet_range("Seuils").unwrap();
    assert!(cell(&s, 0, 0).contains("EMIR"));
    assert!(cell(&s, 1, 0).contains("2026-07-24"));
    assert!(cell(&s, 2, 0).contains("2 of 12"));
    // Summary table: header row then the one class row.
    assert_eq!(cell(&s, 5, 0), "Class");
    assert_eq!(cell(&s, 6, 0), "Interest-rate derivatives");
    assert_eq!(cell(&s, 6, 1), "3000000000");
    assert_eq!(cell(&s, 6, 4), "ok");
    // Detail table: one blank row after the summary block (summary ends at
    // row 6, row 7 blank), header at row 8, rows at 9-10.
    assert_eq!(cell(&s, 8, 0), "Class");
    assert_eq!(cell(&s, 9, 1), "2026-06");
    assert_eq!(cell(&s, 9, 2), "2026-06-26");
    assert_eq!(cell(&s, 10, 2), "missing");
    // Warnings block ends the sheet: blank row 11, "Warnings" at 12, line at 13.
    assert_eq!(cell(&s, 12, 0), "Warnings");
    assert!(cell(&s, 13, 0).contains("2026-05"));

    let c = wb.worksheet_range("Contrats").unwrap();
    assert_eq!(cell(&c, 1, 0), "RX");
    assert_eq!(cell(&c, 1, 3), "false");

    let k = wb.worksheet_range("KPI").unwrap();
    assert_eq!(cell(&k, 1, 0), "2026-07");
    assert_eq!(cell(&k, 1, 2), "not_applicable");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ingest --test emir_evidence`
Expected: FAIL to compile — `ingest::emir_file` does not exist.

- [ ] **Step 3: Implement the builder**

Create `crates/ingest/src/emir_file.rs` (add `pub mod emir_file;` to `crates/ingest/src/lib.rs`):

```rust
//! EMIR threshold-monitoring evidence workbook.
//!
//! The procedure requires the calculation details to be archived (SharePoint);
//! this file IS that artifact: the full month-by-month figures behind the
//! threshold verdicts, the contract inventory with OTC flags, and the manual
//! KPI history. Flat input rows, no dependency on analytics or db — the
//! server maps into them (same pattern as `bloomberg::RequestItem`).

use chrono::NaiveDate;
use rust_xlsxwriter::{Format, Workbook};

pub struct SummaryRow {
    pub label: String,
    pub threshold_eur: f64,
    pub avg_otc_eur: f64,
    pub pct_of_threshold: f64,
    pub verdict: String,
    pub avg_total_eur: f64,
}

pub struct MonthRow {
    pub label: String,
    pub month: String,
    /// `None` renders as "missing" — the month had no snapshot.
    pub snapshot_date: Option<String>,
    pub total_eur: Option<f64>,
    pub otc_eur: Option<f64>,
}

pub struct ContractRow {
    pub root: String,
    pub label: String,
    pub category: String,
    pub otc: bool,
    pub confirmed: bool,
    pub point_value: Option<f64>,
    pub currency: String,
}

pub struct KpiRow {
    pub month: String,
    pub unconfirmed_over_5d: i32,
    pub reconciliation: String,
    pub disputes: i32,
    pub note: String,
}

pub struct EmirEvidence {
    pub anchor: NaiveDate,
    pub months_present: usize,
    pub months_total: usize,
    pub summary: Vec<SummaryRow>,
    pub months: Vec<MonthRow>,
    pub contracts: Vec<ContractRow>,
    pub kpis: Vec<KpiRow>,
    pub warnings: Vec<String>,
}

pub fn build_evidence(e: &EmirEvidence) -> anyhow::Result<Vec<u8>> {
    let mut wb = Workbook::new();
    let bold = Format::new().set_bold();

    // ---- Seuils ----
    let s = wb.add_worksheet();
    s.set_name("Seuils")?;
    s.set_column_width(0, 34)?;
    for c in 1..=5u16 {
        s.set_column_width(c, 20)?;
    }
    s.write_string_with_format(0, 0, "EMIR clearing-threshold monitoring — Borobudur", &bold)?;
    s.write_string(1, 0, &format!("Anchor date: {}", e.anchor))?;
    s.write_string(2, 0, &format!("Months with a snapshot: {} of {}", e.months_present, e.months_total))?;
    s.write_string(3, 0, "Only OTC positions count toward the thresholds; gross notional, no netting. Average of month-end positions.")?;

    let mut row: u32 = 5;
    for (c, h) in ["Class", "Threshold EUR", "Avg OTC notional EUR", "% of threshold", "Verdict", "Avg total notional EUR"].iter().enumerate() {
        s.write_string_with_format(row, c as u16, *h, &bold)?;
    }
    row += 1;
    for r in &e.summary {
        s.write_string(row, 0, &r.label)?;
        s.write_number(row, 1, r.threshold_eur)?;
        s.write_number(row, 2, r.avg_otc_eur)?;
        s.write_number(row, 3, r.pct_of_threshold)?;
        s.write_string(row, 4, &r.verdict)?;
        s.write_number(row, 5, r.avg_total_eur)?;
        row += 1;
    }

    row += 1; // one blank row between the summary and detail tables
    for (c, h) in ["Class", "Month", "Snapshot date", "Total EUR", "OTC EUR"].iter().enumerate() {
        s.write_string_with_format(row, c as u16, *h, &bold)?;
    }
    row += 1;
    for r in &e.months {
        s.write_string(row, 0, &r.label)?;
        s.write_string(row, 1, &r.month)?;
        match &r.snapshot_date {
            Some(d) => s.write_string(row, 2, d)?,
            None => s.write_string(row, 2, "missing")?,
        };
        if let Some(v) = r.total_eur {
            s.write_number(row, 3, v)?;
        }
        if let Some(v) = r.otc_eur {
            s.write_number(row, 4, v)?;
        }
        row += 1;
    }

    row += 1;
    s.write_string_with_format(row, 0, "Warnings", &bold)?;
    row += 1;
    for w in &e.warnings {
        s.write_string(row, 0, w)?;
        row += 1;
    }

    // ---- Contrats ----
    let c = wb.add_worksheet();
    c.set_name("Contrats")?;
    c.set_column_width(0, 10)?;
    c.set_column_width(1, 24)?;
    for (col, h) in ["Root", "Label", "Category", "OTC", "Confirmed", "Point value", "Currency"].iter().enumerate() {
        c.write_string_with_format(0, col as u16, *h, &bold)?;
    }
    for (i, r) in e.contracts.iter().enumerate() {
        let row = (i + 1) as u32;
        c.write_string(row, 0, &r.root)?;
        c.write_string(row, 1, &r.label)?;
        c.write_string(row, 2, &r.category)?;
        c.write_string(row, 3, if r.otc { "true" } else { "false" })?;
        c.write_string(row, 4, if r.confirmed { "true" } else { "false" })?;
        if let Some(pv) = r.point_value {
            c.write_number(row, 5, pv)?;
        }
        c.write_string(row, 6, &r.currency)?;
    }

    // ---- KPI ----
    let k = wb.add_worksheet();
    k.set_name("KPI")?;
    k.set_column_width(4, 60)?;
    for (col, h) in ["Month", "Unconfirmed > 5 days", "Reconciliation", "Disputes", "Note"].iter().enumerate() {
        k.write_string_with_format(0, col as u16, *h, &bold)?;
    }
    for (i, r) in e.kpis.iter().enumerate() {
        let row = (i + 1) as u32;
        k.write_string(row, 0, &r.month)?;
        k.write_number(row, 1, f64::from(r.unconfirmed_over_5d))?;
        k.write_string(row, 2, &r.reconciliation)?;
        k.write_number(row, 3, f64::from(r.disputes))?;
        k.write_string(row, 4, &r.note)?;
    }

    Ok(wb.save_to_buffer()?)
}
```

Row-cursor hand-trace with the test's one summary row: title rows 0–3; summary header 5; summary row 6 (cursor 7); blank row 7; detail header 8; detail rows 9–10 (cursor 11); blank row 11; "Warnings" 12; warning line 13 — exactly the coordinates the Step 1 test asserts.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ingest`
Expected: PASS (all ingest tests including the new round trip).

- [ ] **Step 5: Commit**

```bash
git add crates/ingest/src/emir_file.rs crates/ingest/src/lib.rs crates/ingest/tests/emir_evidence.rs
git commit -m "feat(ingest): EMIR evidence workbook builder (Seuils/Contrats/KPI)"
```

---

### Task 5: `GET /api/emir`

**Files:**
- Create: `crates/server/src/handlers/emir.rs`
- Modify: `crates/server/src/handlers/mod.rs` (add `pub mod emir;`)
- Modify: `crates/server/src/handlers/limits.rs` (make `future_positions` and its return type `pub(crate)`)
- Modify: `crates/server/src/routes.rs` (add the route)
- Create: `crates/server/tests/api_emir.rs`

**Interfaces:**
- Consumes: `analytics::emir::{month_window, thresholds, monitors, EmirPosition, MonthSnapshot}`; `analytics::exposure`; `analytics::contract_root` (re-exported from `analytics::futures` — if the crate root does not re-export it, add it to the existing re-export list in `crates/analytics/src/lib.rs` following how `exposure` is exported); `db::repo::{position_dates, positions_for, contracts_all, emir_kpis_all, FuturesContract}`; the `future_positions` helper in `handlers/limits.rs` (currently private — its return struct bundles `positions: Vec<analytics::FuturePosition>` and `unconfirmed: Vec<String>`; mark the fn and the struct `pub(crate)`).
- Produces: `GET /api/emir?date=` returning the JSON payload below; an internal `async fn assemble(st: &AppState, q_date: &Option<String>) -> Result<Option<Assembly>, AppError>` that Task 7's export reuses (`Assembly` bundles `dates`, `anchor`, `report`, `monitors`, `margin`, `futures_count`, `kpis`, `contracts`).

Payload shape (Task 9's TS types mirror this exactly):

```json
{
  "dates": ["2026-07-24"], "date": "2026-07-24",
  "months_present": 1, "months_total": 12,
  "classes": [ { "class": "credit", "label": "Credit derivatives", "threshold_eur": 1e9,
                 "months": [ { "month": "2025-08-01", "snapshot_date": null, "total_eur": null, "otc_eur": null } ],
                 "avg_total_eur": 0.0, "avg_otc_eur": 0.0, "pct_of_threshold": 0.0, "verdict": "ok" } ],
  "warnings": ["..."],
  "monitors": { "otc_open_contracts": 0, "reconciliation": "not_triggered", "compression_required": false },
  "monitors_note": "Counterparty breakdown unavailable: the reconciliation tier and compression trigger assume all OTC contracts face a single counterparty (the strictest reading).",
  "margin": [ { "name": "...", "currency": "EUR", "valuation_ccy": 1.0, "valuation_eur": 1.0 } ],
  "futures_count": 8,
  "kpis": [ { "month": "2026-07-01", "unconfirmed_over_5d": 0, "reconciliation": "done", "disputes": 0, "note": null } ],
  "otc_note": "Only OTC positions count toward the clearing thresholds. Contracts on an EU regulated market or an equivalent third-country market are not OTC; flag any contract on a non-equivalent venue as OTC on the Data page."
}
```

No-snapshots case: 200 with `{"empty": true, "warnings": ["No snapshots imported yet."]}` (P&L convention, never a 4xx).

- [ ] **Step 1: Write the failing tests**

Create `crates/server/tests/api_emir.rs`. Copy the `BOUNDARY`/`SAMPLE` consts, `upload_req`, `get_json`, `put_json` helpers verbatim from `crates/server/tests/api_futures.rs`, and this fixture fn (adapted from `api_bloomberg.rs` — same inline-harness convention, no shared module):

```rust
/// Fresh embedded database seeded with the sample workbook through the HTTP
/// API, wired into a router. Mirrors `app_with_sample` in `api_pnl.rs`; there
/// is no shared tests/common harness in this crate, so this file inlines its
/// own instance. sample.xlsx has exactly one snapshot date (2026-07-24) with
/// 8 futures positions and 9 Margin Acc rows; import seeds 8 unconfirmed
/// contract roots.
async fn app_with_sample() -> (axum::Router, sqlx::PgPool, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });
    let bytes = std::fs::read(SAMPLE).unwrap();
    assert_eq!(
        app.clone().oneshot(upload_req("/api/imports", "s.xlsx", &bytes)).await.unwrap().status(),
        StatusCode::OK
    );
    (app, pool, edb)
}

#[tokio::test]
async fn emir_empty_before_any_import() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });
    let (status, body) = get_json(&app, "/api/emir").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["empty"], true, "{body}");
    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn emir_report_on_sample() {
    let (app, pool, edb) = app_with_sample().await;

    let (status, body) = get_json(&app, "/api/emir").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["date"], "2026-07-24", "{body}");
    assert_eq!(body["months_total"], 12);
    assert_eq!(body["months_present"], 1);
    let classes = body["classes"].as_array().unwrap();
    assert_eq!(classes.len(), 5);
    // All contracts default to non-OTC, so every threshold line reads zero OK.
    for c in classes {
        assert_eq!(c["avg_otc_eur"], 0.0, "{c}");
        assert_eq!(c["verdict"], "ok", "{c}");
        assert_eq!(c["months"].as_array().unwrap().len(), 12);
    }
    // The seeded specs are unconfirmed: total notional is provisional and the
    // warnings say so per contract.
    assert!(body["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("provisional")), "{body}");
    // Eleven of the twelve months predate the sample's history.
    assert_eq!(
        body["warnings"].as_array().unwrap().iter().filter(|w| w.as_str().unwrap().contains("no snapshot")).count(),
        11, "{body}"
    );
    assert_eq!(body["monitors"]["otc_open_contracts"], 0);
    assert_eq!(body["monitors"]["reconciliation"], "not_triggered");
    assert_eq!(body["monitors"]["compression_required"], false);
    assert_eq!(body["margin"].as_array().unwrap().len(), 9, "{body}");
    assert_eq!(body["futures_count"], 8, "{body}");
    assert_eq!(body["kpis"].as_array().unwrap().len(), 0);

    // Flag one contract OTC: its notional must appear on the OTC line of its
    // class. RX is interest_rate; confirm it with its real point value so the
    // notional is definite.
    let (status, _) = put_json(&app, "/api/futures-contracts/RX", serde_json::json!({
        "label": "Euro-Bund", "category": "interest_rate", "point_value": 1000.0,
        "currency": "EUR", "curve": null, "price_convention": "decimal",
        "confirmed": true, "otc": true,
    })).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = get_json(&app, "/api/emir").await;
    assert_eq!(status, StatusCode::OK);
    let ir = body["classes"].as_array().unwrap().iter().find(|c| c["class"] == "interest_rate").unwrap();
    assert!(ir["avg_otc_eur"].as_f64().unwrap() > 0.0, "{ir}");
    assert!(ir["avg_otc_eur"].as_f64().unwrap() <= ir["avg_total_eur"].as_f64().unwrap(), "{ir}");
    assert_eq!(body["monitors"]["otc_open_contracts"], 1, "{body}");
    assert_eq!(body["monitors"]["reconciliation"], "quarterly", "{body}");

    // Bad date is a 400.
    let (status, _) = get_json(&app, "/api/emir?date=garbage").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    pool.close().await;
    edb.stop().await;
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p server --test api_emir`
Expected: FAIL — 404s (route not registered) or compile errors if helper imports are wrong. Fix imports until failures are the expected 404/assertion kind before implementing.

- [ ] **Step 3: Implement the handler**

In `crates/server/src/handlers/limits.rs`, change `fn future_positions` to `pub(crate) fn future_positions` and make its return struct (and the struct's `positions`/`unconfirmed` fields) `pub(crate)`.

Create `crates/server/src/handlers/emir.rs`:

```rust
//! EMIR monitoring: clearing thresholds, OTC obligation monitors, margin
//! view, monthly KPIs, and the evidence export.

use crate::error::AppError;
use crate::state::AppState;
use analytics::emir;
use axum::extract::{Query, State};
use axum::Json;
use chrono::NaiveDate;

#[derive(serde::Deserialize)]
pub struct DateQuery {
    date: Option<String>,
}

#[derive(serde::Serialize)]
pub struct MarginLine {
    pub name: Option<String>,
    pub currency: Option<String>,
    pub valuation_ccy: Option<f64>,
    pub valuation_eur: Option<f64>,
}

pub struct Assembly {
    pub dates: Vec<NaiveDate>,
    pub anchor: NaiveDate,
    pub report: emir::ThresholdReport,
    pub monitors: emir::Monitors,
    pub margin: Vec<MarginLine>,
    pub futures_count: usize,
    pub kpis: Vec<db::repo::EmirKpi>,
    pub contracts: Vec<db::repo::FuturesContract>,
}

/// One month-end's positions as EMIR sees them: the exposure path computes
/// the EUR notional (aum is irrelevant here, pass 0.0 — pct_nav is unused),
/// then each row picks up its contract's OTC flag by root.
async fn emir_positions(
    st: &AppState,
    date: NaiveDate,
    specs: &[db::repo::FuturesContract],
) -> Result<Vec<emir::EmirPosition>, AppError> {
    let rows = db::repo::positions_for(&st.pool, date).await?;
    let snap = super::limits::future_positions(&rows, specs);
    let rep = analytics::exposure(&snap.positions, 0.0);
    Ok(rep
        .rows
        .into_iter()
        .map(|r| {
            let otc = analytics::contract_root(&r.ticker)
                .and_then(|root| specs.iter().find(|s| s.contract_root == root).map(|s| s.otc))
                .unwrap_or(false);
            emir::EmirPosition {
                ticker: r.ticker,
                category: r.category,
                notional_eur: r.notional_eur,
                otc,
                unconfirmed: r.unconfirmed,
            }
        })
        .collect())
}

pub async fn assemble(st: &AppState, q_date: &Option<String>) -> Result<Option<Assembly>, AppError> {
    let dates = db::repo::position_dates(&st.pool).await?;
    let anchor = match q_date {
        Some(s) => Some(s.parse::<NaiveDate>().map_err(|_| AppError::BadRequest(format!("bad date: {s}")))?),
        None => dates.first().copied(),
    };
    let Some(anchor) = anchor else { return Ok(None) };

    let specs = db::repo::contracts_all(&st.pool).await?;
    let mut months = Vec::with_capacity(12);
    for (month, chosen) in emir::month_window(anchor, &dates) {
        let snapshot = match chosen {
            Some(d) => Some((d, emir_positions(st, d, &specs).await?)),
            None => None,
        };
        months.push(emir::MonthSnapshot { month, snapshot });
    }

    // The anchor month's cell doubles as "the state at the anchor": monitors,
    // margin and the futures count are all struck there.
    let anchor_cell = months.last().and_then(|m| m.snapshot.clone());
    let monitors = emir::monitors(anchor_cell.as_ref().map(|(_, p)| p.as_slice()).unwrap_or(&[]));
    let (margin, futures_count) = match anchor_cell.as_ref().map(|(d, _)| *d) {
        Some(d) => {
            let rows = db::repo::positions_for(&st.pool, d).await?;
            let margin = rows
                .iter()
                .filter(|r| r.asset_type == "Margin Acc")
                .map(|r| MarginLine {
                    name: r.name.clone(),
                    currency: r.currency.clone(),
                    valuation_ccy: r.valuation_ccy,
                    valuation_eur: r.valuation_eur,
                })
                .collect();
            let n = rows.iter().filter(|r| r.asset_type == "Future").count();
            (margin, n)
        }
        None => (Vec::new(), 0),
    };

    let report = emir::thresholds(&months);
    let kpis = db::repo::emir_kpis_all(&st.pool).await?;
    Ok(Some(Assembly { dates, anchor, report, monitors, margin, futures_count, kpis, contracts: specs }))
}

pub async fn get(
    State(st): State<AppState>,
    Query(q): Query<DateQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let Some(a) = assemble(&st, &q.date).await? else {
        return Ok(Json(serde_json::json!({"empty": true, "warnings": ["No snapshots imported yet."]})));
    };
    Ok(Json(serde_json::json!({
        "dates": a.dates,
        "date": a.anchor,
        "months_present": a.report.months_present,
        "months_total": a.report.months_total,
        "classes": a.report.classes,
        "warnings": a.report.warnings,
        "monitors": a.monitors,
        "monitors_note": "Counterparty breakdown unavailable: the reconciliation tier and compression trigger assume all OTC contracts face a single counterparty (the strictest reading).",
        "margin": a.margin,
        "futures_count": a.futures_count,
        "kpis": a.kpis,
        "otc_note": "Only OTC positions count toward the clearing thresholds. Contracts on an EU regulated market or an equivalent third-country market are not OTC; flag any contract on a non-equivalent venue as OTC on the Data page.",
    })))
}
```

Add `pub mod emir;` to `crates/server/src/handlers/mod.rs`, and in `crates/server/src/routes.rs` add (after the `/api/pnl` line):

```rust
        .route("/api/emir", get(handlers::emir::get))
```

If `analytics::contract_root` is not re-exported at the crate root, add it to the existing re-export list in `crates/analytics/src/lib.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p server --test api_emir`
Expected: PASS (both tests). Then `cargo test -p server` to confirm nothing else broke.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/handlers/emir.rs crates/server/src/handlers/mod.rs crates/server/src/handlers/limits.rs crates/server/src/routes.rs crates/server/tests/api_emir.rs crates/analytics/src/lib.rs
git commit -m "feat(server): GET /api/emir with thresholds, monitors, margin and KPIs"
```

---

### Task 6: `PUT /api/emir/kpis/{month}`

**Files:**
- Modify: `crates/server/src/handlers/emir.rs` (add KpiBody + put_kpi)
- Modify: `crates/server/src/routes.rs`
- Modify: `crates/server/tests/api_emir.rs` (new test)

**Interfaces:**
- Consumes: `db::repo::{EmirKpi, emir_kpi_upsert}` from Task 2.
- Produces: `PUT /api/emir/kpis/{month}` (month = `YYYY-MM-01`), body `{unconfirmed_over_5d, reconciliation, disputes, note}`, response `Json<db::repo::EmirKpi>`; the record then appears in `GET /api/emir`'s `kpis` array.

- [ ] **Step 1: Write the failing test**

Add to `crates/server/tests/api_emir.rs`:

```rust
#[tokio::test]
async fn kpi_upsert_validation_and_echo_in_report() {
    let (app, pool, edb) = app_with_sample().await;

    let good = serde_json::json!({
        "unconfirmed_over_5d": 1, "reconciliation": "done", "disputes": 0, "note": "  trimmed  ",
    });
    let (status, body) = put_json(&app, "/api/emir/kpis/2026-07-01", good.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["month"], "2026-07-01");
    assert_eq!(body["note"], "trimmed"); // trimmed, not stored raw

    // Blank note collapses to null.
    let blank = serde_json::json!({
        "unconfirmed_over_5d": 0, "reconciliation": "not_applicable", "disputes": 0, "note": "   ",
    });
    let (status, body) = put_json(&app, "/api/emir/kpis/2026-06-01", blank).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["note"], serde_json::Value::Null);

    // Mid-month date, unknown status and negative counts are rejected.
    let (status, _) = put_json(&app, "/api/emir/kpis/2026-07-15", good.clone()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = put_json(&app, "/api/emir/kpis/2026-07-01", serde_json::json!({
        "unconfirmed_over_5d": 0, "reconciliation": "maybe", "disputes": 0, "note": null,
    })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = put_json(&app, "/api/emir/kpis/2026-07-01", serde_json::json!({
        "unconfirmed_over_5d": -1, "reconciliation": "done", "disputes": 0, "note": null,
    })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = put_json(&app, "/api/emir/kpis/garbage", good).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Both records come back in the report, newest first.
    let (status, body) = get_json(&app, "/api/emir").await;
    assert_eq!(status, StatusCode::OK);
    let kpis = body["kpis"].as_array().unwrap();
    assert_eq!(kpis.len(), 2, "{body}");
    assert_eq!(kpis[0]["month"], "2026-07-01");

    pool.close().await;
    edb.stop().await;
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p server --test api_emir kpi_upsert`
Expected: FAIL — 404 on the PUT (route not registered).

- [ ] **Step 3: Implement**

Add to `crates/server/src/handlers/emir.rs` (new imports: `axum::extract::Path`, `chrono::Datelike`):

```rust
#[derive(serde::Deserialize)]
pub struct KpiBody {
    pub unconfirmed_over_5d: i32,
    pub reconciliation: String,
    pub disputes: i32,
    pub note: Option<String>,
}

pub async fn put_kpi(
    State(st): State<AppState>,
    Path(month): Path<String>,
    Json(b): Json<KpiBody>,
) -> Result<Json<db::repo::EmirKpi>, AppError> {
    let month = month
        .parse::<NaiveDate>()
        .map_err(|_| AppError::BadRequest(format!("bad month: {month}")))?;
    if month.day() != 1 {
        return Err(AppError::Unprocessable("month must be a first-of-month date (YYYY-MM-01)".into()));
    }
    if !["done", "not_done", "not_applicable"].contains(&b.reconciliation.as_str()) {
        return Err(AppError::Unprocessable(
            "reconciliation must be one of done, not_done, not_applicable".into(),
        ));
    }
    if b.unconfirmed_over_5d < 0 || b.disputes < 0 {
        return Err(AppError::Unprocessable("counts must be >= 0".into()));
    }
    let k = db::repo::EmirKpi {
        month,
        unconfirmed_over_5d: b.unconfirmed_over_5d,
        reconciliation: b.reconciliation,
        disputes: b.disputes,
        note: b.note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()),
    };
    db::repo::emir_kpi_upsert(&st.pool, &k).await?;
    Ok(Json(k))
}
```

In `routes.rs`, after the `/api/emir` line:

```rust
        .route("/api/emir/kpis/{month}", axum::routing::put(handlers::emir::put_kpi))
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p server --test api_emir`
Expected: PASS (all three tests).

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/handlers/emir.rs crates/server/src/routes.rs crates/server/tests/api_emir.rs
git commit -m "feat(server): PUT /api/emir/kpis/{month} with first-of-month and enum validation"
```

---

### Task 7: `GET /api/emir/export`

**Files:**
- Modify: `crates/server/src/handlers/emir.rs` (add export handler + mapping)
- Modify: `crates/server/src/routes.rs`
- Modify: `crates/server/tests/api_emir.rs` (new test)

**Interfaces:**
- Consumes: `assemble` from Task 5; `ingest::emir_file::{build_evidence, EmirEvidence, SummaryRow, MonthRow, ContractRow, KpiRow}` from Task 4.
- Produces: `GET /api/emir/export?date=` streaming `EMIR - seuils - {anchor}.xlsx`; 422 `Unprocessable` when there are no snapshots (an empty evidence file would be misleading as an archived control record).

- [ ] **Step 1: Write the failing test**

Add to `crates/server/tests/api_emir.rs` (copy the `get_bytes` helper verbatim from `api_bloomberg.rs`):

```rust
#[tokio::test]
async fn evidence_export_round_trips() {
    let (app, pool, edb) = app_with_sample().await;

    let (status, ctype, bytes) = get_bytes(&app, "/api/emir/export").await;
    assert_eq!(status, 200);
    assert!(ctype.contains("spreadsheet"), "got {ctype}");
    let mut wb: calamine::Xlsx<_> = calamine::Xlsx::new(std::io::Cursor::new(bytes)).expect("valid xlsx");
    let names = calamine::Reader::sheet_names(&wb).to_vec();
    for n in ["Seuils", "Contrats", "KPI"] {
        assert!(names.iter().any(|x| x == n), "missing {n} in {names:?}");
    }
    let s = calamine::Reader::worksheet_range(&mut wb, "Seuils").unwrap();
    let text: Vec<String> = s.cells().filter_map(|(_, _, v)| match v {
        calamine::Data::String(s) => Some(s.clone()),
        _ => None,
    }).collect();
    assert!(text.iter().any(|t| t.contains("2026-07-24")), "{text:?}");
    assert!(text.iter().any(|t| t == "Interest-rate derivatives"), "{text:?}");
    assert!(text.iter().any(|t| t.contains("1 of 12")), "{text:?}");
    let c = calamine::Reader::worksheet_range(&mut wb, "Contrats").unwrap();
    // Header + the 8 seeded contracts.
    assert_eq!(c.rows().count(), 9, "{:?}", c.rows().collect::<Vec<_>>());

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn evidence_export_refuses_empty_db() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });
    let res = app.clone().oneshot(Request::get("/api/emir/export").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    pool.close().await;
    edb.stop().await;
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p server --test api_emir evidence`
Expected: FAIL — 404 (route not registered).

- [ ] **Step 3: Implement**

Add to `crates/server/src/handlers/emir.rs` (new imports: `axum::http::{header, HeaderMap, HeaderValue, StatusCode}`, `axum::response::IntoResponse`, `ingest::emir_file`):

```rust
pub async fn export(
    State(st): State<AppState>,
    Query(q): Query<DateQuery>,
) -> Result<impl IntoResponse, AppError> {
    let Some(a) = assemble(&st, &q.date).await? else {
        return Err(AppError::Unprocessable(
            "no snapshots imported yet; there is nothing to evidence".into(),
        ));
    };
    let summary = a.report.classes.iter().map(|c| emir_file::SummaryRow {
        label: c.label.to_string(),
        threshold_eur: c.threshold_eur,
        avg_otc_eur: c.avg_otc_eur,
        pct_of_threshold: c.pct_of_threshold,
        verdict: c.verdict.as_str().to_string(),
        avg_total_eur: c.avg_total_eur,
    }).collect();
    let months = a.report.classes.iter().flat_map(|c| {
        c.months.iter().map(|m| emir_file::MonthRow {
            label: c.label.to_string(),
            month: m.month.format("%Y-%m").to_string(),
            snapshot_date: m.snapshot_date.map(|d| d.to_string()),
            total_eur: m.total_eur,
            otc_eur: m.otc_eur,
        })
    }).collect();
    let contracts = a.contracts.iter().map(|c| emir_file::ContractRow {
        root: c.contract_root.clone(),
        label: c.label.clone(),
        category: c.category.clone(),
        otc: c.otc,
        confirmed: c.confirmed,
        point_value: c.point_value,
        currency: c.currency.clone(),
    }).collect();
    let kpis = a.kpis.iter().map(|k| emir_file::KpiRow {
        month: k.month.format("%Y-%m").to_string(),
        unconfirmed_over_5d: k.unconfirmed_over_5d,
        reconciliation: k.reconciliation.clone(),
        disputes: k.disputes,
        note: k.note.clone().unwrap_or_default(),
    }).collect();
    let bytes = emir_file::build_evidence(&emir_file::EmirEvidence {
        anchor: a.anchor,
        months_present: a.report.months_present,
        months_total: a.report.months_total,
        summary,
        months,
        contracts,
        kpis,
        warnings: a.report.warnings.clone(),
    })?;

    let mut h = HeaderMap::new();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"));
    h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_str(
        &format!("attachment; filename=\"EMIR - seuils - {}.xlsx\"", a.anchor))?);
    Ok((StatusCode::OK, h, bytes))
}
```

In `routes.rs`:

```rust
        .route("/api/emir/export", get(handlers::emir::export))
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p server`
Expected: PASS (whole server crate).

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/handlers/emir.rs crates/server/src/routes.rs crates/server/tests/api_emir.rs
git commit -m "feat(server): GET /api/emir/export evidence workbook"
```

---

### Task 8: Frontend — OTC flag on the contract panel

**Files:**
- Modify: `frontend/src/api.ts` (FuturesContract type)
- Modify: `frontend/src/components/FuturesContracts.tsx`

**Interfaces:**
- Consumes: `PUT /api/futures-contracts/{root}` now requiring `otc` (Task 1).
- Produces: `FuturesContract` TS interface gains `otc: boolean`; the panel edits it.

- [ ] **Step 1: Update the type and the panel**

In `frontend/src/api.ts`, add `otc: boolean;` to `interface FuturesContract` (after `confirmed`).

In `frontend/src/components/FuturesContracts.tsx`:
- Add `otc: false,` to `BLANK_SPEC`.
- Add to `effective()`: `otc: d.otc !== undefined ? d.otc : r.otc,` (the `Draft` type is `Partial<Omit<FuturesContract, "contract_root" | "confirmed">>`, so it picks up `otc` automatically once the interface has it).
- In the contracts table, add an `OTC` column header after the price-convention column, and in each row:

```tsx
                <td>
                  <input
                    type="checkbox"
                    checked={effective(r).otc}
                    onChange={(e) => setDraft(r.contract_root, { otc: e.target.checked })}
                    title="EMIR: tick if this contract is OTC (executed on a non-equivalent venue or bilaterally). Only OTC notional counts toward the clearing thresholds."
                  />
                </td>
```

- If the panel has a new-spec form row, mirror the same checkbox there bound to the new-spec state's `otc` field.

- [ ] **Step 2: Type-check**

Run: `cd frontend && npm run build`
Expected: clean build (the pre-existing chunk-size advisory is fine). If other files construct a `FuturesContract` literal, the compiler will list them — add `otc: false`.

- [ ] **Step 3: Commit**

```bash
git add frontend/src/api.ts frontend/src/components/FuturesContracts.tsx
git commit -m "feat(ui): OTC checkbox on the futures contract panel"
```

---

### Task 9: Frontend — Derivatives page

**Files:**
- Modify: `frontend/src/api.ts` (EMIR types + fetchers)
- Create: `frontend/src/pages/DerivativesPage.tsx`
- Modify: `frontend/src/App.tsx` (nav link + route)
- Modify: `frontend/src/pages/LimitsPage.tsx` (remove DerivativesExposure)

**Interfaces:**
- Consumes: `GET /api/emir`, `PUT /api/emir/kpis/{month}`, `GET /api/emir/export` (Tasks 5–7); `DerivativesExposure` component (unchanged, just remounted); `useFetch`, `eur`, `num`, `pct` helpers.
- Produces: route `/derivatives`; the Limits page no longer shows derivatives exposure.

- [ ] **Step 1: API types and fetchers**

Add to `frontend/src/api.ts` (field-for-field against Task 5's payload):

```ts
export type EmirVerdict = "ok" | "watch" | "breach";
export interface EmirMonthCell {
  month: string;
  snapshot_date: string | null;
  total_eur: number | null;
  otc_eur: number | null;
}
export interface EmirClass {
  class: string;
  label: string;
  threshold_eur: number;
  months: EmirMonthCell[];
  avg_total_eur: number;
  avg_otc_eur: number;
  pct_of_threshold: number;
  verdict: EmirVerdict;
}
export interface EmirMonitors {
  otc_open_contracts: number;
  reconciliation: "not_triggered" | "quarterly" | "weekly" | "daily";
  compression_required: boolean;
}
export interface EmirMarginLine {
  name: string | null;
  currency: string | null;
  valuation_ccy: number | null;
  valuation_eur: number | null;
}
export interface EmirKpi {
  month: string;
  unconfirmed_over_5d: number;
  reconciliation: "done" | "not_done" | "not_applicable";
  disputes: number;
  note: string | null;
}
export interface EmirResponse {
  empty?: boolean;
  dates?: string[];
  date?: string;
  months_present?: number;
  months_total?: number;
  classes?: EmirClass[];
  warnings: string[];
  monitors?: EmirMonitors;
  monitors_note?: string;
  margin?: EmirMarginLine[];
  futures_count?: number;
  kpis?: EmirKpi[];
  otc_note?: string;
}
export const getEmir = (date?: string) =>
  req<EmirResponse>(`/api/emir${date ? `?date=${date}` : ""}`);
export const putEmirKpi = (month: string, body: Omit<EmirKpi, "month">) =>
  req<EmirKpi>(`/api/emir/kpis/${month}`, {
    method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
  });
export const emirExportUrl = "/api/emir/export";
```

- [ ] **Step 2: The page**

Create `frontend/src/pages/DerivativesPage.tsx`:

```tsx
import { Fragment, useState } from "react";
import DerivativesExposure from "../components/DerivativesExposure";
import { ApiError, EmirKpi, emirExportUrl, getEmir, putEmirKpi } from "../api";
import { eur, num, pct } from "../fmt";
import { useFetch } from "../hooks";

const VERDICT_LABEL: Record<string, string> = { ok: "OK", watch: "WATCH", breach: "BREACH" };
const TIER_LABEL: Record<string, string> = {
  not_triggered: "Not triggered — no OTC contracts outstanding",
  quarterly: "Quarterly",
  weekly: "Weekly",
  daily: "Daily",
};
const REC_LABEL: Record<EmirKpi["reconciliation"], string> = {
  done: "Done", not_done: "Not done", not_applicable: "N/A",
};

function VerdictChip({ v }: { v: string }) {
  const cls = v === "ok" ? "pos" : v === "watch" ? "warn-badge" : "neg";
  return <span className={cls}>{VERDICT_LABEL[v] ?? v}</span>;
}

function KpiForm({ onSaved }: { onSaved: () => void }) {
  const [month, setMonth] = useState(() => new Date().toISOString().slice(0, 7));
  const [unconf, setUnconf] = useState(0);
  const [rec, setRec] = useState<EmirKpi["reconciliation"]>("not_applicable");
  const [disputes, setDisputes] = useState(0);
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);

  async function save() {
    setBusy(true);
    setMsg(null);
    try {
      await putEmirKpi(`${month}-01`, {
        unconfirmed_over_5d: unconf,
        reconciliation: rec,
        disputes,
        note: note.trim() || null,
      });
      setMsg(`Saved ${month}.`);
      onSaved();
    } catch (e) {
      const ae = e as ApiError;
      setMsg(`Error: ${ae.detail ?? ae.message}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="controls">
      <label>Month{" "}
        <input type="month" value={month} onChange={(e) => setMonth(e.target.value)} />
      </label>
      <label>Unconfirmed &gt; 5 days{" "}
        <input type="number" min={0} value={unconf} onChange={(e) => setUnconf(Number(e.target.value))} />
      </label>
      <label>Reconciliation{" "}
        <select value={rec} onChange={(e) => setRec(e.target.value as EmirKpi["reconciliation"])}>
          <option value="done">Done</option>
          <option value="not_done">Not done</option>
          <option value="not_applicable">N/A</option>
        </select>
      </label>
      <label>Disputes{" "}
        <input type="number" min={0} value={disputes} onChange={(e) => setDisputes(Number(e.target.value))} />
      </label>
      <label>Note{" "}
        <input type="text" value={note} onChange={(e) => setNote(e.target.value)} />
      </label>
      <button disabled={busy} onClick={() => void save()}>Save month</button>
      {msg && <span className="kpi-sub">{msg}</span>}
    </div>
  );
}

export default function DerivativesPage() {
  const [date, setDate] = useState<string | undefined>(undefined);
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const emir = useFetch(() => getEmir(date), [date]);
  const data = emir.data;

  return (
    <div>
      <h2>Derivatives / EMIR</h2>
      <div className="controls">
        <label>Snapshot:{" "}
          <select value={data?.date ?? ""} onChange={(e) => setDate(e.target.value || undefined)}>
            {(data?.dates ?? []).map((d) => <option key={d} value={d}>{d}</option>)}
          </select>
        </label>
        <a href={emirExportUrl} download>Export evidence workbook</a>
      </div>
      {emir.error && <p className="neg">{emir.error}</p>}
      {!data && !emir.error && <p>Loading…</p>}
      {data?.empty && (
        <div className="card">
          {data.warnings.map((w, i) => <p key={i}>{w}</p>)}
        </div>
      )}
      {data && !data.empty && (
        <>
          <DerivativesExposure date={date} />

          <div className="card">
            <h3>EMIR clearing thresholds</h3>
            <p className="kpi-sub">
              Average of month-end gross notional over the last 12 months
              ({data.months_present} of {data.months_total} months have a snapshot). {data.otc_note}
            </p>
            <table className="tbl">
              <thead>
                <tr>
                  <th>Class</th><th>Avg OTC notional</th><th>Avg total notional</th>
                  <th>Threshold</th><th>% of threshold</th><th>Verdict</th>
                </tr>
              </thead>
              <tbody>
                {(data.classes ?? []).map((c) => (
                  <Fragment key={c.class}>
                    <tr onClick={() => setOpen({ ...open, [c.class]: !open[c.class] })} style={{ cursor: "pointer" }}>
                      <td>{open[c.class] ? "▾" : "▸"} {c.label}</td>
                      <td>{eur(c.avg_otc_eur)}</td>
                      <td>{eur(c.avg_total_eur)}</td>
                      <td>{eur(c.threshold_eur)}</td>
                      <td>{pct(c.pct_of_threshold)}</td>
                      <td><VerdictChip v={c.verdict} /></td>
                    </tr>
                    {open[c.class] && c.months.map((m) => (
                      <tr key={m.month}>
                        <td style={{ paddingLeft: 24, color: "#64748b" }}>
                          {m.month.slice(0, 7)}
                          {m.snapshot_date ? ` (snapshot ${m.snapshot_date})` : ""}
                        </td>
                        <td>{m.otc_eur === null ? "—" : eur(m.otc_eur)}</td>
                        <td>{m.total_eur === null ? "—" : eur(m.total_eur)}</td>
                        <td colSpan={3}>{m.snapshot_date === null && <span className="warn-badge">no snapshot this month</span>}</td>
                      </tr>
                    ))}
                  </Fragment>
                ))}
              </tbody>
            </table>
            {data.warnings.map((w, i) => <span key={i} className="warn-badge">{w}</span>)}
          </div>

          <div className="card">
            <h3>OTC obligations</h3>
            <table className="tbl">
              <tbody>
                <tr><td>Open OTC contracts</td><td>{num(data.monitors!.otc_open_contracts, 0)}</td></tr>
                <tr><td>Portfolio reconciliation</td><td>{TIER_LABEL[data.monitors!.reconciliation]}</td></tr>
                <tr>
                  <td>Compression analysis (≥ 500 contracts)</td>
                  <td>{data.monitors!.compression_required
                    ? <span className="warn-badge">required semiannually</span>
                    : `not required (${data.monitors!.otc_open_contracts} < 500)`}</td>
                </tr>
              </tbody>
            </table>
            <p className="kpi-sub">{data.monitors_note}</p>
          </div>

          <div className="card">
            <h3>Margin accounts</h3>
            <p className="kpi-sub">
              Margin balances from the snapshot, collateralizing {data.futures_count} futures position(s).
            </p>
            {(data.margin ?? []).length === 0 ? <p>No margin accounts in this snapshot.</p> : (
              <table className="tbl">
                <thead>
                  <tr><th>Account</th><th>Currency</th><th>Local value</th><th>EUR value</th></tr>
                </thead>
                <tbody>
                  {(data.margin ?? []).map((m, i) => (
                    <tr key={i}>
                      <td>{m.name ?? "—"}</td>
                      <td>{m.currency ?? "—"}</td>
                      <td>{num(m.valuation_ccy)}</td>
                      <td>{eur(m.valuation_eur)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>

          <div className="card">
            <h3>Monthly EMIR KPIs</h3>
            <p className="kpi-sub">
              Middle-office facts the tool cannot derive: confirmation follow-up, portfolio
              reconciliation and disputes. One record per calendar month, reviewed in the risk committee.
            </p>
            <KpiForm onSaved={emir.reload} />
            {(data.kpis ?? []).length > 0 && (
              <table className="tbl">
                <thead>
                  <tr><th>Month</th><th>Unconfirmed &gt; 5 days</th><th>Reconciliation</th><th>Disputes</th><th>Note</th></tr>
                </thead>
                <tbody>
                  {(data.kpis ?? []).map((k) => (
                    <tr key={k.month}>
                      <td>{k.month.slice(0, 7)}</td>
                      <td>{num(k.unconfirmed_over_5d, 0)}</td>
                      <td>{REC_LABEL[k.reconciliation]}</td>
                      <td>{num(k.disputes, 0)}</td>
                      <td>{k.note ?? "—"}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
        </>
      )}
    </div>
  );
}
```

- [ ] **Step 3: Wire the route, unmount from Limits**

In `frontend/src/App.tsx`: import `DerivativesPage`, add `{ to: "/derivatives", label: "Derivatives" }` to `links` between Limits and Data, add `<Route path="/derivatives" element={<DerivativesPage />} />`.

In `frontend/src/pages/LimitsPage.tsx`: delete the `import DerivativesExposure …` line and the `<DerivativesExposure date={date} />` mount.

- [ ] **Step 4: Type-check and eyeball**

Run: `cd frontend && npm run build`
Expected: clean. Optionally `npm run dev` against a running server to eyeball the page.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/api.ts frontend/src/pages/DerivativesPage.tsx frontend/src/App.tsx frontend/src/pages/LimitsPage.tsx
git commit -m "feat(ui): Derivatives/EMIR page with thresholds, monitors, margin and KPIs"
```

---

### Task 10: README and full verification

**Files:**
- Modify: `README.md`

**Interfaces:** none new — documentation and a final green run.

- [ ] **Step 1: Document the feature**

In `README.md`'s Features section, after the **Derivatives exposure** bullet, add:

```markdown
- **Derivatives / EMIR page**: the derivatives exposure display (moved here from
  Limits) plus EMIR clearing-threshold monitoring — the average of month-end
  gross notional per asset class over the last 12 months (each month uses the
  latest snapshot inside that month; months without one are reported missing,
  and the average says "N of 12"), with total and of-which-OTC lines. Only OTC
  notional counts against the 1/1/3/3/4 bn EUR thresholds (WATCH at 80%);
  contracts default to non-OTC and are flagged on the Data page's contract
  panel (a contract on a non-equivalent third-country venue is OTC even if
  exchange-listed). Also derives the reconciliation-cadence tier and
  compression trigger from the OTC contract count (conservatively assuming a
  single counterparty), shows margin-account balances, records monthly
  middle-office KPIs (confirmations > 5 days, reconciliation status,
  disputes), and exports the full calculation as an `.xlsx` evidence file for
  archiving per the EMIR procedure.
```

Update the **Derivatives exposure** bullet's first line to say the display now lives on the Derivatives page (it currently implies Limits).

- [ ] **Step 2: Full verification**

Run, and require exit code 0 on each (check `$LASTEXITCODE` explicitly — do not trust the absence of red text):

```powershell
cargo test --workspace
cd frontend; npm run build; npm run lint; cd ..
```

Expected: everything green, no new warnings.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: Derivatives/EMIR page in README"
```

---

## Self-review notes (already applied)

- Spec coverage: thresholds (Tasks 3/5), OTC flag (1/8), exposure move (9), monitors (3/5), margin (5/9), KPIs (2/6/9), evidence export (4/7), README (10). The spec's "expandable to the 12 monthly figures" is Task 9's Fragment rows; the "N of 12" caveat is in both the page copy and the workbook.
- The `otc` field is REQUIRED in `ContractBody` on purpose (full-row upsert; a serde default would silently clear the flag on every save from a stale client). Task 1 and Task 8 must land in this order.
- `thresholds()` divides by `months_present`, not 12 — matches the approved "average over months available" ruling.
- `month_window` never lets an earlier month's snapshot stand in for a missing month, and caps the anchor month at the anchor date; both behaviors are pinned by tests.
- Verdict on the OTC average only; the total line is display-only. Pinned by `emir_report_on_sample` (unconfirmed specs make totals provisional yet verdicts stay OK because OTC is zero).
