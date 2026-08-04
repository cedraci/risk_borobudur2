# Futures Exposure and Bond-Future DV01 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Report the fund's futures exposure as notional by category (long and short in absolute value, % of net assets) and compute precise DV01 for bond futures from weekly cheapest-to-deliver analytics.

**Architecture:** Permanent contract specs live in `futures_contracts` keyed on contract root and are seeded on workbook import; weekly CTD analytics live in `futures_analytics` keyed on `(nav_date, full ticker)` and arrive as a separate uploaded file. The exposure table depends only on the workbook plus specs, so it renders for every historical NAV date; DV01 depends on the weekly file and degrades visibly when absent.

**Tech Stack:** Rust 2024 edition, axum 0.8, sqlx 0.8 + embedded PostgreSQL 17, calamine 0.26 (xlsx), csv 1 (new), React 19 + Vite 8 + TypeScript.

**Spec:** `docs/superpowers/specs/2026-08-04-futures-exposure-design.md` (commit `b38565d`)

## Global Constraints

- Rust edition 2024; workspace deps declared in the root `Cargo.toml` are referenced as `{ workspace = true }`.
- `cargo test --workspace` must pass at the end of every task. Tests touching the DB start their own embedded PostgreSQL via `db::embedded::start(dir.path(), true)` and are slow (~15-60s each); this is normal.
- Category values are exactly `equity`, `interest_rate`, `fx`, `credit`, `commodity`, `other`.
- Price convention values are exactly `decimal`, `th32`.
- Routes: analytics under `/api/metrics/`, reference data at top level. axum 0.8 path syntax is `{param}`, not `:param`.
- Reference data seeded on import must never overwrite user edits (`ON CONFLICT DO NOTHING` or `COALESCE`, per the existing pattern in `import_workbook`).
- Missing data must never raise at read time — return a flag the UI renders.
- No frontend test runner exists. Frontend verification is `cd frontend && npm run build` (runs `tsc -b`, so type errors fail the build).
- `cargo` is at `~/.cargo/bin`, node at `C:\Program Files\nodejs`; neither is on PATH by default in this environment. Prefix commands with `$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:ProgramFiles\nodejs;$env:PATH";` in PowerShell.

## File Structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/analytics/src/futures.rs` | **Create.** Pure: price decoding, root derivation, implied point value, notional, category aggregation, DV01 | 1-3 |
| `crates/analytics/src/lib.rs` | **Modify.** Register the module | 1 |
| `crates/db/migrations/0003_futures.sql` | **Create.** Both tables | 4 |
| `crates/db/Cargo.toml` | **Modify.** Add `analytics` path dep | 4 |
| `crates/db/src/repo.rs` | **Modify.** Contract + analytics accessors, import seeding and cross-check | 4, 5, 7 |
| `crates/ingest/src/futures_file.rs` | **Create.** Companion-file parser | 6 |
| `crates/ingest/Cargo.toml` | **Modify.** Add `csv` dep | 6 |
| `crates/server/src/handlers/futures.rs` | **Create.** Contracts CRUD + analytics upload | 8 |
| `crates/server/src/handlers/limits.rs` | **Modify.** Derivatives handler; rates extension | 9, 10 |
| `crates/server/src/routes.rs` | **Modify.** Register routes | 8 |
| `frontend/src/api.ts` | **Modify.** Types + fetchers | 11 |
| `frontend/src/components/DerivativesExposure.tsx` | **Create.** Category table + contract detail | 11 |
| `frontend/src/components/FuturesContracts.tsx` | **Create.** Editable spec grid + CTD upload | 12 |
| `frontend/src/pages/LimitsPage.tsx` | **Modify.** Mount the exposure section | 11 |
| `frontend/src/pages/DataPage.tsx` | **Modify.** Mount the contracts section | 12 |
| `README.md` | **Modify.** Document the weekly file | 13 |

`DataPage.tsx` is already 270 lines, the largest page in the app. The futures UI goes into its own components rather than growing it further.

---

### Task 1: Price decoding, contract root, implied point value

**Files:**
- Create: `crates/analytics/src/futures.rs`
- Modify: `crates/analytics/src/lib.rs`

**Interfaces:**
- Consumes: nothing
- Produces: `PriceConvention` (enum `Decimal`/`Th32`, with `parse(&str) -> Option<Self>` and `as_str(&self) -> &'static str`), `decode_price(raw: f64, conv: PriceConvention) -> f64`, `contract_root(ticker: &str) -> Option<String>`, `implied_point_value(price: f64, avg_cost: f64, qty: f64, valuation_ccy: f64) -> Option<f64>`

- [ ] **Step 1: Write the failing tests**

Create `crates/analytics/src/futures.rs` containing only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_thirty_seconds_quotes() {
        // 109.145 is the "109-14.5" 32nds quote written without its hyphen.
        // Proven against the workbook: TYU6's only trade was 6 @ 109.453125.
        assert!((decode_price(109.145, PriceConvention::Th32) - 109.453125).abs() < 1e-12);
        assert!((decode_price(108.105, PriceConvention::Th32) - 108.328125).abs() < 1e-12);
        assert!((decode_price(108.0, PriceConvention::Th32) - 108.0).abs() < 1e-12);
        // Decimal contracts pass through untouched.
        assert!((decode_price(124.46, PriceConvention::Decimal) - 124.46).abs() < 1e-12);
        assert!((decode_price(8388.0, PriceConvention::Decimal) - 8388.0).abs() < 1e-12);
    }

    #[test]
    fn parses_convention_names() {
        assert_eq!(PriceConvention::parse("decimal"), Some(PriceConvention::Decimal));
        assert_eq!(PriceConvention::parse("th32"), Some(PriceConvention::Th32));
        assert_eq!(PriceConvention::parse("32nds"), None);
        assert_eq!(PriceConvention::Th32.as_str(), "th32");
    }

    #[test]
    fn derives_contract_root() {
        assert_eq!(contract_root("RXU6 Comdty").as_deref(), Some("RX"));
        assert_eq!(contract_root("OATU6 Comdty").as_deref(), Some("OAT"));
        assert_eq!(contract_root("KOAU6 Comdty").as_deref(), Some("KOA"));
        assert_eq!(contract_root("TYU6 Comdty").as_deref(), Some("TY"));
        assert_eq!(contract_root("CFQ6 Index").as_deref(), Some("CF"));
        assert_eq!(contract_root("NQU6 Index").as_deref(), Some("NQ"));
        assert_eq!(contract_root("RYU6 Curncy").as_deref(), Some("RY"));
        assert_eq!(contract_root(""), None);
        assert_eq!(contract_root("AB"), None); // nothing left after the suffix
    }

    #[test]
    fn recovers_exchange_point_values() {
        // (price, avg_cost, qty, valuation_ccy, expected) - the eight real contracts,
        // with the TY line already decoded out of 32nds.
        let cases = [
            (8388.0, 8336.23333333, -12.0, -6212.0, 10.0),
            (6301.0, 6287.0, -9.0, -1260.0, 10.0),
            (108.328125, 109.453125, -6.0, 6750.0, 1000.0),
            (185.93, 184.13, -7.0, -1575000.0, 125000.0),
            (124.46, 125.83, -8.0, 10960.0, 1000.0),
            (117.12, 118.918, -15.0, 26970.0, 1000.0),
            (28282.25, 28982.5, -1.0, 14005.0, 20.0),
            (119.82, 121.16625, 8.0, -10770.0, 1000.0),
        ];
        for (price, pam, qty, val, want) in cases {
            let got = implied_point_value(price, pam, qty, val).unwrap();
            assert!((got - want).abs() < 1e-6, "price {price}: got {got}, want {want}");
        }
    }

    #[test]
    fn point_value_undeterminable_cases() {
        // marked at average cost -> denominator vanishes
        assert_eq!(implied_point_value(124.46, 124.46, -8.0, 0.0), None);
        // zero quantity
        assert_eq!(implied_point_value(124.46, 125.83, 0.0, 0.0), None);
        // a negative implied value is nonsense, not a spec
        assert_eq!(implied_point_value(124.46, 125.83, -8.0, -10960.0), None);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"; cargo test -p analytics futures
```

Expected: FAIL to compile — `cannot find function decode_price`, `PriceConvention` unresolved.

- [ ] **Step 3: Write the implementation**

Prepend to `crates/analytics/src/futures.rs`, above the test module:

```rust
//! Futures exposure: contract identification, notional, and bond-future DV01.
//!
//! The NAV Recap reports a future's `Valorisation` as variation margin, not
//! market value, so exposure has to be rebuilt from quantity, price and the
//! contract's point value.

/// How a contract's price is quoted in `PORTEFEUILLE_NAV`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceConvention {
    /// The quoted number is the price.
    Decimal,
    /// Thirty-seconds: `108.105` means `108-10.5`, i.e. 108 + 10.5/32.
    Th32,
}

impl PriceConvention {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "decimal" => Some(Self::Decimal),
            "th32" => Some(Self::Th32),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Decimal => "decimal",
            Self::Th32 => "th32",
        }
    }
}

/// Decode a quoted futures price. Under `Th32` the three fractional digits
/// are thirty-seconds multiplied by ten (`.105` = 10.5/32), so dividing by
/// 320 recovers the true price.
pub fn decode_price(raw: f64, conv: PriceConvention) -> f64 {
    match conv {
        PriceConvention::Decimal => raw,
        PriceConvention::Th32 => {
            let whole = raw.trunc();
            let ticks = ((raw - whole) * 1000.0).round();
            whole + ticks / 320.0
        }
    }
}

/// Contract root from a Bloomberg ticker: the symbol before the space, minus
/// its trailing month letter and year digit. `"RXU6 Comdty"` -> `"RX"`.
/// Stable across quarterly rolls, unlike the workbook's futures ISINs.
pub fn contract_root(ticker: &str) -> Option<String> {
    let sym = ticker.split_whitespace().next()?;
    if !sym.is_ascii() || sym.len() < 3 {
        return None;
    }
    Some(sym[..sym.len() - 2].to_string())
}

/// Point value implied by the workbook's own identity
/// `valorisation = (price - avg_cost) * qty * point_value`.
///
/// `None` when the position is marked at its average cost (the denominator
/// vanishes), when quantity is zero, or when the result is not a positive
/// finite number. Prices must already be decoded.
pub fn implied_point_value(price: f64, avg_cost: f64, qty: f64, valuation_ccy: f64) -> Option<f64> {
    let scale = price.abs().max(1.0);
    if (price - avg_cost).abs() < 1e-6 * scale || qty == 0.0 {
        return None;
    }
    let pv = valuation_ccy / ((price - avg_cost) * qty);
    (pv.is_finite() && pv > 0.0).then_some(pv)
}
```

Add to `crates/analytics/src/lib.rs`, keeping both lists alphabetically consistent with the existing order (append after `backtest`):

```rust
pub mod futures;
```

and

```rust
pub use futures::*;
```

- [ ] **Step 4: Run tests to verify they pass**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"; cargo test -p analytics futures
```

