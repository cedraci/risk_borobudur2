# Universal Ingest + CACEIS Adapter Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Formalize a universal ingest contract (`UniversalBatch`) fed by per-source adapters, add the CACEIS CSV adapter (HISINVLUX positions + HISTOVLLUX NAV series), auto-route self-identifying files to portfolios via a `portfolio_codes` mapping, and derive dividends from CPON receivable deltas.

**Architecture:** The existing NAV Recap parser becomes adapter #1 behind the new contract; CACEIS is adapter #2 with declarative column-transposition constants. `db::repo::import_batch` generalizes `import_workbook` (which stays as a thin wrapper). The upload endpoint accepts multiple files and returns per-file results.

**Tech Stack:** Rust workspace (axum 0.8, sqlx, embedded PostgreSQL 17), React+TS frontend (no test runner — `npm run build` is the gate).

**Spec:** `docs/superpowers/specs/2026-08-11-universal-ingest-caceis-design.md`

## Global Constraints

- Asset-type vocabulary is CLOSED: `Action`, `Fonds`, `Obligation`, `Future`, `Cash Acc`, `Margin Acc`, `Dividendes`, `Frais provisionnés`, `Provisions ordres`. Adapters map onto it; an unmappable CACEIS code drops the row with a warning (never a silent "Other"). The NAV Recap keeps its existing strict cell-error rejection.
- `weight` is a FRACTION in the universal model (0.0101, not 1.01). `fx_rate` is EUR per unit of local currency (`valuation_eur / valuation_ccy` convention).
- Ref hints (`country_of_risk`, `region`, `ticker`) fill `instrument_refs` only where the column is currently NULL — never overwrite.
- Sources for `portfolio_codes`: the string `"caceis"` (lowercase) is the only source value in this phase.
- Signal, don't hide: dropped rows, TNA drift, unknown codes all produce explicit warnings/errors naming the row or value.
- No shared server test harness: each `crates/server/tests/api_*.rs` file inlines its own helpers.
- Windows/PowerShell environment. The dev server must be STOPPED before `cargo test`/`cargo build` (it locks `target\debug\server.exe`). Embedded-PG tests spin temporary instances; if a run dies and the next can't start PG: `& "$env:LOCALAPPDATA\borobudur-risk\pg-install\17.10.0\bin\pg_ctl.exe" -D "$env:LOCALAPPDATA\borobudur-risk\pg-data" -m fast stop`.
- Migrations are picked up by `sqlx::migrate!("./migrations")` glob — no registration; run `cargo clean -p db` if a new migration seems invisible.
- Frontend: `import type` for type-only imports (verbatimModuleSyntax); CSS classes `.card/.tbl/.warn-badge/.pos/.neg/.kpi-sub` only; `useFetch` deps are primitives.
- Commit trailer on every commit: `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`
- The untracked repo-root sample files (`HISINVLUX_*.csv`, `HISTOVLLUX_*.csv`, `INVXDVLUX_*.csv`, `Glossary GP CSV Headers.xlsx`, `07-08-2026 - Borobudur - NAV Recap.xlsx`, `*.docx`) must NEVER be committed. Only the trimmed fixtures under `crates/ingest/tests/fixtures/` are committed.

---

### Task 1: Migration 0009 + portfolio_codes repo functions

**Files:**
- Create: `crates/db/migrations/0009_universal_ingest.sql`
- Modify: `crates/db/src/repo.rs` (append a "portfolio codes" section after the existing portfolio section)
- Test: `crates/db/tests/portfolio_codes.rs`

**Interfaces:**
- Consumes: `portfolios` table (id 1 = Borobudur seeded by 0008).
- Produces: `PortfolioCode` struct; `portfolio_codes_for`, `portfolio_codes_replace`, `portfolio_by_code` used by Tasks 6-7. `dividends.derived` column used by Task 5.

- [ ] **Step 1: Write the migration**

`crates/db/migrations/0009_universal_ingest.sql`:

```sql
-- External identifiers used to auto-route self-identifying uploads
-- (e.g. CACEIS fund code 165878) to a portfolio. One code maps to exactly
-- one portfolio per source; a portfolio may hold several codes.
CREATE TABLE portfolio_codes (
  portfolio_id BIGINT NOT NULL REFERENCES portfolios(id),
  source       TEXT NOT NULL,
  code         TEXT NOT NULL,
  PRIMARY KEY (source, code)
);

-- Dividend rows derived from CACEIS CPON receivable deltas are flagged so
-- the derivation can delete-and-rebuild its own rows without touching
-- explicit (file-sourced) journal entries.
ALTER TABLE dividends ADD COLUMN derived BOOLEAN NOT NULL DEFAULT false;
```

- [ ] **Step 2: Add repo functions**

Append to `crates/db/src/repo.rs` (after the portfolio section):

```rust
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
```

- [ ] **Step 3: Write the test**

`crates/db/tests/portfolio_codes.rs`:

```rust
use db::repo;

#[tokio::test]
async fn codes_roundtrip_and_uniqueness() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    // Borobudur is portfolio 1 (seeded by 0008); create a second portfolio.
    let p2 = repo::portfolio_create(&pool, "Mandat A", "mandate").await.unwrap();

    repo::portfolio_codes_replace(&pool, 1, &[("caceis".into(), "165878".into())]).await.unwrap();
    assert_eq!(repo::portfolio_by_code(&pool, "caceis", "165878").await.unwrap(), Some(1));
    assert_eq!(repo::portfolio_by_code(&pool, "caceis", "999999").await.unwrap(), None);

    // Replace removes what the new set omits.
    repo::portfolio_codes_replace(&pool, 1, &[("caceis".into(), "111111".into())]).await.unwrap();
    assert_eq!(repo::portfolio_by_code(&pool, "caceis", "165878").await.unwrap(), None);
    let codes = repo::portfolio_codes_for(&pool, 1).await.unwrap();
    assert_eq!(codes.len(), 1);
    assert_eq!(codes[0].code, "111111");

    // A code claimed by portfolio 1 cannot also be claimed by portfolio 2.
    let err = repo::portfolio_codes_replace(&pool, p2.id, &[("caceis".into(), "111111".into())]).await;
    assert!(err.is_err(), "duplicate (source, code) across portfolios must fail");

    // dividends.derived exists and defaults false.
    let derived: bool = sqlx::query_scalar(
        "INSERT INTO dividends (portfolio_id, provision_date, issuer, amount, currency)
         VALUES (1, '2026-08-07', 'X', 1, 'EUR') RETURNING derived")
        .fetch_one(&pool).await.unwrap();
    assert!(!derived);

    pool.close().await;
    edb.stop().await;
}
```

Note: `portfolio_create` exists in `crates/db/src/repo.rs` (Phase 1); check its exact signature (`pool, name, kind`) and adjust the call if it differs.

- [ ] **Step 4: Run and verify**

Run: `cargo test -p db --test portfolio_codes`
Expected: PASS. Then `cargo test -p db` — all green.

- [ ] **Step 5: Commit**

```bash
git add crates/db/migrations/0009_universal_ingest.sql crates/db/src/repo.rs crates/db/tests/portfolio_codes.rs
git commit -m "feat(db): portfolio_codes routing table + dividends.derived flag"
```

---

### Task 2: UniversalBatch contract + NAV Recap adapter + import_batch

**Files:**
- Create: `crates/ingest/src/adapter.rs`
- Modify: `crates/ingest/src/lib.rs` (add `pub mod adapter;`)
- Modify: `crates/db/src/repo.rs` (`import_batch` generalizing `import_workbook`; `pam_warnings` signature)
- Test: existing suites (regression gate) + `crates/db/tests/import_batch.rs`

**Interfaces:**
- Consumes: `ParsedWorkbook`, `PositionRow`, `NavHistoryRow`, `DividendRow`, `OperationRow`, `RowError`, `ParseFailure` from `ingest`; `import_workbook` internals.
- Produces: `ingest::adapter::{UniversalBatch, Snapshot, RefHint, FileKind, Identification, DetectError, to_batch}` and `db::repo::import_batch(pool, portfolio_id, filename, sha256, &UniversalBatch) -> ImportOutcome`. Tasks 3-6 build on these exact names.

- [ ] **Step 1: Write `crates/ingest/src/adapter.rs`**

```rust
//! The universal ingest contract. Every source adapter produces a
//! `UniversalBatch`; the import pipeline consumes nothing else.

use crate::{DividendRow, NavHistoryRow, OperationRow, ParsedWorkbook, PositionRow};
use chrono::NaiveDate;

#[derive(Debug)]
pub struct Snapshot {
    pub nav_date: NaiveDate,
    pub positions: Vec<PositionRow>,
}

/// Optional reference enrichment a file happens to carry. Applied to the
/// shared `instrument_refs` only where the target column is NULL.
#[derive(Debug, Clone)]
pub struct RefHint {
    pub isin: String,
    pub country_of_risk: Option<String>,
    pub region: Option<String>,
    pub ticker: Option<String>,
}

#[derive(Debug)]
pub struct UniversalBatch {
    /// The file's own NAV date — keys the `imports` row.
    pub primary_date: NaiveDate,
    pub nav_points: Vec<NavHistoryRow>,
    pub snapshots: Vec<Snapshot>,
    /// `Some` = this file carries the authoritative dividend journal
    /// (replace-if-latest, the existing NAV Recap rule). `None` = the
    /// journal is untouched by this import.
    pub dividends: Option<Vec<DividendRow>>,
    pub operations: Option<Vec<OperationRow>>,
    pub ref_hints: Vec<RefHint>,
    /// Row-level anomalies that dropped rows without rejecting the file.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind { NavRecap, CaceisHisinv, CaceisHistovl }

#[derive(Debug)]
pub struct Identification {
    pub kind: FileKind,
    /// `(source, code)` for `portfolio_codes` routing, e.g.
    /// `("caceis", "165878")`. `None` = the file cannot identify its
    /// portfolio (NAV Recap) and lands in the selected one.
    pub fund_code: Option<(String, String)>,
}

#[derive(Debug, thiserror::Error)]
pub enum DetectError {
    #[error("unrecognized file format: {0:?}. Supported: NAV Recap (.xlsx), CACEIS HISINVLUX / HISTOVLLUX (.csv)")]
    Unrecognized(String),
    #[error("{0}")]
    Rejected(String),
}

/// NAV Recap → universal batch. The recap's own NAV row joins the history
/// (the upsert dedupes by date, matching the old import path exactly).
pub fn to_batch(wb: ParsedWorkbook) -> UniversalBatch {
    let mut nav_points = wb.nav_history;
    nav_points.push(NavHistoryRow { date: wb.nav_date, aum: wb.aum, shares: wb.shares, nav: wb.nav });
    UniversalBatch {
        primary_date: wb.nav_date,
        nav_points,
        snapshots: vec![Snapshot { nav_date: wb.nav_date, positions: wb.positions }],
        dividends: Some(wb.dividends),
        operations: Some(wb.operations),
        ref_hints: Vec::new(),
        warnings: Vec::new(),
    }
}
```

