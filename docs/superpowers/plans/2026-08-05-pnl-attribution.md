# P&L Attribution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a P&L page that decomposes profit and loss over a user-chosen period into realized, unrealized, price and FX components, grouped by instrument, asset class, country, region, GICS sector, GICS industry, currency or issuer group, reconciled to the fund's NAV change.

**Architecture:** A pure `analytics::pnl` module computes everything per request from `operations` and `position_snapshots` — no materialized P&L tables, matching how `concentration`, `liquidity`, `rates` and `var` already work. Country and GICS classification plus daily FX history arrive through a Bloomberg round-trip workbook, written with `rust_xlsxwriter` and read back with `calamine`, mirroring the existing weekly CTD companion file.

**Tech Stack:** Rust 2024 edition, axum 0.8, sqlx 0.8 (PostgreSQL), calamine 0.26 (read), rust_xlsxwriter 0.97 (write), React 19 + TypeScript + Vite, ECharts.

**Spec:** `docs/superpowers/specs/2026-08-05-pnl-attribution-design.md`

## Global Constraints

- Rust edition 2024; all new crates entries use the existing `{ workspace = true }` form where the workspace defines the dependency.
- Analytics modules are **pure**: no `sqlx`, no `PgPool`, no I/O. Handlers read the database and pass plain structs in. Follow `crates/analytics/src/rates.rs`.
- Every new analytics module is declared in `crates/analytics/src/lib.rs` with both `pub mod x;` and `pub use x::*;`.
- Numeric columns are read as `f64` via an explicit `::float8` cast in SQL, as in `db::repo::positions_for`.
- Money is `f64` throughout, matching the existing codebase. Do not introduce `rust_decimal`.
- Reference-data writes use `COALESCE(existing, excluded)` so a manual edit is never overwritten by an import. See `repo.rs:126-137`.
- The reconciliation residual is **always present in the response and always rendered**. Tolerance controls presentation only, never visibility.
- Tolerance: residual reads as reconciled when `|residual| <= 0.001 * gross`, where `gross` is the sum of absolute values of the reconciliation lines.
- No `unwrap()` on user-supplied input in handlers; return `AppError::BadRequest` instead.
- Commit after every task with the `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>` trailer.

## File Structure

| File | Responsibility |
|---|---|
| `crates/db/migrations/0004_pnl.sql` | Classification columns on `instrument_refs`; `fx_history` table |
| `crates/db/src/repo.rs` (modify) | Extend `InstrumentRef`; add `operations_all`, `dividends_all`, `fx_upsert_many`, `fx_all`, `classify_upsert_many` |
| `crates/analytics/src/pnl.rs` | Cost-basis walk, decomposition, FX split, futures rule, grouping, reconciliation |
| `crates/ingest/src/bloomberg.rs` | Build the request workbook; parse the returned one |
| `crates/server/src/handlers/pnl.rs` | `GET /api/pnl` |
| `crates/server/src/handlers/bloomberg.rs` | `GET /api/bloomberg/request`, `POST /api/bloomberg/upload` |
| `frontend/src/pages/PnlPage.tsx` | The P&L page |
| `frontend/src/components/BloombergPanel.tsx` | Data-page export/upload panel |

`pnl.rs` is the one large file. It is kept whole because its parts share the
`Basis` and `Decomp` types and are meaningless apart; it is organized into
clearly separated sections with the same `#[cfg(test)] mod tests` convention the
other analytics modules use.

---

### Task 1: Schema and repository access

**Files:**
- Create: `crates/db/migrations/0004_pnl.sql`
- Modify: `crates/db/src/repo.rs` (extend `InstrumentRef` at :311-329, `refs_upsert` at :333)
- Test: `crates/db/tests/pnl_repo.rs`

**Interfaces:**
- Consumes: nothing (first task)
- Produces:
  - `db::repo::InstrumentRef` gains `country_of_risk: Option<String>`, `region: Option<String>`, `gics_sector: Option<String>`, `gics_industry: Option<String>`
  - `db::repo::OperationRecord { trade_date: NaiveDate, side: String, isin: Option<String>, ticker: Option<String>, name: Option<String>, currency: Option<String>, quantity: Option<f64>, net_price: Option<f64>, net_amount: Option<f64>, fees: Option<f64> }`
  - `db::repo::DividendRecord { provision_date: NaiveDate, issuer: String, amount: f64, currency: String }`
  - `db::repo::FxRow { date: NaiveDate, currency: String, rate_to_eur: f64 }`
  - `async fn operations_all(pool) -> anyhow::Result<Vec<OperationRecord>>` — ordered by `trade_date, id`
  - `async fn dividends_all(pool) -> anyhow::Result<Vec<DividendRecord>>`
  - `async fn fx_all(pool) -> anyhow::Result<Vec<FxRow>>`
  - `async fn fx_upsert_many(pool, &[FxRow]) -> anyhow::Result<u64>`
  - `async fn classify_upsert_many(pool, &[(String, Option<String>, Option<String>, Option<String>, Option<String>)]) -> anyhow::Result<u64>` — tuple is `(code, country, region, sector, industry)`

- [ ] **Step 1: Write the migration**

Create `crates/db/migrations/0004_pnl.sql`:

```sql
ALTER TABLE instrument_refs
  ADD COLUMN country_of_risk TEXT,
  ADD COLUMN region          TEXT,
  ADD COLUMN gics_sector     TEXT,
  ADD COLUMN gics_industry   TEXT,
  ADD COLUMN classified_at   TIMESTAMPTZ;

CREATE TABLE fx_history (
  date        DATE NOT NULL,
  currency    TEXT NOT NULL,
  rate_to_eur NUMERIC NOT NULL CHECK (rate_to_eur > 0),
  PRIMARY KEY (date, currency)
);

CREATE INDEX idx_fx_history_currency ON fx_history(currency, date);
```

- [ ] **Step 2: Write the failing test**

Create `crates/db/tests/pnl_repo.rs`. Follow the harness in the existing
`crates/db/tests/futures_contracts.rs` — read that file first and reuse its
pool-setup helper verbatim rather than inventing a new one.

```rust
mod common;

use chrono::NaiveDate;

fn d(y: i32, m: u32, dd: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, dd).unwrap() }

#[tokio::test]
async fn fx_upsert_is_idempotent_and_readable() {
    let (_pg, pool) = common::pool().await;
    let rows = vec![
        db::repo::FxRow { date: d(2026, 7, 24), currency: "USD".into(), rate_to_eur: 0.8788 },
        db::repo::FxRow { date: d(2026, 7, 24), currency: "GBP".into(), rate_to_eur: 1.1726 },
    ];
    assert_eq!(db::repo::fx_upsert_many(&pool, &rows).await.unwrap(), 2);
    // Re-upserting the same dates must overwrite, not duplicate.
    let rows2 = vec![db::repo::FxRow { date: d(2026, 7, 24), currency: "USD".into(), rate_to_eur: 0.88 }];
    db::repo::fx_upsert_many(&pool, &rows2).await.unwrap();

    let all = db::repo::fx_all(&pool).await.unwrap();
    assert_eq!(all.len(), 2);
    let usd = all.iter().find(|r| r.currency == "USD").unwrap();
    assert!((usd.rate_to_eur - 0.88).abs() < 1e-12);
}

#[tokio::test]
async fn classify_upsert_never_overwrites_an_existing_value() {
    let (_pg, pool) = common::pool().await;
    db::repo::classify_upsert_many(
        &pool,
        &[("FR0000121014".into(), Some("France".into()), Some("Europe".into()),
           Some("Consumer Discretionary".into()), Some("Textiles Apparel & Luxury Goods".into()))],
    ).await.unwrap();

    // A second load carrying a different country must not clobber the first.
    db::repo::classify_upsert_many(
        &pool,
        &[("FR0000121014".into(), Some("Wrong".into()), None, None, None)],
    ).await.unwrap();

    let refs = db::repo::refs_all(&pool).await.unwrap();
    let r = refs.iter().find(|r| r.code == "FR0000121014").unwrap();
    assert_eq!(r.country_of_risk.as_deref(), Some("France"));
    assert_eq!(r.gics_sector.as_deref(), Some("Consumer Discretionary"));
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p db --test pnl_repo`
Expected: FAIL to compile — `no function or associated item named fx_upsert_many`.

- [ ] **Step 4: Extend `InstrumentRef` and `refs_all`**

In `crates/db/src/repo.rs`, add four fields to the struct at :311:

```rust
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct InstrumentRef {
    pub code: String,
    pub issuer_group: Option<String>,
    pub liquidity_bucket: Option<String>,
    pub bond_coupon_pct: Option<f64>,
    pub bond_maturity: Option<NaiveDate>,
    pub bond_coupon_freq: Option<i32>,
    pub country_of_risk: Option<String>,
    pub region: Option<String>,
    pub gics_sector: Option<String>,
    pub gics_industry: Option<String>,
}
```

and widen the `refs_all` SELECT:

```rust
pub async fn refs_all(pool: &PgPool) -> anyhow::Result<Vec<InstrumentRef>> {
    Ok(sqlx::query_as(
        "SELECT code, issuer_group, liquidity_bucket,
                bond_coupon_pct::float8 AS bond_coupon_pct, bond_maturity, bond_coupon_freq,
                country_of_risk, region, gics_sector, gics_industry
         FROM instrument_refs ORDER BY code",
    )
    .fetch_all(pool)
    .await?)
}
```

`refs_upsert` writes only the fields it already names, so the classification
columns survive a manual reference-data edit untouched. Leave it alone.

- [ ] **Step 5: Add the new accessors**

Append to `crates/db/src/repo.rs`:

```rust
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

pub async fn operations_all(pool: &PgPool) -> anyhow::Result<Vec<OperationRecord>> {
    Ok(sqlx::query_as(
        "SELECT trade_date, side, isin, ticker, name, currency,
                quantity::float8 AS quantity, net_price::float8 AS net_price,
                net_amount::float8 AS net_amount, fees::float8 AS fees
         FROM operations ORDER BY trade_date, id",
    )
    .fetch_all(pool)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct DividendRecord {
    pub provision_date: NaiveDate,
    pub issuer: String,
    pub amount: f64,
    pub currency: String,
}

pub async fn dividends_all(pool: &PgPool) -> anyhow::Result<Vec<DividendRecord>> {
    Ok(sqlx::query_as(
        "SELECT provision_date, issuer, amount::float8 AS amount, currency
         FROM dividends ORDER BY provision_date",
    )
    .fetch_all(pool)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct FxRow {
    pub date: NaiveDate,
    pub currency: String,
    pub rate_to_eur: f64,
}

pub async fn fx_all(pool: &PgPool) -> anyhow::Result<Vec<FxRow>> {
    Ok(sqlx::query_as(
        "SELECT date, currency, rate_to_eur::float8 AS rate_to_eur
         FROM fx_history ORDER BY currency, date",
    )
    .fetch_all(pool)
    .await?)
}

/// Replace-by-key: an FX rate is market data, so a fresh pull always wins.
pub async fn fx_upsert_many(pool: &PgPool, rows: &[FxRow]) -> anyhow::Result<u64> {
    let mut tx = pool.begin().await?;
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

/// Seed classifications without ever overwriting a value already present,
/// matching the bond-statics discipline at :126-137. A user correction, or an
/// earlier good pull, always wins over a later one.
pub async fn classify_upsert_many(
    pool: &PgPool,
    rows: &[(String, Option<String>, Option<String>, Option<String>, Option<String>)],
) -> anyhow::Result<u64> {
    let mut tx = pool.begin().await?;
    let mut n = 0u64;
    for (code, country, region, sector, industry) in rows {
        n += sqlx::query(
            "INSERT INTO instrument_refs
               (code, country_of_risk, region, gics_sector, gics_industry, classified_at)
             VALUES ($1, $2, $3, $4, $5, now())
             ON CONFLICT (code) DO UPDATE SET
               country_of_risk = COALESCE(instrument_refs.country_of_risk, EXCLUDED.country_of_risk),
               region          = COALESCE(instrument_refs.region,          EXCLUDED.region),
               gics_sector     = COALESCE(instrument_refs.gics_sector,     EXCLUDED.gics_sector),
               gics_industry   = COALESCE(instrument_refs.gics_industry,   EXCLUDED.gics_industry),
               classified_at   = now(),
               updated_at      = now()",
        )
        .bind(code).bind(country).bind(region).bind(sector).bind(industry)
        .execute(&mut *tx).await?
        .rows_affected();
    }
    tx.commit().await?;
    Ok(n)
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p db --test pnl_repo`
Expected: PASS, 2 tests.

Then `cargo test -p db` — the existing suites must still pass, since
`refs_all` now returns four extra fields but `refs_upsert` is unchanged.

- [ ] **Step 7: Commit**

```bash
git add crates/db/migrations/0004_pnl.sql crates/db/src/repo.rs crates/db/tests/pnl_repo.rs
git commit -m "feat(db): classification columns, fx_history, P&L accessors"
```

---

### Task 2: Cost-basis walk

**Files:**
- Create: `crates/analytics/src/pnl.rs`
- Modify: `crates/analytics/src/lib.rs`
- Test: inline `#[cfg(test)] mod tests` in `pnl.rs`

**Interfaces:**
- Consumes: nothing from Task 1 (pure module; the handler bridges them later)
- Produces:
  - `pnl::Trade { trade_date, isin, is_buy, quantity, net_price, net_amount, currency }`
  - `pnl::Basis { qty: f64, avg_cost: f64 }`
  - `pnl::Flow { date: NaiveDate, amount_local: f64 }`
  - `pnl::Walk { basis_start: Basis, basis_end: Basis, realized_local: f64, buys: Vec<Flow>, sells: Vec<Flow>, oversold: bool }`
  - `pub fn is_buy(side: &str) -> Option<bool>`
  - `pub fn walk_instrument(trades: &[Trade], t0: NaiveDate, t1: NaiveDate) -> Walk`

- [ ] **Step 1: Write the failing test**