Expected: PASS, 5 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/analytics/src/futures.rs crates/analytics/src/lib.rs
git commit -m "feat(analytics): futures price decoding, root derivation, implied point value"
```

---

### Task 2: Notional and category aggregation

**Files:**
- Modify: `crates/analytics/src/futures.rs`

**Interfaces:**
- Consumes: `PriceConvention`, `decode_price` from Task 1
- Produces: `Category` (enum with variants `Equity`, `InterestRate`, `Fx`, `Credit`, `Commodity`, `Other`; `parse`, `as_str`, `ALL`), `FuturePosition`, `ExposureRow`, `CategoryTotals`, `ExposureReport`, `exposure(positions: &[FuturePosition], aum: f64) -> ExposureReport`

- [ ] **Step 1: Write the failing tests**

Append inside the existing `mod tests` block in `crates/analytics/src/futures.rs`:

```rust
    fn fut(ticker: &str, cat: Category, qty: f64, price: f64, pv: Option<f64>, fx: Option<f64>) -> FuturePosition {
        FuturePosition {
            ticker: ticker.into(),
            name: ticker.into(),
            currency: "EUR".into(),
            category: cat,
            qty,
            price,
            point_value: pv,
            fx_rate: fx,
        }
    }

    #[test]
    fn notional_is_qty_times_point_value_times_price() {
        let rows = [fut("RXU6 Comdty", Category::InterestRate, -8.0, 124.46, Some(1000.0), Some(1.0))];
        let rep = exposure(&rows, 28_332_753.49);
        let r = &rep.rows[0];
        assert!((r.notional_ccy.unwrap() - -995_680.0).abs() < 1e-6);
        assert!((r.notional_eur.unwrap() - -995_680.0).abs() < 1e-6);
        assert!((r.pct_nav.unwrap() - -0.035142).abs() < 1e-6);
        assert!(!r.spec_missing);
    }

    #[test]
    fn categories_report_long_and_short_in_absolute_value() {
        let aum = 1000.0;
        let rows = [
            fut("A Index", Category::Equity, -1.0, 100.0, Some(1.0), Some(1.0)),   // -100 -> short 10%
            fut("B Comdty", Category::InterestRate, 2.0, 100.0, Some(1.0), Some(1.0)), // +200 -> long 20%
            fut("C Comdty", Category::InterestRate, -1.0, 50.0, Some(1.0), Some(1.0)), // -50 -> short 5%
        ];
        let rep = exposure(&rows, aum);
        let ir = rep.categories.iter().find(|c| c.category == Category::InterestRate).unwrap();
        assert!((ir.long_pct - 0.20).abs() < 1e-12);
        assert!((ir.short_pct - 0.05).abs() < 1e-12, "shorts are reported positive");
        assert!((ir.gross_pct - 0.25).abs() < 1e-12);

        let eq = rep.categories.iter().find(|c| c.category == Category::Equity).unwrap();
        assert!((eq.long_pct - 0.0).abs() < 1e-12);
        assert!((eq.short_pct - 0.10).abs() < 1e-12);

        // all six categories are always present, empty ones included
        assert_eq!(rep.categories.len(), 6);
        assert!((rep.total.long_pct - 0.20).abs() < 1e-12);
        assert!((rep.total.short_pct - 0.15).abs() < 1e-12);
        assert!((rep.total.gross_pct - 0.35).abs() < 1e-12);
    }

    #[test]
    fn missing_spec_or_fx_excludes_from_totals_but_keeps_the_row() {
        let rows = [
            fut("A Comdty", Category::InterestRate, -8.0, 124.46, None, Some(1.0)),      // no point value
            fut("B Comdty", Category::InterestRate, -8.0, 124.46, Some(1000.0), None),   // no fx rate
        ];
        let rep = exposure(&rows, 1000.0);
        assert_eq!(rep.rows.len(), 2, "rows are always listed");
        assert!(rep.rows[0].spec_missing);
        assert_eq!(rep.rows[0].notional_ccy, None);
        assert_eq!(rep.rows[1].notional_eur, None, "no fx rate -> no EUR notional");
        assert!(rep.rows[1].notional_ccy.is_some(), "ccy notional still computable");
        assert_eq!(rep.excluded.len(), 2);
        assert!((rep.total.gross_pct - 0.0).abs() < 1e-12, "neither reaches the totals");
    }

    #[test]
    fn zero_aum_excludes_everything_without_dividing_by_zero() {
        let rows = [fut("A Index", Category::Equity, -1.0, 100.0, Some(1.0), Some(1.0))];
        let rep = exposure(&rows, 0.0);
        assert_eq!(rep.rows[0].pct_nav, None);
        assert_eq!(rep.excluded.len(), 1);
        assert!((rep.total.gross_pct - 0.0).abs() < 1e-12);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"; cargo test -p analytics futures
```

Expected: FAIL to compile — `cannot find type FuturePosition`, `cannot find function exposure`.

- [ ] **Step 3: Write the implementation**

Append to `crates/analytics/src/futures.rs`, above the test module:

```rust
use serde::Serialize;

/// Derivative category for the exposure disclosure. The six standard
/// regulatory categories; only three have holdings today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Equity,
    InterestRate,
    Fx,
    Credit,
    Commodity,
    Other,
}

impl Category {
    pub const ALL: [Category; 6] = [
        Category::Equity,
        Category::InterestRate,
        Category::Fx,
        Category::Credit,
        Category::Commodity,
        Category::Other,
    ];

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "equity" => Some(Self::Equity),
            "interest_rate" => Some(Self::InterestRate),
            "fx" => Some(Self::Fx),
            "credit" => Some(Self::Credit),
            "commodity" => Some(Self::Commodity),
            "other" => Some(Self::Other),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Equity => "equity",
            Self::InterestRate => "interest_rate",
            Self::Fx => "fx",
            Self::Credit => "credit",
            Self::Commodity => "commodity",
            Self::Other => "other",
        }
    }
}