(`detect` and `parse` dispatchers arrive in Task 4 with the CACEIS side; this task only establishes the types and the NAV Recap conversion.)

Add to `crates/ingest/src/lib.rs` line 1 area: `pub mod adapter;`

- [ ] **Step 2: Generalize `import_workbook` into `import_batch`**

In `crates/db/src/repo.rs`, add `import_batch` and shrink `import_workbook` to a wrapper. The body is the existing `import_workbook` logic re-shaped; preserve every current behavior:

```rust
pub async fn import_batch(pool: &PgPool, portfolio_id: i64, filename: &str, sha256: &str, b: &ingest::adapter::UniversalBatch) -> anyhow::Result<ImportOutcome> {
    let all_positions = || b.snapshots.iter().flat_map(|s| s.positions.iter());

    if let Some((id,)) = sqlx::query_as::<_, (i64,)>("SELECT id FROM imports WHERE portfolio_id = $1 AND sha256 = $2")
        .bind(portfolio_id).bind(sha256).fetch_optional(pool).await?
    {
        // Duplicate: nothing re-ingested, but futures spec seeding still runs
        // (same rationale as before — repair path for pre-futures databases).
        let mut tx = pool.begin().await?;
        let positions: Vec<ingest::PositionRow> = all_positions().cloned().collect();
        let warnings = seed_futures_contracts(&mut tx, &positions).await?;
        tx.commit().await?;
        return Ok(ImportOutcome {
            import_id: id, duplicate: true, nav_rows: 0, positions: 0,
            dividends: 0, operations: 0, div_ops_replaced: false, warnings,
        });
    }

    let mut tx = pool.begin().await?;

    let prev_latest: Option<NaiveDate> =
        sqlx::query_scalar("SELECT max(nav_date) FROM imports WHERE portfolio_id = $1")
            .bind(portfolio_id).fetch_one(&mut *tx).await?;
    let has_div_ops = b.dividends.is_some() || b.operations.is_some();
    let replace_div_ops = has_div_ops && prev_latest.is_none_or(|d| b.primary_date >= d);

    let nav_rows = b.nav_points.len();
    let n_positions: usize = b.snapshots.iter().map(|s| s.positions.len()).sum();
    let n_div = b.dividends.as_ref().map_or(0, |d| d.len());
    let n_ops = b.operations.as_ref().map_or(0, |o| o.len());
    let mut row_counts = serde_json::json!({
        "nav_rows": nav_rows, "positions": n_positions,
        "dividends": if replace_div_ops { n_div } else { 0 },
        "operations": if replace_div_ops { n_ops } else { 0 },
    });
    if !b.warnings.is_empty() {
        row_counts["warnings"] = serde_json::json!(b.warnings);
    }
    let (import_id,): (i64,) = sqlx::query_as(
        "INSERT INTO imports (portfolio_id, filename, sha256, nav_date, row_counts) VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(portfolio_id).bind(filename).bind(sha256).bind(b.primary_date).bind(&row_counts)
    .fetch_one(&mut *tx).await?;

    const UPSERT_NAV: &str = "INSERT INTO nav_history (portfolio_id, date, aum, shares, nav) VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (portfolio_id, date) DO UPDATE SET aum = EXCLUDED.aum, shares = EXCLUDED.shares, nav = EXCLUDED.nav";
    for r in &b.nav_points {
        sqlx::query(UPSERT_NAV).bind(portfolio_id).bind(r.date).bind(r.aum).bind(r.shares).bind(r.nav)
            .execute(&mut *tx).await?;
    }

    for snap in &b.snapshots {
        sqlx::query("DELETE FROM position_snapshots WHERE portfolio_id = $1 AND nav_date = $2")
            .bind(portfolio_id).bind(snap.nav_date).execute(&mut *tx).await?;
        for p in &snap.positions {
            sqlx::query(
                "INSERT INTO position_snapshots (portfolio_id, nav_date, import_id, asset_type, isin, name, currency, quantity, avg_cost, price, valuation_ccy, accrued_interest, fx_rate, valuation_eur, weight, ticker)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
            )
            .bind(portfolio_id).bind(snap.nav_date).bind(import_id).bind(&p.asset_type).bind(&p.isin).bind(&p.name)
            .bind(&p.currency).bind(p.quantity).bind(p.avg_cost).bind(p.price).bind(p.valuation_ccy)
            .bind(p.accrued_interest).bind(p.fx_rate).bind(p.valuation_eur).bind(p.weight).bind(&p.ticker)
            .execute(&mut *tx).await?;
        }
    }

    // Bond statics from names — COALESCE keeps existing values (unchanged logic,
    // now over every snapshot's positions).
    for p in all_positions() {
        if p.asset_type != "Obligation" { continue; }
        let Some(name) = &p.name else { continue };
        let Some(bs) = ingest::parse_bond_statics(name, p.currency.as_deref()) else { continue };
        sqlx::query(
            "INSERT INTO instrument_refs (code, bond_coupon_pct, bond_maturity, bond_coupon_freq)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (code) DO UPDATE SET
               bond_coupon_pct = COALESCE(instrument_refs.bond_coupon_pct, EXCLUDED.bond_coupon_pct),
               bond_maturity = COALESCE(instrument_refs.bond_maturity, EXCLUDED.bond_maturity),
               bond_coupon_freq = COALESCE(instrument_refs.bond_coupon_freq, EXCLUDED.bond_coupon_freq),
               updated_at = now()",
        )
        .bind(&p.isin).bind(bs.coupon_pct).bind(bs.maturity).bind(bs.coupon_freq)
        .execute(&mut *tx).await?;
    }

    // Reference hints: fill NULLs only — Bloomberg data is never overwritten.
    for h in &b.ref_hints {
        sqlx::query(
            "INSERT INTO instrument_refs (code, country_of_risk, region, ticker)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (code) DO UPDATE SET
               country_of_risk = COALESCE(instrument_refs.country_of_risk, EXCLUDED.country_of_risk),
               region          = COALESCE(instrument_refs.region,          EXCLUDED.region),
               ticker          = COALESCE(instrument_refs.ticker,          EXCLUDED.ticker),
               updated_at = now()",
        )
        .bind(&h.isin).bind(&h.country_of_risk).bind(&h.region).bind(&h.ticker)
        .execute(&mut *tx).await?;
    }

    let positions: Vec<ingest::PositionRow> = all_positions().cloned().collect();
    let mut warnings = b.warnings.clone();
    warnings.extend(seed_futures_contracts(&mut tx, &positions).await?);
    if let Some(ops) = &b.operations {
        warnings.extend(pam_warnings(&positions, ops));
    }

    if replace_div_ops {
        sqlx::query("DELETE FROM dividends WHERE portfolio_id = $1").bind(portfolio_id).execute(&mut *tx).await?;
        for r in b.dividends.as_deref().unwrap_or(&[]) {
            sqlx::query("INSERT INTO dividends (portfolio_id, provision_date, payment_date, issuer, amount, currency) VALUES ($1, $2, $3, $4, $5, $6)")
                .bind(portfolio_id).bind(r.provision_date).bind(r.payment_date).bind(&r.issuer).bind(r.amount).bind(&r.currency)
                .execute(&mut *tx).await?;
        }
        sqlx::query("DELETE FROM operations WHERE portfolio_id = $1").bind(portfolio_id).execute(&mut *tx).await?;
        for r in b.operations.as_deref().unwrap_or(&[]) {
            sqlx::query(
                "INSERT INTO operations (portfolio_id, trade_date, side, ticker, isin, name, currency, quantity, price, gross_amount, fees, net_price, net_amount)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
            )
            .bind(portfolio_id).bind(r.trade_date).bind(&r.side).bind(&r.ticker).bind(&r.isin).bind(&r.name)
            .bind(&r.currency).bind(r.quantity).bind(r.price).bind(r.gross_amount).bind(r.fees)
            .bind(r.net_price).bind(r.net_amount)
            .execute(&mut *tx).await?;
        }
    }

    // TNA cross-check: for every date this batch touched where BOTH a
    // snapshot and a NAV point now exist, the position sum must match AUM
    // within 0.1% — catches truncated position files and stale NAVs.
    let mut check_dates: Vec<NaiveDate> = b.snapshots.iter().map(|s| s.nav_date)
        .chain(b.nav_points.iter().map(|n| n.date)).collect();
    check_dates.sort();
    check_dates.dedup();
    let drift: Vec<(NaiveDate, f64, f64)> = sqlx::query_as(
        "SELECT n.date, n.aum::float8, s.total
         FROM nav_history n
         JOIN (SELECT nav_date, SUM(valuation_eur)::float8 AS total
               FROM position_snapshots WHERE portfolio_id = $1 GROUP BY nav_date) s
           ON s.nav_date = n.date
         WHERE n.portfolio_id = $1 AND n.date = ANY($2)
           AND n.aum <> 0 AND abs(s.total - n.aum::float8) / abs(n.aum::float8) > 0.001",
    )
    .bind(portfolio_id).bind(&check_dates)
    .fetch_all(&mut *tx).await?;
    for (d, aum, total) in drift {
        warnings.push(format!(
            "TNA cross-check {d}: positions sum to {total:.2} EUR but the NAV file says {aum:.2} EUR ({:+.2}%)",
            (total - aum) / aum * 100.0
        ));
    }

    tx.commit().await?;
    Ok(ImportOutcome {
        import_id,
        duplicate: false,
        nav_rows,
        positions: n_positions,
        dividends: if replace_div_ops { n_div } else { 0 },
        operations: if replace_div_ops { n_ops } else { 0 },
        div_ops_replaced: replace_div_ops,
        warnings,
    })
}

pub async fn import_workbook(pool: &PgPool, portfolio_id: i64, filename: &str, sha256: &str, wb: &ParsedWorkbook) -> anyhow::Result<ImportOutcome> {
    // Clone-into-batch: ParsedWorkbook fields are all Clone.
    let b = ingest::adapter::to_batch(ParsedWorkbook {
        nav_date: wb.nav_date, aum: wb.aum, shares: wb.shares, nav: wb.nav,
        positions: wb.positions.clone(), nav_history: wb.nav_history.clone(),
        dividends: wb.dividends.clone(), operations: wb.operations.clone(),
    });
    import_batch(pool, portfolio_id, filename, sha256, &b).await
}
```

Adapt `pam_warnings` to `fn pam_warnings(positions: &[ingest::PositionRow], operations: &[ingest::OperationRow]) -> Vec<String>` — same body, `wb.operations` → `operations`, `wb.positions` → `positions`.

Adapt `seed_futures_contracts` only if its current signature takes `&[PositionRow]` slices already (it does — it is called with `&wb.positions`); pass the collected `positions` vec.