Create `crates/analytics/src/pnl.rs` containing only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, dd: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, dd).unwrap() }

    fn trade(day: u32, buy: bool, qty: f64, px: f64) -> Trade {
        Trade {
            trade_date: d(2026, 6, day),
            isin: "X".into(),
            is_buy: buy,
            quantity: qty,
            net_price: px,
            net_amount: if buy { -qty * px } else { qty * px },
            currency: "EUR".into(),
        }
    }

    #[test]
    fn weighted_average_matches_worked_example() {
        // Spec: buy 5000 @ 40.76, buy 3000 @ 44.20 -> avg 42.05,
        // sell 2000 @ 46.00 -> realized 7900, avg unchanged.
        let t = vec![trade(1, true, 5000.0, 40.76), trade(2, true, 3000.0, 44.20), trade(3, false, 2000.0, 46.00)];
        let w = walk_instrument(&t, d(2026, 5, 31), d(2026, 6, 30));
        assert!((w.basis_end.avg_cost - 42.055).abs() < 1e-9);
        assert!((w.basis_end.qty - 6000.0).abs() < 1e-9);
        assert!((w.realized_local - 2000.0 * (46.00 - 42.055)).abs() < 1e-6);
    }

    #[test]
    fn trades_on_or_before_t0_build_the_opening_basis_only() {
        let t = vec![trade(1, true, 1000.0, 10.0), trade(20, false, 400.0, 12.0)];
        let w = walk_instrument(&t, d(2026, 6, 10), d(2026, 6, 30));
        assert!((w.basis_start.qty - 1000.0).abs() < 1e-9);
        assert!((w.basis_start.avg_cost - 10.0).abs() < 1e-9);
        // Only the sell falls inside the window.
        assert!((w.realized_local - 400.0 * 2.0).abs() < 1e-9);
        assert_eq!(w.buys.len(), 0);
        assert_eq!(w.sells.len(), 1);
    }

    #[test]
    fn a_trade_exactly_on_t0_is_in_the_opening_basis() {
        let t = vec![trade(10, true, 100.0, 5.0)];
        let w = walk_instrument(&t, d(2026, 6, 10), d(2026, 6, 30));
        assert!((w.basis_start.qty - 100.0).abs() < 1e-9);
        assert_eq!(w.buys.len(), 0);
    }

    #[test]
    fn overselling_is_flagged_and_does_not_go_negative() {
        let t = vec![trade(1, true, 100.0, 5.0), trade(2, false, 250.0, 6.0)];
        let w = walk_instrument(&t, d(2026, 5, 31), d(2026, 6, 30));
        assert!(w.oversold);
        assert!(w.basis_end.qty >= 0.0);
    }

    #[test]
    fn side_parsing_is_case_insensitive() {
        assert_eq!(is_buy("Achat"), Some(true));
        assert_eq!(is_buy("Vente"), Some(false));
        assert_eq!(is_buy("VENTE"), Some(false));
        assert_eq!(is_buy("  achat "), Some(true));
        assert_eq!(is_buy("Nonsense"), None);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p analytics pnl`
Expected: FAIL to compile — `cannot find type Trade in this scope`.

- [ ] **Step 3: Write the implementation**

Prepend to `crates/analytics/src/pnl.rs`:

```rust
//! Period P&L attribution.
//!
//! Pure functions over plain structs: the caller reads the database and passes
//! trades, snapshot positions and FX rates in. Money is `f64` and local
//! currency unless a name says `_eur`.

use chrono::NaiveDate;
use serde::Serialize;
use std::collections::BTreeMap;

/// A trade as the engine needs it. `net_price` includes fees, matching the
/// administrator's PAM convention; `net_amount` is signed, negative for a buy.
#[derive(Debug, Clone)]
pub struct Trade {
    pub trade_date: NaiveDate,
    pub isin: String,
    pub is_buy: bool,
    pub quantity: f64,
    pub net_price: f64,
    pub net_amount: f64,
    pub currency: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Basis { pub qty: f64, pub avg_cost: f64 }

#[derive(Debug, Clone, Copy)]
pub struct Flow { pub date: NaiveDate, pub amount_local: f64 }

#[derive(Debug, Clone)]
pub struct Walk {
    pub basis_start: Basis,
    pub basis_end: Basis,
    /// Realized in (t0, t1], local currency.
    pub realized_local: f64,
    pub buys: Vec<Flow>,
    pub sells: Vec<Flow>,
    /// A sell exceeded the running quantity: history is incomplete.
    pub oversold: bool,
}

/// `Achat` -> buy, `Vente` -> sell, case- and whitespace-insensitive.
/// `None` for anything else, which the caller reports rather than guesses.
pub fn is_buy(side: &str) -> Option<bool> {
    match side.trim().to_lowercase().as_str() {
        "achat" => Some(true),
        "vente" => Some(false),
        _ => None,
    }
}

/// Roll weighted-average cost over `trades` (which must be sorted by date).
/// Trades on or before `t0` build the opening basis; trades in `(t0, t1]`
/// accumulate realized P&L and flows. Trades after `t1` are ignored.
pub fn walk_instrument(trades: &[Trade], t0: NaiveDate, t1: NaiveDate) -> Walk {
    let mut b = Basis::default();
    let mut basis_start = None;
    let mut realized = 0.0;
    let (mut buys, mut sells) = (Vec::new(), Vec::new());
    let mut oversold = false;

    for t in trades {
        if t.trade_date > t1 { break; }
        if basis_start.is_none() && t.trade_date > t0 {
            basis_start = Some(b);
        }
        let in_window = t.trade_date > t0;

        if t.is_buy {
            let total = b.avg_cost * b.qty + t.quantity * t.net_price;
            b.qty += t.quantity;
            b.avg_cost = if b.qty.abs() > f64::EPSILON { total / b.qty } else { 0.0 };
            if in_window { buys.push(Flow { date: t.trade_date, amount_local: t.net_amount }); }
        } else {
            let q = if t.quantity > b.qty + 1e-9 { oversold = true; b.qty } else { t.quantity };
            if in_window { realized += q * (t.net_price - b.avg_cost); }
            b.qty = (b.qty - q).max(0.0);
            if b.qty <= 1e-9 { b.qty = 0.0; b.avg_cost = 0.0; }
            if in_window { sells.push(Flow { date: t.trade_date, amount_local: t.net_amount }); }
        }
    }

    Walk {
        basis_start: basis_start.unwrap_or(b),
        basis_end: b,
        realized_local: realized,
        buys, sells, oversold,
    }
}
```

Register the module. In `crates/analytics/src/lib.rs` add `pub mod pnl;` after
`pub mod futures;` and `pub use pnl::*;` after `pub use futures::*;`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p analytics pnl`
Expected: PASS, 5 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/analytics/src/pnl.rs crates/analytics/src/lib.rs
git commit -m "feat(analytics): weighted-average cost-basis walk"
```

---

### Task 3: Price and FX decomposition, with the realized/unrealized FX split

**Files:**
- Modify: `crates/analytics/src/pnl.rs`
- Test: inline

**Interfaces:**
- Consumes: `Walk`, `Flow`, `Basis` from Task 2
- Produces:
  - `pnl::FxLookup` — trait-free struct: `{ f0: f64, f1: f64, at_trade: BTreeMap<NaiveDate, f64> }` with `fn rate(&self, d: NaiveDate) -> Option<f64>`
  - `pnl::Decomp { realized_price, unrealized_price, realized_fx, unrealized_fx, fx_split_imprecise: bool, fx_missing: Vec<NaiveDate> }`
  - `impl Decomp { fn realized(&self) -> f64; fn unrealized(&self) -> f64; fn fx(&self) -> f64; fn total(&self) -> f64 }`
  - `pub fn decompose(w: &Walk, v0_local: f64, v1_local: f64, fx: &FxLookup) -> Decomp`

- [ ] **Step 1: Write the failing test**

Append to the `tests` module in `crates/analytics/src/pnl.rs`:

```rust
    fn eur_fx() -> FxLookup { FxLookup { f0: 1.0, f1: 1.0, at_trade: BTreeMap::new() } }

    #[test]
    fn eur_position_has_no_fx_effect_and_totals_correctly() {
        // Held throughout, no trades: value 1000 -> 1150.
        let w = walk_instrument(&[], d(2026, 6, 1), d(2026, 6, 30));
        let dec = decompose(&w, 1000.0, 1150.0, &eur_fx());
        assert!((dec.total() - 150.0).abs() < 1e-9);
        assert!(dec.fx().abs() < 1e-12);
        assert!((dec.unrealized() - 150.0).abs() < 1e-9);
    }

    #[test]
    fn price_plus_fx_equals_total_exactly() {
        let t = vec![trade(15, false, 100.0, 12.0)];
        let mut at = BTreeMap::new();
        at.insert(d(2026, 6, 15), 0.90);
        let fx = FxLookup { f0: 0.88, f1: 0.92, at_trade: at };
        // Opening basis needs a prior buy.
        let mut all = vec![trade(1, true, 300.0, 10.0)];
        all.extend(t);
        let w = walk_instrument(&all, d(2026, 6, 10), d(2026, 6, 30));
        let dec = decompose(&w, 3000.0, 2400.0, &fx);

        let expected_total = 2400.0 * 0.92 - 3000.0 * 0.88 + (100.0 * 12.0) * 0.90;
        assert!((dec.total() - expected_total).abs() < 1e-9);
        assert!((dec.realized_price + dec.unrealized_price + dec.realized_fx + dec.unrealized_fx
                 - expected_total).abs() < 1e-9);
    }

    #[test]
    fn fully_exited_position_puts_all_fx_in_realized() {
        let mut at = BTreeMap::new();
        at.insert(d(2026, 6, 15), 0.90);
        let fx = FxLookup { f0: 0.88, f1: 0.92, at_trade: at };
        let all = vec![trade(1, true, 100.0, 10.0), trade(15, false, 100.0, 12.0)];
        let w = walk_instrument(&all, d(2026, 6, 10), d(2026, 6, 30));
        let dec = decompose(&w, 1000.0, 0.0, &fx);
        assert_eq!(w.basis_end.qty, 0.0);
        assert!(dec.unrealized_fx.abs() < 1e-12, "unrealized FX on an empty holding is nonsense");
        assert!(dec.realized_fx.abs() > 0.0);
    }

    #[test]
    fn opened_and_closed_inside_the_period_reports_no_unrealized_fx() {
        let mut at = BTreeMap::new();
        at.insert(d(2026, 6, 12), 0.89);
        at.insert(d(2026, 6, 20), 0.93);
        let fx = FxLookup { f0: 0.88, f1: 0.92, at_trade: at };
        let all = vec![trade(12, true, 50.0, 20.0), trade(20, false, 50.0, 22.0)];
        let w = walk_instrument(&all, d(2026, 6, 10), d(2026, 6, 30));
        let dec = decompose(&w, 0.0, 0.0, &fx);
        assert!(dec.unrealized_fx.abs() < 1e-12);
        assert!(!dec.fx_split_imprecise, "a closed round trip is exact, not imprecise");
    }

    #[test]
    fn partial_sale_after_a_mid_period_purchase_is_flagged() {
        let mut at = BTreeMap::new();
        at.insert(d(2026, 6, 12), 0.89);
        at.insert(d(2026, 6, 20), 0.93);
        let fx = FxLookup { f0: 0.88, f1: 0.92, at_trade: at };
        let all = vec![trade(1, true, 100.0, 10.0), trade(12, true, 50.0, 11.0), trade(20, false, 30.0, 12.0)];
        let w = walk_instrument(&all, d(2026, 6, 10), d(2026, 6, 30));
        let dec = decompose(&w, 1000.0, 1400.0, &fx);
        assert!(dec.fx_split_imprecise);
    }

    #[test]
    fn a_trade_date_with_no_fx_rate_is_reported() {
        let fx = FxLookup { f0: 0.88, f1: 0.92, at_trade: BTreeMap::new() };
        let all = vec![trade(1, true, 100.0, 10.0), trade(15, false, 40.0, 12.0)];
        let w = walk_instrument(&all, d(2026, 6, 10), d(2026, 6, 30));
        let dec = decompose(&w, 1000.0, 700.0, &fx);
        assert_eq!(dec.fx_missing, vec![d(2026, 6, 15)]);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p analytics pnl`
Expected: FAIL to compile — `cannot find type FxLookup in this scope`.

- [ ] **Step 3: Write the implementation**

Append to the non-test part of `crates/analytics/src/pnl.rs`:

```rust
/// FX rates for one currency over one period. `f0`/`f1` are the snapshot rates
/// at the period endpoints; `at_trade` carries daily rates for trade dates.
/// For EUR, every rate is 1.0 and the map is empty.
#[derive(Debug, Clone)]
pub struct FxLookup {
    pub f0: f64,
    pub f1: f64,
    pub at_trade: BTreeMap<NaiveDate, f64>,
}

impl FxLookup {
    pub fn eur() -> Self { Self { f0: 1.0, f1: 1.0, at_trade: BTreeMap::new() } }

    /// Exact-date lookup. No nearest-date fallback: a silently approximated
    /// rate is precisely the error the reconciliation exists to catch.
    pub fn rate(&self, d: NaiveDate) -> Option<f64> {
        if self.f0 == 1.0 && self.f1 == 1.0 && self.at_trade.is_empty() {
            return Some(1.0); // EUR
        }
        self.at_trade.get(&d).copied()
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Decomp {
    pub realized_price: f64,
    pub unrealized_price: f64,
    pub realized_fx: f64,
    pub unrealized_fx: f64,
    /// A partial sale followed a mid-period purchase: weighted-average costing
    /// cannot attribute the purchase's FX exactly. Disclosed, never smoothed.
    pub fx_split_imprecise: bool,
    /// Trade dates with no FX rate available.
    pub fx_missing: Vec<NaiveDate>,
}

impl Decomp {
    pub fn realized(&self) -> f64 { self.realized_price + self.realized_fx }
    pub fn unrealized(&self) -> f64 { self.unrealized_price + self.unrealized_fx }
    pub fn fx(&self) -> f64 { self.realized_fx + self.unrealized_fx }
    pub fn total(&self) -> f64 { self.realized() + self.unrealized() }
}

/// Decompose one instrument's period P&L into price and FX, each split into
/// realized and unrealized.
///
/// ```text
/// LocalP&L      = (v1 - v0) + Σ flows
/// Price         = LocalP&L × f0                       (split realized/unrealized
///                                                      by the walk's realized figure)
/// RealizedFX    = Σ_sells     CF × [F(trade) - f0]
/// UnrealizedFX  = v1 × [f1 - f0] + Σ_buys CF × [F(trade) - f0]
/// ```
///
/// The four sum to `v1·f1 - v0·f0 + Σ CF·F(trade)` exactly, so period
/// additivity — and therefore the reconciliation to ΔNAV — is preserved.
///
/// When the position is closed at `t1`, purchase-flow FX moves to realized:
/// reporting unrealized FX on a holding of nothing is meaningless.
pub fn decompose(w: &Walk, v0_local: f64, v1_local: f64, fx: &FxLookup) -> Decomp {
    let mut out = Decomp::default();

    let flow_sum: f64 = w.buys.iter().chain(w.sells.iter()).map(|f| f.amount_local).sum();
    let local_pnl = (v1_local - v0_local) + flow_sum;
    out.realized_price = w.realized_local * fx.f0;
    out.unrealized_price = (local_pnl - w.realized_local) * fx.f0;

    let mut missing: Vec<NaiveDate> = Vec::new();
    let fx_on = |flows: &[Flow], missing: &mut Vec<NaiveDate>| -> f64 {
        flows.iter().map(|f| match fx.rate(f.date) {
            Some(r) => f.amount_local * (r - fx.f0),
            None => { if !missing.contains(&f.date) { missing.push(f.date); } 0.0 }
        }).sum()
    };
    let sells_fx = fx_on(&w.sells, &mut missing);
    let buys_fx = fx_on(&w.buys, &mut missing);
    missing.sort();
    out.fx_missing = missing;

    out.realized_fx = sells_fx;
    out.unrealized_fx = v1_local * (fx.f1 - fx.f0) + buys_fx;

    if w.basis_end.qty <= 0.0 {
        // Position closed: nothing is left to carry an unrealized figure.
        out.realized_fx += out.unrealized_fx;
        out.unrealized_fx = 0.0;
    } else if !w.buys.is_empty() && !w.sells.is_empty() {
        out.fx_split_imprecise = true;
    }

    out
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p analytics pnl`
Expected: PASS, 11 tests.

- [ ] **Step 5: Add the exactness property test**

Append to the `tests` module:

```rust
    /// Exhaustive small-grid check that the four components always sum to the
    /// EUR total. This is the identity the whole reconciliation rests on.
    #[test]
    fn decomposition_is_exact_across_a_grid() {
        for &f0 in &[0.5, 0.88, 1.0, 1.4] {
            for &f1 in &[0.5, 0.92, 1.0, 1.4] {
                for &ft in &[0.5, 0.90, 1.0, 1.4] {
                    for &(qty_buy, qty_sell) in &[(100.0, 0.0), (100.0, 40.0), (100.0, 100.0), (0.0, 0.0)] {
                        let mut at = BTreeMap::new();
                        at.insert(d(2026, 6, 15), ft);
                        at.insert(d(2026, 6, 16), ft);
                        let fx = FxLookup { f0, f1, at_trade: at };

                        let mut all = vec![trade(1, true, 200.0, 10.0)];
                        if qty_buy > 0.0 { all.push(trade(15, true, qty_buy, 11.0)); }
                        if qty_sell > 0.0 { all.push(trade(16, false, qty_sell, 12.0)); }

                        let w = walk_instrument(&all, d(2026, 6, 10), d(2026, 6, 30));
                        let v0 = 2000.0;
                        let v1 = w.basis_end.qty * 12.5;
                        let dec = decompose(&w, v0, v1, &fx);

                        let flows: f64 = w.buys.iter().chain(w.sells.iter())
                            .map(|f| f.amount_local * ft).sum();
                        let expected = v1 * f1 - v0 * f0 + flows;
                        assert!((dec.total() - expected).abs() < 1e-6,
                            "f0={f0} f1={f1} ft={ft} buy={qty_buy} sell={qty_sell}: {} vs {}",
                            dec.total(), expected);
                        assert!((dec.realized_fx + dec.unrealized_fx - dec.fx()).abs() < 1e-9);
                    }
                }
            }
        }
    }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p analytics pnl`
Expected: PASS, 12 tests.

- [ ] **Step 7: Commit**

```bash
git add crates/analytics/src/pnl.rs
git commit -m "feat(analytics): price/FX decomposition with realized-unrealized FX split"
```

---

### Task 4: Futures P&L

**Files:**
- Modify: `crates/analytics/src/pnl.rs`
- Test: inline

**Interfaces:**
- Consumes: `Decomp` from Task 3; `analytics::futures::{PriceConvention, decode_price}` (existing)
- Produces: `pub fn futures_pnl(v0_ccy: f64, v1_ccy: f64, realized_ccy: f64, fx: &FxLookup) -> Decomp`

**Background the implementer needs:** a future's `Valorisation Dev` in
`PORTEFEUILLE_NAV` is *variation margin* — accumulated unrealized P&L — not
market value. See `docs/superpowers/specs/2026-08-04-futures-exposure-design.md`.
So its period P&L is the change in that figure, with no cost basis involved.
`PORTEFEUILLE_NAV` quotes US Treasury futures in 32nds while `OPERATIONS` uses
true decimal; `decode_price` handles the snapshot side and must not be applied
to operations rows.

- [ ] **Step 1: Write the failing test**

Append to the `tests` module:

```rust
    #[test]
    fn futures_pnl_is_the_change_in_variation_margin() {
        let dec = futures_pnl(6750.0, 9100.0, 0.0, &eur_fx());
        assert!((dec.total() - 2350.0).abs() < 1e-9);
        assert!((dec.unrealized() - 2350.0).abs() < 1e-9);
        assert!(dec.realized().abs() < 1e-12);
    }

    #[test]
    fn closed_futures_contract_reports_its_result_as_realized() {
        // Margin ran to zero because the contract was closed for +1200.
        let dec = futures_pnl(800.0, 0.0, 1200.0, &eur_fx());
        assert!((dec.realized() - 1200.0).abs() < 1e-9);
        assert!((dec.total() - (1200.0 - 800.0)).abs() < 1e-9);
    }

    #[test]
    fn futures_fx_uses_the_closing_margin_balance() {
        let fx = FxLookup { f0: 0.88, f1: 0.92, at_trade: BTreeMap::new() };
        let dec = futures_pnl(1000.0, 1500.0, 0.0, &fx);
        // price effect at f0, FX on the closing balance
        assert!((dec.unrealized_price - 500.0 * 0.88).abs() < 1e-9);
        assert!((dec.unrealized_fx - 1500.0 * (0.92 - 0.88)).abs() < 1e-9);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p analytics pnl`
Expected: FAIL to compile — `cannot find function futures_pnl`.

- [ ] **Step 3: Write the implementation**

Append to the non-test part of `pnl.rs`:

```rust
/// P&L for one futures contract over a period.
///
/// `v0_ccy`/`v1_ccy` are the contract's `Valorisation Dev` at each snapshot,
/// which is variation margin — accumulated unrealized P&L — not market value.
/// `realized_ccy` is the result of contracts closed inside the period, which
/// the caller derives from `OPERATIONS`; pass 0.0 when none were closed.
///
/// There is no cost basis here: a future has no acquisition cost to average,
/// and the fund holds short futures, for which an average cost is meaningless.
pub fn futures_pnl(v0_ccy: f64, v1_ccy: f64, realized_ccy: f64, fx: &FxLookup) -> Decomp {
    let total_local = (v1_ccy - v0_ccy) + realized_ccy;
    Decomp {
        realized_price: realized_ccy * fx.f0,
        unrealized_price: (total_local - realized_ccy) * fx.f0,
        realized_fx: 0.0,
        unrealized_fx: v1_ccy * (fx.f1 - fx.f0),
        fx_split_imprecise: false,
        fx_missing: Vec::new(),
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p analytics pnl`
Expected: PASS, 15 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/analytics/src/pnl.rs
git commit -m "feat(analytics): futures P&L from variation margin"
```

---

### Task 5: Dimension grouping

**Files:**
- Modify: `crates/analytics/src/pnl.rs`
- Test: inline

**Interfaces:**
- Consumes: `Decomp`
- Produces:
  - `pnl::Dimension` enum with `parse(&str) -> Option<Self>` and variants `AssetClass, Country, Region, Sector, Industry, Currency, IssuerGroup`
  - `pnl::InstrumentPnl { isin, name, asset_class, country, region, sector, industry, currency, issuer_group, decomp: Decomp }`
  - `pnl::GroupPnl { key, realized_price, unrealized_price, realized_fx, unrealized_fx, realized, unrealized, fx, total, instruments: Vec<InstrumentPnl> }`
  - `pub fn group_by(rows: Vec<InstrumentPnl>, dim: Dimension) -> Vec<GroupPnl>`
  - `pub const UNCLASSIFIED: &str = "Unclassified";`
  - `pub fn asset_class_of(asset_type: &str) -> &'static str`

- [ ] **Step 1: Write the failing test**

Append to the `tests` module:

```rust
    fn inst(isin: &str, sector: Option<&str>, total: f64) -> InstrumentPnl {
        InstrumentPnl {
            isin: isin.into(), name: isin.into(),
            asset_class: "Equities".into(),
            country: Some("France".into()), region: Some("Europe".into()),
            sector: sector.map(|s| s.to_string()), industry: None,
            currency: "EUR".into(), issuer_group: Some("G".into()),
            decomp: Decomp { unrealized_price: total, ..Default::default() },
        }
    }

    #[test]
    fn groups_sum_their_instruments_and_sort_by_absolute_total() {
        let rows = vec![
            inst("A", Some("Industrials"), 100.0),
            inst("B", Some("Industrials"), 50.0),
            inst("C", Some("Utilities"), -400.0),
        ];
        let g = group_by(rows, Dimension::Sector);
        assert_eq!(g[0].key, "Utilities", "largest absolute mover leads");
        assert!((g[0].total + 400.0).abs() < 1e-9);
        assert_eq!(g[1].key, "Industrials");
        assert!((g[1].total - 150.0).abs() < 1e-9);
        assert_eq!(g[1].instruments.len(), 2);
    }

    #[test]
    fn missing_classification_groups_as_unclassified() {
        let g = group_by(vec![inst("A", None, 10.0)], Dimension::Sector);
        assert_eq!(g[0].key, UNCLASSIFIED);
    }

    #[test]
    fn dimension_parsing_accepts_the_documented_names() {
        assert!(matches!(Dimension::parse("asset_class"), Some(Dimension::AssetClass)));
        assert!(matches!(Dimension::parse("issuer_group"), Some(Dimension::IssuerGroup)));
        assert!(Dimension::parse("nonsense").is_none());
    }

    #[test]
    fn asset_class_maps_the_workbook_french_labels() {
        assert_eq!(asset_class_of("Action"), "Equities");
        assert_eq!(asset_class_of("Obligation"), "Bonds");
        assert_eq!(asset_class_of("Fonds"), "Funds");
        assert_eq!(asset_class_of("Future"), "Futures");
        assert_eq!(asset_class_of("Cash Acc"), "Cash");
        assert_eq!(asset_class_of("Margin Acc"), "Cash");
        assert_eq!(asset_class_of("Frais provisionnés"), "Fees");
        assert_eq!(asset_class_of("Provisions ordres"), "Provisions");
        assert_eq!(asset_class_of("Dividendes"), "Income");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p analytics pnl`
Expected: FAIL to compile — `cannot find type InstrumentPnl`.

- [ ] **Step 3: Write the implementation**

Append to the non-test part of `pnl.rs`:

```rust
pub const UNCLASSIFIED: &str = "Unclassified";

/// Map the workbook's French `Type d'actif` values onto reporting classes.
/// Unknown types fall through to "Other" rather than being dropped, so a new
/// asset type in a future workbook is visible instead of silently missing.
pub fn asset_class_of(asset_type: &str) -> &'static str {
    match asset_type {
        "Action" => "Equities",
        "Obligation" => "Bonds",
        "Fonds" => "Funds",
        "Future" => "Futures",
        "Cash Acc" | "Margin Acc" => "Cash",
        "Frais provisionnés" => "Fees",
        "Provisions ordres" => "Provisions",
        "Dividendes" => "Income",
        _ => "Other",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension { AssetClass, Country, Region, Sector, Industry, Currency, IssuerGroup }

impl Dimension {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "asset_class" => Self::AssetClass,
            "country" => Self::Country,
            "region" => Self::Region,
            "sector" => Self::Sector,
            "industry" => Self::Industry,
            "currency" => Self::Currency,
            "issuer_group" => Self::IssuerGroup,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct InstrumentPnl {
    pub isin: String,
    pub name: String,
    pub asset_class: String,
    pub country: Option<String>,
    pub region: Option<String>,
    pub sector: Option<String>,
    pub industry: Option<String>,
    pub currency: String,
    pub issuer_group: Option<String>,
    #[serde(flatten)]
    pub decomp: Decomp,
}

impl InstrumentPnl {
    fn key(&self, dim: Dimension) -> String {
        let v = match dim {
            Dimension::AssetClass => Some(self.asset_class.clone()),
            Dimension::Country => self.country.clone(),
            Dimension::Region => self.region.clone(),
            Dimension::Sector => self.sector.clone(),
            Dimension::Industry => self.industry.clone(),
            Dimension::Currency => Some(self.currency.clone()),
            Dimension::IssuerGroup => self.issuer_group.clone(),
        };
        v.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| UNCLASSIFIED.to_string())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GroupPnl {
    pub key: String,
    pub realized_price: f64,
    pub unrealized_price: f64,
    pub realized_fx: f64,
    pub unrealized_fx: f64,
    pub realized: f64,
    pub unrealized: f64,
    pub fx: f64,
    pub total: f64,
    pub instruments: Vec<InstrumentPnl>,
}

/// Group instruments by `dim`, sorted by absolute total descending so the
/// biggest movers lead regardless of sign.
pub fn group_by(rows: Vec<InstrumentPnl>, dim: Dimension) -> Vec<GroupPnl> {
    let mut m: BTreeMap<String, Vec<InstrumentPnl>> = BTreeMap::new();
    for r in rows { m.entry(r.key(dim)).or_default().push(r); }

    let mut out: Vec<GroupPnl> = m.into_iter().map(|(key, instruments)| {
        let mut g = GroupPnl {
            key, realized_price: 0.0, unrealized_price: 0.0, realized_fx: 0.0,
            unrealized_fx: 0.0, realized: 0.0, unrealized: 0.0, fx: 0.0, total: 0.0,
            instruments,
        };
        for i in &g.instruments {
            g.realized_price += i.decomp.realized_price;
            g.unrealized_price += i.decomp.unrealized_price;
            g.realized_fx += i.decomp.realized_fx;
            g.unrealized_fx += i.decomp.unrealized_fx;
        }
        g.realized = g.realized_price + g.realized_fx;
        g.unrealized = g.unrealized_price + g.unrealized_fx;
        g.fx = g.realized_fx + g.unrealized_fx;
        g.total = g.realized + g.unrealized;
        g.instruments.sort_by(|a, b| b.decomp.total().abs().total_cmp(&a.decomp.total().abs()));
        g
    }).collect();

    out.sort_by(|a, b| b.total.abs().total_cmp(&a.total.abs()));
    out
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p analytics pnl`
Expected: PASS, 19 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/analytics/src/pnl.rs
git commit -m "feat(analytics): dimension grouping for P&L"
```

---

### Task 6: Reconciliation

**Files:**
- Modify: `crates/analytics/src/pnl.rs`
- Test: inline

**Interfaces:**
- Consumes: `GroupPnl`
- Produces:
  - `pnl::NavPoint { date: NaiveDate, aum: f64, shares: f64, nav: f64 }`
  - `pnl::Reconciliation { investment_pnl, cash_and_margin, accrued_fees, provisions, dividend_income, total_pnl, aum_change, net_flows, residual, gross, within_tolerance }`
  - `pub const RESIDUAL_TOLERANCE: f64 = 0.001;`
  - `pub fn net_flows(nav: &[NavPoint], t0: NaiveDate, t1: NaiveDate) -> f64`
  - `pub fn reconcile(investment_pnl, cash_and_margin, accrued_fees, provisions, dividend_income, aum_change, net_flows) -> Reconciliation`

- [ ] **Step 1: Write the failing test**

Append to the `tests` module:

```rust
    #[test]
    fn net_flows_are_derived_from_share_count_changes() {
        let nav = vec![
            NavPoint { date: d(2026, 6, 10), aum: 1000.0, shares: 10.0, nav: 100.0 },
            NavPoint { date: d(2026, 6, 11), aum: 2020.0, shares: 20.0, nav: 101.0 },
            NavPoint { date: d(2026, 6, 12), aum: 1530.0, shares: 15.0, nav: 102.0 },
        ];
        // +10 shares @ 101 = +1010; -5 shares @ 102 = -510; net +500
        let f = net_flows(&nav, d(2026, 6, 10), d(2026, 6, 12));
        assert!((f - 500.0).abs() < 1e-9);
    }

    #[test]
    fn flows_before_the_period_are_excluded() {
        let nav = vec![
            NavPoint { date: d(2026, 6, 1), aum: 100.0, shares: 1.0, nav: 100.0 },
            NavPoint { date: d(2026, 6, 5), aum: 500.0, shares: 5.0, nav: 100.0 },
            NavPoint { date: d(2026, 6, 20), aum: 600.0, shares: 6.0, nav: 100.0 },
        ];
        let f = net_flows(&nav, d(2026, 6, 10), d(2026, 6, 30));
        assert!((f - 100.0).abs() < 1e-9);
    }

    #[test]
    fn a_small_residual_reads_as_reconciled() {
        // lines sum to 1000, AUM change less flows is 1000.4 -> residual 0.4
        let r = reconcile(900.0, 50.0, -30.0, 5.0, 75.0, 1500.4, 500.0);
        assert!((r.total_pnl - 1000.0).abs() < 1e-9);
        assert!((r.residual - 0.4).abs() < 1e-9);
        assert!(r.within_tolerance);
    }

    #[test]
    fn a_large_residual_breaches_tolerance() {
        let r = reconcile(900.0, 50.0, -30.0, 5.0, 75.0, 1600.0, 500.0);
        assert!((r.residual - 100.0).abs() < 1e-9);
        assert!(!r.within_tolerance);
    }

    #[test]
    fn tolerance_uses_absolute_lines_so_offsetting_periods_are_not_false_breaches() {
        // Lines net to ~0 but are individually large; a 1.0 residual must pass.
        let r = reconcile(5000.0, 0.0, 0.0, 0.0, -5000.0, 1.0, 0.0);
        assert!(r.gross >= 10000.0);
        assert!(r.within_tolerance);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p analytics pnl`
Expected: FAIL to compile — `cannot find type NavPoint`.

- [ ] **Step 3: Write the implementation**

Append to the non-test part of `pnl.rs`:

```rust
/// Residual reads as reconciled at or below this fraction of gross P&L.
pub const RESIDUAL_TOLERANCE: f64 = 0.001;

#[derive(Debug, Clone, Copy)]
pub struct NavPoint { pub date: NaiveDate, pub aum: f64, pub shares: f64, pub nav: f64 }

/// Subscriptions and redemptions are not recorded directly. Derive them from
/// the daily share count: a change in shares priced at that day's NAV is the
/// net flow, exact for a daily-dealing fund priced at the same NAV.
/// Flows are summed over `(t0, t1]`.
pub fn net_flows(nav: &[NavPoint], t0: NaiveDate, t1: NaiveDate) -> f64 {
    let mut sorted: Vec<&NavPoint> = nav.iter().filter(|p| p.date <= t1).collect();
    sorted.sort_by_key(|p| p.date);
    sorted.windows(2)
        .filter(|w| w[1].date > t0)
        .map(|w| (w[1].shares - w[0].shares) * w[1].nav)
        .sum()
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Reconciliation {
    pub investment_pnl: f64,
    pub cash_and_margin: f64,
    pub accrued_fees: f64,
    pub provisions: f64,
    pub dividend_income: f64,
    pub total_pnl: f64,
    pub aum_change: f64,
    pub net_flows: f64,
    pub residual: f64,
    pub gross: f64,
    pub within_tolerance: bool,
}

/// Tie the P&L lines to the fund's own AUM movement.
///
/// The residual is always computed and always returned; `within_tolerance`
/// governs how it is presented, never whether it is shown. `gross` uses
/// absolute values so a period of large offsetting gains and losses does not
/// make every residual look like a breach.
pub fn reconcile(
    investment_pnl: f64,
    cash_and_margin: f64,
    accrued_fees: f64,
    provisions: f64,
    dividend_income: f64,
    aum_change: f64,
    net_flows: f64,
) -> Reconciliation {
    let total_pnl = investment_pnl + cash_and_margin + accrued_fees + provisions + dividend_income;
    let residual = (aum_change - net_flows) - total_pnl;
    let gross = investment_pnl.abs() + cash_and_margin.abs() + accrued_fees.abs()
        + provisions.abs() + dividend_income.abs();
    let within_tolerance = gross <= 0.0 || residual.abs() <= RESIDUAL_TOLERANCE * gross;
    Reconciliation {
        investment_pnl, cash_and_margin, accrued_fees, provisions, dividend_income,
        total_pnl, aum_change, net_flows, residual, gross, within_tolerance,
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p analytics pnl`
Expected: PASS, 24 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/analytics/src/pnl.rs
git commit -m "feat(analytics): NAV reconciliation with always-visible residual"
```

---

### Task 7: Bloomberg request workbook

**Files:**
- Create: `crates/ingest/src/bloomberg.rs`
- Modify: `crates/ingest/src/lib.rs`, `crates/ingest/Cargo.toml`
- Test: `crates/ingest/tests/bloomberg.rs`

**Interfaces:**
- Consumes: nothing
- Produces:
  - `ingest::bloomberg::RequestItem { isin: String, ticker: String }`
  - `pub fn build_request(items: &[RequestItem], currencies: &[String], from: NaiveDate, to: NaiveDate) -> anyhow::Result<Vec<u8>>`

- [ ] **Step 1: Add the dependency**

```bash
cargo add rust_xlsxwriter@0.97 -p ingest
cargo add anyhow --workspace 2>/dev/null || true
```

Confirm `crates/ingest/Cargo.toml` now lists `rust_xlsxwriter = "0.97"`. If
`anyhow` is not already a workspace dependency available to `ingest`, add
`anyhow = { workspace = true }` to `[dependencies]` by hand.

- [ ] **Step 2: Write the failing test**

Create `crates/ingest/tests/bloomberg.rs`:

```rust
use chrono::NaiveDate;
use ingest::bloomberg::{build_request, RequestItem};

fn d(y: i32, m: u32, dd: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, dd).unwrap() }

#[test]
fn request_workbook_has_the_three_expected_sheets() {
    let items = vec![RequestItem { isin: "FR0000121014".into(), ticker: "MC FP Equity".into() }];
    let bytes = build_request(&items, &["USD".into(), "GBP".into()], d(2025, 3, 18), d(2026, 7, 24)).unwrap();

    let mut wb: calamine::Xlsx<_> =
        calamine::Xlsx::new(std::io::Cursor::new(bytes)).expect("valid xlsx");
    let names = calamine::Reader::sheet_names(&wb);
    assert!(names.iter().any(|n| n == "REFS"));
    assert!(names.iter().any(|n| n == "FX"));
    assert!(names.iter().any(|n| n == "README"));

    let refs = calamine::Reader::worksheet_range(&mut wb, "REFS").unwrap();
    let header: Vec<String> = refs.rows().next().unwrap().iter().map(|c| c.to_string()).collect();
    assert_eq!(header[0], "isin");
    assert_eq!(header[1], "ticker");
    assert_eq!(header[2], "country_of_risk");
    assert_eq!(header[3], "gics_sector");
    assert_eq!(header[4], "gics_industry");

    let row1: Vec<String> = refs.rows().nth(1).unwrap().iter().map(|c| c.to_string()).collect();
    assert_eq!(row1[0], "FR0000121014");
    assert_eq!(row1[1], "MC FP Equity");
}

#[test]
fn an_empty_request_still_produces_a_readable_workbook() {
    let bytes = build_request(&[], &[], d(2025, 3, 18), d(2026, 7, 24)).unwrap();
    let wb: calamine::Xlsx<_> = calamine::Xlsx::new(std::io::Cursor::new(bytes)).expect("valid xlsx");
    assert!(calamine::Reader::sheet_names(&wb).iter().any(|n| n == "REFS"));
}
```

`calamine` must be available to the test. Add it under `[dev-dependencies]` in
`crates/ingest/Cargo.toml` if it is not already inherited — it is a normal
dependency of the crate, so it will be.

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p ingest --test bloomberg`
Expected: FAIL to compile — `could not find bloomberg in ingest`.

- [ ] **Step 4: Write the implementation**

Create `crates/ingest/src/bloomberg.rs`:

```rust
//! The Bloomberg round-trip workbook.
//!
//! The Excel add-in resolves `BDP`/`BDH` only inside Excel on a machine with a
//! logged-in Terminal, so a server process cannot query it. The tool therefore
//! writes a workbook of formulas, the user opens and saves it, and uploads it
//! back. Same shape as the weekly CTD companion file.

use chrono::NaiveDate;
use rust_xlsxwriter::{Format, Formula, Workbook};

#[derive(Debug, Clone)]
pub struct RequestItem { pub isin: String, pub ticker: String }

/// Build the request workbook. `items` are instruments still missing a
/// classification; `currencies` are the non-EUR currencies held.
pub fn build_request(
    items: &[RequestItem],
    currencies: &[String],
    from: NaiveDate,
    to: NaiveDate,
) -> anyhow::Result<Vec<u8>> {
    let mut wb = Workbook::new();
    let bold = Format::new().set_bold();

    // ---- REFS ----
    let s = wb.add_worksheet().set_name("REFS")?;
    for (c, h) in ["isin", "ticker", "country_of_risk", "gics_sector", "gics_industry"].iter().enumerate() {
        s.write_string_with_format(0, c as u16, *h, &bold)?;
    }
    s.set_column_width(0, 16)?;
    s.set_column_width(1, 24)?;
    for (i, it) in items.iter().enumerate() {
        let r = (i + 1) as u32;
        s.write_string(r, 0, &it.isin)?;
        s.write_string(r, 1, &it.ticker)?;
        let row = r + 1; // 1-based for the formula text
        s.write_formula(r, 2, Formula::new(format!("=BDP(B{row},\"CNTRY_OF_RISK\")")))?;
        s.write_formula(r, 3, Formula::new(format!("=BDP(B{row},\"GICS_SECTOR_NAME\")")))?;
        s.write_formula(r, 4, Formula::new(format!("=BDP(B{row},\"GICS_INDUSTRY_GROUP_NAME\")")))?;
    }

    // ---- FX ----
    let f = wb.add_worksheet().set_name("FX")?;
    f.write_string_with_format(0, 0, "start", &bold)?;
    f.write_string(1, 0, from.to_string())?;
    f.write_string_with_format(2, 0, "end", &bold)?;
    // Dates are written as text and read back as text: Excel locale settings
    // otherwise reinterpret them, and BDH accepts the ISO form.
    f.write_string(3, 0, to.to_string())?;
    for (i, ccy) in currencies.iter().enumerate() {
        let c = (i + 1) as u16;
        f.write_string_with_format(0, c, ccy, &bold)?;
        f.write_formula(1, c, Formula::new(format!(
            "=BDH(\"EUR{ccy} Curncy\",\"PX_LAST\",$A$2,$A$4)"
        )))?;
    }

    // ---- README ----
    let r = wb.add_worksheet().set_name("README")?;
    r.set_column_width(0, 100)?;
    let lines = [
        "Borobudur Risk - Bloomberg classification request".to_string(),
        format!("Exported {from} to {to}."),
        String::new(),
        "1. Open this file in Excel on a machine with a logged-in Bloomberg Terminal.".into(),
        "2. Wait for every formula to resolve. #N/A cells are reported on upload and not stored.".into(),
        "3. Save the file (keep .xlsx format).".into(),
        "4. Upload it on the Data page, Bloomberg classification panel.".into(),
        String::new(),
        "REFS: one row per instrument still missing a country or GICS classification.".into(),
        "FX:   daily EUR cross rates. The tool inverts these to euros-per-unit and".into(),
        "      cross-checks them against the NAV Recap's own Change column.".into(),
    ];
    for (i, l) in lines.iter().enumerate() {
        r.write_string(i as u32, 0, l)?;
    }

    Ok(wb.save_to_buffer()?)
}
```

Add `pub mod bloomberg;` to `crates/ingest/src/lib.rs`, next to
`pub mod futures_file;`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p ingest --test bloomberg`
Expected: PASS, 2 tests.

- [ ] **Step 6: Commit**

```bash
git add crates/ingest/src/bloomberg.rs crates/ingest/src/lib.rs crates/ingest/Cargo.toml Cargo.lock crates/ingest/tests/bloomberg.rs
git commit -m "feat(ingest): Bloomberg request workbook generation"
```

---

### Task 8: Bloomberg upload parsing and the FX inversion check

**Files:**
- Modify: `crates/ingest/src/bloomberg.rs`
- Test: `crates/ingest/tests/bloomberg.rs`

**Interfaces:**
- Consumes: `build_request` from Task 7 (tests round-trip through it)
- Produces:
  - `ingest::bloomberg::ClassificationRow { isin, country, sector, industry }`
  - `ingest::bloomberg::FxObservation { date: NaiveDate, currency: String, rate_to_eur: f64 }`
  - `ingest::bloomberg::ParsedResponse { classifications: Vec<ClassificationRow>, fx: Vec<FxObservation>, skipped: Vec<RowError> }`
  - `pub fn parse_response(bytes: &[u8]) -> Result<ParsedResponse, ParseFailure>`
  - `pub fn region_for(country: &str) -> Option<&'static str>`

**Note on `#N/A`:** `calamine` surfaces an unresolved Bloomberg cell as
`Data::Error(..)`, and a stale file may carry the literal string `#N/A`. Treat
both as unresolved. `skipped` collects them for reporting; they are never stored.

- [ ] **Step 1: Write the failing test**

Append to `crates/ingest/tests/bloomberg.rs`:

```rust
use ingest::bloomberg::{parse_response, region_for};

/// Build a response workbook the way Excel would leave it: values, not formulas.
fn response_xlsx(refs: &[(&str, &str, &str, &str)], fx: &[(&str, &str, f64)]) -> Vec<u8> {
    let mut wb = rust_xlsxwriter::Workbook::new();
    let s = wb.add_worksheet().set_name("REFS").unwrap();
    for (c, h) in ["isin", "ticker", "country_of_risk", "gics_sector", "gics_industry"].iter().enumerate() {
        s.write_string(0, c as u16, *h).unwrap();
    }
    for (i, (isin, country, sector, industry)) in refs.iter().enumerate() {
        let r = (i + 1) as u32;
        s.write_string(r, 0, *isin).unwrap();
        s.write_string(r, 1, "T").unwrap();
        s.write_string(r, 2, *country).unwrap();
        s.write_string(r, 3, *sector).unwrap();
        s.write_string(r, 4, *industry).unwrap();
    }
    let f = wb.add_worksheet().set_name("FX").unwrap();
    f.write_string(0, 0, "date").unwrap();
    f.write_string(0, 1, "USD").unwrap();
    for (i, (date, ccy, rate)) in fx.iter().enumerate() {
        let r = (i + 1) as u32;
        f.write_string(r, 0, *date).unwrap();
        let _ = ccy;
        f.write_number(r, 1, *rate).unwrap();
    }
    wb.save_to_buffer().unwrap()
}

#[test]
fn parses_classifications_and_derives_region() {
    let bytes = response_xlsx(
        &[("FR0000121014", "France", "Consumer Discretionary", "Textiles Apparel & Luxury Goods")],
        &[],
    );
    let out = parse_response(&bytes).unwrap();
    assert_eq!(out.classifications.len(), 1);
    let c = &out.classifications[0];
    assert_eq!(c.isin, "FR0000121014");
    assert_eq!(c.country.as_deref(), Some("France"));
    assert_eq!(region_for("France"), Some("Europe"));
}

#[test]
fn unresolved_cells_are_skipped_and_reported_never_stored() {
    let bytes = response_xlsx(&[("IE00BYTBXV33", "#N/A", "#N/A N/A", "Industrials")], &[]);
    let out = parse_response(&bytes).unwrap();
    // The row survives for its usable field, but the unresolved ones are None.
    let c = out.classifications.iter().find(|c| c.isin == "IE00BYTBXV33").unwrap();
    assert!(c.country.is_none());
    assert!(c.sector.is_none());
    assert_eq!(c.industry.as_deref(), Some("Industrials"));
    assert!(!out.skipped.is_empty(), "unresolved cells must be reported");
}

#[test]
fn fx_rates_are_inverted_to_euros_per_unit() {
    // Bloomberg EURUSD = dollars per euro. 1.1379 USD per EUR -> 0.8788 EUR per USD.
    let bytes = response_xlsx(&[], &[("2026-07-24", "USD", 1.1379)]);
    let out = parse_response(&bytes).unwrap();
    let obs = out.fx.iter().find(|o| o.currency == "USD").unwrap();
    assert!((obs.rate_to_eur - 1.0 / 1.1379).abs() < 1e-9);
}

#[test]
fn a_non_positive_fx_rate_is_rejected_not_inverted() {
    let bytes = response_xlsx(&[], &[("2026-07-24", "USD", 0.0)]);
    let out = parse_response(&bytes).unwrap();
    assert!(out.fx.is_empty());
    assert!(!out.skipped.is_empty());
}
```

Add `rust_xlsxwriter` to `[dev-dependencies]` in `crates/ingest/Cargo.toml` if
the test cannot see it (it is already a normal dependency, so it can).

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ingest --test bloomberg`
Expected: FAIL to compile — `cannot find function parse_response`.

- [ ] **Step 3: Write the implementation**

Append to `crates/ingest/src/bloomberg.rs`:

```rust
use crate::{ParseFailure, RowError};
use calamine::{Data, Range, Reader, Xlsx};
use std::io::Cursor;

#[derive(Debug, Clone)]
pub struct ClassificationRow {
    pub isin: String,
    pub country: Option<String>,
    pub sector: Option<String>,
    pub industry: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FxObservation {
    pub date: NaiveDate,
    pub currency: String,
    pub rate_to_eur: f64,
}

#[derive(Debug, Default)]
pub struct ParsedResponse {
    pub classifications: Vec<ClassificationRow>,
    pub fx: Vec<FxObservation>,
    /// Cells that did not resolve, reported so the user can fix and re-upload.
    pub skipped: Vec<RowError>,
}

/// True for a cell Bloomberg did not resolve.
fn unresolved(d: Option<&Data>) -> bool {
    match d {
        None | Some(Data::Empty) => true,
        Some(Data::Error(_)) => true,
        Some(Data::String(s)) => {
            let t = s.trim();
            t.is_empty() || t.starts_with("#N/A") || t == "#VALUE!" || t == "#NAME?"
        }
        _ => false,
    }
}

fn text(r: &Range<Data>, row: u32, col: u32) -> Option<String> {
    let v = r.get_value((row, col));
    if unresolved(v) { return None; }
    v.map(|d| d.to_string().trim().to_string()).filter(|s| !s.is_empty())
}

/// Parse the workbook the user saved out of Excel. Values only — a file still
/// holding formulas has not been resolved and its cells read as unresolved.
pub fn parse_response(bytes: &[u8]) -> Result<ParsedResponse, ParseFailure> {
    let mut wb: Xlsx<_> = Xlsx::new(Cursor::new(bytes.to_vec()))
        .map_err(|e| ParseFailure::Workbook(e.to_string()))?;
    let mut out = ParsedResponse::default();

    if let Ok(refs) = wb.worksheet_range("REFS") {
        let end = refs.end().map(|(r, _)| r).unwrap_or(0);
        for row in 1..=end {
            let Some(isin) = text(&refs, row, 0) else { continue };
            let country = text(&refs, row, 2);
            let sector = text(&refs, row, 3);
            let industry = text(&refs, row, 4);
            for (col, name) in [(2u32, "country_of_risk"), (3, "gics_sector"), (4, "gics_industry")] {
                if unresolved(refs.get_value((row, col))) {
                    out.skipped.push(RowError {
                        sheet: "REFS".into(),
                        row: row + 1,
                        message: format!("{isin}: {name} did not resolve; not stored"),
                    });
                }
            }
            if country.is_some() || sector.is_some() || industry.is_some() {
                out.classifications.push(ClassificationRow { isin, country, sector, industry });
            }
        }
    }

    if let Ok(fx) = wb.worksheet_range("FX") {
        let end = fx.end().map(|(r, _)| r).unwrap_or(0);
        let width = fx.end().map(|(_, c)| c).unwrap_or(0);
        let currencies: Vec<(u32, String)> = (1..=width)
            .filter_map(|c| text(&fx, 0, c).map(|n| (c, n)))
            .collect();

        for row in 1..=end {
            let Some(dtxt) = text(&fx, row, 0) else { continue };
            let Some(date) = parse_any_date(&dtxt) else {
                out.skipped.push(RowError {
                    sheet: "FX".into(), row: row + 1,
                    message: format!("date: expected YYYY-MM-DD, got {dtxt:?}"),
                });
                continue;
            };
            for (col, ccy) in &currencies {
                let Some(v) = fx.get_value((row, *col)) else { continue };
                if unresolved(Some(v)) { continue; }
                let raw = match v {
                    Data::Float(f) => *f,
                    Data::Int(i) => *i as f64,
                    _ => continue,
                };
                if !(raw.is_finite() && raw > 0.0) {
                    out.skipped.push(RowError {
                        sheet: "FX".into(), row: row + 1,
                        message: format!("{ccy}: rate must be positive, got {raw}"),
                    });
                    continue;
                }
                // Bloomberg quotes EURXXX as units of XXX per EUR; the tool
                // needs EUR per unit, so invert.
                out.fx.push(FxObservation { date, currency: ccy.clone(), rate_to_eur: 1.0 / raw });
            }
        }
    }

    Ok(out)
}

fn parse_any_date(s: &str) -> Option<NaiveDate> {
    let t = s.trim();
    NaiveDate::parse_from_str(t, "%Y-%m-%d")
        .or_else(|_| NaiveDate::parse_from_str(t, "%d/%m/%Y"))
        .ok()
        .or_else(|| {
            // Excel serial left with its formatting stripped.
            let f: f64 = t.parse().ok()?;
            NaiveDate::from_ymd_opt(1899, 12, 30)?
                .checked_add_days(chrono::Days::new(f as u64))
        })
}

/// Region from country of risk. A fixed table, not fetched: it is reporting
/// policy, not market data. Unknown countries return None and group as
/// "Unclassified" rather than being forced into a wrong bucket.
pub fn region_for(country: &str) -> Option<&'static str> {
    let c = country.trim().to_ascii_uppercase();
    Some(match c.as_str() {
        "FRANCE" | "GERMANY" | "ITALY" | "SPAIN" | "NETHERLANDS" | "BELGIUM" | "AUSTRIA"
        | "PORTUGAL" | "IRELAND" | "LUXEMBOURG" | "FINLAND" | "GREECE" | "UNITED KINGDOM"
        | "SWITZERLAND" | "SWEDEN" | "NORWAY" | "DENMARK" | "POLAND" | "CZECH REPUBLIC" => "Europe",
        "UNITED STATES" | "CANADA" => "North America",
        "BRAZIL" | "MEXICO" | "CHILE" | "ARGENTINA" | "COLOMBIA" | "PERU" => "Latin America",
        "JAPAN" | "CHINA" | "HONG KONG" | "SOUTH KOREA" | "TAIWAN" | "SINGAPORE" | "INDIA"
        | "AUSTRALIA" | "NEW ZEALAND" | "INDONESIA" | "THAILAND" | "MALAYSIA" => "Asia Pacific",
        "SOUTH AFRICA" | "UNITED ARAB EMIRATES" | "SAUDI ARABIA" | "ISRAEL" | "TURKEY"
        | "QATAR" | "EGYPT" | "NIGERIA" | "MOROCCO" => "Middle East & Africa",
        _ => return None,
    })
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p ingest --test bloomberg`
Expected: PASS, 6 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/ingest/src/bloomberg.rs crates/ingest/tests/bloomberg.rs
git commit -m "feat(ingest): parse Bloomberg response, invert FX, derive region"
```

---

### Task 9: PAM reconciliation warning on import

**Files:**
- Modify: `crates/db/src/repo.rs` (inside `import_workbook`, after the futures seeding at :140)
- Test: `crates/db/tests/pam_check.rs`

**Interfaces:**
- Consumes: `analytics::pnl::{Trade, walk_instrument, is_buy}` from Task 2
- Produces: additional strings in the existing `ImportOutcome::warnings`

**Why this matters:** it validates the entire 2,050-row trade walk against the
administrator's independently computed PAM, on every import, forever. It is the
highest-value test in the feature and it runs in production, not just in CI.

- [ ] **Step 1: Write the failing test**

Create `crates/db/tests/pam_check.rs`, following the harness in
`crates/db/tests/futures_seeding.rs`:

```rust
mod common;

/// Importing the sample workbook must not report PAM drift: the engine's
/// weighted-average cost has to reproduce the administrator's own column.
#[tokio::test]
async fn sample_workbook_reconciles_to_its_own_pam_column() {
    let (_pg, pool) = common::pool().await;
    let bytes = std::fs::read("../ingest/tests/fixtures/sample.xlsx").unwrap();
    let wb = ingest::parse_workbook(&bytes).unwrap();
    let out = db::repo::import_workbook(&pool, "sample.xlsx", "sha-pam-1", &wb).await.unwrap();

    let drift: Vec<&String> = out.warnings.iter().filter(|w| w.contains("PAM")).collect();
    assert!(drift.is_empty(), "unexpected PAM drift: {drift:?}");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p db --test pam_check`
Expected: FAIL — the check does not exist yet, so this passes vacuously. **Make
it fail first** by temporarily asserting `assert!(!drift.is_empty())`, confirming
the test executes and the import runs, then restore the real assertion. Note the
observed behaviour in the commit message.

- [ ] **Step 3: Write the implementation**

Add to `crates/db/src/repo.rs`, and call it from `import_workbook` immediately
after `let warnings = seed_futures_contracts(&mut tx, &wb.positions).await?;`,
merging its output into `warnings`:

```rust
/// Cross-check the engine's weighted-average cost against the administrator's
/// PAM column for every cash position in the snapshot.
///
/// A mismatch means the trade history and the valuation file disagree, which
/// invalidates every realized-P&L figure derived from that history. Non-fatal:
/// it warns, it never blocks the weekly import.
fn pam_warnings(wb: &ingest::ParsedWorkbook) -> Vec<String> {
    use analytics::pnl::{is_buy, walk_instrument, Trade};

    let mut trades: Vec<Trade> = Vec::new();
    for o in &wb.operations {
        let (Some(isin), Some(qty), Some(px)) = (o.isin.as_deref(), o.quantity, o.net_price) else { continue };
        let Some(buy) = is_buy(&o.side) else { continue };
        trades.push(Trade {
            trade_date: o.trade_date,
            isin: isin.to_string(),
            is_buy: buy,
            quantity: qty.abs(),
            net_price: px,
            net_amount: o.net_amount.unwrap_or(0.0),
            currency: o.currency.clone().unwrap_or_default(),
        });
    }
    trades.sort_by_key(|t| t.trade_date);

    let mut warnings = Vec::new();
    for p in &wb.positions {
        // Futures have no cost basis; cash rows carry no PAM.
        if !matches!(p.asset_type.as_str(), "Action" | "Fonds" | "Obligation") { continue; }
        let (Some(pam), Some(qty)) = (p.avg_cost, p.quantity) else { continue };
        if qty.abs() < 1e-9 { continue; }
        let mine: Vec<Trade> = trades.iter().filter(|t| t.isin == p.isin).cloned().collect();
        if mine.is_empty() { continue; }

        let w = walk_instrument(&mine, chrono::NaiveDate::MIN, wb.nav_date);
        if w.oversold {
            warnings.push(format!("{}: sells exceed recorded buys; cost basis incomplete", p.isin));
            continue;
        }
        if w.basis_end.qty <= 0.0 { continue; }
        if (w.basis_end.avg_cost - pam).abs() > 0.01 {
            warnings.push(format!(
                "{}: PAM drift - workbook {:.6}, computed {:.6}",
                p.isin, pam, w.basis_end.avg_cost
            ));
        }
    }
    warnings
}
```

Wire it in:

```rust
    let mut warnings = seed_futures_contracts(&mut tx, &wb.positions).await?;
    warnings.extend(pam_warnings(wb));
```

`db` must depend on `analytics`. Check `crates/db/Cargo.toml`; it already does
(`seed_futures_contracts` calls `analytics::contract_root`).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p db --test pam_check`
Expected: PASS.

If PAM drift *is* reported for the sample workbook, **stop and investigate
before proceeding** — either the cost-basis convention is wrong or the sample's
operations history is incomplete. Both are findings worth reporting, not
warnings worth suppressing. Do not loosen the tolerance to make the test pass.

- [ ] **Step 5: Commit**

```bash
git add crates/db/src/repo.rs crates/db/tests/pam_check.rs
git commit -m "feat(db): warn on PAM drift between trade walk and workbook"
```

---

### Task 10: The `/api/pnl` endpoint

**Files:**
- Create: `crates/server/src/handlers/pnl.rs`
- Modify: `crates/server/src/handlers/mod.rs`, `crates/server/src/routes.rs`
- Test: `crates/server/tests/api_pnl.rs`

**Interfaces:**
- Consumes: everything from Tasks 1-6
- Produces: `GET /api/pnl?from&to&dimension`

**Response shape** (the frontend in Task 12 depends on this exactly):

```json
{ "empty": false,
  "period": { "requested_from", "requested_to", "actual_from", "actual_to", "snapshots" },
  "groups": [ { "key", "realized_price", "unrealized_price", "realized_fx", "unrealized_fx",
                "realized", "unrealized", "fx", "total",
                "instruments": [ { "isin", "name", "asset_class", "country", "region",
                                   "sector", "industry", "currency", "issuer_group",
                                   "realized_price", "unrealized_price", "realized_fx",
                                   "unrealized_fx", "fx_split_imprecise", "fx_missing" } ] } ],
  "reconciliation": { ... , "residual", "gross", "within_tolerance" },
  "unclassified": 8,
  "warnings": [] }
```

- [ ] **Step 1: Write the failing test**

Create `crates/server/tests/api_pnl.rs`, following `crates/server/tests/api_derivatives.rs`:

```rust
mod common;

#[tokio::test]
async fn pnl_snaps_to_snapshot_dates_and_reports_which_it_used() {
    let app = common::app_with_sample().await;
    let body: serde_json::Value = common::get_json(&app, "/api/pnl?from=2020-01-01&to=2030-01-01").await;
    assert_eq!(body["empty"], false);
    let p = &body["period"];
    assert!(p["actual_from"].is_string());
    assert!(p["actual_to"].is_string());
    assert!(p["snapshots"].as_i64().unwrap() >= 1);
}

#[tokio::test]
async fn reconciliation_residual_is_always_present() {
    let app = common::app_with_sample().await;
    let body: serde_json::Value = common::get_json(&app, "/api/pnl?from=2020-01-01&to=2030-01-01").await;
    let r = &body["reconciliation"];
    assert!(r["residual"].is_number(), "residual must always be returned");
    assert!(r["within_tolerance"].is_boolean());
    assert!(r["gross"].is_number());
}

#[tokio::test]
async fn groups_by_the_requested_dimension() {
    let app = common::app_with_sample().await;
    let body: serde_json::Value =
        common::get_json(&app, "/api/pnl?from=2020-01-01&to=2030-01-01&dimension=asset_class").await;
    let keys: Vec<String> = body["groups"].as_array().unwrap().iter()
        .map(|g| g["key"].as_str().unwrap().to_string()).collect();
    assert!(keys.iter().any(|k| k == "Equities"), "got {keys:?}");
}

#[tokio::test]
async fn an_unknown_dimension_is_a_bad_request() {
    let app = common::app_with_sample().await;
    let status = common::get_status(&app, "/api/pnl?from=2020-01-01&to=2030-01-01&dimension=zzz").await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn group_totals_equal_the_sum_of_their_instruments() {
    let app = common::app_with_sample().await;
    let body: serde_json::Value =
        common::get_json(&app, "/api/pnl?from=2020-01-01&to=2030-01-01&dimension=currency").await;
    for g in body["groups"].as_array().unwrap() {
        let sum: f64 = g["instruments"].as_array().unwrap().iter()
            .map(|i| i["realized_price"].as_f64().unwrap() + i["unrealized_price"].as_f64().unwrap()
                   + i["realized_fx"].as_f64().unwrap() + i["unrealized_fx"].as_f64().unwrap())
            .sum();
        assert!((g["total"].as_f64().unwrap() - sum).abs() < 1e-6);
    }
}
```

Read `crates/server/tests/common/mod.rs` first. If `app_with_sample`,
`get_json` or `get_status` do not exist under those names, use the equivalents
that are there and adjust these tests — do **not** add a parallel harness.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p server --test api_pnl`
Expected: FAIL — 404, the route does not exist.

- [ ] **Step 3: Write the handler**

Create `crates/server/src/handlers/pnl.rs`:

```rust
use crate::error::AppError;
use crate::state::AppState;
use analytics::pnl::{
    self, asset_class_of, decompose, futures_pnl, group_by, is_buy, net_flows, reconcile,
    Dimension, FxLookup, InstrumentPnl, NavPoint, Trade,
};
use axum::extract::{Query, State};
use axum::Json;
use chrono::NaiveDate;
use std::collections::{BTreeMap, HashMap};

#[derive(serde::Deserialize)]
pub struct PnlQuery {
    from: Option<String>,
    to: Option<String>,
    dimension: Option<String>,
}

fn parse_date(s: &str) -> Result<NaiveDate, AppError> {
    s.parse::<NaiveDate>().map_err(|_| AppError::BadRequest(format!("bad date: {s}")))
}

/// Nearest snapshot on or before `want`, falling back to the earliest
/// available. `dates` is descending, as `position_dates` returns it.
fn snap(dates: &[NaiveDate], want: NaiveDate) -> Option<NaiveDate> {
    dates.iter().copied().find(|d| *d <= want).or_else(|| dates.last().copied())
}

pub async fn get(State(st): State<AppState>, Query(q): Query<PnlQuery>) -> Result<Json<serde_json::Value>, AppError> {
    let dim = match q.dimension.as_deref() {
        None | Some("") => None,
        Some(s) => Some(Dimension::parse(s)
            .ok_or_else(|| AppError::BadRequest(format!("unknown dimension: {s}")))?),
    };

    let dates = db::repo::position_dates(&st.pool).await?;
    if dates.len() < 2 {
        return Ok(Json(serde_json::json!({
            "empty": true,
            "warnings": ["at least two imported NAV dates are needed to strike a P&L period"],
        })));
    }

    let requested_to = match &q.to { Some(s) => parse_date(s)?, None => dates[0] };
    let requested_from = match &q.from { Some(s) => parse_date(s)?, None => dates[dates.len() - 1] };
    if requested_from > requested_to {
        return Err(AppError::BadRequest("from is after to".into()));
    }
    // `dates` is non-empty (guarded above), so `snap` always yields a date;
    // the explicit error keeps that guarantee local instead of an unwrap.
    let nope = || AppError::Internal("no position snapshot dates".into());
    let t1 = snap(&dates, requested_to).ok_or_else(nope)?;
    let t0 = snap(&dates, requested_from).ok_or_else(nope)?;
    if t0 == t1 {
        return Ok(Json(serde_json::json!({
            "empty": true,
            "warnings": [format!("the requested range resolves to a single snapshot ({t0})")],
        })));
    }
    let snapshots = dates.iter().filter(|d| **d >= t0 && **d <= t1).count();

    let p0 = db::repo::positions_for(&st.pool, t0).await?;
    let p1 = db::repo::positions_for(&st.pool, t1).await?;
    let ops = db::repo::operations_all(&st.pool).await?;
    let divs = db::repo::dividends_all(&st.pool).await?;
    let refs = db::repo::refs_all(&st.pool).await?;
    let fx_rows = db::repo::fx_all(&st.pool).await?;
    let navs = db::repo::nav_rows(&st.pool).await?;

    let by_ref: HashMap<&str, &db::repo::InstrumentRef> =
        refs.iter().map(|r| (r.code.as_str(), r)).collect();

    // FX: daily history keyed by currency, plus snapshot rates from the file.
    let mut fx_by_ccy: BTreeMap<String, BTreeMap<NaiveDate, f64>> = BTreeMap::new();
    for r in &fx_rows {
        fx_by_ccy.entry(r.currency.clone()).or_default().insert(r.date, r.rate_to_eur);
    }
    let snap_rate = |rows: &[db::repo::PositionRecord], ccy: &str| -> f64 {
        if ccy == "EUR" { return 1.0; }
        rows.iter()
            .find(|p| p.currency.as_deref() == Some(ccy) && p.fx_rate.is_some_and(|f| f > 0.0))
            .and_then(|p| p.fx_rate)
            .or_else(|| rows.iter()
                .find(|p| p.currency.as_deref() == Some(ccy)
                       && p.valuation_ccy.is_some_and(|v| v.abs() > 1e-9))
                .and_then(|p| Some(p.valuation_eur? / p.valuation_ccy?)))
            .unwrap_or(1.0)
    };

    // Trades, keyed by ISIN.
    let mut trades_by_isin: HashMap<String, Vec<Trade>> = HashMap::new();
    let mut warnings: Vec<String> = Vec::new();
    for o in &ops {
        let (Some(isin), Some(qty), Some(px)) = (o.isin.as_deref(), o.quantity, o.net_price) else { continue };
        let Some(buy) = is_buy(&o.side) else {
            warnings.push(format!("{}: unrecognised side {:?}; trade ignored", isin, o.side));
            continue;
        };
        trades_by_isin.entry(isin.to_string()).or_default().push(Trade {
            trade_date: o.trade_date,
            isin: isin.to_string(),
            is_buy: buy,
            quantity: qty.abs(),
            net_price: px,
            net_amount: o.net_amount.unwrap_or(0.0),
            currency: o.currency.clone().unwrap_or_else(|| "EUR".into()),
        });
    }
    for v in trades_by_isin.values_mut() { v.sort_by_key(|t| t.trade_date); }

    let idx0: HashMap<&str, &db::repo::PositionRecord> = p0.iter().map(|p| (p.isin.as_str(), p)).collect();
    let idx1: HashMap<&str, &db::repo::PositionRecord> = p1.iter().map(|p| (p.isin.as_str(), p)).collect();
    let mut isins: Vec<&str> = idx0.keys().chain(idx1.keys()).copied().collect();
    isins.sort_unstable();
    isins.dedup();

    let mut rows: Vec<InstrumentPnl> = Vec::new();
    let (mut cash_and_margin, mut accrued_fees, mut provisions) = (0.0, 0.0, 0.0);

    for isin in isins {
        // `isin` came from these two maps, so one of them holds it.
        let Some(p) = idx1.get(isin).or_else(|| idx0.get(isin)).copied() else { continue };
        let class = asset_class_of(&p.asset_type);
        let ccy = p.currency.clone().unwrap_or_else(|| "EUR".into());

        let v0 = idx0.get(isin).and_then(|r| r.valuation_ccy).unwrap_or(0.0);
        let v1 = idx1.get(isin).and_then(|r| r.valuation_ccy).unwrap_or(0.0);
        let e0 = idx0.get(isin).and_then(|r| r.valuation_eur).unwrap_or(0.0);
        let e1 = idx1.get(isin).and_then(|r| r.valuation_eur).unwrap_or(0.0);

        // Balance-sheet classes are reconciliation lines, not instrument P&L.
        match class {
            "Cash" => { cash_and_margin += e1 - e0; continue; }
            "Fees" => { accrued_fees += e1 - e0; continue; }
            "Provisions" | "Income" => { provisions += e1 - e0; continue; }
            _ => {}
        }

        let fx = FxLookup {
            f0: snap_rate(&p0, &ccy),
            f1: snap_rate(&p1, &ccy),
            at_trade: fx_by_ccy.get(&ccy).cloned().unwrap_or_default(),
        };

        let decomp = if class == "Futures" {
            futures_pnl(v0, v1, 0.0, &fx)
        } else {
            let empty: Vec<Trade> = Vec::new();
            let t = trades_by_isin.get(isin).unwrap_or(&empty);
            let w = pnl::walk_instrument(t, t0, t1);
            if w.oversold {
                warnings.push(format!("{isin}: sells exceed recorded buys; figures incomplete"));
            }
            decompose(&w, v0, v1, &fx)
        };
        for d in &decomp.fx_missing {
            warnings.push(format!("{isin}: no FX rate for {ccy} on {d}; that flow is excluded"));
        }

        let r = by_ref.get(isin);
        rows.push(InstrumentPnl {
            isin: isin.to_string(),
            name: p.name.clone().unwrap_or_else(|| isin.to_string()),
            asset_class: class.to_string(),
            country: r.and_then(|r| r.country_of_risk.clone()),
            region: r.and_then(|r| r.region.clone()),
            sector: r.and_then(|r| r.gics_sector.clone()),
            industry: r.and_then(|r| r.gics_industry.clone()),
            currency: ccy,
            issuer_group: r.and_then(|r| r.issuer_group.clone()),
            decomp,
        });
    }

    let unclassified = rows.iter().filter(|r| r.country.is_none() && r.sector.is_none()).count();
    let investment_pnl: f64 = rows.iter().map(|r| r.decomp.total()).sum();
    let dividend_income: f64 = divs.iter()
        .filter(|d| d.provision_date > t0 && d.provision_date <= t1)
        .map(|d| d.amount)
        .sum();

    let points: Vec<NavPoint> = navs.iter()
        .map(|n| NavPoint { date: n.date, aum: n.aum, shares: n.shares, nav: n.nav })
        .collect();
    let aum0 = points.iter().find(|p| p.date == t0).map(|p| p.aum).unwrap_or(0.0);
    let aum1 = points.iter().find(|p| p.date == t1).map(|p| p.aum).unwrap_or(0.0);
    let recon = reconcile(
        investment_pnl, cash_and_margin, accrued_fees, provisions, dividend_income,
        aum1 - aum0, net_flows(&points, t0, t1),
    );

    let groups = match dim {
        Some(d) => group_by(rows, d),
        None => group_by(rows, Dimension::AssetClass),
    };

    Ok(Json(serde_json::json!({
        "empty": false,
        "period": {
            "requested_from": requested_from, "requested_to": requested_to,
            "actual_from": t0, "actual_to": t1, "snapshots": snapshots,
        },
        "groups": groups,
        "reconciliation": recon,
        "unclassified": unclassified,
        "warnings": warnings,
    })))
}
```

Register it: add `pub mod pnl;` to `crates/server/src/handlers/mod.rs` and

```rust
        .route("/api/pnl", get(handlers::pnl::get))
```

to `crates/server/src/routes.rs`, after the `/api/metrics/backtest` line.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p server --test api_pnl`
Expected: PASS, 5 tests.

Then `cargo test` for the whole workspace.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/handlers/pnl.rs crates/server/src/handlers/mod.rs crates/server/src/routes.rs crates/server/tests/api_pnl.rs
git commit -m "feat(server): GET /api/pnl with period snapping and reconciliation"
```

---

### Task 11: Bloomberg endpoints

**Files:**
- Create: `crates/server/src/handlers/bloomberg.rs`
- Modify: `crates/server/src/handlers/mod.rs`, `crates/server/src/routes.rs`
- Test: `crates/server/tests/api_bloomberg.rs`

**Interfaces:**
- Consumes: Tasks 1, 7, 8
- Produces:
  - `GET /api/bloomberg/request` → `.xlsx` bytes, `Content-Disposition: attachment`
  - `POST /api/bloomberg/upload` (multipart, field `file`) → `{ classified, fx_rows, skipped: RowError[], fx_check: { currency, date, workbook, bloomberg, drift }[] }`

- [ ] **Step 1: Write the failing test**

Create `crates/server/tests/api_bloomberg.rs`:

```rust
mod common;

#[tokio::test]
async fn request_endpoint_returns_a_readable_workbook() {
    let app = common::app_with_sample().await;
    let (status, ctype, bytes) = common::get_bytes(&app, "/api/bloomberg/request").await;
    assert_eq!(status, 200);
    assert!(ctype.contains("spreadsheet"), "got {ctype}");
    let wb: calamine::Xlsx<_> = calamine::Xlsx::new(std::io::Cursor::new(bytes)).expect("valid xlsx");
    assert!(calamine::Reader::sheet_names(&wb).iter().any(|n| n == "REFS"));
}

#[tokio::test]
async fn upload_stores_classifications_and_reports_unresolved_cells() {
    let app = common::app_with_sample().await;
    let mut wb = rust_xlsxwriter::Workbook::new();
    let s = wb.add_worksheet().set_name("REFS").unwrap();
    for (c, h) in ["isin", "ticker", "country_of_risk", "gics_sector", "gics_industry"].iter().enumerate() {
        s.write_string(0, c as u16, *h).unwrap();
    }
    s.write_string(1, 0, "FR0000121014").unwrap();
    s.write_string(1, 1, "MC FP Equity").unwrap();
    s.write_string(1, 2, "France").unwrap();
    s.write_string(1, 3, "Consumer Discretionary").unwrap();
    s.write_string(1, 4, "#N/A").unwrap();
    let bytes = wb.save_to_buffer().unwrap();

    let body: serde_json::Value =
        common::post_multipart_json(&app, "/api/bloomberg/upload", "resp.xlsx", &bytes).await;
    assert_eq!(body["classified"], 1);
    assert!(body["skipped"].as_array().unwrap().iter()
        .any(|e| e["message"].as_str().unwrap().contains("gics_industry")));

    // The stored value must now appear in the P&L grouping.
    let pnl: serde_json::Value =
        common::get_json(&app, "/api/pnl?from=2020-01-01&to=2030-01-01&dimension=sector").await;
    let keys: Vec<String> = pnl["groups"].as_array().unwrap().iter()
        .map(|g| g["key"].as_str().unwrap().to_string()).collect();
    assert!(keys.iter().any(|k| k == "Consumer Discretionary"), "got {keys:?}");
}
```

Add whatever `common` helpers are missing (`get_bytes`, `post_multipart_json`)
to `crates/server/tests/common/mod.rs`, modelled on the multipart helper the
CTD upload tests already use in `crates/server/tests/api_futures.rs`.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p server --test api_bloomberg`
Expected: FAIL — 404.

- [ ] **Step 3: Write the handlers**

Create `crates/server/src/handlers/bloomberg.rs`:

```rust
use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Multipart, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use ingest::bloomberg::{build_request, parse_response, region_for, RequestItem};
use std::collections::BTreeSet;

/// Export the request workbook for everything still unclassified.
pub async fn request(State(st): State<AppState>) -> Result<impl IntoResponse, AppError> {
    let dates = db::repo::position_dates(&st.pool).await?;
    let latest = dates.first().copied();
    let positions = match latest {
        Some(d) => db::repo::positions_for(&st.pool, d).await?,
        None => Vec::new(),
    };
    let refs = db::repo::refs_all(&st.pool).await?;
    let classified: BTreeSet<&str> = refs.iter()
        .filter(|r| r.country_of_risk.is_some() && r.gics_sector.is_some())
        .map(|r| r.code.as_str())
        .collect();

    let mut items: Vec<RequestItem> = Vec::new();
    let mut currencies: BTreeSet<String> = BTreeSet::new();
    for p in &positions {
        if let Some(c) = &p.currency {
            if c != "EUR" { currencies.insert(c.clone()); }
        }
        // Only instruments a Bloomberg ticker can identify, and only those
        // classification would actually apply to.
        if !matches!(p.asset_type.as_str(), "Action" | "Fonds" | "Obligation") { continue; }
        if classified.contains(p.isin.as_str()) { continue; }
        let Some(ticker) = p.ticker.clone() else { continue };
        items.push(RequestItem { isin: p.isin.clone(), ticker });
    }

    let navs = db::repo::nav_rows(&st.pool).await?;
    let from = navs.first().map(|n| n.date).unwrap_or_else(|| chrono::Utc::now().date_naive());
    let to = latest.unwrap_or_else(|| chrono::Utc::now().date_naive());

    let bytes = build_request(&items, &currencies.into_iter().collect::<Vec<_>>(), from, to)
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let mut h = HeaderMap::new();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"));
    h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_str(
        &format!("attachment; filename=\"bloomberg_request_{to}.xlsx\""))
        .map_err(|e| AppError::Internal(e.to_string()))?);
    Ok((StatusCode::OK, h, bytes))
}

pub async fn upload(State(st): State<AppState>, mut mp: Multipart) -> Result<Json<serde_json::Value>, AppError> {
    let mut bytes: Option<Vec<u8>> = None;
    while let Some(f) = mp.next_field().await.map_err(|e| AppError::BadRequest(e.to_string()))? {
        if f.name() == Some("file") {
            bytes = Some(f.bytes().await.map_err(|e| AppError::BadRequest(e.to_string()))?.to_vec());
        }
    }
    let bytes = bytes.ok_or_else(|| AppError::BadRequest("no file field".into()))?;

    let parsed = parse_response(&bytes).map_err(|e| match e {
        ingest::ParseFailure::Workbook(m) => AppError::BadRequest(m),
        ingest::ParseFailure::Rows(rows) => AppError::Rows(rows),
    })?;

    let classifications: Vec<(String, Option<String>, Option<String>, Option<String>, Option<String>)> =
        parsed.classifications.iter().map(|c| (
            c.isin.clone(),
            c.country.clone(),
            c.country.as_deref().and_then(region_for).map(|s| s.to_string()),
            c.sector.clone(),
            c.industry.clone(),
        )).collect();
    let classified = db::repo::classify_upsert_many(&st.pool, &classifications).await?;

    let fx_rows: Vec<db::repo::FxRow> = parsed.fx.iter().map(|o| db::repo::FxRow {
        date: o.date, currency: o.currency.clone(), rate_to_eur: o.rate_to_eur,
    }).collect();
    let fx_stored = db::repo::fx_upsert_many(&st.pool, &fx_rows).await?;

    // Cross-check the inversion against the workbook's own Change column at
    // every snapshot date. Disagreement means the pull is upside down.
    let mut fx_check = Vec::new();
    for d in db::repo::position_dates(&st.pool).await? {
        let positions = db::repo::positions_for(&st.pool, d).await?;
        for o in parsed.fx.iter().filter(|o| o.date == d) {
            let Some(book) = positions.iter()
                .find(|p| p.currency.as_deref() == Some(o.currency.as_str()) && p.fx_rate.is_some_and(|f| f > 0.0))
                .and_then(|p| p.fx_rate) else { continue };
            let drift = (book - o.rate_to_eur).abs() / book;
            if drift > 0.01 {
                fx_check.push(serde_json::json!({
                    "currency": o.currency, "date": d,
                    "workbook": book, "bloomberg": o.rate_to_eur, "drift": drift,
                }));
            }
        }
    }

    Ok(Json(serde_json::json!({
        "classified": classified,
        "fx_rows": fx_stored,
        "skipped": parsed.skipped,
        "fx_check": fx_check,
    })))
}
```

Register: `pub mod bloomberg;` in `handlers/mod.rs`, and in `routes.rs`:

```rust
        .route("/api/bloomberg/request", get(handlers::bloomberg::request))
        .route("/api/bloomberg/upload", axum::routing::post(handlers::bloomberg::upload))
```

Check `crates/server/src/error.rs` for the exact `AppError` variants. If
`Internal` or `Rows` are named differently, use the existing names — the CTD
upload handler in `handlers/futures.rs` shows the established mapping from
`ParseFailure`. Do not add new variants unless none fit.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p server --test api_bloomberg`
Expected: PASS, 2 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/handlers/bloomberg.rs crates/server/src/handlers/mod.rs crates/server/src/routes.rs crates/server/tests/api_bloomberg.rs crates/server/tests/common/mod.rs
git commit -m "feat(server): Bloomberg request export and response upload"
```

---

### Task 12: The P&L page

**Files:**
- Modify: `frontend/src/api.ts`, `frontend/src/App.tsx`
- Create: `frontend/src/pages/PnlPage.tsx`

**Interfaces:**
- Consumes: `GET /api/pnl` from Task 10
- Produces: the `/pnl` route

- [ ] **Step 1: Add the API types and client**

Append to `frontend/src/api.ts`:

```ts
export type PnlDimension =
  | "asset_class" | "country" | "region" | "sector" | "industry" | "currency" | "issuer_group";

export interface PnlInstrument {
  isin: string; name: string; asset_class: string;
  country: string | null; region: string | null;
  sector: string | null; industry: string | null;
  currency: string; issuer_group: string | null;
  realized_price: number; unrealized_price: number;
  realized_fx: number; unrealized_fx: number;
  fx_split_imprecise: boolean; fx_missing: string[];
}
export interface PnlGroup {
  key: string;
  realized_price: number; unrealized_price: number;
  realized_fx: number; unrealized_fx: number;
  realized: number; unrealized: number; fx: number; total: number;
  instruments: PnlInstrument[];
}
export interface PnlPeriod {
  requested_from: string; requested_to: string;
  actual_from: string; actual_to: string; snapshots: number;
}
export interface PnlReconciliation {
  investment_pnl: number; cash_and_margin: number; accrued_fees: number;
  provisions: number; dividend_income: number; total_pnl: number;
  aum_change: number; net_flows: number;
  residual: number; gross: number; within_tolerance: boolean;
}
export interface Pnl {
  empty: boolean;
  period?: PnlPeriod;
  groups?: PnlGroup[];
  reconciliation?: PnlReconciliation;
  unclassified?: number;
  warnings: string[];
}

export const getPnl = (p: { from: string; to: string; dimension: PnlDimension }) =>
  req<Pnl>(`/api/pnl?from=${p.from}&to=${p.to}&dimension=${p.dimension}`);

export const bloombergRequestUrl = "/api/bloomberg/request";
export interface BloombergUpload {
  classified: number; fx_rows: number; skipped: RowError[];
  fx_check: { currency: string; date: string; workbook: number; bloomberg: number; drift: number }[];
}
export const uploadBloomberg = (f: File) => {
  const fd = new FormData();
  fd.append("file", f);
  return req<BloombergUpload>("/api/bloomberg/upload", { method: "POST", body: fd });
};
```

- [ ] **Step 2: Create the page**

Create `frontend/src/pages/PnlPage.tsx`. Read `frontend/src/pages/LimitsPage.tsx`
first and follow its data-loading and table conventions, and use the existing
formatters in `frontend/src/fmt.ts` rather than inlining `toFixed`.

```tsx
import { useEffect, useMemo, useState } from "react";
import { getPnl, type Pnl, type PnlDimension } from "../api";
import { fmtEur, fmtPct } from "../fmt";

const DIMENSIONS: { value: PnlDimension; label: string }[] = [
  { value: "asset_class", label: "Asset class" },
  { value: "country", label: "Country" },
  { value: "region", label: "Region" },
  { value: "sector", label: "Sector" },
  { value: "industry", label: "Industry" },
  { value: "currency", label: "Currency" },
  { value: "issuer_group", label: "Issuer group" },
];

function presetRange(preset: string): { from: string; to: string } {
  const today = new Date();
  const iso = (d: Date) => d.toISOString().slice(0, 10);
  const y = today.getFullYear();
  switch (preset) {
    case "MTD": return { from: iso(new Date(y, today.getMonth(), 1)), to: iso(today) };
    case "QTD": return { from: iso(new Date(y, Math.floor(today.getMonth() / 3) * 3, 1)), to: iso(today) };
    case "YTD": return { from: `${y}-01-01`, to: iso(today) };
    default: return { from: "2000-01-01", to: iso(today) }; // ITD
  }
}

export default function PnlPage() {
  const [range, setRange] = useState(presetRange("YTD"));
  const [dimension, setDimension] = useState<PnlDimension>("asset_class");
  const [data, setData] = useState<Pnl | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [open, setOpen] = useState<Record<string, boolean>>({});

  useEffect(() => {
    setErr(null);
    getPnl({ ...range, dimension }).then(setData).catch((e) => setErr(String(e)));
  }, [range.from, range.to, dimension]);

  const total = useMemo(
    () => (data?.groups ?? []).reduce((s, g) => s + g.total, 0),
    [data],
  );

  if (err) return <div className="error">{err}</div>;
  if (!data) return <div>Loading…</div>;
  if (data.empty) {
    return (
      <div>
        <h2>P&amp;L</h2>
        {data.warnings.map((w) => <p key={w} className="warn">{w}</p>)}
      </div>
    );
  }

  const p = data.period!;
  const r = data.reconciliation!;
  const snapped = p.actual_from !== p.requested_from || p.actual_to !== p.requested_to;

  return (
    <div>
      <h2>P&amp;L</h2>

      <div className="controls">
        {["MTD", "QTD", "YTD", "ITD"].map((k) => (
          <button key={k} onClick={() => setRange(presetRange(k))}>{k}</button>
        ))}
        <input type="date" value={range.from} onChange={(e) => setRange({ ...range, from: e.target.value })} />
        <input type="date" value={range.to} onChange={(e) => setRange({ ...range, to: e.target.value })} />
        <select value={dimension} onChange={(e) => setDimension(e.target.value as PnlDimension)}>
          {DIMENSIONS.map((d) => <option key={d.value} value={d.value}>{d.label}</option>)}
        </select>
        {!!data.unclassified && <span className="warn">{data.unclassified} unclassified</span>}
      </div>

      {snapped && (
        <p className="note">
          Struck between imported NAV dates {p.actual_from} and {p.actual_to} ({p.snapshots} snapshots).
          You asked for {p.requested_from} → {p.requested_to}.
        </p>
      )}

      <table>
        <thead>
          <tr>
            <th></th><th>Group</th>
            <th className="num">Realized</th><th className="num">Unrealized</th>
            <th className="num">of which FX</th><th className="num">Total</th>
          </tr>
        </thead>
        <tbody>
          {data.groups!.map((g) => (
            <>
              <tr key={g.key} className="group" onClick={() => setOpen({ ...open, [g.key]: !open[g.key] })}>
                <td>{open[g.key] ? "▾" : "▸"}</td>
                <td>{g.key}</td>
                <td className="num">{fmtEur(g.realized)}</td>
                <td className="num">{fmtEur(g.unrealized)}</td>
                <td className="num">{fmtEur(g.fx)}</td>
                <td className="num">{fmtEur(g.total)}</td>
              </tr>
              {open[g.key] && g.instruments.map((i) => (
                <tr key={`${g.key}-${i.isin}`} className="detail">
                  <td></td>
                  <td>
                    {i.name}
                    {i.fx_split_imprecise && (
                      <span title="Partial sale after a mid-period purchase: the FX split for this instrument is approximate."> ⚠</span>
                    )}
                  </td>
                  <td className="num">{fmtEur(i.realized_price + i.realized_fx)}</td>
                  <td className="num">{fmtEur(i.unrealized_price + i.unrealized_fx)}</td>
                  <td className="num">{fmtEur(i.realized_fx + i.unrealized_fx)}</td>
                  <td className="num">{fmtEur(i.realized_price + i.unrealized_price + i.realized_fx + i.unrealized_fx)}</td>
                </tr>
              ))}
            </>
          ))}
          <tr className="total">
            <td></td><td>Total</td><td></td><td></td><td></td>
            <td className="num">{fmtEur(total)}</td>
          </tr>
        </tbody>
      </table>

      <h3>Reconciliation</h3>
      <table>
        <tbody>
          <tr><td>Investment P&amp;L</td><td className="num">{fmtEur(r.investment_pnl)}</td></tr>
          <tr><td>Cash and margin accounts</td><td className="num">{fmtEur(r.cash_and_margin)}</td></tr>
          <tr><td>Accrued fees</td><td className="num">{fmtEur(r.accrued_fees)}</td></tr>
          <tr><td>Provisions</td><td className="num">{fmtEur(r.provisions)}</td></tr>
          <tr><td>Dividend income</td><td className="num">{fmtEur(r.dividend_income)}</td></tr>
          <tr className="total"><td>Total P&amp;L</td><td className="num">{fmtEur(r.total_pnl)}</td></tr>
          <tr><td>AUM change</td><td className="num">{fmtEur(r.aum_change)}</td></tr>
          <tr><td>less subscriptions / redemptions</td><td className="num">{fmtEur(r.net_flows)}</td></tr>
          <tr className={r.within_tolerance ? "ok" : "breach"}>
            <td>Residual</td>
            <td className="num">
              {fmtEur(r.residual)}
              {r.gross > 0 && <> ({fmtPct(Math.abs(r.residual) / r.gross)})</>}
              {r.within_tolerance ? " ✓ reconciled" : " ⚠ above tolerance"}
            </td>
          </tr>
        </tbody>
      </table>

      {data.warnings.map((w) => <p key={w} className="warn">{w}</p>)}
    </div>
  );
}
```

If `fmtEur` or `fmtPct` do not exist in `frontend/src/fmt.ts` under those names,
use the equivalents that are there. Do not add duplicate formatters.

- [ ] **Step 3: Add the route**

In `frontend/src/App.tsx`, import `PnlPage`, add
`{ to: "/pnl", label: "P&L" }` to `links` between Performance and Risk, and
`<Route path="/pnl" element={<PnlPage />} />` to `<Routes>`.

- [ ] **Step 4: Verify it type-checks and builds**

Run: `cd frontend && npm run build`
Expected: clean `tsc -b && vite build`.

Note the `<>` fragment inside `.map()` needs a `key`; if `tsc` complains, switch
to `<React.Fragment key={g.key}>`.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/api.ts frontend/src/App.tsx frontend/src/pages/PnlPage.tsx
git commit -m "feat(ui): P&L page with dimension grouping and reconciliation"
```

---

### Task 13: Bloomberg panel on the Data page

**Files:**
- Create: `frontend/src/components/BloombergPanel.tsx`
- Modify: `frontend/src/pages/DataPage.tsx`

**Interfaces:**
- Consumes: `uploadBloomberg`, `bloombergRequestUrl` from Task 12

- [ ] **Step 1: Create the component**

Read `frontend/src/components/FuturesContracts.tsx` first — the CTD upload panel
there is the pattern to follow for file input, error surfacing and the
`RowError[]` display.

```tsx
import { useState } from "react";
import { bloombergRequestUrl, uploadBloomberg, ApiError, type BloombergUpload } from "../api";

export default function BloombergPanel() {
  const [result, setResult] = useState<BloombergUpload | null>(null);
  const [err, setErr] = useState<ApiError | null>(null);
  const [busy, setBusy] = useState(false);

  async function onFile(e: React.ChangeEvent<HTMLInputElement>) {
    const f = e.target.files?.[0];
    if (!f) return;
    setBusy(true); setErr(null); setResult(null);
    try {
      setResult(await uploadBloomberg(f));
    } catch (x) {
      setErr(x as ApiError);
    } finally {
      setBusy(false);
      e.target.value = "";
    }
  }

  return (
    <section>
      <h3>Bloomberg classification</h3>
      <p>
        Export the request workbook, open it in Excel with a logged-in Bloomberg
        Terminal so the formulas resolve, save it, then upload it back.
      </p>
      <a href={bloombergRequestUrl} download>Export Bloomberg request</a>
      <input type="file" accept=".xlsx" onChange={onFile} disabled={busy} />

      {result && (
        <div>
          <p>{result.classified} instrument(s) classified, {result.fx_rows} FX rate(s) stored.</p>
          {result.skipped.length > 0 && (
            <>
              <p className="warn">{result.skipped.length} cell(s) did not resolve and were not stored:</p>
              <ul>{result.skipped.slice(0, 20).map((s, i) => (
                <li key={i}>{s.sheet} row {s.row}: {s.message}</li>
              ))}</ul>
            </>
          )}
          {result.fx_check.length > 0 && (
            <>
              <p className="breach">
                FX cross-check failed — these rates disagree with the NAV Recap's own
                Change column, which usually means the quote is inverted:
              </p>
              <ul>{result.fx_check.map((c, i) => (
                <li key={i}>
                  {c.currency} {c.date}: workbook {c.workbook.toFixed(4)},
                  Bloomberg {c.bloomberg.toFixed(4)} ({(c.drift * 100).toFixed(1)}% drift)
                </li>
              ))}</ul>
            </>
          )}
        </div>
      )}

      {err && (
        <div className="error">
          <p>{err.detail ?? err.message}</p>
          {err.rows && <ul>{err.rows.map((r, i) => (
            <li key={i}>{r.sheet} row {r.row}: {r.message}</li>
          ))}</ul>}
        </div>
      )}
    </section>
  );
}
```

- [ ] **Step 2: Mount it**

In `frontend/src/pages/DataPage.tsx`, import `BloombergPanel` and render
`<BloombergPanel />` directly below the existing CTD panel.

- [ ] **Step 3: Verify it type-checks and builds**

Run: `cd frontend && npm run build`
Expected: clean.

- [ ] **Step 4: Full verification**

Run, and paste the real output into the commit or the completion report:

```bash
cargo test
cd frontend && npm run build
```

Both must be clean. If any test fails, fix it before committing — do not report
completion with a failing suite.

- [ ] **Step 5: Update the README**

Add a "P&L" bullet to the Features list in `README.md` describing the
decomposition and the snapping behaviour, and add a step 3 to the Weekly
workflow covering the Bloomberg round trip. Match the existing tone: state what
it does and what it refuses to do.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/BloombergPanel.tsx frontend/src/pages/DataPage.tsx README.md
git commit -m "feat(ui): Bloomberg classification panel on the Data page"
```

---

## Self-review notes

**Spec coverage.** Every section of the design maps to a task: schema → 1;
cost basis → 2; price/FX decomposition and the FX split → 3; futures → 4;
grouping → 5; reconciliation and derived flows → 6; request workbook → 7;
upload, inversion check and region table → 8; PAM reconciliation → 9;
API → 10; Bloomberg endpoints → 11; UI → 12 and 13.

**Two spec items deliberately handled inside other tasks rather than alone:**
the fees memo line is a display concern and is covered by the fact that trade
fees sit inside `net_price` (Task 2) and so are never double-counted; the
"never carry forward" rule is enforced by `FxLookup::rate` doing an exact-date
lookup with no fallback (Task 3).

**Known gap, stated rather than hidden.** Realized P&L on futures contracts
closed mid-period is passed as `0.0` by the Task 10 handler, because deriving it
requires matching `OPERATIONS` futures rows to contract specs and applying the
point value and 32nds convention per contract. The variation-margin change still
captures the full economic result across a period where no contract is closed,
and any error from a closed contract surfaces in the residual rather than being
silently absorbed. Closing this gap is a follow-on task; it is called out here so
the implementer does not mistake it for an oversight.