/// One futures position, with its price already decoded and its spec resolved.
#[derive(Debug, Clone)]
pub struct FuturePosition {
    pub ticker: String,
    pub name: String,
    pub currency: String,
    pub category: Category,
    pub qty: f64,
    pub price: f64,
    pub point_value: Option<f64>,
    pub fx_rate: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExposureRow {
    pub ticker: String,
    pub name: String,
    pub currency: String,
    pub category: Category,
    pub qty: f64,
    pub price: f64,
    pub point_value: Option<f64>,
    pub notional_ccy: Option<f64>,
    pub notional_eur: Option<f64>,
    pub pct_nav: Option<f64>,
    pub spec_missing: bool,
}

/// Long, short and gross for one category, each a fraction of net assets
/// expressed in absolute value (shorts are positive numbers).
#[derive(Debug, Clone, Serialize)]
pub struct CategoryTotals {
    pub category: Category,
    pub long_pct: f64,
    pub short_pct: f64,
    pub gross_pct: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExposureReport {
    pub rows: Vec<ExposureRow>,
    pub categories: Vec<CategoryTotals>,
    /// Sum across categories; `category` is `Other` and carries no meaning.
    pub total: CategoryTotals,
    /// Tickers left out of the totals for want of a spec, an FX rate or an AUM.
    pub excluded: Vec<String>,
}

/// Notional exposure by reference to the underlying, aggregated by category.
///
/// `notional = qty * point_value * price`, converted to EUR at the workbook's
/// own FX rate, then expressed as a fraction of net assets. Long and short are
/// summed separately, each in absolute value, without netting.
pub fn exposure(positions: &[FuturePosition], aum: f64) -> ExposureReport {
    let mut rows = Vec::with_capacity(positions.len());
    let mut excluded = Vec::new();

    for p in positions {
        let notional_ccy = p.point_value.map(|pv| p.qty * pv * p.price);
        let notional_eur = match (notional_ccy, p.fx_rate) {
            (Some(n), Some(fx)) => Some(n * fx),
            _ => None,
        };
        let pct_nav = match notional_eur {
            Some(n) if aum > 0.0 => Some(n / aum),
            _ => None,
        };
        if pct_nav.is_none() {
            excluded.push(p.ticker.clone());
        }
        rows.push(ExposureRow {
            ticker: p.ticker.clone(),
            name: p.name.clone(),
            currency: p.currency.clone(),
            category: p.category,
            qty: p.qty,
            price: p.price,
            point_value: p.point_value,
            notional_ccy,
            notional_eur,
            pct_nav,
            spec_missing: p.point_value.is_none(),
        });
    }

    let categories: Vec<CategoryTotals> = Category::ALL
        .iter()
        .map(|cat| {
            let mut long_pct = 0.0;
            let mut short_pct = 0.0;
            for r in rows.iter().filter(|r| r.category == *cat) {
                match r.pct_nav {
                    Some(p) if p > 0.0 => long_pct += p,
                    Some(p) if p < 0.0 => short_pct += -p,
                    _ => {}
                }
            }
            CategoryTotals { category: *cat, long_pct, short_pct, gross_pct: long_pct + short_pct }
        })
        .collect();

    let long_pct: f64 = categories.iter().map(|c| c.long_pct).sum();
    let short_pct: f64 = categories.iter().map(|c| c.short_pct).sum();
    ExposureReport {
        rows,
        categories,
        total: CategoryTotals { category: Category::Other, long_pct, short_pct, gross_pct: long_pct + short_pct },
        excluded,
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"; cargo test -p analytics futures
```

Expected: PASS, 9 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/analytics/src/futures.rs
git commit -m "feat(analytics): futures notional and category exposure aggregation"
```

---

### Task 3: Bond-future DV01

**Files:**
- Modify: `crates/analytics/src/futures.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks
- Produces: `CtdAnalytics { mod_duration: f64, clean_price: f64, accrued: f64, conversion_factor: f64 }`, `dv01_contract(a: &CtdAnalytics, point_value: f64) -> Option<f64>`, `dv01_position(a: &CtdAnalytics, point_value: f64, qty: f64, fx_rate: f64) -> Option<f64>`

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests` in `crates/analytics/src/futures.rs`:

```rust
    fn ctd() -> CtdAnalytics {
        CtdAnalytics { mod_duration: 8.41, clean_price: 98.72, accrued: 0.63, conversion_factor: 0.782145 }
    }

    #[test]
    fn dv01_per_contract_matches_hand_computation() {
        // dirty = 99.35; 8.41 * 99.35 * 1000 * 1e-4 = 83.55335; / 0.782145 = 106.8259
        let d = dv01_contract(&ctd(), 1000.0).unwrap();
        assert!((d - 106.8259).abs() < 1e-3, "got {d}");
    }

    #[test]
    fn dv01_position_scales_by_quantity_and_fx() {
        let per = dv01_contract(&ctd(), 1000.0).unwrap();
        let pos = dv01_position(&ctd(), 1000.0, -8.0, 1.0).unwrap();
        assert!((pos - -8.0 * per).abs() < 1e-9, "a short is negative DV01");
        let usd = dv01_position(&ctd(), 1000.0, -6.0, 0.87881185).unwrap();
        assert!((usd - -6.0 * per * 0.87881185).abs() < 1e-9);
    }

    #[test]
    fn dv01_rejects_degenerate_inputs() {
        let mut a = ctd();
        a.conversion_factor = 0.0;
        assert_eq!(dv01_contract(&a, 1000.0), None);
        assert_eq!(dv01_contract(&ctd(), 0.0), None);
        assert_eq!(dv01_position(&ctd(), 0.0, -8.0, 1.0), None);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"; cargo test -p analytics futures
```

Expected: FAIL to compile — `cannot find type CtdAnalytics`.

- [ ] **Step 3: Write the implementation**

Append to `crates/analytics/src/futures.rs`, above the test module:

```rust
/// Cheapest-to-deliver analytics for one bond future on one NAV date.
/// Supplied weekly; not derivable from the NAV Recap.
#[derive(Debug, Clone)]
pub struct CtdAnalytics {
    pub mod_duration: f64,
    pub clean_price: f64,
    pub accrued: f64,
    pub conversion_factor: f64,
}

/// DV01 of a single contract, in contract currency.
///
/// `mod_duration * dirty_price/100 * face * 1bp / conversion_factor`, where the
/// deliverable face is `point_value * 100` and the price is quoted per 100 —
/// so the two hundreds cancel and only `point_value` remains.
pub fn dv01_contract(a: &CtdAnalytics, point_value: f64) -> Option<f64> {
    if a.conversion_factor <= 0.0 || point_value <= 0.0 {
        return None;
    }
    let dirty = a.clean_price + a.accrued;
    Some(a.mod_duration * dirty * point_value * 1e-4 / a.conversion_factor)
}

/// DV01 of the held position, in EUR. Negative for a short.
pub fn dv01_position(a: &CtdAnalytics, point_value: f64, qty: f64, fx_rate: f64) -> Option<f64> {
    dv01_contract(a, point_value).map(|d| qty * d * fx_rate)
}
```

- [ ] **Step 4: Run tests to verify they pass**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"; cargo test -p analytics futures
```

Expected: PASS, 12 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/analytics/src/futures.rs
git commit -m "feat(analytics): precise bond-future DV01 from CTD analytics"
```

---

### Task 4: Migration and contract accessors

**Files:**
- Create: `crates/db/migrations/0003_futures.sql`
- Modify: `crates/db/Cargo.toml`, `crates/db/src/repo.rs`
- Test: `crates/db/tests/futures_contracts.rs`

**Interfaces:**
- Consumes: `analytics::Category`, `analytics::PriceConvention` (Tasks 1-2)
- Produces: `db::repo::FuturesContract { contract_root: String, label: String, category: String, point_value: Option<f64>, currency: String, curve: Option<String>, price_convention: String, confirmed: bool }`, `db::repo::contracts_all(pool) -> anyhow::Result<Vec<FuturesContract>>`, `db::repo::contracts_upsert(pool, &FuturesContract) -> anyhow::Result<()>`

- [ ] **Step 1: Write the failing test**

Create `crates/db/tests/futures_contracts.rs`:

```rust
#[tokio::test]
async fn contracts_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    assert!(db::repo::contracts_all(&pool).await.unwrap().is_empty());

    let c = db::repo::FuturesContract {
        contract_root: "RX".into(),
        label: "Euro-Bund".into(),
        category: "interest_rate".into(),
        point_value: Some(1000.0),
        currency: "EUR".into(),
        curve: Some("DE-10y".into()),
        price_convention: "decimal".into(),
        confirmed: true,
    };
    db::repo::contracts_upsert(&pool, &c).await.unwrap();

    let all = db::repo::contracts_all(&pool).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].contract_root, "RX");
    assert_eq!(all[0].point_value, Some(1000.0));
    assert_eq!(all[0].curve.as_deref(), Some("DE-10y"));
    assert!(all[0].confirmed);

    // upsert replaces the whole row
    let c2 = db::repo::FuturesContract { curve: None, confirmed: false, ..c };
    db::repo::contracts_upsert(&pool, &c2).await.unwrap();
    let all = db::repo::contracts_all(&pool).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].curve, None);
    assert!(!all[0].confirmed);

    // CHECK constraints reject invalid enums and non-positive point values
    assert!(db::repo::contracts_upsert(&pool, &db::repo::FuturesContract {
        contract_root: "ZZ".into(), category: "nonsense".into(), ..c2.clone()
    }).await.is_err());
    assert!(db::repo::contracts_upsert(&pool, &db::repo::FuturesContract {
        contract_root: "YY".into(), point_value: Some(0.0), ..c2.clone()
    }).await.is_err());

    pool.close().await;
    edb.stop().await;
}
```

- [ ] **Step 2: Run test to verify it fails**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"; cargo test -p db futures_contracts
```

Expected: FAIL to compile — `cannot find type FuturesContract in module db::repo`.

- [ ] **Step 3: Write the implementation**

Create `crates/db/migrations/0003_futures.sql`:

```sql
CREATE TABLE futures_contracts (
  contract_root    TEXT PRIMARY KEY,
  label            TEXT NOT NULL,
  category         TEXT NOT NULL CHECK (category IN
                   ('equity','interest_rate','fx','credit','commodity','other')),
  point_value      NUMERIC CHECK (point_value > 0),
  currency         TEXT NOT NULL,
  curve            TEXT,
  price_convention TEXT NOT NULL DEFAULT 'decimal'
                   CHECK (price_convention IN ('decimal','th32')),
  confirmed        BOOLEAN NOT NULL DEFAULT false,
  updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE futures_analytics (
  nav_date          DATE NOT NULL,
  ticker            TEXT NOT NULL,
  ctd_isin          TEXT NOT NULL,
  ctd_mod_duration  NUMERIC NOT NULL CHECK (ctd_mod_duration > 0),
  ctd_clean_price   NUMERIC NOT NULL CHECK (ctd_clean_price > 0),
  ctd_accrued       NUMERIC NOT NULL DEFAULT 0 CHECK (ctd_accrued >= 0),
  conversion_factor NUMERIC NOT NULL CHECK (conversion_factor > 0),
  source_file       TEXT NOT NULL,
  uploaded_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (nav_date, ticker)
);
```

Add to `crates/db/Cargo.toml` under `[dependencies]` (the import-time cross-check in Task 5 needs the pure helpers):

```toml
analytics = { path = "../analytics" }
```

Append to `crates/db/src/repo.rs`:

```rust
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct FuturesContract {
    pub contract_root: String,
    pub label: String,
    pub category: String,
    pub point_value: Option<f64>,
    pub currency: String,
    pub curve: Option<String>,
    pub price_convention: String,
    pub confirmed: bool,
}

const SELECT_CONTRACTS: &str = "SELECT contract_root, label, category,
        point_value::float8 AS point_value, currency, curve, price_convention, confirmed
     FROM futures_contracts ORDER BY contract_root";

pub async fn contracts_all(pool: &PgPool) -> anyhow::Result<Vec<FuturesContract>> {
    Ok(sqlx::query_as(SELECT_CONTRACTS).fetch_all(pool).await?)
}

/// Full-row replace, like `refs_upsert`: every field is written as given.
pub async fn contracts_upsert(pool: &PgPool, c: &FuturesContract) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO futures_contracts
           (contract_root, label, category, point_value, currency, curve, price_convention, confirmed, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now())
         ON CONFLICT (contract_root) DO UPDATE SET
           label = EXCLUDED.label,
           category = EXCLUDED.category,
           point_value = EXCLUDED.point_value,
           currency = EXCLUDED.currency,
           curve = EXCLUDED.curve,
           price_convention = EXCLUDED.price_convention,
           confirmed = EXCLUDED.confirmed,
           updated_at = now()",
    )
    .bind(&c.contract_root).bind(&c.label).bind(&c.category).bind(c.point_value)
    .bind(&c.currency).bind(&c.curve).bind(&c.price_convention).bind(c.confirmed)
    .execute(pool)
    .await?;
    Ok(())
}
```

- [ ] **Step 4: Run test to verify it passes**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"; cargo test -p db futures_contracts
```

Expected: PASS, 1 test (~20s — it starts PostgreSQL).

- [ ] **Step 5: Commit**

```bash
git add crates/db/migrations/0003_futures.sql crates/db/Cargo.toml crates/db/src/repo.rs crates/db/tests/futures_contracts.rs Cargo.lock
git commit -m "feat(db): futures_contracts and futures_analytics tables"
```

---

### Task 5: Seed contracts and cross-check point value on import

**Files:**
- Modify: `crates/db/src/repo.rs` (`ImportOutcome`, `import_workbook`)
- Test: `crates/db/tests/futures_seeding.rs`

**Interfaces:**
- Consumes: `contracts_all` (Task 4), `analytics::{contract_root, decode_price, implied_point_value, PriceConvention}` (Task 1)
- Produces: `ImportOutcome.warnings: Vec<String>` — a new public field on the existing struct

- [ ] **Step 1: Write the failing test**

Create `crates/db/tests/futures_seeding.rs`:

```rust
const SAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx");

#[tokio::test]
async fn import_seeds_futures_contracts_unconfirmed() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    let bytes = std::fs::read(SAMPLE).unwrap();
    let wb = ingest::parse_workbook(&bytes).unwrap();
    let out = db::repo::import_workbook(&pool, "s.xlsx", "sha-seed", &wb).await.unwrap();
    assert!(!out.duplicate);

    let cs = db::repo::contracts_all(&pool).await.unwrap();
    let roots: Vec<&str> = cs.iter().map(|c| c.contract_root.as_str()).collect();
    assert_eq!(roots, vec!["CF", "KOA", "NQ", "OAT", "RX", "RY", "TY"], "one row per root, sorted");

    let cf = cs.iter().find(|c| c.contract_root == "CF").unwrap();
    assert_eq!(cf.point_value, Some(10.0), "derived from the workbook identity");
    assert_eq!(cf.category, "equity", "Index suffix");
    assert_eq!(cf.currency, "EUR");
    assert!(!cf.confirmed, "seeded rows always need confirmation");

    let ry = cs.iter().find(|c| c.contract_root == "RY").unwrap();
    assert_eq!(ry.category, "fx", "Curncy suffix");
    assert_eq!(ry.point_value, Some(125000.0));

    let rx = cs.iter().find(|c| c.contract_root == "RX").unwrap();
    assert_eq!(rx.category, "other", "Comdty is ambiguous - never guessed");
    assert_eq!(rx.point_value, Some(1000.0));

    // TY is quoted in 32nds; read as decimal its implied point value is ~1081.7,
    // so the seeded value is wrong until the convention is corrected. It is
    // seeded anyway, unconfirmed, rather than being silently dropped.
    let ty = cs.iter().find(|c| c.contract_root == "TY").unwrap();
    assert_eq!(ty.price_convention, "decimal");
    assert!((ty.point_value.unwrap() - 1081.73).abs() < 0.1);

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn reimport_warns_on_point_value_mismatch_and_never_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    let bytes = std::fs::read(SAMPLE).unwrap();
    let wb = ingest::parse_workbook(&bytes).unwrap();
    db::repo::import_workbook(&pool, "s.xlsx", "sha-a", &wb).await.unwrap();

    // Correct TY by hand, exactly as the user would on the Data page.
    let ty = db::repo::FuturesContract {
        contract_root: "TY".into(), label: "US 10Y Note".into(), category: "interest_rate".into(),
        point_value: Some(1000.0), currency: "USD".into(), curve: Some("US-10y".into()),
        price_convention: "th32".into(), confirmed: true,
    };
    db::repo::contracts_upsert(&pool, &ty).await.unwrap();

    // Re-import the same workbook under a new hash.
    let out = db::repo::import_workbook(&pool, "s.xlsx", "sha-b", &wb).await.unwrap();

    let after = db::repo::contracts_all(&pool).await.unwrap();
    let ty2 = after.iter().find(|c| c.contract_root == "TY").unwrap();
    assert_eq!(ty2.point_value, Some(1000.0), "user edits are never overwritten");
    assert_eq!(ty2.price_convention, "th32");
    assert!(ty2.confirmed);
    assert!(out.warnings.is_empty(), "th32 now reconciles exactly, so no warning");

    // Now break it: claim decimal for a contract that is quoted in 32nds.
    db::repo::contracts_upsert(&pool, &db::repo::FuturesContract {
        price_convention: "decimal".into(), ..ty
    }).await.unwrap();
    let out = db::repo::import_workbook(&pool, "s.xlsx", "sha-c", &wb).await.unwrap();
    let w = out.warnings.join(" | ");
    assert!(w.contains("TY"), "warning names the contract: {w}");
    assert!(w.contains("th32"), "warning names the likely convention: {w}");

    pool.close().await;
    edb.stop().await;
}
```

- [ ] **Step 2: Run tests to verify they fail**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"; cargo test -p db futures_seeding
```

Expected: FAIL to compile — `ImportOutcome` has no field `warnings`.

- [ ] **Step 3: Write the implementation**

In `crates/db/src/repo.rs`, add the field to `ImportOutcome`:

```rust
pub struct ImportOutcome {
    pub import_id: i64,
    pub duplicate: bool,
    pub nav_rows: usize,
    pub positions: usize,
    pub dividends: usize,
    pub operations: usize,
    pub div_ops_replaced: bool,
    /// Non-fatal futures spec problems. A new or mis-specified contract must
    /// never block the weekly NAV import.
    pub warnings: Vec<String>,
}
```

Set `warnings: Vec::new()` in the early-return duplicate branch. Then, in `import_workbook`, after the bond-statics seeding loop and before `if replace_div_ops {`, insert:

```rust
    // Futures: seed unknown contract roots and cross-check the point value
    // implied by the workbook against the stored spec.
    let mut warnings: Vec<String> = Vec::new();
    let known: Vec<FuturesContract> = sqlx::query_as(SELECT_CONTRACTS).fetch_all(&mut *tx).await?;
    let by_root: std::collections::HashMap<String, FuturesContract> =
        known.into_iter().map(|c| (c.contract_root.clone(), c)).collect();

    for p in wb.positions.iter().filter(|p| p.asset_type == "Future") {
        let Some(ticker) = p.ticker.as_deref() else {
            warnings.push(format!("{}: futures row has no ticker; contract not identified", p.isin));
            continue;
        };
        let Some(root) = analytics::contract_root(ticker) else {
            warnings.push(format!("{ticker}: cannot derive a contract root"));
            continue;
        };
        let (Some(raw_price), Some(raw_pam), Some(qty), Some(val)) =
            (p.price, p.avg_cost, p.quantity, p.valuation_ccy)
        else {
            warnings.push(format!("{ticker}: incomplete row; point value not verified"));
            continue;
        };

        match by_root.get(&root) {
            None => {
                // Guess only what the ticker suffix states unambiguously.
                // "Comdty" covers bond and commodity futures alike, so it is
                // never guessed - the user confirms it.
                let category = match ticker.rsplit_whitespace().next() {
                    Some("Index") => "equity",
                    Some("Curncy") => "fx",
                    _ => "other",
                };
                let pv = analytics::implied_point_value(raw_price, raw_pam, qty, val);
                sqlx::query(
                    "INSERT INTO futures_contracts
                       (contract_root, label, category, point_value, currency, price_convention, confirmed)
                     VALUES ($1, $2, $3, $4, $5, 'decimal', false)
                     ON CONFLICT (contract_root) DO NOTHING",
                )
                .bind(&root)
                .bind(p.name.clone().unwrap_or_else(|| root.clone()))
                .bind(category)
                .bind(pv)
                .bind(p.currency.clone().unwrap_or_else(|| "EUR".into()))
                .execute(&mut *tx)
                .await?;
                warnings.push(format!("{root}: new contract seeded from {ticker}; confirm its spec on the Data page"));
            }
            Some(spec) => {
                let Some(stored) = spec.point_value else { continue };
                let conv = analytics::PriceConvention::parse(&spec.price_convention)
                    .unwrap_or(analytics::PriceConvention::Decimal);
                let implied = analytics::implied_point_value(
                    analytics::decode_price(raw_price, conv),
                    analytics::decode_price(raw_pam, conv),
                    qty,
                    val,
                );
                let Some(implied) = implied else { continue }; // marked at cost: undeterminable
                if (implied - stored).abs() <= 0.005 * stored {
                    continue;
                }
                // Mismatch. If the opposite convention reconciles, say so.
                let other = match conv {
                    analytics::PriceConvention::Decimal => analytics::PriceConvention::Th32,
                    analytics::PriceConvention::Th32 => analytics::PriceConvention::Decimal,
                };
                let alt = analytics::implied_point_value(
                    analytics::decode_price(raw_price, other),
                    analytics::decode_price(raw_pam, other),
                    qty,
                    val,
                );
                match alt {
                    Some(a) if (a - stored).abs() <= 0.005 * stored => warnings.push(format!(
                        "{root}: point value implies convention {}, stored {}",
                        other.as_str(),
                        conv.as_str()
                    )),
                    _ => warnings.push(format!(
                        "{root}: point value mismatch - stored {stored}, implied {implied:.1}"
                    )),
                }
            }
        }
    }
```

Add `warnings,` to the final `Ok(ImportOutcome { ... })` literal.

- [ ] **Step 4: Run tests to verify they pass**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"; cargo test -p db futures_seeding
```

Expected: PASS, 2 tests.

- [ ] **Step 5: Run the whole workspace to catch the struct-literal break**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"; cargo test --workspace
```

Expected: PASS. `ImportOutcome` gained a field, so any other construction site must be updated; there is one in the duplicate branch of `import_workbook`.

- [ ] **Step 6: Commit**

```bash
git add crates/db/src/repo.rs crates/db/tests/futures_seeding.rs
git commit -m "feat(db): seed futures contracts and cross-check point value on import"
```

---

### Task 6: Companion-file parser

**Files:**
- Create: `crates/ingest/src/futures_file.rs`, `crates/ingest/tests/parse_ctd.rs`, `crates/ingest/tests/fixtures/ctd_sample.csv`
- Modify: `crates/ingest/Cargo.toml`, `crates/ingest/src/lib.rs`

**Interfaces:**
- Consumes: `ingest::{RowError, ParseFailure}` (existing)
- Produces: `ingest::CtdRow { nav_date: NaiveDate, ticker: String, ctd_isin: String, ctd_mod_duration: f64, ctd_clean_price: f64, ctd_accrued: f64, conversion_factor: f64 }`, `ingest::parse_ctd_file(bytes: &[u8], filename: &str) -> Result<Vec<CtdRow>, ParseFailure>`

- [ ] **Step 1: Write the failing test**

Create `crates/ingest/tests/fixtures/ctd_sample.csv`:

```csv
nav_date,ticker,ctd_isin,ctd_mod_duration,ctd_clean_price,ctd_accrued,conversion_factor
2026-07-24,RXU6 Comdty,DE0001102580,8.41,98.72,0.63,0.782145
2026-07-24,OATU6 Comdty,FR0014007L00,7.92,95.31,1.12,0.741203
2026-07-24,KOAU6 Comdty,ES0000012L44,7.61,97.05,0.88,0.760118
2026-07-24,TYU6 Comdty,US91282CJK17,6.38,99.14,0.41,0.812004
```

Create `crates/ingest/tests/parse_ctd.rs`:

```rust
use chrono::NaiveDate;

const CSV: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ctd_sample.csv");

#[test]
fn parses_the_sample_csv() {
    let bytes = std::fs::read(CSV).unwrap();
    let rows = ingest::parse_ctd_file(&bytes, "ctd_sample.csv").unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].nav_date, NaiveDate::from_ymd_opt(2026, 7, 24).unwrap());
    assert_eq!(rows[0].ticker, "RXU6 Comdty");
    assert_eq!(rows[0].ctd_isin, "DE0001102580");
    assert!((rows[0].ctd_mod_duration - 8.41).abs() < 1e-12);
    assert!((rows[0].conversion_factor - 0.782145).abs() < 1e-12);
}

#[test]
fn header_order_is_free_and_case_insensitive() {
    let src = "Ticker, NAV_DATE ,conversion_factor,ctd_accrued,ctd_clean_price,ctd_mod_duration,ctd_isin\n\
               RXU6 Comdty,2026-07-24,0.78,0.6,98.7,8.4,DE0001102580\n";
    let rows = ingest::parse_ctd_file(src.as_bytes(), "x.csv").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].ticker, "RXU6 Comdty");
    assert!((rows[0].ctd_mod_duration - 8.4).abs() < 1e-12);
}