One behavioral nuance to preserve exactly: the old `nav_rows` count was `wb.nav_history.len() + 1` (history + the file's own row); `to_batch` pushes the file's own row into `nav_points`, so `b.nav_points.len()` equals the same number. Do NOT add another `+ 1`.

- [ ] **Step 3: Write the batch-level test**

`crates/db/tests/import_batch.rs`:

```rust
use chrono::NaiveDate;
use db::repo;
use ingest::adapter::{Snapshot, UniversalBatch};
use ingest::{NavHistoryRow, PositionRow};

fn d(s: &str) -> NaiveDate { s.parse().unwrap() }

fn pos(asset_type: &str, isin: &str, valuation_eur: f64) -> PositionRow {
    PositionRow {
        asset_type: asset_type.into(), isin: isin.into(), name: Some(isin.into()),
        currency: Some("EUR".into()), quantity: Some(1.0), avg_cost: None, price: None,
        valuation_ccy: Some(valuation_eur), accrued_interest: None, fx_rate: Some(1.0),
        valuation_eur: Some(valuation_eur), weight: None, ticker: None,
    }
}

#[tokio::test]
async fn batch_without_div_ops_leaves_journals_untouched_and_checks_tna() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    // Seed an explicit dividend so we can prove a journal-less batch leaves it alone.
    sqlx::query("INSERT INTO dividends (portfolio_id, provision_date, issuer, amount, currency) VALUES (1, '2026-08-01', 'SEED', 10, 'EUR')")
        .execute(&pool).await.unwrap();

    // Positions sum 1000, NAV point says 1500 -> TNA warning expected.
    let b = UniversalBatch {
        primary_date: d("2026-08-07"),
        nav_points: vec![NavHistoryRow { date: d("2026-08-07"), aum: 1500.0, shares: 10.0, nav: 150.0 }],
        snapshots: vec![Snapshot { nav_date: d("2026-08-07"), positions: vec![pos("Action", "FR0000000001", 1000.0)] }],
        dividends: None,
        operations: None,
        ref_hints: vec![ingest::adapter::RefHint {
            isin: "FR0000000001".into(),
            country_of_risk: Some("France".into()), region: Some("Europe".into()), ticker: Some("AAA FP".into()),
        }],
        warnings: vec!["row 5: dropped".into()],
    };
    let out = repo::import_batch(&pool, 1, "f.csv", "sha-batch-1", &b).await.unwrap();

    assert!(!out.duplicate);
    assert_eq!(out.nav_rows, 1);
    assert_eq!(out.positions, 1);
    assert_eq!(out.dividends, 0);
    assert!(!out.div_ops_replaced);
    assert!(out.warnings.iter().any(|w| w.contains("TNA cross-check")), "{:?}", out.warnings);
    assert!(out.warnings.iter().any(|w| w.contains("dropped")), "{:?}", out.warnings);

    // Explicit dividend survived a journal-less import.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM dividends WHERE portfolio_id = 1")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(n, 1);

    // Ref hint filled NULL columns.
    let (country, ticker): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT country_of_risk, ticker FROM instrument_refs WHERE code = 'FR0000000001'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(country.as_deref(), Some("France"));
    assert_eq!(ticker.as_deref(), Some("AAA FP"));

    // A second batch must NOT overwrite: hint with a different country is ignored.
    let b2 = UniversalBatch {
        primary_date: d("2026-08-08"),
        nav_points: vec![NavHistoryRow { date: d("2026-08-08"), aum: 1000.0, shares: 10.0, nav: 100.0 }],
        snapshots: vec![Snapshot { nav_date: d("2026-08-08"), positions: vec![pos("Action", "FR0000000001", 1000.0)] }],
        dividends: None, operations: None,
        ref_hints: vec![ingest::adapter::RefHint {
            isin: "FR0000000001".into(), country_of_risk: Some("Germany".into()), region: None, ticker: None,
        }],
        warnings: vec![],
    };
    repo::import_batch(&pool, 1, "f2.csv", "sha-batch-2", &b2).await.unwrap();
    let country2: Option<String> = sqlx::query_scalar("SELECT country_of_risk FROM instrument_refs WHERE code = 'FR0000000001'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(country2.as_deref(), Some("France"), "hint must never overwrite");

    pool.close().await;
    edb.stop().await;
}
```

- [ ] **Step 4: Run the full workspace suite (regression gate)**

Run: `cargo test`
Expected: ALL green — in particular `crates/db/tests/import_workbook.rs` and every `crates/server/tests/api_*.rs` must pass unchanged, proving the NAV Recap path behaves identically through the wrapper.

- [ ] **Step 5: Commit**

```bash
git add crates/ingest/src/adapter.rs crates/ingest/src/lib.rs crates/db/src/repo.rs crates/db/tests/import_batch.rs
git commit -m "feat(ingest+db): UniversalBatch contract; import_batch generalizes import_workbook"
```

---

### Task 3: CACEIS HISINVLUX parser + fixtures

**Files:**
- Create: `crates/ingest/src/caceis.rs`
- Modify: `crates/ingest/src/lib.rs` (add `pub mod caceis;`)
- Create: `crates/ingest/tests/fixtures/caceis_hisinv.csv` (extracted from the untracked repo-root sample)
- Test: `crates/ingest/tests/caceis.rs`

**Interfaces:**
- Consumes: `PositionRow`, `ParseFailure`, `RowError`, `adapter::{UniversalBatch, Snapshot, RefHint}`.
- Produces: `caceis::SOURCE: &str = "caceis"`, `caceis::parse_hisinv(filename, bytes) -> Result<UniversalBatch, ParseFailure>`, `caceis::filename_meta(filename) -> Option<(FileKind-agnostic fund_code String, NaiveDate)>` helper used by Task 4's `detect`.

- [ ] **Step 1: Extract the fixture from the real sample**

From the repo root (Git Bash). The grep patterns pick 13 representative rows: 2 futures (EUR index + JPY currency), 2 equities (EUR + GBP), 1 fund, 1 ETC (13900), the 13101 bond-typed row, 2 cash accounts (EUR + CHF), 1 JPY margin account, 1 fee provision, 2 CPON receivables (EUR + GBP):

```bash
grep -E ';(CFIN2608|RYCU2609) +;|;(AT000000STR1|BMG4209G2077|FR0010599399|DE000A1EK0G3|FR0000121485|GB0009895292);|;(BK001EUR|BK001CHF|DG1C7JPY|FP201EUR) +;' \
  HISINVLUX_165878_20260807_20260810130151.csv > crates/ingest/tests/fixtures/caceis_hisinv.csv
grep ';13101 ' HISINVLUX_165878_20260807_20260810130151.csv >> crates/ingest/tests/fixtures/caceis_hisinv.csv
wc -l crates/ingest/tests/fixtures/caceis_hisinv.csv
```

Expected: 14-16 lines (FR0000121485 and GB0009895292 may each match both a VMOB and a CPON row — that is intended: it exercises the legitimate same-ISIN-twice case). Inspect the file; confirm at least one line each with `;CPON;`, `;FUTU;`, `;TRES;`, `;VMOB;`. If `grep ';13101 '` matched nothing, check the exact spacing with `grep -c '13101' <sample>` and adjust (the GP3 column is space-padded to 12 chars).

- [ ] **Step 2: Write the failing test**

`crates/ingest/tests/caceis.rs`:

```rust
use ingest::caceis;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/caceis_hisinv.csv");
const FNAME: &str = "HISINVLUX_165878_20260807_20260810130151.csv";

fn batch() -> ingest::adapter::UniversalBatch {
    let bytes = std::fs::read(FIXTURE).unwrap();
    caceis::parse_hisinv(FNAME, &bytes).expect("fixture parses")
}

#[test]
fn transposes_an_equity_row_exactly() {
    let b = batch();
    assert_eq!(b.primary_date, chrono::NaiveDate::from_ymd_opt(2026, 8, 7).unwrap());
    assert_eq!(b.snapshots.len(), 1);
    let s = &b.snapshots[0];
    let p = s.positions.iter().find(|p| p.isin == "AT000000STR1").expect("STRABAG present");
    assert_eq!(p.asset_type, "Action");
    assert_eq!(p.name.as_deref(), Some("STRABAG SE-BR"));
    assert_eq!(p.currency.as_deref(), Some("EUR"));
    assert_eq!(p.quantity, Some(3400.0));
    assert_eq!(p.price, Some(85.5));
    assert_eq!(p.avg_cost, Some(91.0));
    assert_eq!(p.valuation_eur, Some(290700.0));
    assert_eq!(p.valuation_ccy, Some(290700.0));
    assert_eq!(p.fx_rate, Some(1.0));
    assert!((p.weight.unwrap() - 0.0101).abs() < 1e-9, "weight is a fraction: {:?}", p.weight);
    assert_eq!(p.ticker.as_deref(), Some("STR AV"));
}

#[test]
fn transposes_fx_futures_cash_and_receivables() {
    let b = batch();
    let s = &b.snapshots[0];

    // GBP equity: fx_rate = EUR per GBP = valuation_eur / valuation_ccy.
    let gkp = s.positions.iter().find(|p| p.isin == "BMG4209G2077").unwrap();
    assert_eq!(gkp.asset_type, "Action");
    assert!((gkp.fx_rate.unwrap() - 306425.21 / 262468.51).abs() < 1e-9);

    // JPY currency future: mark-to-market in the valuation column, ticker kept.
    let fut = s.positions.iter().find(|p| p.isin == "RYCU2609").unwrap();
    assert_eq!(fut.asset_type, "Future");
    assert_eq!(fut.quantity, Some(-7.0));
    assert_eq!(fut.valuation_eur, Some(10453.76));
    assert_eq!(fut.ticker.as_deref(), Some("RYU6 Curncy"));

    // Cash account: price is the conversion rate in the file -> None here.
    let cash = s.positions.iter().find(|p| p.isin == "BK001CHF").unwrap();
    assert_eq!(cash.asset_type, "Cash Acc");
    assert_eq!(cash.price, None);
    assert_eq!(cash.quantity, Some(125894.78));
    assert_eq!(cash.valuation_eur, Some(134805.42));

    // Margin account and fee provision map to their NAV Recap labels.
    assert_eq!(s.positions.iter().find(|p| p.isin == "DG1C7JPY").unwrap().asset_type, "Margin Acc");
    assert_eq!(s.positions.iter().find(|p| p.isin == "FP201EUR").unwrap().asset_type, "Frais provisionnés");

    // CPON receivable -> Dividendes, GBP local value preserved.
    let cpon = s.positions.iter().find(|p| p.isin == "GB0009895292" && p.asset_type == "Dividendes").unwrap();
    assert_eq!(cpon.currency.as_deref(), Some("GBP"));
    assert_eq!(cpon.valuation_ccy, Some(636.8));
    assert_eq!(cpon.valuation_eur, Some(743.45));

    // The fund and the 13900 ETC.
    assert_eq!(s.positions.iter().find(|p| p.isin == "FR0010599399").unwrap().asset_type, "Fonds");
    assert_eq!(s.positions.iter().find(|p| p.isin == "DE000A1EK0G3").unwrap().asset_type, "Obligation");
}

#[test]
fn emits_ref_hints_for_securities_only() {
    let b = batch();
    let strabag = b.ref_hints.iter().find(|h| h.isin == "AT000000STR1").expect("hint for STRABAG");
    assert_eq!(strabag.country_of_risk.as_deref(), Some("Germany")); // risk country col 41 = DEU
    assert_eq!(strabag.region.as_deref(), Some("Europe"));
    assert_eq!(strabag.ticker.as_deref(), Some("STR AV"));
    // No hints for cash/margin/CPON rows.
    assert!(!b.ref_hints.iter().any(|h| h.isin.starts_with("BK001") || h.isin.starts_with("DG1C7")));
    // The batch carries no journals.
    assert!(b.dividends.is_none() && b.operations.is_none());
    assert!(b.nav_points.is_empty());
}

#[test]
fn filename_and_row_disagreement_is_a_file_error() {
    let bytes = std::fs::read(FIXTURE).unwrap();
    let err = caceis::parse_hisinv("HISINVLUX_165878_20991231_20260810130151.csv", &bytes);
    assert!(matches!(err, Err(ingest::ParseFailure::Workbook(_))), "date mismatch must reject the file");
    let err2 = caceis::parse_hisinv("HISINVLUX_999999_20260807_20260810130151.csv", &bytes);
    assert!(matches!(err2, Err(ingest::ParseFailure::Workbook(_))), "fund-code mismatch must reject the file");
}

#[test]
fn unmappable_asset_code_drops_the_row_with_a_warning() {
    let bytes = std::fs::read(FIXTURE).unwrap();
    let text: String = bytes.iter().map(|&b| b as char).collect();
    // Corrupt one row's CATVAL to an unknown category.
    let bad = text.replacen(";VMOB;", ";XXXX;", 1);
    let bad_bytes: Vec<u8> = bad.chars().map(|c| c as u8).collect();
    let b = caceis::parse_hisinv(FNAME, &bad_bytes).unwrap();
    assert!(b.warnings.iter().any(|w| w.contains("XXXX")), "warning names the code: {:?}", b.warnings);
    let total: usize = b.snapshots[0].positions.len();
    let full = batch().snapshots[0].positions.len();
    assert_eq!(total, full - 1, "exactly the corrupted row dropped");
}
```

Run: `cargo test -p ingest --test caceis`
Expected: FAIL — `caceis` module does not exist.

- [ ] **Step 3: Write `crates/ingest/src/caceis.rs`**

```rust
//! CACEIS Bank Luxembourg adapter. Files are semicolon-delimited,
//! headerless, Latin-1, dates `yyyymmdd`, numbers space-padded with
//! trailing dots ("8336.23333333", "-12."). Column indices come from the
//! depositary's header glossary ("Glossary GP CSV Headers.xlsx") and are
//! the single place to edit if CACEIS changes the layout.

use crate::adapter::{RefHint, Snapshot, UniversalBatch};
use crate::{ParseFailure, PositionRow};
use chrono::NaiveDate;

pub const SOURCE: &str = "caceis";

// HISINVLUX columns (0-based).
const H_NAV_DATE: usize = 0;
const H_FUND_CODE: usize = 3;
const H_CATVAL: usize = 5;      // VMOB / FUTU / TRES / CPON
const H_INSTR_CODE: usize = 6;  // fallback code when no ISIN (futures, cash accounts)
const H_NAME: usize = 8;
const H_ASSET_CCY: usize = 9;
const H_GP3: usize = 16;        // detail type: 11101, 12400, 18120, COMPTE, MARGES, FP...
const H_QUANTITY: usize = 25;
const H_MARKET_PRICE: usize = 28;
const H_UNIT_COST: usize = 30;
const H_MV_FUND_CCY: usize = 32;
const H_ACCRUED_FUND_CCY: usize = 33;
const H_WEIGHT_PCT: usize = 35; // percent of TNA; the universal model wants a fraction
const H_RISK_COUNTRY: usize = 41; // ISO alpha-3
const H_ISIN: usize = 45;
const H_MV_LOCAL: usize = 51;
const H_BLOOMBERG: usize = 65;
const H_MIN_FIELDS: usize = 66;

/// `HISINVLUX_165878_20260807_20260810130151.csv` -> ("165878", 2026-08-07).
/// Case-insensitive on the prefix; also used for HISTOVLLUX by Task 4.
pub fn filename_meta(filename: &str) -> Option<(String, NaiveDate)> {
    let re = regex::Regex::new(r"(?i)^[A-Z]+_(\d+)_(\d{8})_\d+\.csv$").unwrap();
    let caps = re.captures(filename)?;
    let code = caps.get(1)?.as_str().to_string();
    let date = NaiveDate::parse_from_str(caps.get(2)?.as_str(), "%Y%m%d").ok()?;
    Some((code, date))
}

fn decode_latin1(bytes: &[u8]) -> String {
    // Latin-1 maps byte n to Unicode code point n; no external crate needed.
    bytes.iter().map(|&b| b as char).collect()
}

fn field<'a>(fields: &'a [&str], i: usize) -> &'a str {
    fields.get(i).map(|s| s.trim()).unwrap_or("")
}

fn num(fields: &[&str], i: usize) -> Option<f64> {
    let t = field(fields, i);
    if t.is_empty() { None } else { t.parse::<f64>().ok() }
}

fn text(fields: &[&str], i: usize) -> Option<String> {
    let t = field(fields, i);
    if t.is_empty() { None } else { Some(t.to_string()) }
}

/// CACEIS category + detail code -> the closed universal vocabulary.
/// `None` = unmappable; the row is dropped with a warning (signal, don't hide).
fn asset_type_of(catval: &str, gp3: &str) -> Option<&'static str> {
    match catval {
        "CPON" => Some("Dividendes"),
        "VMOB" if gp3.starts_with("111") => Some("Action"),
        "VMOB" if gp3.starts_with("12") => Some("Fonds"),
        "VMOB" if gp3.starts_with("13") => Some("Obligation"),
        "FUTU" if gp3.starts_with("18") => Some("Future"),
        "TRES" => match gp3 {
            "COMPTE" => Some("Cash Acc"),
            "MARGES" => Some("Margin Acc"),
            "FP" | "PF" => Some("Frais provisionnés"),
            "PS" | "PU" => Some("Provisions ordres"),
            _ => None,
        },
        _ => None,
    }
}

/// Risk-country ISO alpha-3 -> the full names the Bloomberg pipeline stores
/// (see `bloomberg::region_for`). Unknown codes yield no country hint.
fn country_name(alpha3: &str) -> Option<&'static str> {
    Some(match alpha3 {
        "FRA" => "France", "DEU" => "Germany", "ITA" => "Italy", "ESP" => "Spain",
        "NLD" => "Netherlands", "BEL" => "Belgium", "AUT" => "Austria", "PRT" => "Portugal",
        "IRL" => "Ireland", "LUX" => "Luxembourg", "FIN" => "Finland", "GRC" => "Greece",
        "GBR" => "United Kingdom", "CHE" => "Switzerland", "SWE" => "Sweden", "NOR" => "Norway",
        "DNK" => "Denmark", "POL" => "Poland", "CZE" => "Czech Republic",
        "USA" => "United States", "CAN" => "Canada",
        "BRA" => "Brazil", "MEX" => "Mexico", "CHL" => "Chile", "ARG" => "Argentina",
        "COL" => "Colombia", "PER" => "Peru",
        "JPN" => "Japan", "CHN" => "China", "HKG" => "Hong Kong", "KOR" => "South Korea",
        "TWN" => "Taiwan", "SGP" => "Singapore", "IND" => "India", "AUS" => "Australia",
        "NZL" => "New Zealand", "IDN" => "Indonesia", "THA" => "Thailand", "MYS" => "Malaysia",
        "ZAF" => "South Africa", "ARE" => "United Arab Emirates", "SAU" => "Saudi Arabia",
        "ISR" => "Israel", "TUR" => "Turkey", "QAT" => "Qatar", "EGY" => "Egypt",
        "NGA" => "Nigeria", "MAR" => "Morocco",
        _ => return None,
    })
}

pub fn parse_hisinv(filename: &str, bytes: &[u8]) -> Result<UniversalBatch, ParseFailure> {
    let (fund_code, file_date) = filename_meta(filename)
        .ok_or_else(|| ParseFailure::Workbook(format!("filename {filename:?} does not match HISINVLUX_<fund>_<yyyymmdd>_<ts>.csv")))?;

    let textual = decode_latin1(bytes);
    let mut positions: Vec<PositionRow> = Vec::new();
    let mut ref_hints: Vec<RefHint> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    for (i, line) in textual.lines().enumerate() {
        let lineno = i + 1;
        if line.trim().is_empty() { continue; }
        let fields: Vec<&str> = line.split(';').collect();
        if fields.len() < H_MIN_FIELDS {
            return Err(ParseFailure::Workbook(format!(
                "line {lineno}: {} columns, expected at least {H_MIN_FIELDS} — not a HISINVLUX layout", fields.len())));
        }
        let row_date = NaiveDate::parse_from_str(field(&fields, H_NAV_DATE), "%Y%m%d")
            .map_err(|_| ParseFailure::Workbook(format!("line {lineno}: bad NAV date {:?}", field(&fields, H_NAV_DATE))))?;
        if row_date != file_date {
            return Err(ParseFailure::Workbook(format!(
                "line {lineno}: row date {row_date} differs from filename date {file_date}")));
        }
        if field(&fields, H_FUND_CODE) != fund_code {
            return Err(ParseFailure::Workbook(format!(
                "line {lineno}: fund code {:?} differs from filename code {fund_code:?}", field(&fields, H_FUND_CODE))));
        }

        let catval = field(&fields, H_CATVAL);
        let gp3 = field(&fields, H_GP3);
        let Some(asset_type) = asset_type_of(catval, gp3) else {
            warnings.push(format!("line {lineno}: unmappable asset code {catval}/{gp3} — row dropped ({})",
                field(&fields, H_NAME)));
            continue;
        };

        let isin = text(&fields, H_ISIN).or_else(|| text(&fields, H_INSTR_CODE));
        let Some(isin) = isin else {
            warnings.push(format!("line {lineno}: no ISIN or instrument code — row dropped"));
            continue;
        };

        let valuation_eur = num(&fields, H_MV_FUND_CCY);
        let valuation_ccy = num(&fields, H_MV_LOCAL);
        let currency = text(&fields, H_ASSET_CCY);
        let is_cashlike = matches!(asset_type, "Cash Acc" | "Margin Acc" | "Frais provisionnés" | "Provisions ordres" | "Dividendes");
        let fx_rate = if currency.as_deref() == Some("EUR") {
            Some(1.0)
        } else {
            match (valuation_eur, valuation_ccy) {
                (Some(e), Some(l)) if l.abs() > 1e-12 => Some(e / l),
                _ => None,
            }
        };
        let ticker = text(&fields, H_BLOOMBERG).filter(|t| t != "-1");

        if catval == "VMOB" {
            let country = text(&fields, H_RISK_COUNTRY)
                .and_then(|a3| country_name(&a3).map(str::to_string));
            let region = country.as_deref().and_then(crate::bloomberg::region_for).map(str::to_string);
            if country.is_some() || ticker.is_some() {
                ref_hints.push(RefHint {
                    isin: isin.clone(),
                    country_of_risk: country,
                    region,
                    ticker: ticker.clone(),
                });
            }
        }

        positions.push(PositionRow {
            asset_type: asset_type.to_string(),
            isin,
            name: text(&fields, H_NAME),
            currency,
            quantity: num(&fields, H_QUANTITY),
            avg_cost: if is_cashlike { None } else { num(&fields, H_UNIT_COST) },
            price: if is_cashlike { None } else { num(&fields, H_MARKET_PRICE) },
            valuation_ccy,
            accrued_interest: num(&fields, H_ACCRUED_FUND_CCY),
            fx_rate,
            valuation_eur,
            weight: num(&fields, H_WEIGHT_PCT).map(|w| w / 100.0),
            ticker,
        });
    }

    if positions.is_empty() {
        return Err(ParseFailure::Workbook("no position rows found".into()));
    }

    Ok(UniversalBatch {
        primary_date: file_date,
        nav_points: Vec::new(),
        snapshots: vec![Snapshot { nav_date: file_date, positions }],
        dividends: None,
        operations: None,
        ref_hints,
        warnings,
    })
}
```

Add `pub mod caceis;` to `crates/ingest/src/lib.rs`.

- [ ] **Step 4: Run the tests until green**

Run: `cargo test -p ingest --test caceis`
Expected: PASS. If an exact-value assertion fails, check the fixture line by hand (`awk -F';' '{print NR": "$1" "$6" "$7" "$46}' crates/ingest/tests/fixtures/caceis_hisinv.csv`) — the assertion values above were read off the real 2026-08-07 sample and are authoritative; a mismatch means a column-index bug, not a wrong test.

- [ ] **Step 5: Commit**

```bash
git add crates/ingest/src/caceis.rs crates/ingest/src/lib.rs crates/ingest/tests/caceis.rs crates/ingest/tests/fixtures/caceis_hisinv.csv
git commit -m "feat(ingest): CACEIS HISINVLUX adapter with declarative transposition"
```

---

### Task 4: HISTOVLLUX parser + detection dispatcher

**Files:**
- Modify: `crates/ingest/src/caceis.rs` (add `parse_histovl`)
- Modify: `crates/ingest/src/adapter.rs` (add `detect` and `parse` dispatchers)
- Create: `crates/ingest/tests/fixtures/caceis_histovl.csv`, `crates/ingest/tests/fixtures/caceis_histovl_multiclass.csv`
- Test: extend `crates/ingest/tests/caceis.rs`

**Interfaces:**
- Consumes: Task 3's `caceis` module; Task 2's adapter types.
- Produces: `adapter::detect(filename, bytes) -> Result<Identification, DetectError>` and `adapter::parse(kind: FileKind, filename: &str, bytes: &[u8]) -> Result<UniversalBatch, ParseFailure>` — the only two functions Task 6's handler calls.

- [ ] **Step 1: Create the HISTOVLLUX fixtures**

From the repo root:

```bash
cp HISTOVLLUX_165878_20260729_20260730170850.csv crates/ingest/tests/fixtures/caceis_histovl.csv
```

Hand-write `crates/ingest/tests/fixtures/caceis_histovl_multiclass.csv` (two share classes — content below verbatim, single trailing newline):

```
165878;FUND WITH TWO CLASSES                             ;20260729;C1;EUR;             104.04        ;        18224487.14        ;          171295.542       ;P;          0.        ;LU3007631891;            ;165878C1    ;        15450007.5         ;        16039170.88        ;         1185316.26        ;              -0.239716176 ;   769559514;20260728;             104.29        ;              -0.25        ;         ;
165878;FUND WITH TWO CLASSES                             ;20260729;C2;EUR;             204.04        ;        10000000.00        ;           49010.001       ;P;          0.        ;LU3007631909;            ;165878C2    ;         9450007.5         ;         9039170.88        ;         1000000.00        ;              -0.239716176 ;   769559514;20260728;             204.29        ;              -0.25        ;         ;
```

- [ ] **Step 2: Write the failing tests**

Append to `crates/ingest/tests/caceis.rs`:

```rust
const HV_FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/caceis_histovl.csv");
const HV_MULTI: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/caceis_histovl_multiclass.csv");
const HV_FNAME: &str = "HISTOVLLUX_165878_20260729_20260730170850.csv";

#[test]
fn histovl_yields_one_nav_point() {
    let bytes = std::fs::read(HV_FIXTURE).unwrap();
    let b = caceis::parse_histovl(HV_FNAME, &bytes).unwrap();
    assert_eq!(b.primary_date, chrono::NaiveDate::from_ymd_opt(2026, 7, 29).unwrap());
    assert_eq!(b.nav_points.len(), 1);
    let n = &b.nav_points[0];
    assert_eq!(n.nav, 104.04);
    assert_eq!(n.aum, 28224487.14);
    assert_eq!(n.shares, 271295.542);
    assert!(b.snapshots.is_empty() && b.dividends.is_none() && b.operations.is_none());
}

#[test]
fn histovl_rejects_multiple_share_classes() {
    let bytes = std::fs::read(HV_MULTI).unwrap();
    let err = caceis::parse_histovl(HV_FNAME, &bytes);
    match err {
        Err(ingest::ParseFailure::Workbook(m)) => assert!(m.contains("share class"), "{m}"),
        other => panic!("expected multi-share-class rejection, got {other:?}"),
    }
}

#[test]
fn detect_routes_recognizes_and_rejects() {
    use ingest::adapter::{detect, DetectError, FileKind};
    let hisinv = std::fs::read(FIXTURE).unwrap();
    let id = detect(FNAME, &hisinv).unwrap();
    assert_eq!(id.kind, FileKind::CaceisHisinv);
    assert_eq!(id.fund_code, Some(("caceis".to_string(), "165878".to_string())));

    let histovl = std::fs::read(HV_FIXTURE).unwrap();
    let id2 = detect(HV_FNAME, &histovl).unwrap();
    assert_eq!(id2.kind, FileKind::CaceisHistovl);

    // xlsx magic bytes -> NAV Recap, no fund code.
    let id3 = detect("07-08-2026 - Borobudur - NAV Recap.xlsx", b"PK\x03\x04rest").unwrap();
    assert_eq!(id3.kind, FileKind::NavRecap);
    assert_eq!(id3.fund_code, None);

    // Recognized-but-rejected families say why.
    match detect("INVXDVLUX_165878_20260804_20260805132350.csv", b"x") {
        Err(DetectError::Rejected(m)) => assert!(m.contains("HISINVLUX"), "{m}"),
        other => panic!("{other:?}"),
    }
    match detect("JOUROPLUX_165878_20260804_20260805132350.csv", b"x") {
        Err(DetectError::Rejected(m)) => assert!(m.to_lowercase().contains("sample"), "{m}"),
        other => panic!("{other:?}"),
    }
    // Garbage -> Unrecognized.
    assert!(matches!(detect("notes.txt", b"hello"), Err(DetectError::Unrecognized(_))));
    // A renamed random CSV must not slip through the content sniff.
    assert!(matches!(
        detect("HISINVLUX_1_20260101_1.csv", b"just,a,comma,file\n"),
        Err(DetectError::Unrecognized(_))
    ));
}
```

Run: `cargo test -p ingest --test caceis`
Expected: FAIL — `parse_histovl`, `detect` missing.

- [ ] **Step 3: Implement `parse_histovl` and the dispatchers**

Append to `crates/ingest/src/caceis.rs`:

```rust
// HISTOVLLUX columns (0-based).
const V_FUND_CODE: usize = 0;
const V_NAV_DATE: usize = 2;
const V_SHARE_CLASS: usize = 3;
const V_NAV: usize = 5;
const V_TNA: usize = 6;
const V_OUTSTANDING: usize = 7;
const V_MIN_FIELDS: usize = 20;

pub fn parse_histovl(filename: &str, bytes: &[u8]) -> Result<UniversalBatch, ParseFailure> {
    let (fund_code, file_date) = filename_meta(filename)
        .ok_or_else(|| ParseFailure::Workbook(format!("filename {filename:?} does not match HISTOVLLUX_<fund>_<yyyymmdd>_<ts>.csv")))?;

    let textual = decode_latin1(bytes);
    let mut rows: Vec<(String, crate::NavHistoryRow)> = Vec::new();
    for (i, line) in textual.lines().enumerate() {
        let lineno = i + 1;
        if line.trim().is_empty() { continue; }
        let fields: Vec<&str> = line.split(';').collect();
        if fields.len() < V_MIN_FIELDS {
            return Err(ParseFailure::Workbook(format!(
                "line {lineno}: {} columns, expected at least {V_MIN_FIELDS} — not a HISTOVLLUX layout", fields.len())));
        }
        if field(&fields, V_FUND_CODE) != fund_code {
            return Err(ParseFailure::Workbook(format!(
                "line {lineno}: fund code {:?} differs from filename code {fund_code:?}", field(&fields, V_FUND_CODE))));
        }
        let date = NaiveDate::parse_from_str(field(&fields, V_NAV_DATE), "%Y%m%d")
            .map_err(|_| ParseFailure::Workbook(format!("line {lineno}: bad NAV date {:?}", field(&fields, V_NAV_DATE))))?;
        if date != file_date {
            return Err(ParseFailure::Workbook(format!(
                "line {lineno}: row date {date} differs from filename date {file_date}")));
        }
        let (Some(nav), Some(aum), Some(shares)) = (num(&fields, V_NAV), num(&fields, V_TNA), num(&fields, V_OUTSTANDING)) else {
            return Err(ParseFailure::Workbook(format!("line {lineno}: NAV/TNA/outstanding missing or unparsable")));
        };
        rows.push((field(&fields, V_SHARE_CLASS).to_string(), crate::NavHistoryRow { date, aum, shares, nav }));
    }

    match rows.len() {
        0 => Err(ParseFailure::Workbook("no NAV rows found".into())),
        1 => {
            let (_, nav_point) = rows.into_iter().next().unwrap();
            Ok(UniversalBatch {
                primary_date: file_date,
                nav_points: vec![nav_point],
                snapshots: Vec::new(),
                dividends: None,
                operations: None,
                ref_hints: Vec::new(),
                warnings: Vec::new(),
            })
        }
        _ => {
            let classes: Vec<String> = rows.iter().map(|(c, _)| c.clone()).collect();
            Err(ParseFailure::Workbook(format!(
                "multi share class not supported yet (classes {classes:?}) — a silent sum would make NAV-per-share analytics meaningless")))
        }
    }
}
```

Append to `crates/ingest/src/adapter.rs`:

```rust
/// Route a file to its adapter. Content sniffs guard against renamed files:
/// a CACEIS CSV must actually parse its first line's column count and date.
pub fn detect(filename: &str, bytes: &[u8]) -> Result<Identification, DetectError> {
    let lower = filename.to_ascii_lowercase();
    let caceis_meta = || crate::caceis::filename_meta(filename)
        .map(|(code, _)| (crate::caceis::SOURCE.to_string(), code));

    if lower.starts_with("hisinvlux_") {
        let Some(fund_code) = caceis_meta() else {
            return Err(DetectError::Unrecognized(filename.to_string()));
        };
        if !sniff_semicolons(bytes, 66) { return Err(DetectError::Unrecognized(filename.to_string())); }
        return Ok(Identification { kind: FileKind::CaceisHisinv, fund_code: Some(fund_code) });
    }
    if lower.starts_with("histovllux_") {
        let Some(fund_code) = caceis_meta() else {
            return Err(DetectError::Unrecognized(filename.to_string()));
        };
        if !sniff_semicolons(bytes, 20) { return Err(DetectError::Unrecognized(filename.to_string())); }
        return Ok(Identification { kind: FileKind::CaceisHistovl, fund_code: Some(fund_code) });
    }
    if lower.starts_with("invxdvlux_") {
        return Err(DetectError::Rejected(
            "INVXDVLUX is not needed: HISINVLUX already carries the positions. Upload HISINVLUX and HISTOVLLUX.".into()));
    }
    if lower.starts_with("jouroplux_") {
        return Err(DetectError::Rejected(
            "JOUROPLUX recognized, but its parser is pending a sample file — request the feed from CACEIS and provide one sample so the parser can be written.".into()));
    }
    if lower.ends_with(".xlsx") && bytes.starts_with(b"PK\x03\x04") {
        return Ok(Identification { kind: FileKind::NavRecap, fund_code: None });
    }
    Err(DetectError::Unrecognized(filename.to_string()))
}

fn sniff_semicolons(bytes: &[u8], min_fields: usize) -> bool {
    let first_line: Vec<u8> = bytes.iter().copied().take_while(|&b| b != b'\n').collect();
    first_line.iter().filter(|&&b| b == b';').count() + 1 >= min_fields
}

pub fn parse(kind: FileKind, filename: &str, bytes: &[u8]) -> Result<UniversalBatch, crate::ParseFailure> {
    match kind {
        FileKind::NavRecap => crate::parse_workbook(bytes).map(to_batch),
        FileKind::CaceisHisinv => crate::caceis::parse_hisinv(filename, bytes),
        FileKind::CaceisHistovl => crate::caceis::parse_histovl(filename, bytes),
    }
}
```

- [ ] **Step 4: Run tests, then the ingest suite**

Run: `cargo test -p ingest`
Expected: all green (including the pre-existing bloomberg/bond tests).

- [ ] **Step 5: Commit**

```bash
git add crates/ingest/src/caceis.rs crates/ingest/src/adapter.rs crates/ingest/tests/caceis.rs crates/ingest/tests/fixtures/caceis_histovl.csv crates/ingest/tests/fixtures/caceis_histovl_multiclass.csv
git commit -m "feat(ingest): HISTOVLLUX parser + detect/parse dispatch with recognized-rejections"
```

---

### Task 5: Derived dividends from CPON deltas

**Files:**
- Modify: `crates/db/src/repo.rs` (add `derive_dividends`; call it from `import_batch`)
- Test: `crates/db/tests/derive_dividends.rs`

**Interfaces:**
- Consumes: `position_snapshots` rows with `asset_type = 'Dividendes'`; `dividends.derived` (Task 1).
- Produces: `derive_dividends(pool, portfolio_id) -> anyhow::Result<usize>`; `import_batch` calls it after commit when `b.dividends.is_none() && !b.snapshots.is_empty()`.

- [ ] **Step 1: Write the failing test**

`crates/db/tests/derive_dividends.rs`:

```rust
use chrono::NaiveDate;
use db::repo;

fn d(s: &str) -> NaiveDate { s.parse().unwrap() }

async fn seed_snapshot(pool: &sqlx::PgPool, date: &str, rows: &[(&str, &str, &str, f64, f64)]) {
    // rows: (asset_type, isin, currency, valuation_ccy, valuation_eur)
    let (import_id,): (i64,) = sqlx::query_as(
        "INSERT INTO imports (portfolio_id, filename, sha256, nav_date, row_counts) VALUES (1, $1, $2, $3, '{}') RETURNING id")
        .bind(format!("seed-{date}.csv")).bind(format!("sha-{date}")).bind(d(date))
        .fetch_one(pool).await.unwrap();
    for (at, isin, ccy, vl, ve) in rows {
        sqlx::query(
            "INSERT INTO position_snapshots (portfolio_id, nav_date, import_id, asset_type, isin, name, currency, valuation_ccy, valuation_eur)
             VALUES (1, $1, $2, $3, $4, $4, $5, $6, $7)")
            .bind(d(date)).bind(import_id).bind(at).bind(isin).bind(ccy).bind(vl).bind(ve)
            .execute(pool).await.unwrap();
    }
}

async fn derived_rows(pool: &sqlx::PgPool) -> Vec<(NaiveDate, String, f64, String)> {
    sqlx::query_as(
        "SELECT provision_date, issuer, amount::float8, currency FROM dividends
         WHERE portfolio_id = 1 AND derived ORDER BY provision_date, issuer")
        .fetch_all(pool).await.unwrap()
}

#[tokio::test]
async fn cpon_deltas_become_derived_dividends() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    // Day 1 (baseline): a GBP receivable of 500 local and an equity (noise).
    seed_snapshot(&pool, "2026-08-05", &[
        ("Dividendes", "GB0000000001", "GBP", 500.0, 580.0),
        ("Action", "FR0000000001", "EUR", 1000.0, 1000.0),
    ]).await;
    // Day 2: GBP receivable grows to 800 local (event: +300 GBP); a new EUR
    // receivable appears at 200 (event: +200 EUR).
    seed_snapshot(&pool, "2026-08-06", &[
        ("Dividendes", "GB0000000001", "GBP", 800.0, 920.0),
        ("Dividendes", "FR0000000002", "EUR", 200.0, 200.0),
        ("Action", "FR0000000001", "EUR", 1000.0, 1000.0),
    ]).await;
    // Day 3: GBP local value unchanged but EUR value moved (FX only — no
    // event); the EUR receivable disappears (paid — no event).
    seed_snapshot(&pool, "2026-08-07", &[
        ("Dividendes", "GB0000000001", "GBP", 800.0, 935.0),
        ("Action", "FR0000000001", "EUR", 1000.0, 1000.0),
    ]).await;

    let n = repo::derive_dividends(&pool, 1).await.unwrap();
    assert_eq!(n, 2, "one growth event + one appearance event");
    let rows = derived_rows(&pool).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], (d("2026-08-06"), "FR0000000002".into(), 200.0, "EUR".into()));
    assert_eq!(rows[1], (d("2026-08-06"), "GB0000000001".into(), 300.0, "GBP".into()));

    // Convergence: re-running (as every import does) yields the same set.
    let n2 = repo::derive_dividends(&pool, 1).await.unwrap();
    assert_eq!(n2, 2);
    assert_eq!(derived_rows(&pool).await.len(), 2);

    // Explicit-beats-derived: an explicit dividend on 2026-08-06 suppresses
    // the derived events on that date.
    sqlx::query("INSERT INTO dividends (portfolio_id, provision_date, issuer, amount, currency, derived) VALUES (1, '2026-08-06', 'EXPLICIT', 99, 'EUR', false)")
        .execute(&pool).await.unwrap();
    let n3 = repo::derive_dividends(&pool, 1).await.unwrap();
    assert_eq!(n3, 0, "explicit journal covers the date");
    assert!(derived_rows(&pool).await.is_empty());

    pool.close().await;
    edb.stop().await;
}
```

Run: `cargo test -p db --test derive_dividends`
Expected: FAIL — `derive_dividends` missing.

- [ ] **Step 2: Implement `derive_dividends`**

Append to `crates/db/src/repo.rs`:

```rust
/// Recompute the derived dividend set for a portfolio from its `Dividendes`
/// snapshot rows (CACEIS CPON receivables). A pure function of the
/// snapshots — delete-and-rebuild — so backlog uploads converge in any
/// order. Change detection runs on the LOCAL-currency value: FX moves on a
/// foreign receivable emit nothing. The first snapshot is baseline only
/// (its receivables existed before monitoring started). Dates carrying an
/// explicit (derived = false) dividend are skipped entirely.
pub async fn derive_dividends(pool: &PgPool, portfolio_id: i64) -> anyhow::Result<usize> {
    use std::collections::BTreeMap;

    let rows: Vec<(NaiveDate, String, Option<String>, Option<String>, Option<f64>)> = sqlx::query_as(
        "SELECT nav_date, isin, name, currency, valuation_ccy::float8 FROM position_snapshots
         WHERE portfolio_id = $1 AND asset_type = 'Dividendes' ORDER BY nav_date")
        .bind(portfolio_id).fetch_all(pool).await?;
    let dates: Vec<NaiveDate> = sqlx::query_scalar(
        "SELECT DISTINCT nav_date FROM position_snapshots WHERE portfolio_id = $1 ORDER BY nav_date")
        .bind(portfolio_id).fetch_all(pool).await?;
    let explicit: Vec<NaiveDate> = sqlx::query_scalar(
        "SELECT DISTINCT provision_date FROM dividends WHERE portfolio_id = $1 AND NOT derived")
        .bind(portfolio_id).fetch_all(pool).await?;

    // (isin, currency) -> date -> summed local value (a code may appear twice).
    let mut by_key: BTreeMap<(String, String), BTreeMap<NaiveDate, (f64, Option<String>)>> = BTreeMap::new();
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

    let mut tx = pool.begin().await?;
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
```

- [ ] **Step 3: Wire it into `import_batch`**

At the end of `import_batch`, after `tx.commit().await?;` and before building the `ImportOutcome`:

```rust
    if b.dividends.is_none() && !b.snapshots.is_empty() {
        let n = derive_dividends(pool, portfolio_id).await?;
        if n > 0 {
            warnings.push(format!("{n} dividend event(s) derived from receivable deltas"));
        }
    }
```

(Note: `warnings` must still be mutable at that point — move the `ImportOutcome` construction after this block.)

- [ ] **Step 4: Run tests**

Run: `cargo test -p db`
Expected: all green (new test + import_batch + import_workbook regressions).

- [ ] **Step 5: Commit**

```bash
git add crates/db/src/repo.rs crates/db/tests/derive_dividends.rs
git commit -m "feat(db): derive dividends from CPON receivable deltas, explicit rows win"
```

---

### Task 6: Multi-file upload with auto-routing + portfolio codes API

**Files:**
- Modify: `crates/server/src/handlers/imports.rs` (multi-file, routing, per-file results)
- Modify: `crates/server/src/handlers/portfolios.rs` (codes_list, codes_put)
- Modify: `crates/server/src/routes.rs` (codes route)
- Modify: `crates/server/tests/api_imports.rs`, `crates/server/tests/api_portfolio_isolation.rs`, `crates/server/tests/api_derivatives.rs` (response-shape updates)
- Test: `crates/server/tests/api_ingest_routing.rs`

**Interfaces:**
- Consumes: `adapter::{detect, parse, DetectError, FileKind}`, `repo::{import_batch, portfolio_by_code, portfolio_codes_for, portfolio_codes_replace}`, `portfolios::ensure`.
- Produces: `POST /api/portfolios/{id}/imports` now returns `Vec<FileImportResult>` (JSON array — breaking change consumed by Task 7); `GET/PUT /api/portfolios/{id}/codes`.

- [ ] **Step 1: Rewrite `crates/server/src/handlers/imports.rs`**

```rust
use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Multipart, Path, State};
use axum::Json;
use sha2::Digest;

#[derive(serde::Serialize)]
pub struct FileImportResult {
    pub filename: String,
    /// "nav_recap" | "caceis_hisinv" | "caceis_histovl"; None when detection failed.
    pub kind: Option<String>,
    pub portfolio_id: Option<i64>,
    pub portfolio_name: Option<String>,
    pub outcome: Option<db::repo::ImportOutcome>,
    pub error: Option<String>,
    pub error_rows: Option<Vec<ingest::RowError>>,
}

fn kind_label(k: ingest::adapter::FileKind) -> &'static str {
    match k {
        ingest::adapter::FileKind::NavRecap => "nav_recap",
        ingest::adapter::FileKind::CaceisHisinv => "caceis_hisinv",
        ingest::adapter::FileKind::CaceisHistovl => "caceis_histovl",
    }
}

/// Multi-file upload. The URL portfolio is where non-identifying files
/// (NAV Recap) land, and must be active — 404/409 up front, preserving the
/// existing single-file contract. Self-identifying files (CACEIS) route by
/// `portfolio_codes` REGARDLESS of the URL portfolio; problems with an
/// individual file are reported per file, not as a request failure.
pub async fn upload(State(st): State<AppState>, Path(pid): Path<i64>, mut multipart: Multipart) -> Result<Json<Vec<FileImportResult>>, AppError> {
    let selected = super::portfolios::ensure(&st.pool, pid, true).await?;

    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart error: {e}")))?
    {
        if field.name() != Some("file") { continue; }
        let filename = field.file_name().unwrap_or("upload.bin").to_string();
        let bytes = field.bytes().await
            .map_err(|e| AppError::BadRequest(format!("read error: {e}")))?;
        files.push((filename, bytes.to_vec()));
    }
    if files.is_empty() {
        return Err(AppError::BadRequest("missing multipart field 'file'".into()));
    }

    let mut results = Vec::with_capacity(files.len());
    for (filename, bytes) in files {
        results.push(import_one(&st, &selected, filename, &bytes).await);
    }
    Ok(Json(results))
}

async fn import_one(st: &AppState, selected: &db::repo::Portfolio, filename: String, bytes: &[u8]) -> FileImportResult {
    let mut r = FileImportResult {
        filename: filename.clone(), kind: None, portfolio_id: None,
        portfolio_name: None, outcome: None, error: None, error_rows: None,
    };

    let id = match ingest::adapter::detect(&filename, bytes) {
        Ok(id) => id,
        Err(e) => { r.error = Some(e.to_string()); return r; }
    };
    r.kind = Some(kind_label(id.kind).to_string());

    // Route: self-identifying files by code lookup; others to the URL portfolio.
    let (target_id, target_name) = match &id.fund_code {
        None => (selected.id, selected.name.clone()),
        Some((source, code)) => {
            match db::repo::portfolio_by_code(&st.pool, source, code).await {
                Err(e) => { r.error = Some(e.to_string()); return r; }
                Ok(None) => {
                    r.error = Some(format!(
                        "unknown {source} code {code:?} — map it to a portfolio in the Portfolios panel, then re-upload"));
                    return r;
                }
                Ok(Some(tid)) => match super::portfolios::ensure(&st.pool, tid, true).await {
                    Ok(p) => (p.id, p.name),
                    Err(e) => { r.error = Some(e.to_string()); return r; }
                },
            }
        }
    };
    r.portfolio_id = Some(target_id);
    r.portfolio_name = Some(target_name);

    let batch = match ingest::adapter::parse(id.kind, &filename, bytes) {
        Ok(b) => b,
        Err(ingest::ParseFailure::Workbook(m)) => { r.error = Some(m); return r; }
        Err(ingest::ParseFailure::Rows(rows)) => {
            r.error = Some(format!("{} row error(s)", rows.len()));
            r.error_rows = Some(rows);
            return r;
        }
    };

    let sha = hex::encode(sha2::Sha256::digest(bytes));
    match db::repo::import_batch(&st.pool, target_id, &filename, &sha, &batch).await {
        Ok(outcome) => r.outcome = Some(outcome),
        Err(e) => r.error = Some(e.to_string()),
    }
    r
}

pub async fn list(State(st): State<AppState>, Path(pid): Path<i64>) -> Result<Json<Vec<db::repo::ImportRecord>>, AppError> {
    super::portfolios::ensure(&st.pool, pid, false).await?;
    Ok(Json(db::repo::imports_list(&st.pool, pid).await?))
}
```

`ensure`'s error type must stringify usefully for per-file entries. Check `AppError`'s `Display`/`to_string()`: if `AppError::NotFound(m)`/`Conflict(m)` render as bare messages, fine; if not, match those two variants explicitly and use their inner message. `ImportOutcome` already derives `Serialize` (it is returned as JSON today); `db::repo::Portfolio` needs `name: String` and `id: i64` public (they are — Phase 1).

- [ ] **Step 2: Add the codes endpoints**

Append to `crates/server/src/handlers/portfolios.rs`:

```rust
#[derive(serde::Deserialize)]
pub struct CodeBody {
    pub source: String,
    pub code: String,
}

pub async fn codes_list(State(st): State<AppState>, Path(pid): Path<i64>) -> Result<Json<Vec<db::repo::PortfolioCode>>, AppError> {
    ensure(&st.pool, pid, false).await?;
    Ok(Json(db::repo::portfolio_codes_for(&st.pool, pid).await?))
}

/// Replace the portfolio's full code set. Codes are trimmed; empty entries
/// are 422; a code already claimed by another portfolio is 422 too.
pub async fn codes_put(State(st): State<AppState>, Path(pid): Path<i64>, Json(body): Json<Vec<CodeBody>>) -> Result<Json<Vec<db::repo::PortfolioCode>>, AppError> {
    ensure(&st.pool, pid, true).await?;
    let mut codes: Vec<(String, String)> = Vec::with_capacity(body.len());
    for c in &body {
        let source = c.source.trim().to_lowercase();
        let code = c.code.trim().to_string();
        if source.is_empty() || code.is_empty() {
            return Err(AppError::Unprocessable("source and code must be non-empty".into()));
        }
        codes.push((source, code));
    }
    db::repo::portfolio_codes_replace(&st.pool, pid, &codes).await.map_err(|e| {
        let is_unique = e.downcast_ref::<sqlx::Error>()
            .and_then(|se| se.as_database_error())
            .is_some_and(|de| de.is_unique_violation());
        if is_unique {
            AppError::Unprocessable("one of these codes is already mapped to another portfolio".into())
        } else {
            e.into()
        }
    })?;
    Ok(Json(db::repo::portfolio_codes_for(&st.pool, pid).await?))
}
```

Check the exact 422 variant name in `crates/server/src/error.rs` (Phase 1 used a 422 for validation — reuse it; if it is named differently, e.g. `AppError::Unprocessable(String)` vs something else, use the existing one; do the same for the unique-violation downcast pattern already present in `map_name_conflict`).

In `crates/server/src/routes.rs`, next to the existing portfolio routes:

```rust
.route("/api/portfolios/{id}/codes", get(portfolios::codes_list).put(portfolios::codes_put))
```

- [ ] **Step 3: Update the three response-shape test sites**

The upload response is now a JSON ARRAY of per-file results. Update assertions:

- `crates/server/tests/api_imports.rs` (~lines 45-51): `body["duplicate"]` → `body[0]["outcome"]["duplicate"]`; a parse-failure test that expected HTTP 422 for a bad workbook now expects HTTP 200 with `body[0]["error"]` non-null and (for row errors) `body[0]["error_rows"]` an array. Read the whole file and convert every assertion on the upload response accordingly; status-only assertions (`assert_eq!(res.status(), OK)`) stay.
- `crates/server/tests/api_portfolio_isolation.rs` (~lines 96-110): same `body[...]` → `body[0]["outcome"][...]` conversion; the archived-portfolio 409 and unknown-portfolio 404 assertions stay unchanged (the guard still runs on the URL portfolio before any file is processed).
- `crates/server/tests/api_derivatives.rs` (~line 90): `out["duplicate"]` → `out[0]["outcome"]["duplicate"]`.

Search the whole `crates/server/tests/` directory for other upload-response body parses (`rg 'oneshot\(upload_req' crates/server/tests -l` then inspect each) — status-code-only call sites need no change.

- [ ] **Step 4: Write the routing test**

`crates/server/tests/api_ingest_routing.rs`:

```rust
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

const BOUNDARY: &str = "XBOUNDARYX";
const HISINV: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/caceis_hisinv.csv");
const HISTOVL: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/caceis_histovl.csv");

fn multi_upload_req(uri: &str, files: &[(&str, &[u8])]) -> Request<Body> {
    let mut body = Vec::new();
    for (filename, bytes) in files {
        body.extend_from_slice(format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        ).as_bytes());
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    Request::post(uri)
        .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(Body::from(body)).unwrap()
}

async fn json_of(res: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

#[tokio::test]
async fn caceis_files_route_by_code_regardless_of_url_portfolio() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    let hisinv = std::fs::read(HISINV).unwrap();
    let histovl = std::fs::read(HISTOVL).unwrap();

    // Create a mandate and map the CACEIS fund code to it.
    let res = app.clone().oneshot(
        Request::post("/api/portfolios")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"Mandat CSV","kind":"mandate"}"#)).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let pid2 = json_of(res).await["id"].as_i64().unwrap();

    // Before mapping: upload reports an unknown code, writes nothing.
    let res = app.clone().oneshot(multi_upload_req(
        "/api/portfolios/1/imports",
        &[("HISINVLUX_165878_20260807_20260810130151.csv", &hisinv)],
    )).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_of(res).await;
    assert!(body[0]["error"].as_str().unwrap().contains("165878"), "{body}");
    assert!(body[0]["outcome"].is_null());

    // Map the code, re-upload BOTH files through portfolio 1's URL: they must
    // land in the mandate.
    let res = app.clone().oneshot(
        Request::put(format!("/api/portfolios/{pid2}/codes"))
            .header("content-type", "application/json")
            .body(Body::from(r#"[{"source":"caceis","code":"165878"}]"#)).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let res = app.clone().oneshot(multi_upload_req(
        "/api/portfolios/1/imports",
        &[
            ("HISINVLUX_165878_20260807_20260810130151.csv", &hisinv),
            ("HISTOVLLUX_165878_20260729_20260730170850.csv", &histovl),
        ],
    )).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_of(res).await;
    assert_eq!(body.as_array().unwrap().len(), 2);
    for item in body.as_array().unwrap() {
        assert_eq!(item["portfolio_id"].as_i64().unwrap(), pid2, "{item}");
        assert!(item["error"].is_null(), "{item}");
        assert!(item["outcome"]["import_id"].is_i64(), "{item}");
    }

    // The mandate has the snapshot and the NAV point; portfolio 1 has neither.
    let n2: i64 = sqlx::query_scalar("SELECT count(*) FROM position_snapshots WHERE portfolio_id = $1")
        .bind(pid2).fetch_one(&pool).await.unwrap();
    assert!(n2 > 0);
    let n1: i64 = sqlx::query_scalar("SELECT count(*) FROM position_snapshots WHERE portfolio_id = 1")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(n1, 0);
    let nav2: i64 = sqlx::query_scalar("SELECT count(*) FROM nav_history WHERE portfolio_id = $1")
        .bind(pid2).fetch_one(&pool).await.unwrap();
    assert_eq!(nav2, 1);

    // Dedupe is per portfolio: same file again -> duplicate outcome.
    let res = app.clone().oneshot(multi_upload_req(
        "/api/portfolios/1/imports",
        &[("HISINVLUX_165878_20260807_20260810130151.csv", &hisinv)],
    )).await.unwrap();
    let body = json_of(res).await;
    assert_eq!(body[0]["outcome"]["duplicate"], true, "{body}");

    // Archive the mandate: a routed file now fails per-file, request stays 200.
    let res = app.clone().oneshot(
        Request::put(format!("/api/portfolios/{pid2}"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"Mandat CSV","archived":true}"#)).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let res = app.clone().oneshot(multi_upload_req(
        "/api/portfolios/1/imports",
        &[("HISTOVLLUX_165878_20260729_20260730170850.csv", &histovl)],
    )).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_of(res).await;
    assert!(body[0]["error"].as_str().is_some(), "{body}");

    // Rejected families explain themselves.
    let res = app.clone().oneshot(multi_upload_req(
        "/api/portfolios/1/imports",
        &[("JOUROPLUX_165878_20260807_20260810130151.csv", b"x".as_slice())],
    )).await.unwrap();
    let body = json_of(res).await;
    assert!(body[0]["error"].as_str().unwrap().to_lowercase().contains("sample"), "{body}");

    pool.close().await;
    edb.stop().await;
}
```

- [ ] **Step 5: Run the server suite**

Run: `cargo test -p server`
Expected: all green, including the three updated test files.

- [ ] **Step 6: Commit**

```bash
git add crates/server/src/handlers/imports.rs crates/server/src/handlers/portfolios.rs crates/server/src/routes.rs crates/server/tests/api_ingest_routing.rs crates/server/tests/api_imports.rs crates/server/tests/api_portfolio_isolation.rs crates/server/tests/api_derivatives.rs
git commit -m "feat(server): multi-file upload with CACEIS auto-routing + portfolio codes API"
```

---

### Task 7: Frontend — multi-file upload, per-file results, codes editor

**Files:**
- Modify: `frontend/src/api.ts`
- Modify: `frontend/src/pages/DataPage.tsx`
- Modify: `frontend/src/components/PortfoliosAdmin.tsx`

**Interfaces:**
- Consumes: Task 6's response shapes verbatim.
- Produces: nothing downstream; `npm run build` is the gate.

- [ ] **Step 1: api.ts**

Replace `uploadFile` and add the new types/functions:

```ts
export interface FileImportResult {
  filename: string;
  kind: string | null;
  portfolio_id: number | null;
  portfolio_name: string | null;
  outcome: ImportOutcome | null;
  error: string | null;
  error_rows: { sheet: string; row: number; message: string }[] | null;
}
export const uploadFiles = (pid: number, files: File[]) => {
  const fd = new FormData();
  for (const f of files) fd.append("file", f);
  return req<FileImportResult[]>(`/api/portfolios/${pid}/imports`, { method: "POST", body: fd });
};

export interface PortfolioCode { portfolio_id: number; source: string; code: string }
export const getCodes = (pid: number) => req<PortfolioCode[]>(`/api/portfolios/${pid}/codes`);
export const putCodes = (pid: number, codes: { source: string; code: string }[]) =>
  req<PortfolioCode[]>(`/api/portfolios/${pid}/codes`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(codes) });
```

Delete `uploadFile` (single) after updating its one caller (DataPage). Keep `ImportOutcome` as-is.

- [ ] **Step 2: DataPage upload panel**

In `frontend/src/pages/DataPage.tsx`, replace the upload state and panel. State changes:

```ts
const [results, setResults] = useState<FileImportResult[] | null>(null);
const [uploadErr, setUploadErr] = useState<string | null>(null);
```

(delete the old `outcome` state and the `{ msg, rows }` error shape — per-file row errors now live inside each result). New `doUpload`:

```ts
async function doUpload(files: File[]) {
  if (files.length === 0) return;
  setBusy(true);
  setResults(null);
  setUploadErr(null);
  try {
    setResults(await uploadFiles(portfolio.id, files));
    imports.reload();
    positions.reload();
  } catch (e) {
    const ae = e as ApiError;
    setUploadErr(ae.detail ?? ae.message);
  } finally {
    setBusy(false);
  }
}
```

Panel (replaces the current drop card body; drop handler passes `Array.from(e.dataTransfer.files)`, input gains `multiple` and passes `Array.from(e.target.files ?? [])`):

```tsx
<h3>Import — {portfolio.name}</h3>
<p>{busy ? "Importing…" : "Drop files here (NAV Recap .xlsx, CACEIS HISINVLUX / HISTOVLLUX .csv) — CACEIS files auto-route to the portfolio mapped to their fund code."}</p>
<input
  type="file"
  accept=".xlsx,.csv"
  multiple
  disabled={busy}
  onChange={(e) => void doUpload(Array.from(e.target.files ?? []))}
/>
{uploadErr && <p className="neg">Upload failed: {uploadErr}</p>}
{results && (
  <table className="tbl">
    <thead><tr><th>File</th><th>Kind</th><th>Portfolio</th><th>Result</th></tr></thead>
    <tbody>
      {results.map((r, i) => (
        <tr key={i}>
          <td>{r.filename}</td>
          <td>{r.kind ?? "—"}</td>
          <td>{r.portfolio_name ?? "—"}</td>
          <td>
            {r.error ? (
              <>
                <span className="neg">{r.error}</span>
                {r.error_rows && (
                  <table className="tbl"><tbody>
                    {r.error_rows.slice(0, 10).map((er, j) => (
                      <tr key={j}><td>{er.sheet}</td><td>row {er.row}</td><td>{er.message}</td></tr>
                    ))}
                  </tbody></table>
                )}
              </>
            ) : r.outcome ? (
              <>
                <span className="pos">
                  {r.outcome.duplicate
                    ? "Already imported (identical file)."
                    : `Imported: ${r.outcome.nav_rows} NAV rows, ${r.outcome.positions} positions, ${r.outcome.dividends} dividends, ${r.outcome.operations} operations.`}
                </span>
                {r.outcome.warnings.map((w, j) => <p key={j} className="warn-badge">{w}</p>)}
              </>
            ) : null}
          </td>
        </tr>
      ))}
    </tbody>
  </table>
)}
```

Import changes at the top of the file: `uploadFiles, type FileImportResult` in, `uploadFile, type ImportOutcome` out (keep `ImportOutcome` only if still referenced).

- [ ] **Step 3: PortfoliosAdmin codes column**

In `frontend/src/components/PortfoliosAdmin.tsx`, add a "CACEIS code" column. New self-contained cell component appended at the bottom of the file:

```tsx
function CodeCell({ portfolioId }: { portfolioId: number }) {
  const codes = useFetch(() => getCodes(portfolioId), [portfolioId]);
  const [draft, setDraft] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const current = codes.data?.find((c) => c.source === "caceis")?.code ?? "";
  const value = draft ?? current;
  const dirty = draft !== null && draft.trim() !== current;
  async function save() {
    setErr(null);
    try {
      const others = (codes.data ?? []).filter((c) => c.source !== "caceis")
        .map((c) => ({ source: c.source, code: c.code }));
      const next = value.trim() ? [...others, { source: "caceis", code: value.trim() }] : others;
      await putCodes(portfolioId, next);
      setDraft(null);
      codes.reload();
    } catch (e) {
      const ae = e as ApiError;
      setErr(ae.detail ?? ae.message);
    }
  }
  return (
    <>
      <input
        style={{ width: 80 }}
        placeholder="fund code"
        value={value}
        onChange={(e) => setDraft(e.target.value)}
      />
      <button disabled={!dirty} onClick={() => void save()}>Save</button>
      {err && <span className="neg"> {err}</span>}
    </>
  );
}
```

Table integration: header row becomes `<tr><th>Name</th><th>Kind</th><th>CACEIS code</th><th>Latest NAV</th><th>Status</th><th></th></tr>` and each body row gains `<td><CodeCell portfolioId={p.id} /></td>` right after the `<td>{p.kind}</td>` cell. Imports at the top: add `getCodes, putCodes` to the `../api` import list.

- [ ] **Step 4: Build**

Run: `cd frontend && npm run build`
Expected: clean type-check and build.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/api.ts frontend/src/pages/DataPage.tsx frontend/src/components/PortfoliosAdmin.tsx
git commit -m "feat(frontend): multi-file upload with per-file results + CACEIS code editor"
```

---

### Task 8: Documentation

**Files:**
- Modify: `README.md`

**Interfaces:** none — prose only, but every claim must match the shipped behavior of Tasks 1-7.

- [ ] **Step 1: Update README**

Update the workflow/features sections:
- The Data page imports three formats: NAV Recap workbook (lands in the selected portfolio) and CACEIS HISINVLUX/HISTOVLLUX CSVs (auto-routed by fund code via the Portfolios panel's CACEIS-code mapping). Multiple files per drop; per-file results.
- INVXDVLUX is recognized and declined (redundant); JOUROPLUX is recognized and declined pending a sample file — until it flows, CSV-fed portfolios have no trade journal, so P&L shows price/FX effects without realized-trade attribution.
- Dividends for CSV-fed portfolios are derived from CPON receivable deltas (flagged, explicit journals win).
- CACEIS lines pre-fill country of risk / region / Bloomberg ticker in shared reference data (never overwriting Bloomberg values); bonds classified this way drop out of the Bloomberg request.
- The TNA cross-check warning (positions vs NAV file, 0.1%).

- [ ] **Step 2: Verify claims against code, build docs sanity**

Re-read the touched handlers/parsers; confirm each README statement. Run: `cargo test` once more at branch tip (full suite).

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: universal ingest + CACEIS adapter workflow"
```