#[test]
fn rejects_missing_header_column() {
    let src = "nav_date,ticker,ctd_isin,ctd_mod_duration,ctd_clean_price,ctd_accrued\n\
               2026-07-24,RXU6 Comdty,DE0001102580,8.4,98.7,0.6\n";
    match ingest::parse_ctd_file(src.as_bytes(), "x.csv") {
        Err(ingest::ParseFailure::Workbook(m)) => assert!(m.contains("conversion_factor"), "{m}"),
        other => panic!("expected a workbook-level failure, got {other:?}"),
    }
}

#[test]
fn rejects_empty_file() {
    let src = "nav_date,ticker,ctd_isin,ctd_mod_duration,ctd_clean_price,ctd_accrued,conversion_factor\n";
    match ingest::parse_ctd_file(src.as_bytes(), "x.csv") {
        Err(ingest::ParseFailure::Workbook(m)) => assert!(m.contains("no data rows"), "{m}"),
        other => panic!("expected a workbook-level failure, got {other:?}"),
    }
}

#[test]
fn rejects_disagreeing_nav_dates() {
    let src = "nav_date,ticker,ctd_isin,ctd_mod_duration,ctd_clean_price,ctd_accrued,conversion_factor\n\
               2026-07-24,RXU6 Comdty,DE0001102580,8.4,98.7,0.6,0.78\n\
               2026-07-17,OATU6 Comdty,FR0014007L00,7.9,95.3,1.1,0.74\n";
    match ingest::parse_ctd_file(src.as_bytes(), "x.csv") {
        Err(ingest::ParseFailure::Workbook(m)) => assert!(m.contains("nav_date"), "{m}"),
        other => panic!("expected a workbook-level failure, got {other:?}"),
    }
}

#[test]
fn collects_all_row_errors_before_failing() {
    let src = "nav_date,ticker,ctd_isin,ctd_mod_duration,ctd_clean_price,ctd_accrued,conversion_factor\n\
               2026-07-24,RXU6 Comdty,DE0001102580,0,98.7,0.6,0.78\n\
               2026-07-24,,FR0014007L00,7.9,95.3,1.1,0.74\n\
               2026-07-24,KOAU6 Comdty,ES0000012L44,7.6,97.0,-1,0.76\n\
               2026-07-24,TYU6 Comdty,US91282CJK17,6.4,99.1,0.4,abc\n\
               2026-07-24,RXU6 Comdty,DE0001102580,8.4,98.7,0.6,0.78\n";
    match ingest::parse_ctd_file(src.as_bytes(), "x.csv") {
        Err(ingest::ParseFailure::Rows(rows)) => {
            assert_eq!(rows.len(), 5, "one per bad row, all collected");
            assert_eq!(rows[0].row, 2, "1-based, header is row 1");
            assert!(rows[0].message.contains("ctd_mod_duration"));
            assert!(rows[1].message.contains("ticker"));
            assert!(rows[2].message.contains("ctd_accrued"));
            assert!(rows[3].message.contains("conversion_factor"));
            assert!(rows[4].message.contains("duplicate"), "{}", rows[4].message);
        }
        other => panic!("expected row failures, got {other:?}"),
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"; cargo test -p ingest parse_ctd
```

Expected: FAIL to compile — `cannot find function parse_ctd_file`.

- [ ] **Step 3: Write the implementation**

Add to `crates/ingest/Cargo.toml` under `[dependencies]`:

```toml
csv = "1"
```

Add `#[derive(Debug)]` to `ParseFailure` if it is not already derived — the tests format it with `{other:?}`. It already derives `Debug`; no change needed.

Create `crates/ingest/src/futures_file.rs`:

```rust
//! Parser for the weekly cheapest-to-deliver companion file.
//!
//! One row per bond future. The header is matched by name so column order is
//! free; `nav_date` repeats on every row and all rows must agree.

use crate::{ParseFailure, RowError};
use calamine::{Data, Reader, Xlsx};
use chrono::NaiveDate;
use std::io::Cursor;

const SHEET: &str = "CTD";
const COLUMNS: [&str; 7] = [
    "nav_date",
    "ticker",
    "ctd_isin",
    "ctd_mod_duration",
    "ctd_clean_price",
    "ctd_accrued",
    "conversion_factor",
];

#[derive(Debug, Clone)]
pub struct CtdRow {
    pub nav_date: NaiveDate,
    pub ticker: String,
    pub ctd_isin: String,
    pub ctd_mod_duration: f64,
    pub ctd_clean_price: f64,
    pub ctd_accrued: f64,
    pub conversion_factor: f64,
}

/// Parse the companion file. `.xlsx` is read from its first worksheet;
/// anything else is treated as CSV.
pub fn parse_ctd_file(bytes: &[u8], filename: &str) -> Result<Vec<CtdRow>, ParseFailure> {
    let grid = if filename.to_ascii_lowercase().ends_with(".xlsx") {
        read_xlsx(bytes)?
    } else {
        read_csv(bytes)?
    };
    rows_from_grid(grid)
}

fn read_csv(bytes: &[u8]) -> Result<Vec<Vec<String>>, ParseFailure> {
    let mut rdr = csv::ReaderBuilder::new().has_headers(false).flexible(true).from_reader(bytes);
    let mut out = Vec::new();
    for rec in rdr.records() {
        let rec = rec.map_err(|e| ParseFailure::Workbook(format!("CSV error: {e}")))?;
        out.push(rec.iter().map(|s| s.trim().to_string()).collect());
    }
    Ok(out)
}

fn read_xlsx(bytes: &[u8]) -> Result<Vec<Vec<String>>, ParseFailure> {
    let mut wb: Xlsx<_> =
        Xlsx::new(Cursor::new(bytes.to_vec())).map_err(|e| ParseFailure::Workbook(e.to_string()))?;
    let name = wb
        .sheet_names()
        .iter()
        .find(|n| n.eq_ignore_ascii_case(SHEET))
        .cloned()
        .or_else(|| wb.sheet_names().first().cloned())
        .ok_or_else(|| ParseFailure::Workbook("workbook has no sheets".into()))?;
    let range = wb
        .worksheet_range(&name)
        .map_err(|e| ParseFailure::Workbook(format!("sheet {name}: {e}")))?;
    Ok(range
        .rows()
        .map(|r| {
            r.iter()
                .map(|c| match c {
                    Data::Empty => String::new(),
                    Data::DateTime(dt) => dt
                        .as_datetime()
                        .map(|d| d.date().to_string())
                        .unwrap_or_default(),
                    other => other.to_string().trim().to_string(),
                })
                .collect()
        })
        .collect())
}

fn rows_from_grid(grid: Vec<Vec<String>>) -> Result<Vec<CtdRow>, ParseFailure> {
    let header = grid
        .first()
        .ok_or_else(|| ParseFailure::Workbook("file is empty".into()))?;
    let norm: Vec<String> = header.iter().map(|h| h.trim().to_ascii_lowercase()).collect();

    let mut idx = [0usize; 7];
    for (i, want) in COLUMNS.iter().enumerate() {
        idx[i] = norm
            .iter()
            .position(|h| h == want)
            .ok_or_else(|| ParseFailure::Workbook(format!("missing required column '{want}'")))?;
    }

    let body: Vec<&Vec<String>> = grid
        .iter()
        .skip(1)
        .filter(|r| r.iter().any(|c| !c.trim().is_empty()))
        .collect();
    if body.is_empty() {
        return Err(ParseFailure::Workbook("file has no data rows".into()));
    }

    let mut errors: Vec<RowError> = Vec::new();
    let mut out: Vec<CtdRow> = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    for (n, r) in body.iter().enumerate() {
        let row0 = (n + 1) as u32; // 0-based for RowError, which adds 1; header is row 1
        let cell = |i: usize| r.get(idx[i]).map(|s| s.trim()).unwrap_or("");

        let mut err = |msg: String| errors.push(RowError { sheet: "CTD".into(), row: row0 + 1, message: msg });

        let date = match NaiveDate::parse_from_str(cell(0), "%Y-%m-%d") {
            Ok(d) => Some(d),
            Err(_) => {
                err(format!("nav_date: expected YYYY-MM-DD, got {:?}", cell(0)));
                None
            }
        };
        let ticker = cell(1).to_string();
        if ticker.is_empty() {
            err("ticker: must not be blank".into());
        }
        if !ticker.is_empty() {
            if seen.contains(&ticker) {
                err(format!("duplicate ticker {ticker}"));
            } else {
                seen.push(ticker.clone());
            }
        }
        let isin = cell(2).to_string();
        if isin.is_empty() {
            err("ctd_isin: must not be blank".into());
        }

        let mut num = |i: usize, name: &str, allow_zero: bool| -> Option<f64> {
            match cell(i).replace(',', ".").parse::<f64>() {
                Ok(v) if v.is_finite() && (v > 0.0 || (allow_zero && v == 0.0)) => Some(v),
                Ok(_) => {
                    errors.push(RowError {
                        sheet: "CTD".into(),
                        row: row0 + 1,
                        message: format!("{name}: must be {}", if allow_zero { "zero or positive" } else { "positive" }),
                    });
                    None
                }
                Err(_) => {
                    errors.push(RowError {
                        sheet: "CTD".into(),
                        row: row0 + 1,
                        message: format!("{name}: expected a number, got {:?}", cell(i)),
                    });
                    None
                }
            }
        };
        let dur = num(3, "ctd_mod_duration", false);
        let clean = num(4, "ctd_clean_price", false);
        let accrued = num(5, "ctd_accrued", true);
        let cf = num(6, "conversion_factor", false);

        if let (Some(nav_date), false, false, Some(d), Some(c), Some(a), Some(f)) =
            (date, ticker.is_empty(), isin.is_empty(), dur, clean, accrued, cf)
        {
            out.push(CtdRow {
                nav_date,
                ticker,
                ctd_isin: isin,
                ctd_mod_duration: d,
                ctd_clean_price: c,
                ctd_accrued: a,
                conversion_factor: f,
            });
        }
    }

    if !errors.is_empty() {
        return Err(ParseFailure::Rows(errors));
    }
    let first = out[0].nav_date;
    if out.iter().any(|r| r.nav_date != first) {
        return Err(ParseFailure::Workbook("rows disagree on nav_date".into()));
    }
    Ok(out)
}
```

Add to `crates/ingest/src/lib.rs`, at the top of the file:

```rust
pub mod futures_file;
pub use futures_file::{parse_ctd_file, CtdRow};
```

Note: `RowError.row` is documented as the 1-based Excel row. `row0 + 1` where `row0` counts data rows from 1 gives row 2 for the first data row, correct when the header occupies row 1.

- [ ] **Step 4: Run tests to verify they pass**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"; cargo test -p ingest parse_ctd
```

Expected: PASS, 6 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/ingest/src/futures_file.rs crates/ingest/src/lib.rs crates/ingest/Cargo.toml crates/ingest/tests/parse_ctd.rs crates/ingest/tests/fixtures/ctd_sample.csv Cargo.lock
git commit -m "feat(ingest): weekly CTD companion file parser"
```

---

### Task 7: Persist CTD analytics

**Files:**
- Modify: `crates/db/src/repo.rs`
- Test: `crates/db/tests/futures_analytics.rs`

**Interfaces:**
- Consumes: `ingest::CtdRow` (Task 6)
- Produces: `db::repo::CtdRecord { nav_date: NaiveDate, ticker: String, ctd_isin: String, ctd_mod_duration: f64, ctd_clean_price: f64, ctd_accrued: f64, conversion_factor: f64 }`, `db::repo::ctd_replace(pool, date: NaiveDate, filename: &str, rows: &[ingest::CtdRow]) -> anyhow::Result<usize>`, `db::repo::ctd_for(pool, date: NaiveDate) -> anyhow::Result<Vec<CtdRecord>>`

- [ ] **Step 1: Write the failing test**

Create `crates/db/tests/futures_analytics.rs`:

```rust
use chrono::NaiveDate;

fn d(s: &str) -> NaiveDate { s.parse().unwrap() }

fn row(ticker: &str, dur: f64) -> ingest::CtdRow {
    ingest::CtdRow {
        nav_date: d("2026-07-24"),
        ticker: ticker.into(),
        ctd_isin: "DE0001102580".into(),
        ctd_mod_duration: dur,
        ctd_clean_price: 98.72,
        ctd_accrued: 0.63,
        conversion_factor: 0.782145,
    }
}

#[tokio::test]
async fn ctd_upload_replaces_the_whole_date() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    assert!(db::repo::ctd_for(&pool, d("2026-07-24")).await.unwrap().is_empty());

    let n = db::repo::ctd_replace(&pool, d("2026-07-24"), "a.csv",
        &[row("RXU6 Comdty", 8.41), row("OATU6 Comdty", 7.92)]).await.unwrap();
    assert_eq!(n, 2);
    let got = db::repo::ctd_for(&pool, d("2026-07-24")).await.unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].ticker, "OATU6 Comdty", "sorted by ticker");

    // A corrected re-upload replaces the date wholesale rather than merging.
    let n = db::repo::ctd_replace(&pool, d("2026-07-24"), "b.csv",
        &[row("RXU6 Comdty", 9.99)]).await.unwrap();
    assert_eq!(n, 1);
    let got = db::repo::ctd_for(&pool, d("2026-07-24")).await.unwrap();
    assert_eq!(got.len(), 1, "the OAT row from the first upload is gone");
    assert!((got[0].ctd_mod_duration - 9.99).abs() < 1e-12);

    // Other dates are untouched.
    db::repo::ctd_replace(&pool, d("2026-07-17"), "c.csv", &[row("RXU6 Comdty", 8.0)]).await.unwrap();
    assert_eq!(db::repo::ctd_for(&pool, d("2026-07-24")).await.unwrap().len(), 1);
    assert_eq!(db::repo::ctd_for(&pool, d("2026-07-17")).await.unwrap().len(), 1);

    pool.close().await;
    edb.stop().await;
}
```

- [ ] **Step 2: Run test to verify it fails**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"; cargo test -p db futures_analytics
```

Expected: FAIL to compile — `cannot find function ctd_replace`.

- [ ] **Step 3: Write the implementation**

Append to `crates/db/src/repo.rs`:

```rust
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

/// Replace every analytics row for `date` in one transaction. Unlike the
/// workbook import there is no content dedupe: the expected reason to
/// re-upload is a corrected pull, which must win.
pub async fn ctd_replace(
    pool: &PgPool,
    date: NaiveDate,
    filename: &str,
    rows: &[ingest::CtdRow],
) -> anyhow::Result<usize> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM futures_analytics WHERE nav_date = $1")
        .bind(date)
        .execute(&mut *tx)
        .await?;
    for r in rows {
        sqlx::query(
            "INSERT INTO futures_analytics
               (nav_date, ticker, ctd_isin, ctd_mod_duration, ctd_clean_price,
                ctd_accrued, conversion_factor, source_file)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(r.nav_date).bind(&r.ticker).bind(&r.ctd_isin).bind(r.ctd_mod_duration)
        .bind(r.ctd_clean_price).bind(r.ctd_accrued).bind(r.conversion_factor).bind(filename)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(rows.len())
}

pub async fn ctd_for(pool: &PgPool, date: NaiveDate) -> anyhow::Result<Vec<CtdRecord>> {
    Ok(sqlx::query_as(
        "SELECT nav_date, ticker, ctd_isin,
                ctd_mod_duration::float8 AS ctd_mod_duration,
                ctd_clean_price::float8 AS ctd_clean_price,
                ctd_accrued::float8 AS ctd_accrued,
                conversion_factor::float8 AS conversion_factor
         FROM futures_analytics WHERE nav_date = $1 ORDER BY ticker",
    )
    .bind(date)
    .fetch_all(pool)
    .await?)
}

/// AUM recorded for a NAV date, used as the denominator for exposure.
pub async fn aum_for(pool: &PgPool, date: NaiveDate) -> anyhow::Result<Option<f64>> {
    Ok(sqlx::query_scalar("SELECT aum::float8 FROM nav_history WHERE date = $1")
        .bind(date)
        .fetch_optional(pool)
        .await?)
}
```

- [ ] **Step 4: Run test to verify it passes**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"; cargo test -p db futures_analytics
```

Expected: PASS, 1 test.

- [ ] **Step 5: Commit**

```bash
git add crates/db/src/repo.rs crates/db/tests/futures_analytics.rs
git commit -m "feat(db): persist weekly CTD analytics with per-date replace"
```

---

### Task 8: Contracts CRUD and CTD upload endpoints

**Files:**
- Create: `crates/server/src/handlers/futures.rs`
- Modify: `crates/server/src/handlers/mod.rs`, `crates/server/src/routes.rs`
- Test: `crates/server/tests/api_futures.rs`

**Interfaces:**
- Consumes: `db::repo::{contracts_all, contracts_upsert, ctd_replace, ctd_for, FuturesContract}` (Tasks 4, 7), `ingest::parse_ctd_file` (Task 6)
- Produces: routes `GET /api/futures-contracts`, `PUT /api/futures-contracts/{root}`, `POST /api/futures-analytics`, `GET /api/futures-analytics?date=`; `handlers::futures::CtdUploadOutcome { nav_date: NaiveDate, rows: usize, replaced: bool }`

- [ ] **Step 1: Write the failing test**

Create `crates/server/tests/api_futures.rs`:

```rust
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

const BOUNDARY: &str = "XBOUNDARYX";
const SAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx");
const CTD: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/ctd_sample.csv");

fn upload_req(uri: &str, name: &str, bytes: &[u8]) -> Request<Body> {
    let mut body = Vec::new();
    body.extend_from_slice(format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    ).as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    Request::post(uri)
        .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(Body::from(body))
        .unwrap()
}

async fn get_json(app: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let res = app.clone().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    (status, serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap())
}

async fn put_json(app: &axum::Router, uri: &str, payload: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let req = Request::builder().method(Method::PUT).uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap())).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    (status, serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap())
}

#[tokio::test]
async fn contracts_and_ctd_upload() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    // Uploading CTD before any NAV snapshot exists is rejected, with guidance.
    let ctd = std::fs::read(CTD).unwrap();
    let res = app.clone().oneshot(upload_req("/api/futures-analytics", "ctd.csv", &ctd)).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(body["detail"].as_str().unwrap().contains("NAV Recap"), "{body}");

    // Import the workbook: contracts are seeded unconfirmed.
    let wb = std::fs::read(SAMPLE).unwrap();
    let res = app.clone().oneshot(upload_req("/api/imports", "s.xlsx", &wb)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let (st, cs) = get_json(&app, "/api/futures-contracts").await;
    assert_eq!(st, StatusCode::OK);
    let cs = cs.as_array().unwrap();
    assert_eq!(cs.len(), 7);
    assert!(cs.iter().all(|c| c["confirmed"] == false));

    // Confirm RX by hand.
    let (st, _) = put_json(&app, "/api/futures-contracts/RX", serde_json::json!({
        "label": "Euro-Bund", "category": "interest_rate", "point_value": 1000.0,
        "currency": "EUR", "curve": "DE-10y", "price_convention": "decimal", "confirmed": true,
    })).await;
    assert_eq!(st, StatusCode::OK);

    // Invalid category and point value are rejected.
    let (st, _) = put_json(&app, "/api/futures-contracts/RX", serde_json::json!({
        "label": "x", "category": "bogus", "point_value": 1000.0,
        "currency": "EUR", "curve": null, "price_convention": "decimal", "confirmed": true,
    })).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    let (st, _) = put_json(&app, "/api/futures-contracts/RX", serde_json::json!({
        "label": "x", "category": "interest_rate", "point_value": -1.0,
        "currency": "EUR", "curve": null, "price_convention": "decimal", "confirmed": true,
    })).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);

    // Now the CTD file is accepted.
    let res = app.clone().oneshot(upload_req("/api/futures-analytics", "ctd.csv", &ctd)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["rows"], 4);
    assert_eq!(body["nav_date"], "2026-07-24");

    let (_, rows) = get_json(&app, "/api/futures-analytics?date=2026-07-24").await;
    assert_eq!(rows.as_array().unwrap().len(), 4);

    // A ticker absent from that snapshot is a row error.
    let bad = "nav_date,ticker,ctd_isin,ctd_mod_duration,ctd_clean_price,ctd_accrued,conversion_factor\n\
               2026-07-24,ZZZ9 Comdty,DE0001102580,8.4,98.7,0.6,0.78\n";
    let res = app.clone().oneshot(upload_req("/api/futures-analytics", "bad.csv", bad.as_bytes())).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(body["rows"][0]["message"].as_str().unwrap().contains("ZZZ9"), "{body}");

    pool.close().await;
    edb.stop().await;
}
```

- [ ] **Step 2: Run test to verify it fails**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"; cargo test -p server api_futures
```

Expected: FAIL — 404 on `/api/futures-contracts` (routes not registered).

- [ ] **Step 3: Write the implementation**

Create `crates/server/src/handlers/futures.rs`:

```rust
use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Multipart, Path, Query, State};
use axum::Json;
use chrono::NaiveDate;

#[derive(serde::Deserialize)]
pub struct DateQuery {
    date: Option<String>,
}

pub async fn contracts(State(st): State<AppState>) -> Result<Json<Vec<db::repo::FuturesContract>>, AppError> {
    Ok(Json(db::repo::contracts_all(&st.pool).await?))
}

#[derive(serde::Deserialize)]
pub struct ContractBody {
    pub label: String,
    pub category: String,
    pub point_value: Option<f64>,
    pub currency: String,
    pub curve: Option<String>,
    pub price_convention: String,
    pub confirmed: bool,
}

pub async fn put_contract(
    State(st): State<AppState>,
    Path(root): Path<String>,
    Json(b): Json<ContractBody>,
) -> Result<Json<db::repo::FuturesContract>, AppError> {
    if analytics::Category::parse(&b.category).is_none() {
        return Err(AppError::Unprocessable(format!(
            "category must be one of equity, interest_rate, fx, credit, commodity, other (got {:?})",
            b.category
        )));
    }
    if analytics::PriceConvention::parse(&b.price_convention).is_none() {
        return Err(AppError::Unprocessable("price_convention must be 'decimal' or 'th32'".into()));
    }
    if let Some(pv) = b.point_value {
        if !(pv.is_finite() && pv > 0.0) {
            return Err(AppError::Unprocessable("point_value must be a positive number".into()));
        }
    }
    if b.label.trim().is_empty() || b.currency.trim().is_empty() {
        return Err(AppError::Unprocessable("label and currency must not be blank".into()));
    }
    let c = db::repo::FuturesContract {
        contract_root: root,
        label: b.label.trim().to_string(),
        category: b.category,
        point_value: b.point_value,
        currency: b.currency.trim().to_string(),
        curve: b.curve.map(|c| c.trim().to_string()).filter(|c| !c.is_empty()),
        price_convention: b.price_convention,
        confirmed: b.confirmed,
    };
    db::repo::contracts_upsert(&st.pool, &c).await?;
    Ok(Json(c))
}

#[derive(serde::Serialize)]
pub struct CtdUploadOutcome {
    pub nav_date: NaiveDate,
    pub rows: usize,
    pub replaced: bool,
}

pub async fn upload_ctd(
    State(st): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<CtdUploadOutcome>, AppError> {
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart error: {e}")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field.file_name().unwrap_or("ctd.csv").to_string();
        let bytes = field
            .bytes()
            .await
            .map_err(|e| AppError::BadRequest(format!("read error: {e}")))?;
        let rows = ingest::parse_ctd_file(&bytes, &filename).map_err(|e| match e {
            ingest::ParseFailure::Workbook(m) => AppError::BadRequest(m),
            ingest::ParseFailure::Rows(rows) => AppError::UnprocessableRows(rows),
        })?;

        let date = rows[0].nav_date;
        let known = db::repo::positions_for(&st.pool, date).await?;
        if known.is_empty() {
            return Err(AppError::Unprocessable(format!(
                "no NAV snapshot for {date}; upload the NAV Recap first"
            )));
        }
        let tickers: Vec<&str> = known
            .iter()
            .filter(|p| p.asset_type == "Future")
            .filter_map(|p| p.ticker.as_deref())
            .collect();
        let unknown: Vec<ingest::RowError> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| !tickers.contains(&r.ticker.as_str()))
            .map(|(i, r)| ingest::RowError {
                sheet: "CTD".into(),
                row: (i + 2) as u32,
                message: format!("{} is not a future in the {date} snapshot", r.ticker),
            })
            .collect();
        if !unknown.is_empty() {
            return Err(AppError::UnprocessableRows(unknown));
        }

        let replaced = !db::repo::ctd_for(&st.pool, date).await?.is_empty();
        let n = db::repo::ctd_replace(&st.pool, date, &filename, &rows).await?;
        return Ok(Json(CtdUploadOutcome { nav_date: date, rows: n, replaced }));
    }
    Err(AppError::BadRequest("missing multipart field 'file'".into()))
}

pub async fn list_ctd(
    State(st): State<AppState>,
    Query(q): Query<DateQuery>,
) -> Result<Json<Vec<db::repo::CtdRecord>>, AppError> {
    let date = match &q.date {
        Some(s) => s.parse::<NaiveDate>().map_err(|_| AppError::BadRequest(format!("bad date: {s}")))?,
        None => match db::repo::position_dates(&st.pool).await?.first().copied() {
            Some(d) => d,
            None => return Ok(Json(Vec::new())),
        },
    };
    Ok(Json(db::repo::ctd_for(&st.pool, date).await?))
}
```

Add to `crates/server/src/handlers/mod.rs`:

```rust
pub mod futures;
```

Add to `crates/server/src/routes.rs`, after the `/api/refs/{code}` line:

```rust
        .route("/api/futures-contracts", get(handlers::futures::contracts))
        .route("/api/futures-contracts/{root}", axum::routing::put(handlers::futures::put_contract))
        .route(
            "/api/futures-analytics",
            get(handlers::futures::list_ctd).post(handlers::futures::upload_ctd),
        )
```

- [ ] **Step 4: Run test to verify it passes**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"; cargo test -p server api_futures
```

Expected: PASS, 1 test.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/handlers/futures.rs crates/server/src/handlers/mod.rs crates/server/src/routes.rs crates/server/tests/api_futures.rs
git commit -m "feat(server): futures contract CRUD and CTD upload endpoints"
```

---

### Task 9: Derivatives exposure endpoint

**Files:**
- Modify: `crates/server/src/handlers/limits.rs`, `crates/server/src/routes.rs`
- Test: `crates/server/tests/api_derivatives.rs`

**Interfaces:**
- Consumes: `analytics::{exposure, FuturePosition, Category, PriceConvention, decode_price, contract_root}` (Tasks 1-2), `db::repo::{contracts_all, aum_for}` (Tasks 4, 7)
- Produces: `GET /api/metrics/derivatives?date=` returning `{ dates, date, aum, categories, total, rows, excluded, unconfirmed }`

- [ ] **Step 1: Write the failing test**

Create `crates/server/tests/api_derivatives.rs`:

```rust
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

const BOUNDARY: &str = "XBOUNDARYX";
const SAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx");

fn upload_req(bytes: &[u8]) -> Request<Body> {
    let mut body = Vec::new();
    body.extend_from_slice(format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"s.xlsx\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    ).as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    Request::post("/api/imports")
        .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(Body::from(body)).unwrap()
}

async fn get_json(app: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let res = app.clone().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    (status, serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap())
}

async fn put_json(app: &axum::Router, uri: &str, payload: serde_json::Value) -> StatusCode {
    let req = Request::builder().method(Method::PUT).uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap())).unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

fn spec(cat: &str, pv: f64, ccy: &str, conv: &str) -> serde_json::Value {
    serde_json::json!({
        "label": "x", "category": cat, "point_value": pv, "currency": ccy,
        "curve": null, "price_convention": conv, "confirmed": true,
    })
}

#[tokio::test]
async fn derivatives_exposure_on_sample() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    let bytes = std::fs::read(SAMPLE).unwrap();
    assert_eq!(app.clone().oneshot(upload_req(&bytes)).await.unwrap().status(), StatusCode::OK);

    // Seeded but unconfirmed: rows are listed and flagged.
    let (st, d) = get_json(&app, "/api/metrics/derivatives").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(d["date"], "2026-07-24");
    assert_eq!(d["rows"].as_array().unwrap().len(), 8);
    assert_eq!(d["unconfirmed"].as_array().unwrap().len(), 7);

    // Confirm every contract with its true spec. TY is the 32nds one.
    for (root, cat, pv, ccy, conv) in [
        ("CF", "equity", 10.0, "EUR", "decimal"),
        ("VG", "equity", 10.0, "EUR", "decimal"),
        ("NQ", "equity", 20.0, "USD", "decimal"),
        ("RX", "interest_rate", 1000.0, "EUR", "decimal"),
        ("OAT", "interest_rate", 1000.0, "EUR", "decimal"),
        ("KOA", "interest_rate", 1000.0, "EUR", "decimal"),
        ("TY", "interest_rate", 1000.0, "USD", "th32"),
        ("RY", "fx", 125000.0, "JPY", "decimal"),
    ] {
        assert_eq!(put_json(&app, &format!("/api/futures-contracts/{root}"), spec(cat, pv, ccy, conv)).await,
                   StatusCode::OK, "{root}");
    }

    let (_, d) = get_json(&app, "/api/metrics/derivatives").await;
    assert!(d["unconfirmed"].as_array().unwrap().is_empty());
    assert!(d["excluded"].as_array().unwrap().is_empty());
    assert!((d["aum"].as_f64().unwrap() - 28_332_753.49).abs() < 1e-6);

    let cat = |name: &str| -> serde_json::Value {
        d["categories"].as_array().unwrap().iter()
            .find(|c| c["category"] == name).unwrap().clone()
    };
    let eq = cat("equity");
    assert!((eq["long_pct"].as_f64().unwrap() - 0.0).abs() < 1e-9);
    assert!((eq["short_pct"].as_f64().unwrap() - 0.073086).abs() < 1e-5, "{eq}");

    let ir = cat("interest_rate");
    assert!((ir["long_pct"].as_f64().unwrap() - 0.033832).abs() < 1e-5, "{ir}");
    assert!((ir["short_pct"].as_f64().unwrap() - 0.117307).abs() < 1e-5, "{ir}");

    let fx = cat("fx");
    assert!((fx["short_pct"].as_f64().unwrap() - 0.030817).abs() < 1e-5, "{fx}");

    assert!((d["total"]["gross_pct"].as_f64().unwrap() - 0.255045).abs() < 1e-5, "{}", d["total"]);

    // The TY row proves the 32nds path: notional is qty * 1000 * 108.328125.
    let ty = d["rows"].as_array().unwrap().iter()
        .find(|r| r["ticker"] == "TYU6 Comdty").unwrap();
    assert!((ty["price"].as_f64().unwrap() - 108.328125).abs() < 1e-9, "{ty}");
    assert!((ty["notional_ccy"].as_f64().unwrap() - -649_968.75).abs() < 1e-6, "{ty}");

    // Empty categories are still present, at zero.
    assert!((cat("commodity")["gross_pct"].as_f64().unwrap()).abs() < 1e-12);
    assert_eq!(d["categories"].as_array().unwrap().len(), 6);

    // Bad date -> 400, consistent with the other limits endpoints.
    let (st, _) = get_json(&app, "/api/metrics/derivatives?date=notadate").await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    pool.close().await;
    edb.stop().await;
}
```

- [ ] **Step 2: Run test to verify it fails**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"; cargo test -p server api_derivatives
```

Expected: FAIL — 404 on `/api/metrics/derivatives`.

- [ ] **Step 3: Write the implementation**

Append to `crates/server/src/handlers/limits.rs`:

```rust
/// Futures positions for a snapshot, joined to their contract specs, with
/// prices decoded. Also reports roots whose spec is still unconfirmed.
fn future_positions(
    rows: &[db::repo::PositionRecord],
    specs: &[db::repo::FuturesContract],
) -> (Vec<analytics::FuturePosition>, Vec<String>) {
    let by_root: HashMap<&str, &db::repo::FuturesContract> =
        specs.iter().map(|c| (c.contract_root.as_str(), c)).collect();
    let mut out = Vec::new();
    let mut unconfirmed = Vec::new();
    for p in rows.iter().filter(|p| p.asset_type == "Future") {
        let ticker = p.ticker.clone().unwrap_or_else(|| p.isin.clone());
        let spec = analytics::contract_root(&ticker).and_then(|r| by_root.get(r.as_str()).copied());
        if let Some(s) = spec {
            if !s.confirmed {
                unconfirmed.push(s.contract_root.clone());
            }
        }
        let conv = spec
            .and_then(|s| analytics::PriceConvention::parse(&s.price_convention))
            .unwrap_or(analytics::PriceConvention::Decimal);
        out.push(analytics::FuturePosition {
            ticker,
            name: p.name.clone().unwrap_or_default(),
            currency: p.currency.clone().unwrap_or_default(),
            category: spec
                .and_then(|s| analytics::Category::parse(&s.category))
                .unwrap_or(analytics::Category::Other),
            qty: p.quantity.unwrap_or(0.0),
            price: analytics::decode_price(p.price.unwrap_or(0.0), conv),
            point_value: spec.and_then(|s| s.point_value),
            fx_rate: p.fx_rate,
        });
    }
    unconfirmed.sort();
    unconfirmed.dedup();
    (out, unconfirmed)
}

pub async fn derivatives_h(
    State(st): State<AppState>,
    Query(q): Query<DateQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (dates, date, rows, _refs) = snapshot(&st, &q).await?;
    let specs = db::repo::contracts_all(&st.pool).await?;
    let aum = match date {
        Some(d) => db::repo::aum_for(&st.pool, d).await?.unwrap_or(0.0),
        None => 0.0,
    };
    let (positions, unconfirmed) = future_positions(&rows, &specs);
    let rep = analytics::exposure(&positions, aum);
    Ok(Json(serde_json::json!({
        "dates": dates,
        "date": date,
        "aum": aum,
        "categories": rep.categories,
        "total": rep.total,
        "rows": rep.rows,
        "excluded": rep.excluded,
        "unconfirmed": unconfirmed,
        "note": "Notional by reference to the underlying; long and short each in absolute value as a percentage of net assets. No netting.",
    })))
}
```

Add to `crates/server/src/routes.rs`, after the `/api/metrics/rates` line:

```rust
        .route("/api/metrics/derivatives", get(handlers::limits::derivatives_h))
```

- [ ] **Step 4: Run test to verify it passes**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"; cargo test -p server api_derivatives
```

Expected: PASS, 1 test.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/handlers/limits.rs crates/server/src/routes.rs crates/server/tests/api_derivatives.rs
git commit -m "feat(server): derivatives exposure endpoint"
```

---

### Task 10: Bond futures in the rates section

**Files:**
- Modify: `crates/server/src/handlers/limits.rs` (`rates_h`)
- Test: `crates/server/tests/api_rates_futures.rs`

**Interfaces:**
- Consumes: `future_positions` (Task 9), `analytics::{CtdAnalytics, dv01_position}` (Task 3), `db::repo::ctd_for` (Task 7)
- Produces: `/api/metrics/rates` gains `futures: [...]` and `futures_missing_any: bool`; `futures_note` is removed; `total_dv01_eur` includes futures; `nav_sensitivity_100bp` becomes `100 * total_dv01_eur / aum`

- [ ] **Step 1: Write the failing test**

Create `crates/server/tests/api_rates_futures.rs` using the same `upload_req`, `get_json`, `put_json` and `spec` helpers as `api_derivatives.rs` (repeated here in full so this file stands alone):

```rust
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

const BOUNDARY: &str = "XBOUNDARYX";
const SAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx");
const CTD: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/ctd_sample.csv");

fn upload_req(uri: &str, name: &str, bytes: &[u8]) -> Request<Body> {
    let mut body = Vec::new();
    body.extend_from_slice(format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    ).as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    Request::post(uri)
        .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(Body::from(body)).unwrap()
}

async fn get_json(app: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let res = app.clone().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    (status, serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap())
}

async fn put_json(app: &axum::Router, uri: &str, payload: serde_json::Value) -> StatusCode {
    let req = Request::builder().method(Method::PUT).uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap())).unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

fn spec(cat: &str, pv: f64, ccy: &str, conv: &str) -> serde_json::Value {
    serde_json::json!({
        "label": "x", "category": cat, "point_value": pv, "currency": ccy,
        "curve": null, "price_convention": conv, "confirmed": true,
    })
}

#[tokio::test]
async fn rates_includes_bond_futures_when_ctd_present() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    let wb = std::fs::read(SAMPLE).unwrap();
    assert_eq!(app.clone().oneshot(upload_req("/api/imports", "s.xlsx", &wb)).await.unwrap().status(), StatusCode::OK);

    // Baseline: the cash bond only. Capture it so the restatement can be checked.
    let (_, r0) = get_json(&app, "/api/metrics/rates").await;
    let bond_dv01 = r0["bonds"][0]["dv01_eur"].as_f64().unwrap();
    let total0 = r0["total_dv01_eur"].as_f64().unwrap();
    assert!((total0 - bond_dv01).abs() < 1e-9, "no futures yet");
    assert!(r0["futures"].as_array().unwrap().len() == 4, "four bond futures listed");
    assert!(r0["futures"].as_array().unwrap().iter().all(|f| f["missing"] == true),
            "no CTD analytics uploaded yet");
    assert_eq!(r0["futures_missing_any"], true);

    // The restatement is self-consistent: 100bp sensitivity is 100 x DV01 / AUM.
    let aum = 28_332_753.49f64;
    assert!((r0["nav_sensitivity_100bp"].as_f64().unwrap() - 100.0 * total0 / aum).abs() < 1e-12);
    // And the bond's own DV01 is unchanged: modified duration x market value x 1bp.
    let mv = r0["bonds"][0]["weight"].as_f64().unwrap() * aum;
    let md = r0["bonds"][0]["mod_duration"].as_f64().unwrap();
    assert!((bond_dv01 - md * mv * 1e-4).abs() < 1e-6, "bond figures must not move");

    // Confirm the four bond-future specs, then upload CTD analytics.
    for (root, ccy, conv) in [
        ("RX", "EUR", "decimal"), ("OAT", "EUR", "decimal"),
        ("KOA", "EUR", "decimal"), ("TY", "USD", "th32"),
    ] {
        assert_eq!(put_json(&app, &format!("/api/futures-contracts/{root}"),
                            spec("interest_rate", 1000.0, ccy, conv)).await, StatusCode::OK);
    }
    let ctd = std::fs::read(CTD).unwrap();
    assert_eq!(app.clone().oneshot(upload_req("/api/futures-analytics", "ctd.csv", &ctd)).await.unwrap().status(),
               StatusCode::OK);

    let (_, r) = get_json(&app, "/api/metrics/rates").await;
    let futs = r["futures"].as_array().unwrap();
    assert_eq!(futs.len(), 4);
    assert!(futs.iter().all(|f| f["missing"] == false));
    assert_eq!(r["futures_missing_any"], false);

    // RX: 8.41 * (98.72 + 0.63) * 1000 * 1e-4 / 0.782145 = 106.8259 per contract,
    // held -8, fx 1.0.
    let rx = futs.iter().find(|f| f["ticker"] == "RXU6 Comdty").unwrap();
    assert!((rx["dv01_eur"].as_f64().unwrap() - -854.607).abs() < 1e-2, "{rx}");
    assert!(rx["dv01_eur"].as_f64().unwrap() < 0.0, "a short is negative DV01");

    // Totals move by exactly the futures' contribution.
    let total = r["total_dv01_eur"].as_f64().unwrap();
    let fut_sum: f64 = futs.iter().map(|f| f["dv01_eur"].as_f64().unwrap()).sum();
    assert!((total - (bond_dv01 + fut_sum)).abs() < 1e-6);
    assert!((r["nav_sensitivity_100bp"].as_f64().unwrap() - 100.0 * total / aum).abs() < 1e-12);
    assert!(total < 0.0, "the book is net short rates once futures are counted");

    pool.close().await;
    edb.stop().await;
}
```

- [ ] **Step 2: Run test to verify it fails**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"; cargo test -p server api_rates_futures
```

Expected: FAIL — `r0["futures"]` is null; the response still has `futures_note`.

- [ ] **Step 3: Write the implementation**

In `crates/server/src/handlers/limits.rs`, inside `rates_h`, delete the `futures_note` block:

```rust
    let futures_note: Vec<String> = rows.iter()
        .filter(|p| p.asset_type == "Future")
        .map(|p| p.name.clone().unwrap_or_else(|| p.isin.clone()))
        .collect();
```

and replace it with:

```rust
    // Bond futures: only contracts classified interest_rate, and only where
    // CTD analytics exist for this exact NAV date. No carry-forward.
    let specs = db::repo::contracts_all(&st.pool).await?;
    let (fut_positions, _) = future_positions(&rows, &specs);
    let ctd = match date {
        Some(d) => db::repo::ctd_for(&st.pool, d).await?,
        None => Vec::new(),
    };
    let mut futures = Vec::new();
    let mut futures_missing_any = false;
    for f in fut_positions.iter().filter(|f| f.category == analytics::Category::InterestRate) {
        let a = ctd.iter().find(|c| c.ticker == f.ticker);
        let dv01 = match (a, f.point_value, f.fx_rate) {
            (Some(a), Some(pv), Some(fx)) => analytics::dv01_position(
                &analytics::CtdAnalytics {
                    mod_duration: a.ctd_mod_duration,
                    clean_price: a.ctd_clean_price,
                    accrued: a.ctd_accrued,
                    conversion_factor: a.conversion_factor,
                },
                pv,
                f.qty,
                fx,
            ),
            _ => None,
        };
        match dv01 {
            Some(d) => {
                total_dv01 += d;
                let a = a.unwrap();
                futures.push(serde_json::json!({
                    "ticker": f.ticker, "name": f.name, "missing": false,
                    "qty": f.qty, "price": f.price, "point_value": f.point_value,
                    "ctd_isin": a.ctd_isin, "ctd_mod_duration": a.ctd_mod_duration,
                    "conversion_factor": a.conversion_factor, "dv01_eur": d,
                    "curve": specs.iter()
                        .find(|s| Some(&s.contract_root) == analytics::contract_root(&f.ticker).as_ref())
                        .and_then(|s| s.curve.clone()),
                }));
            }
            None => {
                futures_missing_any = true;
                futures.push(serde_json::json!({
                    "ticker": f.ticker, "name": f.name, "missing": true,
                    "qty": f.qty, "price": f.price, "point_value": f.point_value,
                }));
            }
        }
    }
```

Then replace the response literal's tail. Remove `md_weight_sum` entirely (declare it no longer; delete `let mut md_weight_sum = 0.0f64;` and the `md_weight_sum += m.modified * w;` line), and build the response as:

```rust
    let aum = match date {
        Some(d) => db::repo::aum_for(&st.pool, d).await?.unwrap_or(0.0),
        None => 0.0,
    };
    Ok(Json(serde_json::json!({
        "dates": dates,
        "date": date,
        "bonds": bonds,
        "futures": futures,
        "total_dv01_eur": total_dv01,
        // 100bp in EUR as a fraction of net assets. Algebraically identical to
        // the previous sum(modified x weight) x 0.01 for cash bonds, and it
        // also accepts futures, which have no market-value weight.
        "nav_sensitivity_100bp": if aum > 0.0 { 100.0 * total_dv01 / aum } else { 0.0 },
        "missing_any": missing_any,
        "futures_missing_any": futures_missing_any,
    })))
```

- [ ] **Step 4: Run the test and the existing suite**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"; cargo test -p server
```

Expected: `api_rates_futures` PASSES. `api_limits::limits_and_backtest_on_sample` FAILS on its last rates assertion, `assert!(!r["futures_note"].as_array().unwrap().is_empty())`, because that field is gone.

- [ ] **Step 5: Update the superseded assertion**

In `crates/server/tests/api_limits.rs`, replace:

```rust
    assert!(!r["futures_note"].as_array().unwrap().is_empty());
```

with:

```rust
    // Futures now carry real DV01 rows rather than a text note; without CTD
    // analytics uploaded they are listed as missing.
    assert_eq!(r["futures"].as_array().unwrap().len(), 4);
    assert_eq!(r["futures_missing_any"], true);
```

- [ ] **Step 6: Run the whole workspace**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"; cargo test --workspace
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/server/src/handlers/limits.rs crates/server/tests/api_rates_futures.rs crates/server/tests/api_limits.rs
git commit -m "feat(server): bond-future DV01 in the rates section"
```

---

### Task 11: Derivatives exposure UI

**Files:**
- Create: `frontend/src/components/DerivativesExposure.tsx`
- Modify: `frontend/src/api.ts`, `frontend/src/pages/LimitsPage.tsx`

**Interfaces:**
- Consumes: `GET /api/metrics/derivatives` (Task 9)
- Produces: `Derivatives`, `CategoryTotals`, `ExposureRow` types and `getDerivatives(date?)` in `api.ts`; default-exported `<DerivativesExposure date={...} />`

- [ ] **Step 1: Add the API types and fetcher**

Append to `frontend/src/api.ts`:

```ts
export type Category = "equity" | "interest_rate" | "fx" | "credit" | "commodity" | "other";
export interface CategoryTotals { category: Category; long_pct: number; short_pct: number; gross_pct: number }
export interface ExposureRow {
  ticker: string; name: string; currency: string; category: Category;
  qty: number; price: number; point_value: number | null;
  notional_ccy: number | null; notional_eur: number | null;
  pct_nav: number | null; spec_missing: boolean;
}
export interface Derivatives {
  dates: string[]; date: string | null; aum: number;
  categories: CategoryTotals[]; total: CategoryTotals;
  rows: ExposureRow[]; excluded: string[]; unconfirmed: string[]; note: string;
}
export interface FuturesContract {
  contract_root: string; label: string; category: Category; point_value: number | null;
  currency: string; curve: string | null; price_convention: "decimal" | "th32"; confirmed: boolean;
}
export interface CtdRecord {
  nav_date: string; ticker: string; ctd_isin: string; ctd_mod_duration: number;
  ctd_clean_price: number; ctd_accrued: number; conversion_factor: number;
}
export interface CtdUploadOutcome { nav_date: string; rows: number; replaced: boolean }
export interface FutureRow {
  ticker: string; name: string; missing: boolean; qty: number; price: number;
  point_value: number | null; ctd_isin?: string; ctd_mod_duration?: number;
  conversion_factor?: number; dv01_eur?: number; curve?: string | null;
}

export const getDerivatives = (date?: string) =>
  req<Derivatives>(`/api/metrics/derivatives${date ? `?date=${date}` : ""}`);
export const getFuturesContracts = () => req<FuturesContract[]>("/api/futures-contracts");
export const putFuturesContract = (root: string, body: Omit<FuturesContract, "contract_root">) =>
  req<FuturesContract>(`/api/futures-contracts/${root}`, {
    method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
  });
export const getCtd = (date?: string) => req<CtdRecord[]>(`/api/futures-analytics${date ? `?date=${date}` : ""}`);
export const uploadCtd = (f: File) => {
  const fd = new FormData();
  fd.append("file", f);
  return req<CtdUploadOutcome>("/api/futures-analytics", { method: "POST", body: fd });
};
```

Also update the existing `Rates` interface — `futures_note` no longer exists:

```ts
export interface Rates {
  dates: string[]; date: string | null; bonds: BondRow[]; futures: FutureRow[];
  total_dv01_eur: number; nav_sensitivity_100bp: number;
  missing_any: boolean; futures_missing_any: boolean;
}
```

- [ ] **Step 2: Write the component**

Create `frontend/src/components/DerivativesExposure.tsx`:

```tsx
import { useEffect, useState } from "react";
import { getDerivatives, type Derivatives, type Category } from "../api";
import { pct } from "../fmt";

const LABELS: Record<Category, string> = {
  equity: "Equity",
  interest_rate: "Interest rate",
  fx: "Foreign exchange",
  credit: "Credit",
  commodity: "Commodity",
  other: "Other",
};

export default function DerivativesExposure({ date }: { date?: string }) {
  const [d, setD] = useState<Derivatives | null>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    getDerivatives(date)
      .then((r) => live && setD(r))
      .catch((e) => live && setErr(String(e)));
    return () => {
      live = false;
    };
  }, [date]);

  if (err) return <p className="error">{err}</p>;
  if (!d) return <p>Loading…</p>;
  if (d.rows.length === 0) return <p>No derivative positions in this snapshot.</p>;

  const shown = d.categories.filter((c) => c.gross_pct > 0);

  return (
    <section>
      <h3>Derivatives exposure</h3>
      <p className="muted">{d.note}</p>

      {d.unconfirmed.length > 0 && (
        <p className="warn">
          Unconfirmed contract specs: {d.unconfirmed.join(", ")}. Confirm them on the Data page.
        </p>
      )}
      {d.excluded.length > 0 && (
        <p className="warn">
          Excluded from the totals for want of a spec or FX rate: {d.excluded.join(", ")}.
        </p>
      )}

      <table>
        <thead>
          <tr><th>Category</th><th>Long</th><th>Short</th><th>Gross</th></tr>
        </thead>
        <tbody>
          {shown.map((c) => (
            <tr key={c.category}>
              <td>{LABELS[c.category]}</td>
              <td>{c.long_pct > 0 ? pct(c.long_pct) : "—"}</td>
              <td>{c.short_pct > 0 ? pct(c.short_pct) : "—"}</td>
              <td>{pct(c.gross_pct)}</td>
            </tr>
          ))}
          <tr className="total">
            <td>Total notional</td>
            <td>{pct(d.total.long_pct)}</td>
            <td>{pct(d.total.short_pct)}</td>
            <td>{pct(d.total.gross_pct)}</td>
          </tr>
        </tbody>
      </table>

      <h4>Contracts</h4>
      <table>
        <thead>
          <tr>
            <th>Ticker</th><th>Name</th><th>Category</th><th>Qty</th>
            <th>Price</th><th>Point value</th><th>Notional (EUR)</th><th>% NAV</th>
          </tr>
        </thead>
        <tbody>
          {d.rows.map((r) => (
            <tr key={r.ticker}>
              <td>{r.ticker}</td>
              <td>{r.name}</td>
              <td>{LABELS[r.category]}</td>
              <td>{r.qty}</td>
              <td>{r.price}</td>
              <td>{r.point_value ?? <span className="warn">spec missing</span>}</td>
              <td>{r.notional_eur === null ? "—" : Math.round(r.notional_eur).toLocaleString()}</td>
              <td>{r.pct_nav === null ? "—" : pct(r.pct_nav)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}
```

- [ ] **Step 3: Mount it on the Limits page**

In `frontend/src/pages/LimitsPage.tsx`, add the import at the top:

```tsx
import DerivativesExposure from "../components/DerivativesExposure";
```

and render `<DerivativesExposure date={date} />` after the concentration section, passing whatever state variable that page already uses for the selected date.

- [ ] **Step 4: Verify the build type-checks**

```powershell
$env:PATH = "$env:ProgramFiles\nodejs;$env:PATH"; Set-Location frontend; npm run build
```

Expected: PASS. If `pct` is not exported from `fmt.ts`, read that file and use whichever percentage formatter it exports; if none exists, format inline with `(v * 100).toFixed(2) + "%"`.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/api.ts frontend/src/components/DerivativesExposure.tsx frontend/src/pages/LimitsPage.tsx
git commit -m "feat(ui): derivatives exposure section on the Limits page"
```

---

### Task 12: Contract specs and CTD upload UI

**Files:**
- Create: `frontend/src/components/FuturesContracts.tsx`
- Modify: `frontend/src/pages/DataPage.tsx`

**Interfaces:**
- Consumes: `getFuturesContracts`, `putFuturesContract`, `getCtd`, `uploadCtd` (Task 11)
- Produces: default-exported `<FuturesContracts />`

- [ ] **Step 1: Write the component**

Create `frontend/src/components/FuturesContracts.tsx`:

```tsx
import { useEffect, useState } from "react";
import {
  getFuturesContracts, putFuturesContract, getCtd, uploadCtd,
  ApiError, type FuturesContract, type CtdRecord, type Category,
} from "../api";

const CATEGORIES: Category[] = ["equity", "interest_rate", "fx", "credit", "commodity", "other"];

export default function FuturesContracts() {
  const [rows, setRows] = useState<FuturesContract[]>([]);
  const [ctd, setCtd] = useState<CtdRecord[]>([]);
  const [msg, setMsg] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);

  const reload = () => {
    getFuturesContracts().then(setRows).catch((e) => setErr(String(e)));
    getCtd().then(setCtd).catch(() => setCtd([]));
  };
  useEffect(reload, []);

  const save = async (c: FuturesContract, patch: Partial<FuturesContract>) => {
    setErr(null);
    const { contract_root, ...body } = { ...c, ...patch };
    try {
      await putFuturesContract(contract_root, body);
      reload();
    } catch (e) {
      setErr(e instanceof ApiError ? (e.detail ?? e.message) : String(e));
    }
  };

  const onUpload = async (f: File) => {
    setErr(null);
    setMsg(null);
    try {
      const out = await uploadCtd(f);
      setMsg(`${out.rows} row(s) stored for ${out.nav_date}${out.replaced ? " (replaced)" : ""}.`);
      reload();
    } catch (e) {
      if (e instanceof ApiError && e.rows) {
        setErr(e.rows.map((r) => `row ${r.row}: ${r.message}`).join("; "));
      } else {
        setErr(e instanceof ApiError ? (e.detail ?? e.message) : String(e));
      }
    }
  };

  const unconfirmed = rows.filter((r) => !r.confirmed).length;

  return (
    <section>
      <h3>Futures contracts</h3>
      {unconfirmed > 0 && (
        <p className="warn">
          {unconfirmed} contract spec(s) seeded from the workbook still need confirming. Point value
          is derived from the file and may be wrong where the price convention differs.
        </p>
      )}
      {err && <p className="error">{err}</p>}
      {msg && <p className="muted">{msg}</p>}

      <table>
        <thead>
          <tr>
            <th>Root</th><th>Label</th><th>Category</th><th>Point value</th>
            <th>Ccy</th><th>Curve</th><th>Price convention</th><th>Confirmed</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((c) => (
            <tr key={c.contract_root}>
              <td>{c.contract_root}</td>
              <td>
                <input defaultValue={c.label} onBlur={(e) => save(c, { label: e.target.value })} />
              </td>
              <td>
                <select value={c.category} onChange={(e) => save(c, { category: e.target.value as Category })}>
                  {CATEGORIES.map((x) => <option key={x} value={x}>{x}</option>)}
                </select>
              </td>
              <td>
                <input
                  type="number" step="any" defaultValue={c.point_value ?? ""}
                  onBlur={(e) => save(c, { point_value: e.target.value === "" ? null : Number(e.target.value) })}
                />
              </td>
              <td>
                <input defaultValue={c.currency} onBlur={(e) => save(c, { currency: e.target.value })} />
              </td>
              <td>
                <input
                  defaultValue={c.curve ?? ""}
                  onBlur={(e) => save(c, { curve: e.target.value === "" ? null : e.target.value })}
                />
              </td>
              <td>
                <select
                  value={c.price_convention}
                  onChange={(e) => save(c, { price_convention: e.target.value as "decimal" | "th32" })}
                >
                  <option value="decimal">decimal</option>
                  <option value="th32">th32 (32nds)</option>
                </select>
              </td>
              <td>
                <input
                  type="checkbox" checked={c.confirmed}
                  onChange={(e) => save(c, { confirmed: e.target.checked })}
                />
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      <h4>Weekly CTD analytics</h4>
      <p className="muted">
        One row per bond future, with columns nav_date, ticker, ctd_isin, ctd_mod_duration,
        ctd_clean_price, ctd_accrued, conversion_factor. Re-uploading replaces that NAV date.
      </p>
      <input
        type="file" accept=".csv,.xlsx"
        onChange={(e) => {
          const f = e.target.files?.[0];
          if (f) void onUpload(f);
          e.target.value = "";
        }}
      />
      {ctd.length > 0 && (
        <table>
          <thead>
            <tr><th>Ticker</th><th>CTD ISIN</th><th>Mod duration</th><th>Clean</th><th>Accrued</th><th>CF</th></tr>
          </thead>
          <tbody>
            {ctd.map((r) => (
              <tr key={r.ticker}>
                <td>{r.ticker}</td><td>{r.ctd_isin}</td><td>{r.ctd_mod_duration}</td>
                <td>{r.ctd_clean_price}</td><td>{r.ctd_accrued}</td><td>{r.conversion_factor}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
```

- [ ] **Step 2: Mount it on the Data page**

In `frontend/src/pages/DataPage.tsx`, add the import:

```tsx
import FuturesContracts from "../components/FuturesContracts";
```

and render `<FuturesContracts />` after the existing reference-data section.

- [ ] **Step 3: Surface import warnings**

`ImportOutcome` gained a `warnings` field in Task 5. Add it to the interface in `frontend/src/api.ts`:

```ts
export interface ImportOutcome {
  import_id: number; duplicate: boolean; nav_rows: number; positions: number;
  dividends: number; operations: number; div_ops_replaced: boolean; warnings: string[];
}
```

In `DataPage.tsx`, wherever the upload result is rendered, display the warnings when the array is non-empty — one line each, styled with the existing `warn` class.

- [ ] **Step 4: Verify the build type-checks**

```powershell
$env:PATH = "$env:ProgramFiles\nodejs;$env:PATH"; Set-Location frontend; npm run build
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/api.ts frontend/src/components/FuturesContracts.tsx frontend/src/pages/DataPage.tsx
git commit -m "feat(ui): futures contract specs and weekly CTD upload on the Data page"
```

---

### Task 13: Documentation

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update the Features section**

In `README.md`, replace the parenthetical in the Limits bullet — `(bond futures excluded — no notional data in the file)` — since it is no longer true, and add a bullet:

```markdown
- **Derivatives exposure**: notional by reference to the underlying, by category
  (equity / interest rate / FX / credit / commodity / other), long and short each
  shown in absolute value as a percentage of net assets. Contract point values are
  derived from the workbook on import and confirmed on the Data page.
- **Bond-future DV01**: computed from cheapest-to-deliver analytics uploaded weekly
  as a companion file (`nav_date, ticker, ctd_isin, ctd_mod_duration,
  ctd_clean_price, ctd_accrued, conversion_factor`). A NAV date without analytics
  shows notional exposure normally and marks its DV01 as missing — values are never
  carried forward from a previous week.
```

- [ ] **Step 2: Document the weekly workflow**

Add after the Run section:

```markdown
## Weekly workflow

1. Upload the NAV Recap on the Data page. New futures contracts are seeded with a
   point value derived from the file and flagged unconfirmed; confirm each one
   once, setting its category, curve and price convention. US Treasury futures are
   quoted in 32nds on the portfolio sheet — set their convention to `th32`.
2. Upload the CTD companion file for the same NAV date. Re-uploading replaces that
   date's rows, so a corrected pull simply overwrites.
```

- [ ] **Step 3: Run the full suite one final time**

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:ProgramFiles\nodejs;$env:PATH"; cargo test --workspace; if ($?) { Set-Location frontend; npm run build }
```

Expected: all Rust tests pass; frontend builds.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: futures exposure and weekly CTD workflow"
```

---

## Self-Review Notes

Checked against the spec:

- Every spec section maps to a task: data model → 4; companion file → 6; price decoding, notional, categories, DV01 → 1-3; `nav_sensitivity_100bp` restatement → 10; import cross-check → 5; degradation table → 2 (`spec_missing`, `excluded`), 9 (`unconfirmed`), 10 (`missing`); API table → 8-9; code layout → all; error handling → 6, 8; testing → each task's tests plus the regression assertions in 10.
- The spec's `dv01_override` omission, carry-forward omission and per-curve subtotal omission are all honoured: no task implements them.
- Type consistency: `FuturesContract` field names are identical in `db::repo` (Task 4), the `ContractBody` handler payload (Task 8) and the TypeScript interface (Task 11). `CtdRow` (ingest, Task 6) and `CtdRecord` (db, Task 7) are deliberately distinct — the former is parsed input, the latter is a stored row.
- `future_positions` is defined once in Task 9 and reused by Task 10; Task 10 depends on Task 9 having landed.
