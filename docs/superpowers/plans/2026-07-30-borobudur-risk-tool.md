# Borobudur Risk Tool Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A local Rust web app that imports the Borobudur "NAV Recap" xlsx into embedded PostgreSQL and serves a React dashboard with performance, volatility, drawdown, Sharpe and VaR/ES analytics for a UCITS risk manager.

**Architecture:** Cargo workspace with four crates — `analytics` (pure math, no I/O), `ingest` (calamine xlsx parsing), `db` (sqlx + postgresql_embedded), `server` (axum JSON API + embedded SPA) — plus a Vite/React/TypeScript/ECharts frontend in `frontend/`. Spec: `docs/superpowers/specs/2026-07-30-borobudur-risk-tool-design.md`.

**Tech Stack:** Rust edition 2024; axum 0.8, sqlx 0.8 (runtime-tokio, postgres), calamine 0.26 (`dates` feature), postgresql_embedded 0.18, rust-embed 8, chrono 0.4, thiserror 2, sha2; Vite 7 + React 19 + TypeScript + echarts 6 + react-router-dom 7.

## Global Constraints

- Repo root: `C:\Users\Laurent\Desktop\CC\riskborobudur\borobudur-risk` (already a git repo; run all commands from here unless stated).
- Sample workbook (do NOT commit the original; a fixture copy IS committed): `..\24-07-2026 - Borobudur - NAV Recap.xlsx`.
- Server binds `127.0.0.1:8787`. App data dir: `%LOCALAPPDATA%\borobudur-risk` (via `dirs::data_local_dir()`); embedded PG creds `postgres` / `borobudur-local`, database `borobudur`, PostgreSQL version `=17`.
- All returns/vols/VaR are decimal fractions (0.02 = 2%). VaR/ES reported **positive = loss**. Annualization: 252 trading days, vol factor √252, horizon scaling √h.
- Headline metrics and VaR need ≥ `MIN_OBS = 30` return observations, else `null` + warning string (enforced in `server`, NOT in `analytics`).
- DB columns for money/NAV are `NUMERIC`; bind Rust `f64` on insert (PG assignment-casts float8→numeric) and always read back with `::float8` casts.
- Dates are `chrono::NaiveDate` everywhere; JSON dates are `"YYYY-MM-DD"` strings (chrono serde default).
- Settings defaults: risk_free_rate 0.02, var_confidence 0.99, var_horizon_days 20, var_window_days 252, var_limit 0.20, short_dd_max_days 50.
- Test commands: `cargo test -p <crate>`; frontend check: `npm run build` in `frontend/`. Embedded-PG tests download PG binaries on first run (network needed; allow ~2 min first time).
- Commit after every task (steps include the exact commands). Git identity is already configured repo-locally.

---

### Task 1: Workspace scaffold + analytics crate (NavPoint, daily returns, mean/std)

**Files:**
- Create: `Cargo.toml` (workspace), `.gitignore`, `crates/analytics/Cargo.toml`, `crates/analytics/src/lib.rs`, `crates/analytics/src/returns.rs`, `crates/analytics/src/stats.rs`

**Interfaces:**
- Produces: `analytics::NavPoint { date: NaiveDate, value: f64 }`, `daily_returns(&[NavPoint]) -> Vec<NavPoint>`, `mean(&[f64]) -> Option<f64>`, `sample_std(&[f64]) -> Option<f64>` (n−1 denominator, needs n≥2), test helper pattern `d(y,m,d)`.

- [ ] **Step 1: Scaffold workspace**

`Cargo.toml` (repo root):

```toml
[workspace]
members = ["crates/analytics"]
resolver = "2"

[workspace.dependencies]
chrono = { version = "0.4", features = ["serde"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
```

`.gitignore`:

```
/target
node_modules/
frontend/dist/
```

`crates/analytics/Cargo.toml`:

```toml
[package]
name = "analytics"
version = "0.1.0"
edition = "2024"

[dependencies]
chrono = { workspace = true }
serde = { workspace = true }
```

`crates/analytics/src/lib.rs`:

```rust
pub mod returns;
pub mod stats;

pub use returns::*;
pub use stats::*;
```

- [ ] **Step 2: Write failing tests**

`crates/analytics/src/returns.rs`:

```rust
use chrono::NaiveDate;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct NavPoint {
    pub date: NaiveDate,
    pub value: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn nav(points: &[(i32, u32, u32, f64)]) -> Vec<NavPoint> {
        points.iter().map(|&(y, m, dd, v)| NavPoint { date: d(y, m, dd), value: v }).collect()
    }

    #[test]
    fn daily_returns_basic() {
        let n = nav(&[(2025, 1, 6, 100.0), (2025, 1, 7, 102.0), (2025, 1, 8, 101.0)]);
        let r = daily_returns(&n);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].date, d(2025, 1, 7));
        assert!((r[0].value - 0.02).abs() < 1e-12);
        assert!((r[1].value - (101.0 / 102.0 - 1.0)).abs() < 1e-12);
    }

    #[test]
    fn daily_returns_empty_and_single() {
        assert!(daily_returns(&[]).is_empty());
        assert!(daily_returns(&nav(&[(2025, 1, 6, 100.0)])).is_empty());
    }
}
```

`crates/analytics/src/stats.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mean_and_std() {
        assert_eq!(mean(&[]), None);
        assert!((mean(&[1.0, 2.0, 3.0]).unwrap() - 2.0).abs() < 1e-12);
        assert_eq!(sample_std(&[1.0]), None);
        // std of [0.1, -0.1]: mean 0, var = 0.02/(2-1), std = sqrt(0.02)
        assert!((sample_std(&[0.1, -0.1]).unwrap() - 0.02f64.sqrt()).abs() < 1e-12);
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p analytics`
Expected: compile FAIL (`daily_returns`, `mean`, `sample_std` not found).

- [ ] **Step 4: Implement**

Add to `returns.rs` (above the tests module):

```rust
/// Daily simple returns. Point dated at the later observation. Zero/negative
/// previous NAV rows are skipped (cannot produce a meaningful return).
pub fn daily_returns(nav: &[NavPoint]) -> Vec<NavPoint> {
    nav.windows(2)
        .filter(|w| w[0].value > 0.0)
        .map(|w| NavPoint { date: w[1].date, value: w[1].value / w[0].value - 1.0 })
        .collect()
}
```

Add to `stats.rs`:

```rust
pub fn mean(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() { return None; }
    Some(xs.iter().sum::<f64>() / xs.len() as f64)
}

/// Sample standard deviation (n-1 denominator). None if fewer than 2 values.
pub fn sample_std(xs: &[f64]) -> Option<f64> {
    if xs.len() < 2 { return None; }
    let m = mean(xs)?;
    let var = xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (xs.len() - 1) as f64;
    Some(var.sqrt())
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p analytics`
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat: workspace scaffold + analytics returns/stats"
```

---

### Task 2: Analytics — annualized metrics, YTD, rolling engine

**Files:**
- Create: `crates/analytics/src/metrics.rs`
- Modify: `crates/analytics/src/lib.rs` (add `pub mod metrics; pub use metrics::*;`)

**Interfaces:**
- Consumes: `NavPoint`, `daily_returns`, `mean`, `sample_std` (Task 1).
- Produces: `TRADING_DAYS: f64 = 252.0`; `annualized_vol(&[f64]) -> Option<f64>`; `annualized_return_from_returns(&[f64]) -> Option<f64>`; `sharpe_ratio(ann_ret: f64, ann_vol: f64, rf: f64) -> Option<f64>`; `ytd_performance(&[NavPoint], as_of: NaiveDate) -> Option<f64>`; `rolling(&[NavPoint], window: usize, f: impl Fn(&[f64]) -> Option<f64>) -> Vec<NavPoint>`; `rolling_vol(&[NavPoint], usize) -> Vec<NavPoint>`; `rolling_yield_vol(&[NavPoint], usize) -> Vec<NavPoint>`; `rolling_sharpe(&[NavPoint], usize, rf: f64) -> Vec<NavPoint>`.

- [ ] **Step 1: Write failing tests**

`crates/analytics/src/metrics.rs` (tests module; implementation added in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::NavPoint;
    use chrono::NaiveDate;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, day).unwrap() }
    fn nav(points: &[(i32, u32, u32, f64)]) -> Vec<NavPoint> {
        points.iter().map(|&(y, m, dd, v)| NavPoint { date: d(y, m, dd), value: v }).collect()
    }

    #[test]
    fn annualized_vol_known_value() {
        // returns [0.1, -0.1]: sample std sqrt(0.02) -> ann vol sqrt(0.02*252) = sqrt(5.04)
        let v = annualized_vol(&[0.1, -0.1]).unwrap();
        assert!((v - 5.04f64.sqrt()).abs() < 1e-9);
        assert_eq!(annualized_vol(&[0.1]), None);
    }

    #[test]
    fn annualized_return_round_trip() {
        // 252 equal returns compounding to +10% over one year -> annualized 10%
        let r = 1.1f64.powf(1.0 / 252.0) - 1.0;
        let a = annualized_return_from_returns(&vec![r; 252]).unwrap();
        assert!((a - 0.1).abs() < 1e-9);
        assert_eq!(annualized_return_from_returns(&[]), None);
    }

    #[test]
    fn sharpe_known_value() {
        assert!((sharpe_ratio(0.10, 0.20, 0.02).unwrap() - 0.4).abs() < 1e-12);
        assert_eq!(sharpe_ratio(0.10, 0.0, 0.02), None);
    }

    #[test]
    fn ytd_uses_prior_year_close() {
        let n = nav(&[
            (2024, 12, 30, 100.0), (2024, 12, 31, 102.0),
            (2025, 1, 2, 105.0), (2025, 1, 3, 107.0),
        ]);
        let y = ytd_performance(&n, d(2025, 1, 3)).unwrap();
        assert!((y - (107.0 / 102.0 - 1.0)).abs() < 1e-12);
    }

    #[test]
    fn ytd_falls_back_to_inception() {
        let n = nav(&[(2025, 3, 1, 100.0), (2025, 3, 5, 104.0)]);
        assert!((ytd_performance(&n, d(2025, 3, 5)).unwrap() - 0.04).abs() < 1e-12);
        assert_eq!(ytd_performance(&n, d(2025, 2, 1)), None); // as_of before series
    }

    #[test]
    fn rolling_windows_mechanics() {
        let n = nav(&[
            (2025, 1, 6, 100.0), (2025, 1, 7, 102.0), (2025, 1, 8, 101.0),
            (2025, 1, 9, 103.0), (2025, 1, 10, 104.0),
        ]);
        // 4 returns, window 2 -> 3 output points dated at each window's last return
        let out = rolling_vol(&n, 2);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].date, d(2025, 1, 8));
        assert_eq!(out[2].date, d(2025, 1, 10));
        // each value must equal annualized_vol of that trailing slice
        let rets: Vec<f64> = crate::daily_returns(&n).iter().map(|p| p.value).collect();
        assert!((out[0].value - annualized_vol(&rets[0..2]).unwrap()).abs() < 1e-12);
        assert!((out[2].value - annualized_vol(&rets[2..4]).unwrap()).abs() < 1e-12);
        // window larger than returns -> empty
        assert!(rolling_vol(&n, 5).is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p analytics metrics`
Expected: compile FAIL (functions not found).

- [ ] **Step 3: Implement**

Top of `metrics.rs`:

```rust
use crate::{daily_returns, sample_std, NavPoint};
use chrono::{Datelike, NaiveDate};

pub const TRADING_DAYS: f64 = 252.0;

pub fn annualized_vol(returns: &[f64]) -> Option<f64> {
    Some(sample_std(returns)? * TRADING_DAYS.sqrt())
}

/// Geometric annualization of a window of daily returns:
/// (prod(1+r))^(252/n) - 1. None on empty input or wipeout (growth <= 0).
pub fn annualized_return_from_returns(returns: &[f64]) -> Option<f64> {
    if returns.is_empty() { return None; }
    let growth = returns.iter().fold(1.0, |a, r| a * (1.0 + r));
    if growth <= 0.0 { return None; }
    Some(growth.powf(TRADING_DAYS / returns.len() as f64) - 1.0)
}

pub fn sharpe_ratio(ann_return: f64, ann_vol: f64, risk_free: f64) -> Option<f64> {
    if ann_vol <= 0.0 { return None; }
    Some((ann_return - risk_free) / ann_vol)
}

/// Yield/vol ratio = annualized return / annualized vol (no risk-free deduction).
pub fn yield_vol_ratio(ann_return: f64, ann_vol: f64) -> Option<f64> {
    if ann_vol <= 0.0 { return None; }
    Some(ann_return / ann_vol)
}

/// NAV_last / NAV_(latest date in a prior year) - 1; inception fallback.
pub fn ytd_performance(nav: &[NavPoint], as_of: NaiveDate) -> Option<f64> {
    let last = nav.iter().rev().find(|p| p.date <= as_of)?;
    let base = nav
        .iter()
        .rev()
        .find(|p| p.date.year() < as_of.year())
        .map(|p| p.value)
        .unwrap_or(nav.first()?.value);
    if base <= 0.0 { return None; }
    Some(last.value / base - 1.0)
}

/// Rolling window over DAILY RETURNS of `nav`. Each output point is dated at
/// the last return date of its window. Windows with f(...)==None are skipped.
pub fn rolling(nav: &[NavPoint], window: usize, f: impl Fn(&[f64]) -> Option<f64>) -> Vec<NavPoint> {
    let rets = daily_returns(nav);
    if window < 2 || rets.len() < window { return Vec::new(); }
    let values: Vec<f64> = rets.iter().map(|p| p.value).collect();
    (window..=values.len())
        .filter_map(|end| {
            f(&values[end - window..end]).map(|v| NavPoint { date: rets[end - 1].date, value: v })
        })
        .collect()
}

pub fn rolling_vol(nav: &[NavPoint], window: usize) -> Vec<NavPoint> {
    rolling(nav, window, annualized_vol)
}

pub fn rolling_yield_vol(nav: &[NavPoint], window: usize) -> Vec<NavPoint> {
    rolling(nav, window, |r| {
        yield_vol_ratio(annualized_return_from_returns(r)?, annualized_vol(r)?)
    })
}

pub fn rolling_sharpe(nav: &[NavPoint], window: usize, risk_free: f64) -> Vec<NavPoint> {
    rolling(nav, window, move |r| {
        sharpe_ratio(annualized_return_from_returns(r)?, annualized_vol(r)?, risk_free)
    })
}
```

Update `lib.rs`: add `pub mod metrics;` and `pub use metrics::*;`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p analytics`
Expected: PASS (all tests, tasks 1–2).

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(analytics): annualized vol/return, ytd, sharpe, rolling engine"
```

---

### Task 3: Analytics — drawdown series, yearly max, episodes, top-5 short

**Files:**
- Create: `crates/analytics/src/drawdown.rs`
- Modify: `crates/analytics/src/lib.rs` (add `pub mod drawdown; pub use drawdown::*;`)

**Interfaces:**
- Consumes: `NavPoint`.
- Produces: `drawdown_series(&[NavPoint]) -> Vec<NavPoint>`; `YearlyDrawdown { year: i32, max_drawdown: f64 }`; `yearly_max_drawdowns(&[NavPoint]) -> Vec<YearlyDrawdown>` (peak resets each calendar year); `DrawdownEpisode { peak_date, trough_date: NaiveDate, depth: f64, duration_days: i64, recovery_date: Option<NaiveDate> }`; `drawdown_episodes(&[NavPoint]) -> Vec<DrawdownEpisode>`; `top_short_drawdowns(&[NavPoint], max_calendar_days: i64, top_n: usize) -> Vec<DrawdownEpisode>`. Depth is negative (e.g. −0.10); duration_days = calendar days peak→trough.

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::NavPoint;
    use chrono::NaiveDate;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, day).unwrap() }
    fn nav(points: &[(i32, u32, u32, f64)]) -> Vec<NavPoint> {
        points.iter().map(|&(y, m, dd, v)| NavPoint { date: d(y, m, dd), value: v }).collect()
    }

    // 100,110,99,105,120,108 on Jan 1..6
    fn fixture() -> Vec<NavPoint> {
        nav(&[
            (2025, 1, 1, 100.0), (2025, 1, 2, 110.0), (2025, 1, 3, 99.0),
            (2025, 1, 4, 105.0), (2025, 1, 5, 120.0), (2025, 1, 6, 108.0),
        ])
    }

    #[test]
    fn underwater_series() {
        let dd = drawdown_series(&fixture());
        let vals: Vec<f64> = dd.iter().map(|p| p.value).collect();
        let expect = [0.0, 0.0, -0.1, 105.0 / 110.0 - 1.0, 0.0, -0.1];
        for (v, e) in vals.iter().zip(expect) { assert!((v - e).abs() < 1e-12); }
        assert_eq!(dd[2].date, d(2025, 1, 3));
    }

    #[test]
    fn episodes_detected() {
        let eps = drawdown_episodes(&fixture());
        assert_eq!(eps.len(), 2);
        assert_eq!(eps[0].peak_date, d(2025, 1, 2));
        assert_eq!(eps[0].trough_date, d(2025, 1, 3));
        assert!((eps[0].depth - (-0.1)).abs() < 1e-12);
        assert_eq!(eps[0].duration_days, 1);
        assert_eq!(eps[0].recovery_date, Some(d(2025, 1, 5)));
        assert_eq!(eps[1].peak_date, d(2025, 1, 5));
        assert_eq!(eps[1].recovery_date, None); // ongoing
    }

    #[test]
    fn top_short_filters_and_ranks() {
        let eps = top_short_drawdowns(&fixture(), 50, 5);
        assert_eq!(eps.len(), 2);
        // both -10%; deeper-or-equal first, stable by date
        assert!(eps[0].depth <= eps[1].depth);
        // duration filter: max 0 days excludes both
        assert!(top_short_drawdowns(&fixture(), 0, 5).is_empty());
    }

    #[test]
    fn yearly_max_resets_peak_at_year_start() {
        let n = nav(&[
            (2024, 12, 30, 100.0), (2024, 12, 31, 90.0),
            (2025, 1, 2, 95.0), (2025, 1, 3, 85.0),
        ]);
        let y = yearly_max_drawdowns(&n);
        assert_eq!(y.len(), 2);
        assert_eq!(y[0].year, 2024);
        assert!((y[0].max_drawdown - (-0.10)).abs() < 1e-12);
        assert_eq!(y[1].year, 2025);
        // peak resets to 95 in 2025 -> 85/95-1, NOT 85/100-1
        assert!((y[1].max_drawdown - (85.0 / 95.0 - 1.0)).abs() < 1e-12);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p analytics drawdown`
Expected: compile FAIL.

- [ ] **Step 3: Implement**

```rust
use crate::NavPoint;
use chrono::{Datelike, NaiveDate};

#[derive(Debug, Clone, serde::Serialize)]
pub struct YearlyDrawdown {
    pub year: i32,
    pub max_drawdown: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DrawdownEpisode {
    pub peak_date: NaiveDate,
    pub trough_date: NaiveDate,
    pub depth: f64,
    pub duration_days: i64,
    pub recovery_date: Option<NaiveDate>,
}

/// NAV_t / running_peak - 1 (values <= 0).
pub fn drawdown_series(nav: &[NavPoint]) -> Vec<NavPoint> {
    let mut peak = f64::NEG_INFINITY;
    nav.iter()
        .map(|p| {
            peak = peak.max(p.value);
            NavPoint { date: p.date, value: p.value / peak - 1.0 }
        })
        .collect()
}

/// Deepest drawdown per calendar year; the running peak RESETS at each
/// year start (per spec). Years in ascending order.
pub fn yearly_max_drawdowns(nav: &[NavPoint]) -> Vec<YearlyDrawdown> {
    let mut out: Vec<YearlyDrawdown> = Vec::new();
    let mut peak = f64::NEG_INFINITY;
    for p in nav {
        let year = p.date.year();
        if out.last().map(|y| y.year) != Some(year) {
            out.push(YearlyDrawdown { year, max_drawdown: 0.0 });
            peak = p.value;
        }
        peak = peak.max(p.value);
        let dd = p.value / peak - 1.0;
        let cur = out.last_mut().unwrap();
        if dd < cur.max_drawdown { cur.max_drawdown = dd; }
    }
    out
}

/// Distinct peak->trough episodes. An episode opens when NAV drops below the
/// running peak and closes at the first NAV >= that peak (recovery) or at
/// series end (recovery_date = None).
pub fn drawdown_episodes(nav: &[NavPoint]) -> Vec<DrawdownEpisode> {
    let mut episodes = Vec::new();
    let Some(first) = nav.first() else { return episodes; };
    let mut peak = first.clone();
    let mut trough: Option<NavPoint> = None;
    for p in &nav[1..] {
        if p.value >= peak.value {
            if let Some(t) = trough.take() {
                episodes.push(make_episode(&peak, &t, Some(p.date)));
            }
            peak = p.clone();
        } else if trough.as_ref().is_none_or(|t| p.value < t.value) {
            trough = Some(p.clone());
        }
    }
    if let Some(t) = trough {
        episodes.push(make_episode(&peak, &t, None));
    }
    episodes
}

fn make_episode(peak: &NavPoint, trough: &NavPoint, recovery: Option<NaiveDate>) -> DrawdownEpisode {
    DrawdownEpisode {
        peak_date: peak.date,
        trough_date: trough.date,
        depth: trough.value / peak.value - 1.0,
        duration_days: (trough.date - peak.date).num_days(),
        recovery_date: recovery,
    }
}

/// Episodes with peak->trough duration <= max_calendar_days, deepest first.
pub fn top_short_drawdowns(nav: &[NavPoint], max_calendar_days: i64, top_n: usize) -> Vec<DrawdownEpisode> {
    let mut eps: Vec<DrawdownEpisode> = drawdown_episodes(nav)
        .into_iter()
        .filter(|e| e.duration_days <= max_calendar_days && e.duration_days >= 1)
        .collect();
    eps.sort_by(|a, b| a.depth.partial_cmp(&b.depth).unwrap());
    eps.truncate(top_n);
    eps
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p analytics`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(analytics): drawdown series, yearly max, episodes, top short"
```

---

### Task 4: Analytics — monthly / quarterly / annual calendar returns

**Files:**
- Create: `crates/analytics/src/calendar.rs`
- Modify: `crates/analytics/src/lib.rs` (add `pub mod calendar; pub use calendar::*;`)

**Interfaces:**
- Consumes: `NavPoint`.
- Produces: `PeriodReturn { year: i32, period: u32, value: f64 }` (period = month 1–12 or quarter 1–4); `monthly_returns(&[NavPoint]) -> Vec<PeriodReturn>`; `quarterly_returns(&[NavPoint]) -> Vec<PeriodReturn>`; `annual_returns(&[NavPoint]) -> Vec<PeriodReturn>` (period = 0). Reference for the first period is the series' first point (inception).

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::NavPoint;
    use chrono::NaiveDate;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, day).unwrap() }
    fn nav(points: &[(i32, u32, u32, f64)]) -> Vec<NavPoint> {
        points.iter().map(|&(y, m, dd, v)| NavPoint { date: d(y, m, dd), value: v }).collect()
    }

    fn fixture() -> Vec<NavPoint> {
        nav(&[
            (2025, 1, 31, 100.0), (2025, 2, 15, 103.0),
            (2025, 2, 28, 104.0), (2025, 3, 31, 102.0),
        ])
    }

    #[test]
    fn monthly_last_of_month_vs_prev() {
        let m = monthly_returns(&fixture());
        assert_eq!(m.len(), 3);
        assert_eq!((m[0].year, m[0].period), (2025, 1));
        assert!((m[0].value - 0.0).abs() < 1e-12); // inception month: 100/100-1
        assert!((m[1].value - 0.04).abs() < 1e-12); // 104/100-1 (mid-month 103 ignored)
        assert!((m[2].value - (102.0 / 104.0 - 1.0)).abs() < 1e-12);
    }

    #[test]
    fn quarterly_compounds_across_months() {
        let q = quarterly_returns(&fixture());
        assert_eq!(q.len(), 1);
        assert_eq!((q[0].year, q[0].period), (2025, 1));
        assert!((q[0].value - 0.02).abs() < 1e-12); // 102/100-1
    }

    #[test]
    fn annual_totals() {
        let n = nav(&[(2024, 12, 31, 100.0), (2025, 3, 31, 104.0), (2025, 6, 30, 106.0)]);
        let a = annual_returns(&n);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].year, 2024);
        assert!((a[0].value - 0.0).abs() < 1e-12);
        assert_eq!(a[1].year, 2025);
        assert!((a[1].value - 0.06).abs() < 1e-12); // 106/100-1
    }

    #[test]
    fn empty_series() {
        assert!(monthly_returns(&[]).is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p analytics calendar`
Expected: compile FAIL.

- [ ] **Step 3: Implement**

```rust
use crate::NavPoint;
use chrono::Datelike;

#[derive(Debug, Clone, serde::Serialize)]
pub struct PeriodReturn {
    pub year: i32,
    pub period: u32,
    pub value: f64,
}

/// Generic period-end returns: `key` maps a point to its (year, period)
/// bucket; return = last NAV of bucket / last NAV of previous bucket - 1
/// (first bucket: vs first point of the series).
fn period_returns(nav: &[NavPoint], key: impl Fn(&NavPoint) -> (i32, u32)) -> Vec<PeriodReturn> {
    let Some(first) = nav.first() else { return Vec::new(); };
    let mut ends: Vec<(i32, u32, f64)> = Vec::new();
    for p in nav {
        let (y, per) = key(p);
        match ends.last_mut() {
            Some(last) if (last.0, last.1) == (y, per) => last.2 = p.value,
            _ => ends.push((y, per, p.value)),
        }
    }
    let mut prev = first.value;
    ends.into_iter()
        .map(|(year, period, v)| {
            let r = PeriodReturn { year, period, value: v / prev - 1.0 };
            prev = v;
            r
        })
        .collect()
}

pub fn monthly_returns(nav: &[NavPoint]) -> Vec<PeriodReturn> {
    period_returns(nav, |p| (p.date.year(), p.date.month()))
}

pub fn quarterly_returns(nav: &[NavPoint]) -> Vec<PeriodReturn> {
    period_returns(nav, |p| (p.date.year(), (p.date.month() - 1) / 3 + 1))
}

/// Annual totals; `period` is always 0.
pub fn annual_returns(nav: &[NavPoint]) -> Vec<PeriodReturn> {
    period_returns(nav, |p| (p.date.year(), 0))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p analytics`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(analytics): monthly/quarterly/annual calendar returns"
```

---

### Task 5: Analytics — VaR / Expected Shortfall (historical, Gaussian, Cornish-Fisher)

**Files:**
- Create: `crates/analytics/src/var.rs`
- Modify: `crates/analytics/src/lib.rs` (add `pub mod var; pub use var::*;`), `crates/analytics/src/stats.rs` (add skewness/kurtosis)

**Interfaces:**
- Consumes: `mean`, `sample_std`, `NavPoint`, `daily_returns`, `rolling` (Task 2).
- Produces: `VarMethod { Historical, Gaussian, CornishFisher }`; `VarEs { var: f64, es: f64 }` (positive = loss, decimal fraction, horizon-scaled); `var_es(returns: &[f64], method: VarMethod, confidence: f64, horizon_days: f64) -> Option<VarEs>`; `rolling_var(nav: &[NavPoint], window: usize, method: VarMethod, confidence: f64, horizon_days: f64) -> Vec<NavPoint>`; `inverse_normal_cdf(p: f64) -> f64`; `stats::skewness(&[f64]) -> Option<f64>`, `stats::excess_kurtosis(&[f64]) -> Option<f64>` (population moment estimators m3/m2^1.5 and m4/m2²−3, need n≥2 and m2>0).

- [ ] **Step 1: Write failing tests**

Append to `stats.rs` tests:

```rust
    #[test]
    fn skew_kurt_known_values() {
        let sym = [-0.02, -0.01, 0.0, 0.01, 0.02];
        assert!(skewness(&sym).unwrap().abs() < 1e-12);
        // m2=2e-4, m4=6.8e-8 -> kurt=1.7 -> excess=-1.3
        assert!((excess_kurtosis(&sym).unwrap() - (-1.3)).abs() < 1e-9);
        assert_eq!(skewness(&[1.0]), None);
    }
```

`crates/analytics/src/var.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::NavPoint;
    use chrono::NaiveDate;

    const RETS: [f64; 10] = [-0.05, -0.03, -0.02, -0.01, 0.0, 0.01, 0.02, 0.03, 0.04, 0.06];

    #[test]
    fn inverse_normal_cdf_known_values() {
        assert!(inverse_normal_cdf(0.5).abs() < 1e-9);
        assert!((inverse_normal_cdf(0.975) - 1.959964).abs() < 1e-5);
        assert!((inverse_normal_cdf(0.99) - 2.326348).abs() < 1e-5);
        assert!((inverse_normal_cdf(0.01) + 2.326348).abs() < 1e-5);
        assert!(inverse_normal_cdf(0.0).is_nan());
    }

    #[test]
    fn historical_var_es() {
        // p=0.1, idx=0.9 -> quantile between -0.05 and -0.03 = -0.032
        let v = var_es(&RETS, VarMethod::Historical, 0.90, 1.0).unwrap();
        assert!((v.var - 0.032).abs() < 1e-12);
        // ES = mean of worst ceil(0.1*10)=1 obs = 0.05
        assert!((v.es - 0.05).abs() < 1e-12);
        // horizon scaling sqrt(4)=2
        let v4 = var_es(&RETS, VarMethod::Historical, 0.90, 4.0).unwrap();
        assert!((v4.var - 0.064).abs() < 1e-12);
        assert!((v4.es - 0.10).abs() < 1e-12);
    }

    #[test]
    fn gaussian_var_es_from_moments() {
        // mu=0, sigma=0.01 via symmetric two-point sample is invalid (std uses n-1);
        // instead test the internal on a crafted sample: use returns with known
        // sample stats: [-0.01, 0.01] -> mean 0, sample std = sqrt(2e-4/1)=0.0141421...
        // Simpler: call the moment-level helper directly.
        let g = gaussian_var_es(0.0, 0.01, 0.99).unwrap();
        assert!((g.0 - 0.023263479).abs() < 1e-6); // z_.99 * sigma - mu
        assert!((g.1 - 0.026652142).abs() < 1e-5); // sigma * phi(z)/(1-c) - mu
    }

    #[test]
    fn cornish_fisher_reduces_to_gaussian_when_normal() {
        // symmetric sample -> skew 0; CF with s=0,k=0 must match Gaussian closely
        // (CF ES uses numerical tail integration; allow 1e-3 relative slack)
        let sample = [-0.02, -0.01, 0.0, 0.01, 0.02];
        let g = var_es(&sample, VarMethod::Gaussian, 0.99, 1.0).unwrap();
        let cf = var_es_with_moments(&sample, 0.99, 1.0, 0.0, 0.0).unwrap();
        assert!((g.var - cf.var).abs() < 1e-9);
        assert!((g.es - cf.es).abs() / g.es < 2e-3);
    }

    #[test]
    fn rolling_var_mechanics() {
        let mut nav = Vec::new();
        let mut v = 100.0;
        for i in 0..40u32 {
            v *= if i % 2 == 0 { 1.01 } else { 0.995 };
            nav.push(NavPoint {
                date: NaiveDate::from_ymd_opt(2025, 1, 1).unwrap() + chrono::Days::new(i as u64),
                value: v,
            });
        }
        let out = rolling_var(&nav, 20, VarMethod::Historical, 0.99, 1.0);
        assert_eq!(out.len(), nav.len() - 1 - 20 + 1); // returns n-1, windows
        assert!(out.iter().all(|p| p.value.is_finite()));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p analytics var`
Expected: compile FAIL.

- [ ] **Step 3: Implement**

Append to `stats.rs`:

```rust
/// Population skewness m3 / m2^1.5. None if n<2 or zero variance.
pub fn skewness(xs: &[f64]) -> Option<f64> {
    let (m2, m3, _) = central_moments(xs)?;
    if m2 <= 0.0 { return None; }
    Some(m3 / m2.powf(1.5))
}

/// Population excess kurtosis m4 / m2^2 - 3. None if n<2 or zero variance.
pub fn excess_kurtosis(xs: &[f64]) -> Option<f64> {
    let (m2, _, m4) = central_moments(xs)?;
    if m2 <= 0.0 { return None; }
    Some(m4 / (m2 * m2) - 3.0)
}

fn central_moments(xs: &[f64]) -> Option<(f64, f64, f64)> {
    if xs.len() < 2 { return None; }
    let m = mean(xs)?;
    let n = xs.len() as f64;
    let (mut m2, mut m3, mut m4) = (0.0, 0.0, 0.0);
    for x in xs {
        let d = x - m;
        m2 += d * d;
        m3 += d * d * d;
        m4 += d * d * d * d;
    }
    Some((m2 / n, m3 / n, m4 / n))
}
```

`crates/analytics/src/var.rs` implementation:

```rust
use crate::{excess_kurtosis, mean, rolling, sample_std, skewness, NavPoint};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VarMethod {
    Historical,
    Gaussian,
    CornishFisher,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct VarEs {
    /// Positive = loss, decimal fraction of NAV, horizon-scaled.
    pub var: f64,
    pub es: f64,
}

/// 1-day VaR/ES from daily returns, scaled by sqrt(horizon_days).
/// Needs n>=2, confidence in (0.5, 1), horizon >= 1.
pub fn var_es(returns: &[f64], method: VarMethod, confidence: f64, horizon_days: f64) -> Option<VarEs> {
    if returns.len() < 2 || !(0.5..1.0).contains(&confidence) || horizon_days < 1.0 {
        return None;
    }
    let (var1, es1) = match method {
        VarMethod::Historical => historical_var_es(returns, confidence)?,
        VarMethod::Gaussian => {
            gaussian_var_es(mean(returns)?, sample_std(returns)?, confidence)?
        }
        VarMethod::CornishFisher => {
            let s = skewness(returns)?;
            let k = excess_kurtosis(returns)?;
            let r = var_es_with_moments(returns, confidence, 1.0, s, k)?;
            (r.var, r.es)
        }
    };
    let scale = horizon_days.sqrt();
    Some(VarEs { var: var1 * scale, es: es1 * scale })
}

/// Empirical quantile with linear interpolation at index p*(n-1) on the
/// ascending-sorted returns; ES = mean of the worst ceil(p*n) observations.
fn historical_var_es(returns: &[f64], confidence: f64) -> Option<(f64, f64)> {
    let mut sorted = returns.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = sorted.len();
    let p = 1.0 - confidence;
    let idx = p * (n - 1) as f64;
    let lo = idx.floor() as usize;
    let hi = idx.ceil() as usize;
    let q = sorted[lo] + (idx - lo as f64) * (sorted[hi] - sorted[lo]);
    let tail_n = ((p * n as f64).ceil() as usize).clamp(1, n);
    let es = -(sorted[..tail_n].iter().sum::<f64>() / tail_n as f64);
    Some((-q, es))
}

/// Analytic Gaussian 1-day (VaR, ES): (z_c*sigma - mu, sigma*phi(z_c)/(1-c) - mu).
pub fn gaussian_var_es(mu: f64, sigma: f64, confidence: f64) -> Option<(f64, f64)> {
    if sigma <= 0.0 { return None; }
    let z = inverse_normal_cdf(confidence);
    let var = z * sigma - mu;
    let es = sigma * normal_pdf(z) / (1.0 - confidence) - mu;
    Some((var, es))
}

/// Cornish-Fisher VaR/ES with explicit skew/kurtosis (exposed for tests).
/// ES via 200-step midpoint integration of the CF quantile over the tail.
pub fn var_es_with_moments(returns: &[f64], confidence: f64, horizon_days: f64, s: f64, k: f64) -> Option<VarEs> {
    let mu = mean(returns)?;
    let sd = sample_std(returns)?;
    if sd <= 0.0 { return None; }
    let z = inverse_normal_cdf(1.0 - confidence); // negative tail z
    let var1 = -(mu + sd * cornish_fisher_z(z, s, k));
    let p_tail = 1.0 - confidence;
    const STEPS: usize = 200;
    let mut acc = 0.0;
    for i in 0..STEPS {
        let p = p_tail * (i as f64 + 0.5) / STEPS as f64;
        acc += mu + sd * cornish_fisher_z(inverse_normal_cdf(p), s, k);
    }
    let es1 = -(acc / STEPS as f64);
    let scale = horizon_days.sqrt();
    Some(VarEs { var: var1 * scale, es: es1 * scale })
}

fn cornish_fisher_z(z: f64, s: f64, k: f64) -> f64 {
    z + (z * z - 1.0) * s / 6.0 + (z * z * z - 3.0 * z) * k / 24.0
        - (2.0 * z * z * z - 5.0 * z) * s * s / 36.0
}

fn normal_pdf(z: f64) -> f64 {
    (-0.5 * z * z).exp() / (2.0 * std::f64::consts::PI).sqrt()
}

/// Acklam's inverse normal CDF approximation, |relative error| < 1.15e-9.
/// Reference: https://web.archive.org/web/20151110174102/http://home.online.no/~pjacklam/notes/invnorm/
pub fn inverse_normal_cdf(p: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969683028665376e+01, 2.209460984245205e+02, -2.759285104469687e+02,
        1.383577518672690e+02, -3.066479806614716e+01, 2.506628277459239e+00,
    ];
    const B: [f64; 5] = [
        -5.447609879822406e+01, 1.615858368580409e+02, -1.556989798598866e+02,
        6.680131188771972e+01, -1.328068155288572e+01,
    ];
    const C: [f64; 6] = [
        -7.784894002430293e-03, -3.223964580411365e-01, -2.400758277161838e+00,
        -2.549732539343734e+00, 4.374664141464968e+00, 2.938163982698783e+00,
    ];
    const D: [f64; 4] = [
        7.784695709041462e-03, 3.224671290700398e-01, 2.445134137142996e+00,
        3.754408661907416e+00,
    ];
    const P_LOW: f64 = 0.02425;
    if !(0.0..=1.0).contains(&p) || p == 0.0 || p == 1.0 {
        return f64::NAN;
    }
    if p < P_LOW {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p <= 1.0 - P_LOW {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        -((((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0))
    }
}

/// Rolling VaR over trailing `window` daily returns; value = horizon-scaled VaR.
pub fn rolling_var(nav: &[NavPoint], window: usize, method: VarMethod, confidence: f64, horizon_days: f64) -> Vec<NavPoint> {
    rolling(nav, window, move |r| var_es(r, method, confidence, horizon_days).map(|v| v.var))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p analytics`
Expected: PASS (all analytics tests).

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(analytics): VaR/ES historical, gaussian, cornish-fisher + rolling VaR"
```

---

### Task 6: Ingest crate — parse the 4-sheet workbook

**Files:**
- Create: `crates/ingest/Cargo.toml`, `crates/ingest/src/lib.rs`, `crates/ingest/tests/parse_sample.rs`, `crates/ingest/tests/fixtures/sample.xlsx` (copied from the real file)
- Modify: root `Cargo.toml` members (add `"crates/ingest"`)

**Interfaces:**
- Consumes: nothing from other crates (chrono only).
- Produces:

```rust
pub struct ParsedWorkbook {
    pub nav_date: NaiveDate, pub aum: f64, pub shares: f64, pub nav: f64,
    pub positions: Vec<PositionRow>, pub nav_history: Vec<NavHistoryRow>,
    pub dividends: Vec<DividendRow>, pub operations: Vec<OperationRow>,
}
pub struct PositionRow {
    pub asset_type: String, pub isin: String, pub name: Option<String>,
    pub currency: Option<String>, pub quantity: Option<f64>, pub avg_cost: Option<f64>,
    pub price: Option<f64>, pub valuation_ccy: Option<f64>, pub accrued_interest: Option<f64>,
    pub fx_rate: Option<f64>, pub valuation_eur: Option<f64>, pub weight: Option<f64>,
    pub ticker: Option<String>,
}
pub struct NavHistoryRow { pub date: NaiveDate, pub aum: f64, pub shares: f64, pub nav: f64 }
pub struct DividendRow { pub provision_date: NaiveDate, pub payment_date: Option<NaiveDate>, pub issuer: String, pub amount: f64, pub currency: String }
pub struct OperationRow { pub trade_date: NaiveDate, pub side: String, pub ticker: Option<String>, pub isin: Option<String>, pub name: Option<String>, pub currency: Option<String>, pub quantity: Option<f64>, pub price: Option<f64>, pub gross_amount: Option<f64>, pub fees: Option<f64>, pub net_price: Option<f64>, pub net_amount: Option<f64> }
pub struct RowError { pub sheet: String, pub row: u32, pub message: String }   // row is 1-based Excel row
pub enum ParseFailure { Workbook(String), Rows(Vec<RowError>) }
pub fn parse_workbook(bytes: &[u8]) -> Result<ParsedWorkbook, ParseFailure>
```

**Sheet layout facts (verified against the sample):**
- `PORTEFEUILLE_NAV`: B3 = NAV date; F2 = AUM; F3 = shares; F4 = NAV. Headers Excel row 7; data rows 8+. Main columns A–M: asset_type, isin, name, currency, quantity, avg_cost, price, valuation_ccy, accrued_interest, fx_rate, valuation_eur, weight, ticker. Mid-sheet there is a separator row whose col A = `CASH` with everything else empty (skip), then a sub-header row with col A = `Type` that SWITCHES the column layout for all following rows to: A asset_type, B isin ("Code"), C name, D currency, E quantity, F valuation_ccy, G valuation_eur, H accrued_interest, I weight (avg_cost/price/fx_rate/ticker absent). Sample yields **111** position rows.
- `HISTO_NAV`: headers row 1 (Date, AUM, Nb parts, NAV); data rows 2+; **343** rows from 2025-02-28 (NAV 100) to 2026-07-23 (NAV 103.99). All 4 cells required.
- `DIV`: headers row 1; **53** rows; provision_date, issuer, amount, currency required; payment_date optional.
- `OPERATIONS`: headers Excel row 3; data rows 4+; **2050** rows; trade_date and side required (values include `Achat`, `Vente`, `VENTE` — store as-is); numeric columns lenient (empty allowed, garbage = error). Columns A–L: date, side, ticker, isin, name, currency, quantity, price, gross_amount, fees, net_price, net_amount (cols M/N repeat isin/ticker — ignore).
- Strictness: a non-empty cell that fails to parse as its expected type is a `RowError`; collect ALL errors and return `ParseFailure::Rows` (no partial results). Sample-file sanity check: every `nav_history` date must be `<= nav_date` else RowError.

- [ ] **Step 1: Copy the fixture and scaffold the crate**

```bash
mkdir -p crates/ingest/tests/fixtures
cp "../24-07-2026 - Borobudur - NAV Recap.xlsx" crates/ingest/tests/fixtures/sample.xlsx
```

`crates/ingest/Cargo.toml`:

```toml
[package]
name = "ingest"
version = "0.1.0"
edition = "2024"

[dependencies]
calamine = { version = "0.26", features = ["dates"] }
chrono = { workspace = true }
serde = { workspace = true }
thiserror = { workspace = true }
```

Add `"crates/ingest"` to workspace members.

- [ ] **Step 2: Write the failing fixture test**

`crates/ingest/tests/parse_sample.rs`:

```rust
use chrono::NaiveDate;

fn d(y: i32, m: u32, day: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, day).unwrap() }

#[test]
fn parses_the_real_sample_workbook() {
    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample.xlsx")).unwrap();
    let wb = ingest::parse_workbook(&bytes).expect("sample must parse cleanly");

    assert_eq!(wb.nav_date, d(2026, 7, 24));
    assert!((wb.nav - 104.42).abs() < 1e-9);
    assert!((wb.aum - 28_332_753.49).abs() < 1e-2);
    assert!((wb.shares - 271_342.492).abs() < 1e-3);

    assert_eq!(wb.positions.len(), 111);
    let p0 = &wb.positions[0];
    assert_eq!(p0.asset_type, "Action");
    assert_eq!(p0.isin, "GRS145003000");
    assert_eq!(p0.quantity, Some(7400.0));
    assert_eq!(p0.valuation_eur, Some(316_572.0));
    assert_eq!(p0.ticker.as_deref(), Some("GEKTERNA GA Equity"));
    // cash-section remap: margin row weight must land in `weight`, not accrued_interest
    let margin = wb.positions.iter().find(|p| p.isin == "MA1C7EUR").unwrap();
    assert_eq!(margin.asset_type, "Margin Acc");
    assert_eq!(margin.valuation_eur, Some(-19_688.0));
    assert!(margin.weight.unwrap() < 0.0);
    assert_eq!(margin.price, None);

    assert_eq!(wb.nav_history.len(), 343);
    assert_eq!(wb.nav_history[0].date, d(2025, 2, 28));
    assert!((wb.nav_history[0].nav - 100.0).abs() < 1e-9);
    assert_eq!(wb.nav_history.last().unwrap().date, d(2026, 7, 23));
    assert!((wb.nav_history.last().unwrap().nav - 103.99).abs() < 1e-9);

    assert_eq!(wb.dividends.len(), 53);
    assert_eq!(wb.operations.len(), 2050);
    assert_eq!(wb.operations[0].trade_date, d(2025, 3, 18));
    assert_eq!(wb.operations[0].side, "Achat");
}

#[test]
fn garbage_bytes_fail_cleanly() {
    assert!(matches!(ingest::parse_workbook(b"not an xlsx"), Err(ingest::ParseFailure::Workbook(_))));
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p ingest`
Expected: compile FAIL (`parse_workbook` not found).

- [ ] **Step 4: Implement `crates/ingest/src/lib.rs`**

Docs if the calamine API differs: https://docs.rs/calamine. `Range::get_value((row, col))` takes ABSOLUTE 0-based sheet coordinates.

```rust
use calamine::{Data, Range, Reader, Xlsx};
use chrono::{Days, NaiveDate};
use std::io::Cursor;

#[derive(Debug)]
pub struct ParsedWorkbook {
    pub nav_date: NaiveDate,
    pub aum: f64,
    pub shares: f64,
    pub nav: f64,
    pub positions: Vec<PositionRow>,
    pub nav_history: Vec<NavHistoryRow>,
    pub dividends: Vec<DividendRow>,
    pub operations: Vec<OperationRow>,
}

#[derive(Debug, Clone)]
pub struct PositionRow {
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

#[derive(Debug, Clone)]
pub struct NavHistoryRow {
    pub date: NaiveDate,
    pub aum: f64,
    pub shares: f64,
    pub nav: f64,
}

#[derive(Debug, Clone)]
pub struct DividendRow {
    pub provision_date: NaiveDate,
    pub payment_date: Option<NaiveDate>,
    pub issuer: String,
    pub amount: f64,
    pub currency: String,
}

#[derive(Debug, Clone)]
pub struct OperationRow {
    pub trade_date: NaiveDate,
    pub side: String,
    pub ticker: Option<String>,
    pub isin: Option<String>,
    pub name: Option<String>,
    pub currency: Option<String>,
    pub quantity: Option<f64>,
    pub price: Option<f64>,
    pub gross_amount: Option<f64>,
    pub fees: Option<f64>,
    pub net_price: Option<f64>,
    pub net_amount: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RowError {
    pub sheet: String,
    /// 1-based Excel row number.
    pub row: u32,
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ParseFailure {
    #[error("workbook error: {0}")]
    Workbook(String),
    #[error("{} row error(s)", .0.len())]
    Rows(Vec<RowError>),
}

struct Ctx {
    errors: Vec<RowError>,
}

impl Ctx {
    fn err(&mut self, sheet: &str, row0: u32, msg: impl Into<String>) {
        self.errors.push(RowError { sheet: sheet.into(), row: row0 + 1, message: msg.into() });
    }
}

pub fn parse_workbook(bytes: &[u8]) -> Result<ParsedWorkbook, ParseFailure> {
    let mut wb: Xlsx<_> = Xlsx::new(Cursor::new(bytes.to_vec()))
        .map_err(|e| ParseFailure::Workbook(e.to_string()))?;
    let pf = sheet(&mut wb, "PORTEFEUILLE_NAV")?;
    let hist = sheet(&mut wb, "HISTO_NAV")?;
    let div = sheet(&mut wb, "DIV")?;
    let ops = sheet(&mut wb, "OPERATIONS")?;

    let mut ctx = Ctx { errors: Vec::new() };

    let nav_date = req_date(&pf, 2, 1, "PORTEFEUILLE_NAV", &mut ctx); // B3
    let aum = req_f64(&pf, 1, 5, "PORTEFEUILLE_NAV", &mut ctx); // F2
    let shares = req_f64(&pf, 2, 5, "PORTEFEUILLE_NAV", &mut ctx); // F3
    let nav = req_f64(&pf, 3, 5, "PORTEFEUILLE_NAV", &mut ctx); // F4

    let positions = parse_positions(&pf, &mut ctx);
    let nav_history = parse_hist(&hist, &mut ctx);
    let dividends = parse_div(&div, &mut ctx);
    let operations = parse_ops(&ops, &mut ctx);

    if let (Some(nd), false) = (nav_date, nav_history.is_empty()) {
        if let Some(bad) = nav_history.iter().find(|r| r.date > nd) {
            ctx.err("HISTO_NAV", 0, format!("date {} is after the file's NAV date {}", bad.date, nd));
        }
    }

    if !ctx.errors.is_empty() {
        return Err(ParseFailure::Rows(ctx.errors));
    }
    Ok(ParsedWorkbook {
        nav_date: nav_date.unwrap(),
        aum: aum.unwrap(),
        shares: shares.unwrap(),
        nav: nav.unwrap(),
        positions,
        nav_history,
        dividends,
        operations,
    })
}

fn sheet(wb: &mut Xlsx<Cursor<Vec<u8>>>, name: &str) -> Result<Range<Data>, ParseFailure> {
    wb.worksheet_range(name)
        .map_err(|e| ParseFailure::Workbook(format!("sheet {name}: {e}")))
}

// ---- cell helpers (absolute 0-based coordinates) ----

fn get<'a>(r: &'a Range<Data>, row: u32, col: u32) -> Option<&'a Data> {
    r.get_value((row, col)).filter(|d| !matches!(d, Data::Empty))
}

fn cell_str(r: &Range<Data>, row: u32, col: u32) -> Option<String> {
    match get(r, row, col) {
        Some(Data::String(s)) => {
            let t = s.trim();
            if t.is_empty() { None } else { Some(t.to_string()) }
        }
        Some(other) => Some(other.to_string()),
        None => None,
    }
}

fn cell_f64(r: &Range<Data>, row: u32, col: u32, sheet: &str, ctx: &mut Ctx) -> Option<f64> {
    match get(r, row, col) {
        Some(Data::Float(f)) => Some(*f),
        Some(Data::Int(i)) => Some(*i as f64),
        Some(Data::String(s)) if s.trim().is_empty() => None,
        Some(other) => {
            ctx.err(sheet, row, format!("col {}: expected number, got {other:?}", col + 1));
            None
        }
        None => None,
    }
}

fn cell_date(r: &Range<Data>, row: u32, col: u32, sheet: &str, ctx: &mut Ctx) -> Option<NaiveDate> {
    match get(r, row, col) {
        Some(Data::DateTime(dt)) => match dt.as_datetime() {
            Some(ndt) => Some(ndt.date()),
            None => {
                ctx.err(sheet, row, format!("col {}: invalid Excel datetime", col + 1));
                None
            }
        },
        // raw serial number with date formatting stripped
        Some(Data::Float(f)) => NaiveDate::from_ymd_opt(1899, 12, 30)
            .unwrap()
            .checked_add_days(Days::new(*f as u64)),
        Some(other) => {
            ctx.err(sheet, row, format!("col {}: expected date, got {other:?}", col + 1));
            None
        }
        None => None,
    }
}

fn req_f64(r: &Range<Data>, row: u32, col: u32, sheet: &str, ctx: &mut Ctx) -> Option<f64> {
    let v = cell_f64(r, row, col, sheet, ctx);
    if v.is_none() && ctx.errors.last().map(|e| e.row) != Some(row + 1) {
        ctx.err(sheet, row, format!("col {}: required number missing", col + 1));
    }
    v
}

fn req_date(r: &Range<Data>, row: u32, col: u32, sheet: &str, ctx: &mut Ctx) -> Option<NaiveDate> {
    let v = cell_date(r, row, col, sheet, ctx);
    if v.is_none() && ctx.errors.last().map(|e| e.row) != Some(row + 1) {
        ctx.err(sheet, row, format!("col {}: required date missing", col + 1));
    }
    v
}

// ---- sheet parsers ----

fn parse_positions(r: &Range<Data>, ctx: &mut Ctx) -> Vec<PositionRow> {
    const SHEET: &str = "PORTEFEUILLE_NAV";
    let mut out = Vec::new();
    let mut cash_section = false;
    let end = r.end().map(|(er, _)| er).unwrap_or(0);
    for row in 7..=end {
        let Some(asset_type) = cell_str(r, row, 0) else { continue };
        if asset_type == "Type" {
            cash_section = true; // sub-header switches the column layout
            continue;
        }
        let Some(isin) = cell_str(r, row, 1) else { continue }; // separator rows (e.g. "CASH")
        let p = if !cash_section {
            PositionRow {
                asset_type,
                isin,
                name: cell_str(r, row, 2),
                currency: cell_str(r, row, 3),
                quantity: cell_f64(r, row, 4, SHEET, ctx),
                avg_cost: cell_f64(r, row, 5, SHEET, ctx),
                price: cell_f64(r, row, 6, SHEET, ctx),
                valuation_ccy: cell_f64(r, row, 7, SHEET, ctx),
                accrued_interest: cell_f64(r, row, 8, SHEET, ctx),
                fx_rate: cell_f64(r, row, 9, SHEET, ctx),
                valuation_eur: cell_f64(r, row, 10, SHEET, ctx),
                weight: cell_f64(r, row, 11, SHEET, ctx),
                ticker: cell_str(r, row, 12),
            }
        } else {
            PositionRow {
                asset_type,
                isin,
                name: cell_str(r, row, 2),
                currency: cell_str(r, row, 3),
                quantity: cell_f64(r, row, 4, SHEET, ctx),
                avg_cost: None,
                price: None,
                valuation_ccy: cell_f64(r, row, 5, SHEET, ctx),
                valuation_eur: cell_f64(r, row, 6, SHEET, ctx),
                accrued_interest: cell_f64(r, row, 7, SHEET, ctx),
                weight: cell_f64(r, row, 8, SHEET, ctx),
                fx_rate: None,
                ticker: None,
            }
        };
        out.push(p);
    }
    out
}

fn parse_hist(r: &Range<Data>, ctx: &mut Ctx) -> Vec<NavHistoryRow> {
    const SHEET: &str = "HISTO_NAV";
    let mut out = Vec::new();
    let end = r.end().map(|(er, _)| er).unwrap_or(0);
    for row in 1..=end {
        if get(r, row, 0).is_none() { continue; }
        let date = req_date(r, row, 0, SHEET, ctx);
        let aum = req_f64(r, row, 1, SHEET, ctx);
        let shares = req_f64(r, row, 2, SHEET, ctx);
        let nav = req_f64(r, row, 3, SHEET, ctx);
        if let (Some(date), Some(aum), Some(shares), Some(nav)) = (date, aum, shares, nav) {
            out.push(NavHistoryRow { date, aum, shares, nav });
        }
    }
    out.sort_by_key(|x| x.date);
    out
}

fn parse_div(r: &Range<Data>, ctx: &mut Ctx) -> Vec<DividendRow> {
    const SHEET: &str = "DIV";
    let mut out = Vec::new();
    let end = r.end().map(|(er, _)| er).unwrap_or(0);
    for row in 1..=end {
        if get(r, row, 0).is_none() { continue; }
        let provision_date = req_date(r, row, 0, SHEET, ctx);
        let payment_date = cell_date(r, row, 1, SHEET, ctx);
        let issuer = cell_str(r, row, 2);
        let amount = req_f64(r, row, 3, SHEET, ctx);
        let currency = cell_str(r, row, 4);
        if issuer.is_none() { ctx.err(SHEET, row, "issuer missing"); }
        if currency.is_none() { ctx.err(SHEET, row, "currency missing"); }
        if let (Some(pd), Some(issuer), Some(amount), Some(currency)) = (provision_date, issuer, amount, currency) {
            out.push(DividendRow { provision_date: pd, payment_date, issuer, amount, currency });
        }
    }
    out
}

fn parse_ops(r: &Range<Data>, ctx: &mut Ctx) -> Vec<OperationRow> {
    const SHEET: &str = "OPERATIONS";
    let mut out = Vec::new();
    let end = r.end().map(|(er, _)| er).unwrap_or(0);
    for row in 3..=end {
        if get(r, row, 0).is_none() { continue; }
        let trade_date = req_date(r, row, 0, SHEET, ctx);
        let side = cell_str(r, row, 1);
        if side.is_none() { ctx.err(SHEET, row, "side missing"); }
        let rec = OperationRow {
            trade_date: match trade_date { Some(d) => d, None => continue },
            side: match side { Some(s) => s, None => continue },
            ticker: cell_str(r, row, 2),
            isin: cell_str(r, row, 3),
            name: cell_str(r, row, 4),
            currency: cell_str(r, row, 5),
            quantity: cell_f64(r, row, 6, SHEET, ctx),
            price: cell_f64(r, row, 7, SHEET, ctx),
            gross_amount: cell_f64(r, row, 8, SHEET, ctx),
            fees: cell_f64(r, row, 9, SHEET, ctx),
            net_price: cell_f64(r, row, 10, SHEET, ctx),
            net_amount: cell_f64(r, row, 11, SHEET, ctx),
        };
        out.push(rec);
    }
    out
}
```

Note on `Range::end()`: in calamine 0.26 it returns `Option<(u32, u32)>` with the absolute last cell. If the API differs on the installed version, check https://docs.rs/calamine and adapt (the loop just needs the last used row index).

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ingest`
Expected: PASS (2 tests). If `positions.len()` differs from 111, debug with the skip rules above (exactly one `CASH` separator row and one `Type` sub-header row must be skipped) — do NOT change the assertion to match a wrong count.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat(ingest): parse 4-sheet NAV recap workbook with section-aware positions"
```

---

### Task 7: db crate — embedded PostgreSQL, migrations, settings repo

**Files:**
- Create: `crates/db/Cargo.toml`, `crates/db/src/lib.rs`, `crates/db/src/embedded.rs`, `crates/db/src/settings.rs`, `crates/db/migrations/0001_init.sql`, `crates/db/tests/settings_roundtrip.rs`
- Modify: root `Cargo.toml` members (add `"crates/db"`)

**Interfaces:**
- Produces: `db::embedded::EmbeddedDb { pub url: String, .. }` with `pub async fn start(data_root: &Path, temporary: bool) -> anyhow::Result<EmbeddedDb>` and `pub async fn stop(self)`; `db::connect(url: &str) -> anyhow::Result<sqlx::PgPool>` (runs migrations); `db::settings::AppSettings { risk_free_rate: f64, var_confidence: f64, var_horizon_days: u32, var_window_days: u32, var_limit: f64, short_dd_max_days: u32 }` (Serialize/Deserialize) with `get_settings(&PgPool) -> anyhow::Result<AppSettings>` and `put_settings(&PgPool, &AppSettings) -> anyhow::Result<()>`.
- Docs: https://docs.rs/postgresql_embedded (API drifts between minor versions — adjust field names if needed, keep the behavior).

- [ ] **Step 1: Scaffold crate**

`crates/db/Cargo.toml`:

```toml
[package]
name = "db"
version = "0.1.0"
edition = "2024"

[dependencies]
anyhow = "1"
chrono = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio", "tls-rustls", "postgres", "chrono", "json", "migrate", "macros"] }
postgresql_embedded = "0.18"
ingest = { path = "../ingest" }

[dev-dependencies]
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
tempfile = "3"
```

Add `"crates/db"` to workspace members, and `anyhow = "1"`, `tokio = { version = "1", features = ["full"] }` to `[workspace.dependencies]`.

`crates/db/migrations/0001_init.sql`:

```sql
CREATE TABLE imports (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  filename TEXT NOT NULL,
  sha256 TEXT NOT NULL UNIQUE,
  nav_date DATE NOT NULL,
  imported_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  row_counts JSONB NOT NULL
);

CREATE TABLE nav_history (
  date DATE PRIMARY KEY,
  aum NUMERIC NOT NULL,
  shares NUMERIC NOT NULL,
  nav NUMERIC NOT NULL
);

CREATE TABLE position_snapshots (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  nav_date DATE NOT NULL,
  import_id BIGINT NOT NULL REFERENCES imports(id) ON DELETE CASCADE,
  asset_type TEXT NOT NULL,
  isin TEXT NOT NULL,
  name TEXT,
  currency TEXT,
  quantity NUMERIC,
  avg_cost NUMERIC,
  price NUMERIC,
  valuation_ccy NUMERIC,
  accrued_interest NUMERIC,
  fx_rate NUMERIC,
  valuation_eur NUMERIC,
  weight NUMERIC,
  ticker TEXT
);
CREATE INDEX idx_positions_nav_date ON position_snapshots(nav_date);

CREATE TABLE dividends (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  provision_date DATE NOT NULL,
  payment_date DATE,
  issuer TEXT NOT NULL,
  amount NUMERIC NOT NULL,
  currency TEXT NOT NULL
);

CREATE TABLE operations (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  trade_date DATE NOT NULL,
  side TEXT NOT NULL,
  ticker TEXT,
  isin TEXT,
  name TEXT,
  currency TEXT,
  quantity NUMERIC,
  price NUMERIC,
  gross_amount NUMERIC,
  fees NUMERIC,
  net_price NUMERIC,
  net_amount NUMERIC
);

CREATE TABLE settings (
  key TEXT PRIMARY KEY,
  value JSONB NOT NULL
);

INSERT INTO settings (key, value) VALUES
  ('risk_free_rate', '0.02'),
  ('var_confidence', '0.99'),
  ('var_horizon_days', '20'),
  ('var_window_days', '252'),
  ('var_limit', '0.20'),
  ('short_dd_max_days', '50');
```

- [ ] **Step 2: Write failing test**

`crates/db/tests/settings_roundtrip.rs`:

```rust
#[tokio::test]
async fn settings_defaults_and_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    let s = db::settings::get_settings(&pool).await.unwrap();
    assert!((s.risk_free_rate - 0.02).abs() < 1e-12);
    assert!((s.var_confidence - 0.99).abs() < 1e-12);
    assert_eq!(s.var_horizon_days, 20);
    assert_eq!(s.var_window_days, 252);
    assert!((s.var_limit - 0.20).abs() < 1e-12);
    assert_eq!(s.short_dd_max_days, 50);

    let mut s2 = s.clone();
    s2.risk_free_rate = 0.031;
    s2.var_horizon_days = 10;
    db::settings::put_settings(&pool, &s2).await.unwrap();
    let s3 = db::settings::get_settings(&pool).await.unwrap();
    assert!((s3.risk_free_rate - 0.031).abs() < 1e-12);
    assert_eq!(s3.var_horizon_days, 10);

    pool.close().await;
    edb.stop().await;
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p db`
Expected: compile FAIL.

- [ ] **Step 4: Implement**

`crates/db/src/lib.rs`:

```rust
pub mod embedded;
pub mod settings;

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

pub async fn connect(url: &str) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new().max_connections(5).connect(url).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}
```

`crates/db/src/embedded.rs`:

```rust
use postgresql_embedded::{PostgreSQL, Settings, VersionReq};
use std::path::Path;

pub const DB_NAME: &str = "borobudur";

pub struct EmbeddedDb {
    pg: PostgreSQL,
    pub url: String,
}

/// Start (installing on first run) an embedded PostgreSQL 17.
/// `temporary = true` uses throwaway dirs + random port (tests);
/// `false` persists under `data_root` for the real app.
pub async fn start(data_root: &Path, temporary: bool) -> anyhow::Result<EmbeddedDb> {
    let mut settings = Settings::default();
    settings.version = VersionReq::parse("=17")?;
    settings.temporary = temporary;
    settings.username = "postgres".to_string();
    settings.password = "borobudur-local".to_string();
    if !temporary {
        settings.installation_dir = data_root.join("pg-install");
        settings.data_dir = data_root.join("pg-data");
        settings.password_file = data_root.join(".pgpass");
    }
    let mut pg = PostgreSQL::new(settings);
    pg.setup().await?;
    pg.start().await?;
    if !pg.database_exists(DB_NAME).await? {
        pg.create_database(DB_NAME).await?;
    }
    let url = pg.settings().url(DB_NAME);
    Ok(EmbeddedDb { pg, url })
}

impl EmbeddedDb {
    pub async fn stop(mut self) {
        let _ = self.pg.stop().await;
    }
}
```

`crates/db/src/settings.rs`:

```rust
use sqlx::PgPool;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppSettings {
    pub risk_free_rate: f64,
    pub var_confidence: f64,
    pub var_horizon_days: u32,
    pub var_window_days: u32,
    pub var_limit: f64,
    pub short_dd_max_days: u32,
}

pub async fn get_settings(pool: &PgPool) -> anyhow::Result<AppSettings> {
    let rows: Vec<(String, serde_json::Value)> =
        sqlx::query_as("SELECT key, value FROM settings").fetch_all(pool).await?;
    let get_f = |k: &str, d: f64| rows.iter().find(|(key, _)| key == k).and_then(|(_, v)| v.as_f64()).unwrap_or(d);
    let get_u = |k: &str, d: u32| rows.iter().find(|(key, _)| key == k).and_then(|(_, v)| v.as_u64()).map(|v| v as u32).unwrap_or(d);
    Ok(AppSettings {
        risk_free_rate: get_f("risk_free_rate", 0.02),
        var_confidence: get_f("var_confidence", 0.99),
        var_horizon_days: get_u("var_horizon_days", 20),
        var_window_days: get_u("var_window_days", 252),
        var_limit: get_f("var_limit", 0.20),
        short_dd_max_days: get_u("short_dd_max_days", 50),
    })
}

pub async fn put_settings(pool: &PgPool, s: &AppSettings) -> anyhow::Result<()> {
    let pairs: Vec<(&str, serde_json::Value)> = vec![
        ("risk_free_rate", s.risk_free_rate.into()),
        ("var_confidence", s.var_confidence.into()),
        ("var_horizon_days", s.var_horizon_days.into()),
        ("var_window_days", s.var_window_days.into()),
        ("var_limit", s.var_limit.into()),
        ("short_dd_max_days", s.short_dd_max_days.into()),
    ];
    for (k, v) in pairs {
        sqlx::query("INSERT INTO settings (key, value) VALUES ($1, $2) ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value")
            .bind(k)
            .bind(v)
            .execute(pool)
            .await?;
    }
    Ok(())
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p db`
Expected: PASS. First run downloads PostgreSQL binaries (network; ~2 min). If `Settings` field names differ (e.g. `password_file`), consult https://docs.rs/postgresql_embedded and adapt.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat(db): embedded postgres lifecycle, schema migration, settings repo"
```

---

### Task 8: db crate — import transaction + read repositories

**Files:**
- Create: `crates/db/src/repo.rs`, `crates/db/tests/import_workbook.rs`
- Modify: `crates/db/src/lib.rs` (add `pub mod repo;`)

**Interfaces:**
- Consumes: `ingest::ParsedWorkbook` and row types (Task 6), `AppSettings` (Task 7).
- Produces (all `anyhow::Result`, in `db::repo`):

```rust
pub struct ImportOutcome { pub import_id: i64, pub duplicate: bool, pub nav_rows: usize, pub positions: usize, pub dividends: usize, pub operations: usize, pub div_ops_replaced: bool }
pub async fn import_workbook(pool: &PgPool, filename: &str, sha256: &str, wb: &ParsedWorkbook) -> Result<ImportOutcome>
#[derive(sqlx::FromRow, serde::Serialize, Clone)] pub struct NavRow { pub date: NaiveDate, pub aum: f64, pub shares: f64, pub nav: f64 }
pub async fn nav_rows(pool: &PgPool) -> Result<Vec<NavRow>>                       // ordered by date
#[derive(sqlx::FromRow, serde::Serialize)] pub struct PositionRecord { pub nav_date: NaiveDate, pub asset_type: String, pub isin: String, pub name: Option<String>, pub currency: Option<String>, pub quantity: Option<f64>, pub avg_cost: Option<f64>, pub price: Option<f64>, pub valuation_ccy: Option<f64>, pub accrued_interest: Option<f64>, pub fx_rate: Option<f64>, pub valuation_eur: Option<f64>, pub weight: Option<f64>, pub ticker: Option<String> }
pub async fn position_dates(pool: &PgPool) -> Result<Vec<NaiveDate>>              // desc
pub async fn positions_for(pool: &PgPool, date: NaiveDate) -> Result<Vec<PositionRecord>>
#[derive(sqlx::FromRow, serde::Serialize)] pub struct ImportRecord { pub id: i64, pub filename: String, pub nav_date: NaiveDate, pub imported_at: chrono::DateTime<chrono::Utc>, pub row_counts: serde_json::Value }
pub async fn imports_list(pool: &PgPool) -> Result<Vec<ImportRecord>>             // desc by imported_at
```

**Import semantics (from spec):** duplicate sha → return existing id with `duplicate: true`, write nothing. Otherwise in ONE transaction: insert `imports` row; upsert every `nav_history` row by date **plus the file's own `(nav_date, aum, shares, nav)` row** (the recap's current NAV is not yet in HISTO_NAV); delete + insert `position_snapshots` for `wb.nav_date`; if `wb.nav_date >=` the latest previously imported nav_date (or none exists) delete-all + insert `dividends` and `operations` (`div_ops_replaced: true`), else leave them (`false`).

- [ ] **Step 1: Write failing test**

`crates/db/tests/import_workbook.rs`:

```rust
use chrono::NaiveDate;

fn d(y: i32, m: u32, day: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, day).unwrap() }

fn sample() -> ingest::ParsedWorkbook {
    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx")).unwrap();
    ingest::parse_workbook(&bytes).unwrap()
}

#[tokio::test]
async fn import_upsert_and_duplicate_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let wb = sample();

    let o1 = db::repo::import_workbook(&pool, "sample.xlsx", "sha-1", &wb).await.unwrap();
    assert!(!o1.duplicate);
    assert_eq!(o1.positions, 111);
    assert_eq!(o1.dividends, 53);
    assert_eq!(o1.operations, 2050);
    assert!(o1.div_ops_replaced);
    // 343 HISTO rows + the file's own nav_date row
    assert_eq!(o1.nav_rows, 344);

    let nav = db::repo::nav_rows(&pool).await.unwrap();
    assert_eq!(nav.len(), 344);
    assert_eq!(nav.last().unwrap().date, d(2026, 7, 24));
    assert!((nav.last().unwrap().nav - 104.42).abs() < 1e-9);
    assert_eq!(nav[0].date, d(2025, 2, 28));

    // same sha -> duplicate no-op
    let o2 = db::repo::import_workbook(&pool, "sample.xlsx", "sha-1", &wb).await.unwrap();
    assert!(o2.duplicate);
    assert_eq!(o2.import_id, o1.import_id);
    assert_eq!(db::repo::nav_rows(&pool).await.unwrap().len(), 344);

    // same file, new sha -> re-import replaces the snapshot (still 111 rows, one date)
    let o3 = db::repo::import_workbook(&pool, "sample2.xlsx", "sha-2", &wb).await.unwrap();
    assert!(!o3.duplicate);
    assert!(o3.div_ops_replaced); // equal nav_date counts as >=
    let dates = db::repo::position_dates(&pool).await.unwrap();
    assert_eq!(dates, vec![d(2026, 7, 24)]);
    let pos = db::repo::positions_for(&pool, d(2026, 7, 24)).await.unwrap();
    assert_eq!(pos.len(), 111);
    assert!(pos.iter().any(|p| p.isin == "GRS145003000"));

    let imports = db::repo::imports_list(&pool).await.unwrap();
    assert_eq!(imports.len(), 2);

    pool.close().await;
    edb.stop().await;
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p db import_workbook`
Expected: compile FAIL.

- [ ] **Step 3: Implement `crates/db/src/repo.rs`**

```rust
use chrono::NaiveDate;
use ingest::ParsedWorkbook;
use sqlx::PgPool;

#[derive(Debug, serde::Serialize)]
pub struct ImportOutcome {
    pub import_id: i64,
    pub duplicate: bool,
    pub nav_rows: usize,
    pub positions: usize,
    pub dividends: usize,
    pub operations: usize,
    pub div_ops_replaced: bool,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct NavRow {
    pub date: NaiveDate,
    pub aum: f64,
    pub shares: f64,
    pub nav: f64,
}

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

#[derive(Debug, sqlx::FromRow, serde::Serialize)]
pub struct ImportRecord {
    pub id: i64,
    pub filename: String,
    pub nav_date: NaiveDate,
    pub imported_at: chrono::DateTime<chrono::Utc>,
    pub row_counts: serde_json::Value,
}

pub async fn import_workbook(pool: &PgPool, filename: &str, sha256: &str, wb: &ParsedWorkbook) -> anyhow::Result<ImportOutcome> {
    if let Some((id,)) = sqlx::query_as::<_, (i64,)>("SELECT id FROM imports WHERE sha256 = $1")
        .bind(sha256)
        .fetch_optional(pool)
        .await?
    {
        return Ok(ImportOutcome {
            import_id: id, duplicate: true, nav_rows: 0, positions: 0,
            dividends: 0, operations: 0, div_ops_replaced: false,
        });
    }

    let mut tx = pool.begin().await?;

    let prev_latest: Option<NaiveDate> =
        sqlx::query_scalar("SELECT max(nav_date) FROM imports").fetch_one(&mut *tx).await?;
    let replace_div_ops = prev_latest.is_none_or(|d| wb.nav_date >= d);

    let nav_rows = wb.nav_history.len() + 1;
    let row_counts = serde_json::json!({
        "nav_rows": nav_rows, "positions": wb.positions.len(),
        "dividends": wb.dividends.len(), "operations": wb.operations.len(),
    });
    let (import_id,): (i64,) = sqlx::query_as(
        "INSERT INTO imports (filename, sha256, nav_date, row_counts) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(filename).bind(sha256).bind(wb.nav_date).bind(&row_counts)
    .fetch_one(&mut *tx)
    .await?;

    const UPSERT_NAV: &str = "INSERT INTO nav_history (date, aum, shares, nav) VALUES ($1, $2, $3, $4)
        ON CONFLICT (date) DO UPDATE SET aum = EXCLUDED.aum, shares = EXCLUDED.shares, nav = EXCLUDED.nav";
    for r in &wb.nav_history {
        sqlx::query(UPSERT_NAV).bind(r.date).bind(r.aum).bind(r.shares).bind(r.nav)
            .execute(&mut *tx).await?;
    }
    // the recap's own NAV row (not yet in HISTO_NAV)
    sqlx::query(UPSERT_NAV).bind(wb.nav_date).bind(wb.aum).bind(wb.shares).bind(wb.nav)
        .execute(&mut *tx).await?;

    sqlx::query("DELETE FROM position_snapshots WHERE nav_date = $1")
        .bind(wb.nav_date).execute(&mut *tx).await?;
    for p in &wb.positions {
        sqlx::query(
            "INSERT INTO position_snapshots (nav_date, import_id, asset_type, isin, name, currency, quantity, avg_cost, price, valuation_ccy, accrued_interest, fx_rate, valuation_eur, weight, ticker)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
        )
        .bind(wb.nav_date).bind(import_id).bind(&p.asset_type).bind(&p.isin).bind(&p.name)
        .bind(&p.currency).bind(p.quantity).bind(p.avg_cost).bind(p.price).bind(p.valuation_ccy)
        .bind(p.accrued_interest).bind(p.fx_rate).bind(p.valuation_eur).bind(p.weight).bind(&p.ticker)
        .execute(&mut *tx)
        .await?;
    }

    if replace_div_ops {
        sqlx::query("DELETE FROM dividends").execute(&mut *tx).await?;
        for r in &wb.dividends {
            sqlx::query("INSERT INTO dividends (provision_date, payment_date, issuer, amount, currency) VALUES ($1, $2, $3, $4, $5)")
                .bind(r.provision_date).bind(r.payment_date).bind(&r.issuer).bind(r.amount).bind(&r.currency)
                .execute(&mut *tx).await?;
        }
        sqlx::query("DELETE FROM operations").execute(&mut *tx).await?;
        for r in &wb.operations {
            sqlx::query(
                "INSERT INTO operations (trade_date, side, ticker, isin, name, currency, quantity, price, gross_amount, fees, net_price, net_amount)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
            )
            .bind(r.trade_date).bind(&r.side).bind(&r.ticker).bind(&r.isin).bind(&r.name)
            .bind(&r.currency).bind(r.quantity).bind(r.price).bind(r.gross_amount).bind(r.fees)
            .bind(r.net_price).bind(r.net_amount)
            .execute(&mut *tx)
            .await?;
        }
    }

    tx.commit().await?;
    Ok(ImportOutcome {
        import_id,
        duplicate: false,
        nav_rows,
        positions: wb.positions.len(),
        dividends: if replace_div_ops { wb.dividends.len() } else { 0 },
        operations: if replace_div_ops { wb.operations.len() } else { 0 },
        div_ops_replaced: replace_div_ops,
    })
}

pub async fn nav_rows(pool: &PgPool) -> anyhow::Result<Vec<NavRow>> {
    Ok(sqlx::query_as(
        "SELECT date, aum::float8 AS aum, shares::float8 AS shares, nav::float8 AS nav FROM nav_history ORDER BY date",
    )
    .fetch_all(pool)
    .await?)
}

pub async fn position_dates(pool: &PgPool) -> anyhow::Result<Vec<NaiveDate>> {
    Ok(sqlx::query_scalar("SELECT DISTINCT nav_date FROM position_snapshots ORDER BY nav_date DESC")
        .fetch_all(pool)
        .await?)
}

pub async fn positions_for(pool: &PgPool, date: NaiveDate) -> anyhow::Result<Vec<PositionRecord>> {
    Ok(sqlx::query_as(
        "SELECT nav_date, asset_type, isin, name, currency,
                quantity::float8 AS quantity, avg_cost::float8 AS avg_cost, price::float8 AS price,
                valuation_ccy::float8 AS valuation_ccy, accrued_interest::float8 AS accrued_interest,
                fx_rate::float8 AS fx_rate, valuation_eur::float8 AS valuation_eur,
                weight::float8 AS weight, ticker
         FROM position_snapshots WHERE nav_date = $1 ORDER BY id",
    )
    .bind(date)
    .fetch_all(pool)
    .await?)
}

pub async fn imports_list(pool: &PgPool) -> anyhow::Result<Vec<ImportRecord>> {
    Ok(sqlx::query_as("SELECT id, filename, nav_date, imported_at, row_counts FROM imports ORDER BY imported_at DESC")
        .fetch_all(pool)
        .await?)
}
```

Note: `f64` binds against `NUMERIC` columns work on insert (PostgreSQL assignment-casts); reads MUST keep the `::float8` casts shown.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p db`
Expected: PASS (both db tests).

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(db): import transaction with snapshot/upsert/replace semantics + read repos"
```

---

### Task 9: server crate — axum skeleton, errors, settings endpoints

**Files:**
- Create: `crates/server/Cargo.toml`, `crates/server/src/main.rs`, `crates/server/src/lib.rs`, `crates/server/src/state.rs`, `crates/server/src/error.rs`, `crates/server/src/routes.rs`, `crates/server/src/handlers/mod.rs`, `crates/server/src/handlers/settings.rs`, `crates/server/tests/api_settings.rs`
- Modify: root `Cargo.toml` members (add `"crates/server"`)

**Interfaces:**
- Consumes: `db::{connect, embedded, settings::{AppSettings, get_settings, put_settings}}`.
- Produces: `server::routes::router(state: AppState) -> axum::Router`; `server::state::AppState { pub pool: PgPool }` (Clone); `server::error::AppError` with variants `Internal(anyhow::Error)` → 500, `BadRequest(String)` → 400, `UnprocessableRows(Vec<ingest::RowError>)` → 422; all as problem-details JSON `{"title": ..., "status": ..., "detail"?: ..., "rows"?: [...]}`. Endpoints this task: `GET /api/health` → `{"status":"ok"}`; `GET /api/settings` → `AppSettings` JSON; `PUT /api/settings` (validates: 0.5 < var_confidence < 1, var_horizon_days ≥ 1, var_window_days ≥ 30, 0 < var_limit ≤ 1, short_dd_max_days ≥ 1, −0.05 ≤ risk_free_rate ≤ 0.2 → else 400).

- [ ] **Step 1: Scaffold crate**

`crates/server/Cargo.toml`:

```toml
[package]
name = "server"
version = "0.1.0"
edition = "2024"

[dependencies]
analytics = { path = "../analytics" }
ingest = { path = "../ingest" }
db = { path = "../db" }
anyhow = { workspace = true }
axum = { version = "0.8", features = ["multipart"] }
chrono = { workspace = true }
dirs = "6"
serde = { workspace = true }
serde_json = { workspace = true }
sha2 = "0.10"
hex = "0.4"
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio", "postgres", "chrono"] }
tokio = { workspace = true }
tower-http = { version = "0.6", features = ["trace"] }
tracing = "0.1"
tracing-subscriber = "0.3"
webbrowser = "1"

[dev-dependencies]
tempfile = "3"
tower = { version = "0.5", features = ["util"] }
http-body-util = "0.1"
```

`crates/server/src/lib.rs`:

```rust
pub mod error;
pub mod handlers;
pub mod routes;
pub mod state;
```

`crates/server/src/state.rs`:

```rust
#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::PgPool,
}
```

`crates/server/src/error.rs`:

```rust
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

pub enum AppError {
    Internal(anyhow::Error),
    BadRequest(String),
    UnprocessableRows(Vec<ingest::RowError>),
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        AppError::Internal(e.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::Internal(e) => {
                tracing::error!("internal error: {e:#}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"title": "Internal Server Error", "status": 500, "detail": e.to_string()})),
                )
                    .into_response()
            }
            AppError::BadRequest(msg) => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"title": "Bad Request", "status": 400, "detail": msg})),
            )
                .into_response(),
            AppError::UnprocessableRows(rows) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({"title": "File rejected", "status": 422, "rows": rows})),
            )
                .into_response(),
        }
    }
}
```

`crates/server/src/routes.rs`:

```rust
use crate::handlers;
use crate::state::AppState;
use axum::routing::{get, put};
use axum::Router;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(|| async { axum::Json(serde_json::json!({"status": "ok"})) }))
        .route("/api/settings", get(handlers::settings::get).put(handlers::settings::put))
        .with_state(state)
}
```

(`put` import stays unused until later tasks add more routes; silence with the combined `get(...).put(...)` form as shown.)

`crates/server/src/handlers/mod.rs`:

```rust
pub mod settings;
```

`crates/server/src/handlers/settings.rs`:

```rust
use crate::error::AppError;
use crate::state::AppState;
use axum::extract::State;
use axum::Json;
use db::settings::AppSettings;

pub async fn get(State(st): State<AppState>) -> Result<Json<AppSettings>, AppError> {
    Ok(Json(db::settings::get_settings(&st.pool).await?))
}

pub async fn put(State(st): State<AppState>, Json(s): Json<AppSettings>) -> Result<Json<AppSettings>, AppError> {
    validate(&s).map_err(AppError::BadRequest)?;
    db::settings::put_settings(&st.pool, &s).await?;
    Ok(Json(db::settings::get_settings(&st.pool).await?))
}

fn validate(s: &AppSettings) -> Result<(), String> {
    if !(0.5..1.0).contains(&s.var_confidence) { return Err("var_confidence must be in (0.5, 1)".into()); }
    if s.var_horizon_days < 1 { return Err("var_horizon_days must be >= 1".into()); }
    if s.var_window_days < 30 { return Err("var_window_days must be >= 30".into()); }
    if !(0.0..=1.0).contains(&s.var_limit) || s.var_limit == 0.0 { return Err("var_limit must be in (0, 1]".into()); }
    if s.short_dd_max_days < 1 { return Err("short_dd_max_days must be >= 1".into()); }
    if !(-0.05..=0.2).contains(&s.risk_free_rate) { return Err("risk_free_rate must be in [-5%, 20%]".into()); }
    Ok(())
}
```

`crates/server/src/main.rs`:

```rust
use server::routes::router;
use server::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info,sqlx=warn").init();
    let root = dirs::data_local_dir()
        .ok_or_else(|| anyhow::anyhow!("no local data dir"))?
        .join("borobudur-risk");
    std::fs::create_dir_all(&root)?;
    tracing::info!("starting embedded PostgreSQL under {}", root.display());
    let edb = db::embedded::start(&root, false).await?;
    let pool = db::connect(&edb.url).await?;
    let app = router(AppState { pool });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8787").await?;
    tracing::info!("listening on http://127.0.0.1:8787");
    let _ = webbrowser::open("http://127.0.0.1:8787");
    axum::serve(listener, app).await?;
    edb.stop().await; // keep edb alive until server exits
    Ok(())
}
```

- [ ] **Step 2: Write failing test**

`crates/server/tests/api_settings.rs`:

```rust
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

async fn test_app() -> (axum::Router, db::embedded::EmbeddedDb, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    (server::routes::router(server::state::AppState { pool }), edb, dir)
}

#[tokio::test]
async fn settings_get_put_and_validation() {
    let (app, _edb, _dir) = test_app().await;

    let res = app.clone().oneshot(Request::get("/api/settings").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["var_horizon_days"], 20);

    let mut s = body.clone();
    s["risk_free_rate"] = serde_json::json!(0.025);
    let res = app.clone().oneshot(
        Request::put("/api/settings").header("content-type", "application/json")
            .body(Body::from(s.to_string())).unwrap(),
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let mut bad = body.clone();
    bad["var_confidence"] = serde_json::json!(1.5);
    let res = app.clone().oneshot(
        Request::put("/api/settings").header("content-type", "application/json")
            .body(Body::from(bad.to_string())).unwrap(),
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let res = app.oneshot(Request::get("/api/health").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}
```

- [ ] **Step 3: Run test to verify it fails, then compile/implement until green**

Run: `cargo test -p server`
Expected first: compile FAIL; after wiring the files above: PASS.

- [ ] **Step 4: Sanity-run the binary**

Run: `cargo run -p server` (Ctrl-C after it logs `listening on http://127.0.0.1:8787`; browser will open a 404 page — fine, no static assets yet).
Expected: embedded PG starts, no panics.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(server): axum skeleton, problem-details errors, settings API"
```

---

### Task 10: server — import upload + data read endpoints

**Files:**
- Create: `crates/server/src/handlers/imports.rs`, `crates/server/src/handlers/data.rs`, `crates/server/tests/api_imports.rs`
- Modify: `crates/server/src/handlers/mod.rs` (add `pub mod imports; pub mod data;`), `crates/server/src/routes.rs`

**Interfaces:**
- Consumes: `ingest::parse_workbook`, `ingest::ParseFailure`, `db::repo::{import_workbook, imports_list, nav_rows, position_dates, positions_for, ImportOutcome}`.
- Produces endpoints: `POST /api/imports` (multipart field `file`) → `ImportOutcome` JSON (422 on row errors, 400 on non-xlsx / missing field); `GET /api/imports` → `Vec<ImportRecord>`; `GET /api/nav` → `Vec<NavRow>`; `GET /api/positions?date=YYYY-MM-DD` → `{"dates": [...], "date": ..., "rows": [...]}` (date defaults to latest; 400 on bad date).

- [ ] **Step 1: Write failing test**

`crates/server/tests/api_imports.rs`:

```rust
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

const BOUNDARY: &str = "XBOUNDARYX";

fn multipart_body(bytes: &[u8], filename: &str) -> Body {
    let mut body = Vec::new();
    body.extend_from_slice(format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    ).as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    Body::from(body)
}

fn upload_req(bytes: &[u8], filename: &str) -> Request<Body> {
    Request::post("/api/imports")
        .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(multipart_body(bytes, filename))
        .unwrap()
}

async fn test_app() -> (axum::Router, db::embedded::EmbeddedDb, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    (server::routes::router(server::state::AppState { pool }), edb, dir)
}

fn sample_bytes() -> Vec<u8> {
    std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx")).unwrap()
}

#[tokio::test]
async fn upload_then_read_back() {
    let (app, _edb, _dir) = test_app().await;

    let res = app.clone().oneshot(upload_req(&sample_bytes(), "sample.xlsx")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["positions"], 111);
    assert_eq!(body["duplicate"], false);

    // duplicate upload
    let res = app.clone().oneshot(upload_req(&sample_bytes(), "sample.xlsx")).await.unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["duplicate"], true);

    // garbage upload -> 400
    let res = app.clone().oneshot(upload_req(b"not an xlsx", "junk.xlsx")).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let res = app.clone().oneshot(Request::get("/api/nav").body(Body::empty()).unwrap()).await.unwrap();
    let nav: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(nav.as_array().unwrap().len(), 344);

    let res = app.clone().oneshot(Request::get("/api/positions").body(Body::empty()).unwrap()).await.unwrap();
    let pos: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(pos["date"], "2026-07-24");
    assert_eq!(pos["rows"].as_array().unwrap().len(), 111);

    let res = app.oneshot(Request::get("/api/imports").body(Body::empty()).unwrap()).await.unwrap();
    let imports: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(imports.as_array().unwrap().len(), 1);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p server api_imports`
Expected: compile FAIL (handlers missing).

- [ ] **Step 3: Implement**

`crates/server/src/handlers/imports.rs`:

```rust
use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Multipart, State};
use axum::Json;
use sha2::Digest;

pub async fn upload(State(st): State<AppState>, mut multipart: Multipart) -> Result<Json<db::repo::ImportOutcome>, AppError> {
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart error: {e}")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field.file_name().unwrap_or("upload.xlsx").to_string();
        let bytes = field
            .bytes()
            .await
            .map_err(|e| AppError::BadRequest(format!("read error: {e}")))?;
        let sha = hex::encode(sha2::Sha256::digest(&bytes));
        let parsed = ingest::parse_workbook(&bytes).map_err(|e| match e {
            ingest::ParseFailure::Workbook(m) => AppError::BadRequest(m),
            ingest::ParseFailure::Rows(rows) => AppError::UnprocessableRows(rows),
        })?;
        let outcome = db::repo::import_workbook(&st.pool, &filename, &sha, &parsed).await?;
        return Ok(Json(outcome));
    }
    Err(AppError::BadRequest("missing multipart field 'file'".into()))
}

pub async fn list(State(st): State<AppState>) -> Result<Json<Vec<db::repo::ImportRecord>>, AppError> {
    Ok(Json(db::repo::imports_list(&st.pool).await?))
}
```

`crates/server/src/handlers/data.rs`:

```rust
use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::Json;
use chrono::NaiveDate;

pub async fn nav(State(st): State<AppState>) -> Result<Json<Vec<db::repo::NavRow>>, AppError> {
    Ok(Json(db::repo::nav_rows(&st.pool).await?))
}

#[derive(serde::Deserialize)]
pub struct PositionsQuery {
    date: Option<String>,
}

#[derive(serde::Serialize)]
pub struct PositionsResponse {
    dates: Vec<NaiveDate>,
    date: Option<NaiveDate>,
    rows: Vec<db::repo::PositionRecord>,
}

pub async fn positions(
    State(st): State<AppState>,
    Query(q): Query<PositionsQuery>,
) -> Result<Json<PositionsResponse>, AppError> {
    let dates = db::repo::position_dates(&st.pool).await?;
    let date = match q.date {
        Some(s) => Some(s.parse::<NaiveDate>().map_err(|_| AppError::BadRequest(format!("bad date: {s}")))?),
        None => dates.first().copied(),
    };
    let rows = match date {
        Some(d) => db::repo::positions_for(&st.pool, d).await?,
        None => Vec::new(),
    };
    Ok(Json(PositionsResponse { dates, date, rows }))
}
```

Update `routes.rs` router (add before `.with_state`):

```rust
        .route("/api/imports", get(handlers::imports::list).post(handlers::imports::upload))
        .route("/api/nav", get(handlers::data::nav))
        .route("/api/positions", get(handlers::data::positions))
        .layer(axum::extract::DefaultBodyLimit::max(20 * 1024 * 1024))
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p server`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(server): xlsx upload endpoint + imports/nav/positions reads"
```

---

### Task 11: server — metrics endpoints (summary, rolling, drawdowns, calendar, VaR)

**Files:**
- Create: `crates/server/src/handlers/metrics.rs`, `crates/server/tests/api_metrics.rs`
- Modify: `crates/server/src/handlers/mod.rs` (add `pub mod metrics;`), `crates/server/src/routes.rs`

**Interfaces:**
- Consumes: everything from `analytics`; `db::repo::nav_rows`; `db::settings::get_settings`.
- Produces endpoints (all GET, all may return the empty-state shape `{"empty": true}` with status 200 when no NAV data exists):
  - `/api/metrics/summary` → `SummaryResponse` (below)
  - `/api/metrics/rolling?window=60` → `{"window": 60, "vol": [NavPoint], "sharpe": [...], "yield_vol": [...]}` (window clamped to 2..=1000, default 60)
  - `/api/metrics/drawdowns` → `{"underwater": [NavPoint], "yearly": [YearlyDrawdown], "top_short": [DrawdownEpisode], "overall_max": f64, "max_days": u32}`
  - `/api/metrics/calendar` → `{"monthly": [PeriodReturn], "quarterly": [PeriodReturn], "annual": [PeriodReturn]}`
  - `/api/metrics/var?confidence=0.99&horizon=20&window=252` → `VarResponse` (params default from settings)
- `MIN_OBS: usize = 30` lives here: headline metrics/VaR are `null` + warning if fewer return observations.

`SummaryResponse` (serde; `Option` renders as `null`):

```rust
#[derive(serde::Serialize)]
pub struct SummaryResponse {
    pub empty: bool,
    pub as_of: Option<chrono::NaiveDate>,
    pub nav: Option<f64>,
    pub aum: Option<f64>,
    pub ytd: Option<f64>,
    pub vol_1y: Option<f64>,
    pub vol_inception: Option<f64>,
    pub ann_return_1y: Option<f64>,
    pub yield_vol_1y: Option<f64>,
    pub sharpe_1y: Option<f64>,
    pub max_drawdown: Option<f64>,
    pub var_ucits: Option<VarBlock>,
    pub warnings: Vec<String>,
}

#[derive(serde::Serialize)]
pub struct VarBlock {
    pub confidence: f64,
    pub horizon_days: u32,
    pub window_days: u32,
    pub historical: Option<analytics::VarEs>,
    pub gaussian: Option<analytics::VarEs>,
    pub cornish_fisher: Option<analytics::VarEs>,
    pub limit: f64,
    pub utilization: Option<f64>,  // historical.var / limit
    pub var_eur: Option<f64>,      // historical.var * latest AUM
}

#[derive(serde::Serialize)]
pub struct VarResponse {
    pub empty: bool,
    pub confidence: f64,
    pub horizon_days: u32,
    pub window_days: u32,
    pub methods: Option<VarBlock>,
    pub rolling: Vec<analytics::NavPoint>,   // historical method, given params
    pub breaches: Vec<analytics::NavPoint>,  // rolling points with value > limit
    pub limit: f64,
    pub warnings: Vec<String>,
}
```

- [ ] **Step 1: Write failing test**

`crates/server/tests/api_metrics.rs` (reuses the multipart helper — copy it, tests can't share code across integration test files without a common module; keep the copy):

```rust
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

const BOUNDARY: &str = "XBOUNDARYX";

fn upload_req(bytes: &[u8]) -> Request<Body> {
    let mut body = Vec::new();
    body.extend_from_slice(format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"s.xlsx\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    ).as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    Request::post("/api/imports")
        .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(Body::from(body))
        .unwrap()
}

async fn get_json(app: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let res = app.clone().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    let body = serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    (status, body)
}

#[tokio::test]
async fn metrics_pipeline_on_sample() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    // empty state first
    let (st, body) = get_json(&app, "/api/metrics/summary").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["empty"], true);

    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx")).unwrap();
    let res = app.clone().oneshot(upload_req(&bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let (_, s) = get_json(&app, "/api/metrics/summary").await;
    assert_eq!(s["empty"], false);
    assert_eq!(s["as_of"], "2026-07-24");
    assert!((s["nav"].as_f64().unwrap() - 104.42).abs() < 1e-9);
    assert!(s["ytd"].as_f64().is_some());
    assert!(s["vol_1y"].as_f64().unwrap() > 0.0);
    assert!(s["max_drawdown"].as_f64().unwrap() <= 0.0);
    let var = &s["var_ucits"];
    assert_eq!(var["confidence"].as_f64().unwrap(), 0.99);
    assert!(var["historical"]["var"].as_f64().unwrap() > 0.0);
    assert!(var["gaussian"]["es"].as_f64().unwrap() >= var["gaussian"]["var"].as_f64().unwrap());

    let (_, r) = get_json(&app, "/api/metrics/rolling?window=60").await;
    assert_eq!(r["window"], 60);
    assert!(!r["vol"].as_array().unwrap().is_empty());
    assert_eq!(r["vol"].as_array().unwrap().len(), r["sharpe"].as_array().unwrap().len());

    let (_, dd) = get_json(&app, "/api/metrics/drawdowns").await;
    assert_eq!(dd["underwater"].as_array().unwrap().len(), 344);
    assert!(!dd["yearly"].as_array().unwrap().is_empty());
    assert!(dd["top_short"].as_array().unwrap().len() <= 5);

    let (_, cal) = get_json(&app, "/api/metrics/calendar").await;
    let monthly = cal["monthly"].as_array().unwrap();
    assert!(monthly.len() >= 17); // Feb 2025 .. Jul 2026
    assert_eq!(monthly[0]["year"], 2025);
    assert_eq!(monthly[0]["period"], 2);

    let (_, v) = get_json(&app, "/api/metrics/var?confidence=0.95&horizon=1&window=252").await;
    assert_eq!(v["confidence"].as_f64().unwrap(), 0.95);
    assert!(v["methods"]["historical"]["var"].as_f64().unwrap() > 0.0);
    assert!(!v["rolling"].as_array().unwrap().is_empty());

    pool.close().await;
    edb.stop().await;
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p server api_metrics`
Expected: compile FAIL.

- [ ] **Step 3: Implement `crates/server/src/handlers/metrics.rs`**

```rust
use crate::error::AppError;
use crate::state::AppState;
use analytics::{
    annual_returns, annualized_return_from_returns, annualized_vol, daily_returns,
    drawdown_series, monthly_returns, quarterly_returns, rolling_sharpe, rolling_var,
    rolling_vol, rolling_yield_vol, sharpe_ratio, top_short_drawdowns, var_es,
    yearly_max_drawdowns, yield_vol_ratio, ytd_performance, NavPoint, VarEs, VarMethod,
};
use axum::extract::{Query, State};
use axum::Json;

pub const MIN_OBS: usize = 30;

// (SummaryResponse, VarBlock, VarResponse struct definitions exactly as in
//  the Interfaces block above — paste them here.)

fn to_points(rows: &[db::repo::NavRow]) -> Vec<NavPoint> {
    rows.iter().map(|r| NavPoint { date: r.date, value: r.nav }).collect()
}

fn var_block(rets: &[f64], confidence: f64, horizon: u32, window: u32, limit: f64, aum: Option<f64>, warnings: &mut Vec<String>) -> Option<VarBlock> {
    let window_rets: &[f64] = if rets.len() > window as usize { &rets[rets.len() - window as usize..] } else { rets };
    if window_rets.len() < MIN_OBS {
        warnings.push(format!("VaR n/a: only {} observations (< {MIN_OBS})", window_rets.len()));
        return None;
    }
    if (window_rets.len() as u32) < window {
        warnings.push(format!("VaR window shrunk to available history ({} obs < {window})", window_rets.len()));
    }
    let h = horizon as f64;
    let historical = var_es(window_rets, VarMethod::Historical, confidence, h);
    let gaussian = var_es(window_rets, VarMethod::Gaussian, confidence, h);
    let cornish_fisher = var_es(window_rets, VarMethod::CornishFisher, confidence, h);
    let utilization = historical.map(|v| v.var / limit);
    let var_eur = match (historical, aum) { (Some(v), Some(a)) => Some(v.var * a), _ => None };
    Some(VarBlock { confidence, horizon_days: horizon, window_days: window, historical, gaussian, cornish_fisher, limit, utilization, var_eur })
}

pub async fn summary(State(st): State<AppState>) -> Result<Json<SummaryResponse>, AppError> {
    let rows = db::repo::nav_rows(&st.pool).await?;
    let settings = db::settings::get_settings(&st.pool).await?;
    if rows.is_empty() {
        return Ok(Json(SummaryResponse {
            empty: true, as_of: None, nav: None, aum: None, ytd: None, vol_1y: None,
            vol_inception: None, ann_return_1y: None, yield_vol_1y: None, sharpe_1y: None,
            max_drawdown: None, var_ucits: None, warnings: vec!["No data imported yet".into()],
        }));
    }
    let nav = to_points(&rows);
    let last = rows.last().unwrap();
    let rets: Vec<f64> = daily_returns(&nav).iter().map(|p| p.value).collect();
    let mut warnings = Vec::new();

    let (ytd, vol_1y, vol_inception, ann_return_1y, yield_vol_1y, sharpe_1y, max_drawdown) =
        if rets.len() < MIN_OBS {
            warnings.push(format!("Metrics n/a: only {} observations (< {MIN_OBS})", rets.len()));
            (None, None, None, None, None, None, None)
        } else {
            if rets.len() < 252 {
                warnings.push(format!("1Y metrics use full available history ({} obs < 252)", rets.len()));
            }
            let r1y: &[f64] = if rets.len() > 252 { &rets[rets.len() - 252..] } else { &rets };
            let vol_1y = annualized_vol(r1y);
            let ann_1y = annualized_return_from_returns(r1y);
            (
                ytd_performance(&nav, last.date),
                vol_1y,
                annualized_vol(&rets),
                ann_1y,
                match (ann_1y, vol_1y) { (Some(r), Some(v)) => yield_vol_ratio(r, v), _ => None },
                match (ann_1y, vol_1y) { (Some(r), Some(v)) => sharpe_ratio(r, v, settings.risk_free_rate), _ => None },
                drawdown_series(&nav).iter().map(|p| p.value).fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.min(v)))),
            )
        };

    let var_ucits = var_block(
        &rets, settings.var_confidence, settings.var_horizon_days,
        settings.var_window_days, settings.var_limit, Some(last.aum), &mut warnings,
    );

    Ok(Json(SummaryResponse {
        empty: false, as_of: Some(last.date), nav: Some(last.nav), aum: Some(last.aum),
        ytd, vol_1y, vol_inception, ann_return_1y, yield_vol_1y, sharpe_1y, max_drawdown,
        var_ucits, warnings,
    }))
}

#[derive(serde::Deserialize)]
pub struct RollingQuery { window: Option<usize> }

pub async fn rolling(State(st): State<AppState>, Query(q): Query<RollingQuery>) -> Result<Json<serde_json::Value>, AppError> {
    let rows = db::repo::nav_rows(&st.pool).await?;
    let settings = db::settings::get_settings(&st.pool).await?;
    let window = q.window.unwrap_or(60).clamp(2, 1000);
    let nav = to_points(&rows);
    Ok(Json(serde_json::json!({
        "empty": rows.is_empty(),
        "window": window,
        "vol": rolling_vol(&nav, window),
        "sharpe": rolling_sharpe(&nav, window, settings.risk_free_rate),
        "yield_vol": rolling_yield_vol(&nav, window),
    })))
}

pub async fn drawdowns(State(st): State<AppState>) -> Result<Json<serde_json::Value>, AppError> {
    let rows = db::repo::nav_rows(&st.pool).await?;
    let settings = db::settings::get_settings(&st.pool).await?;
    let nav = to_points(&rows);
    let underwater = drawdown_series(&nav);
    let overall_max = underwater.iter().map(|p| p.value).fold(0.0f64, f64::min);
    Ok(Json(serde_json::json!({
        "empty": rows.is_empty(),
        "underwater": underwater,
        "yearly": yearly_max_drawdowns(&nav),
        "top_short": top_short_drawdowns(&nav, settings.short_dd_max_days as i64, 5),
        "overall_max": overall_max,
        "max_days": settings.short_dd_max_days,
    })))
}

pub async fn calendar(State(st): State<AppState>) -> Result<Json<serde_json::Value>, AppError> {
    let rows = db::repo::nav_rows(&st.pool).await?;
    let nav = to_points(&rows);
    Ok(Json(serde_json::json!({
        "empty": rows.is_empty(),
        "monthly": monthly_returns(&nav),
        "quarterly": quarterly_returns(&nav),
        "annual": annual_returns(&nav),
    })))
}

#[derive(serde::Deserialize)]
pub struct VarQuery { confidence: Option<f64>, horizon: Option<u32>, window: Option<u32> }

pub async fn var(State(st): State<AppState>, Query(q): Query<VarQuery>) -> Result<Json<VarResponse>, AppError> {
    let rows = db::repo::nav_rows(&st.pool).await?;
    let settings = db::settings::get_settings(&st.pool).await?;
    let confidence = q.confidence.unwrap_or(settings.var_confidence);
    if !(0.5..1.0).contains(&confidence) {
        return Err(AppError::BadRequest("confidence must be in (0.5, 1)".into()));
    }
    let horizon = q.horizon.unwrap_or(settings.var_horizon_days).max(1);
    let window = q.window.unwrap_or(settings.var_window_days).max(30);
    let nav = to_points(&rows);
    let rets: Vec<f64> = daily_returns(&nav).iter().map(|p| p.value).collect();
    let mut warnings = Vec::new();
    let aum = rows.last().map(|r| r.aum);
    let methods = var_block(&rets, confidence, horizon, window, settings.var_limit, aum, &mut warnings);
    let effective_window = (window as usize).min(rets.len().max(2));
    let rolling = if rets.len() >= MIN_OBS {
        rolling_var(&nav, effective_window, VarMethod::Historical, confidence, horizon as f64)
    } else {
        Vec::new()
    };
    let breaches: Vec<NavPoint> = rolling.iter().filter(|p| p.value > settings.var_limit).cloned().collect();
    Ok(Json(VarResponse {
        empty: rows.is_empty(), confidence, horizon_days: horizon, window_days: window,
        methods, rolling, breaches, limit: settings.var_limit, warnings,
    }))
}
```

Routes to add in `routes.rs`:

```rust
        .route("/api/metrics/summary", get(handlers::metrics::summary))
        .route("/api/metrics/rolling", get(handlers::metrics::rolling))
        .route("/api/metrics/drawdowns", get(handlers::metrics::drawdowns))
        .route("/api/metrics/calendar", get(handlers::metrics::calendar))
        .route("/api/metrics/var", get(handlers::metrics::var))
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p server` then `cargo test` (whole workspace)
Expected: PASS everywhere.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(server): metrics endpoints - summary, rolling, drawdowns, calendar, VaR"
```

---

### Task 12: Frontend scaffold — Vite/React/TS, ECharts wrapper, API client, layout

**Files:**
- Create: `frontend/` (Vite react-ts template), `frontend/src/api.ts`, `frontend/src/fmt.ts`, `frontend/src/hooks.ts`, `frontend/src/components/EChart.tsx`, `frontend/src/components/KpiCard.tsx`, `frontend/src/App.tsx` (replace), `frontend/src/index.css` (replace), `frontend/src/pages/{Overview,Performance,Risk,VarPage,DataPage}.tsx` (placeholders that render their titles), `frontend/vite.config.ts` (replace)

**Interfaces:**
- Produces (used by all page tasks): `api.ts` typed fetchers `getSummary(): Promise<Summary>`, `getNav(): Promise<NavRow[]>`, `getRolling(window: number): Promise<Rolling>`, `getDrawdowns(): Promise<Drawdowns>`, `getCalendar(): Promise<Calendar>`, `getVar(p: {confidence: number; horizon: number; window: number}): Promise<VarResp>`, `getPositions(date?: string): Promise<Positions>`, `getImports(): Promise<ImportRec[]>`, `uploadFile(f: File): Promise<ImportOutcome>` (throws `ApiError` with `.detail`/`.rows`), `getSettings(): Promise<Settings>`, `putSettings(s: Settings): Promise<Settings>`; `hooks.ts` `useFetch<T>(fn: () => Promise<T>, deps: unknown[]): { data: T | null; error: string | null; reload: () => void }`; `fmt.ts` `pct(x: number | null | undefined, digits = 2): string` (e.g. `"4.20%"`, `"–"` for nullish), `num(x, digits = 2)`, `eur(x)`; `<EChart option={...} height={320}/>`; `<KpiCard label value sub?/>`. CSS classes used by pages: `.layout`, `.sidebar`, `.content`, `.card`, `.cards-row`, `.tbl` (styled table), `.warn-badge`, `.pos`, `.neg`.

- [ ] **Step 1: Scaffold**

```bash
npm create vite@latest frontend -- --template react-ts
cd frontend && npm install && npm install echarts react-router-dom && cd ..
```

`frontend/vite.config.ts`:

```ts
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: { proxy: { "/api": "http://127.0.0.1:8787" } },
});
```

- [ ] **Step 2: API client + helpers**

`frontend/src/api.ts` — mirror the server DTOs:

```ts
export interface NavPoint { date: string; value: number }
export interface NavRow { date: string; aum: number; shares: number; nav: number }
export interface VarEs { var: number; es: number }
export interface VarBlock {
  confidence: number; horizon_days: number; window_days: number;
  historical: VarEs | null; gaussian: VarEs | null; cornish_fisher: VarEs | null;
  limit: number; utilization: number | null; var_eur: number | null;
}
export interface Summary {
  empty: boolean; as_of: string | null; nav: number | null; aum: number | null;
  ytd: number | null; vol_1y: number | null; vol_inception: number | null;
  ann_return_1y: number | null; yield_vol_1y: number | null; sharpe_1y: number | null;
  max_drawdown: number | null; var_ucits: VarBlock | null; warnings: string[];
}
export interface Rolling { empty: boolean; window: number; vol: NavPoint[]; sharpe: NavPoint[]; yield_vol: NavPoint[] }
export interface Episode { peak_date: string; trough_date: string; depth: number; duration_days: number; recovery_date: string | null }
export interface Drawdowns { empty: boolean; underwater: NavPoint[]; yearly: { year: number; max_drawdown: number }[]; top_short: Episode[]; overall_max: number; max_days: number }
export interface PeriodReturn { year: number; period: number; value: number }
export interface Calendar { empty: boolean; monthly: PeriodReturn[]; quarterly: PeriodReturn[]; annual: PeriodReturn[] }
export interface VarResp {
  empty: boolean; confidence: number; horizon_days: number; window_days: number;
  methods: VarBlock | null; rolling: NavPoint[]; breaches: NavPoint[]; limit: number; warnings: string[];
}
export interface PositionRecord {
  nav_date: string; asset_type: string; isin: string; name: string | null; currency: string | null;
  quantity: number | null; avg_cost: number | null; price: number | null; valuation_ccy: number | null;
  accrued_interest: number | null; fx_rate: number | null; valuation_eur: number | null;
  weight: number | null; ticker: string | null;
}
export interface Positions { dates: string[]; date: string | null; rows: PositionRecord[] }
export interface ImportRec { id: number; filename: string; nav_date: string; imported_at: string; row_counts: Record<string, number> }
export interface ImportOutcome { import_id: number; duplicate: boolean; nav_rows: number; positions: number; dividends: number; operations: number; div_ops_replaced: boolean }
export interface Settings {
  risk_free_rate: number; var_confidence: number; var_horizon_days: number;
  var_window_days: number; var_limit: number; short_dd_max_days: number;
}
export interface RowError { sheet: string; row: number; message: string }

export class ApiError extends Error {
  detail?: string;
  rows?: RowError[];
  constructor(message: string, detail?: string, rows?: RowError[]) {
    super(message);
    this.detail = detail;
    this.rows = rows;
  }
}

async function req<T>(url: string, init?: RequestInit): Promise<T> {
  const res = await fetch(url, init);
  if (!res.ok) {
    let detail: string | undefined, rows: RowError[] | undefined;
    try {
      const body = await res.json();
      detail = body.detail; rows = body.rows;
    } catch { /* non-JSON error body */ }
    throw new ApiError(`${res.status} ${res.statusText}`, detail, rows);
  }
  return res.json() as Promise<T>;
}

export const getSummary = () => req<Summary>("/api/metrics/summary");
export const getNav = () => req<NavRow[]>("/api/nav");
export const getRolling = (window: number) => req<Rolling>(`/api/metrics/rolling?window=${window}`);
export const getDrawdowns = () => req<Drawdowns>("/api/metrics/drawdowns");
export const getCalendar = () => req<Calendar>("/api/metrics/calendar");
export const getVar = (p: { confidence: number; horizon: number; window: number }) =>
  req<VarResp>(`/api/metrics/var?confidence=${p.confidence}&horizon=${p.horizon}&window=${p.window}`);
export const getPositions = (date?: string) => req<Positions>(`/api/positions${date ? `?date=${date}` : ""}`);
export const getImports = () => req<ImportRec[]>("/api/imports");
export const getSettings = () => req<Settings>("/api/settings");
export const putSettings = (s: Settings) =>
  req<Settings>("/api/settings", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(s) });
export const uploadFile = (f: File) => {
  const fd = new FormData();
  fd.append("file", f);
  return req<ImportOutcome>("/api/imports", { method: "POST", body: fd });
};
```

`frontend/src/fmt.ts`:

```ts
export const pct = (x: number | null | undefined, digits = 2) =>
  x == null ? "–" : `${(x * 100).toFixed(digits)}%`;
export const num = (x: number | null | undefined, digits = 2) =>
  x == null ? "–" : x.toLocaleString("fr-FR", { maximumFractionDigits: digits, minimumFractionDigits: digits });
export const eur = (x: number | null | undefined) =>
  x == null ? "–" : x.toLocaleString("fr-FR", { style: "currency", currency: "EUR", maximumFractionDigits: 0 });
```

`frontend/src/hooks.ts`:

```ts
import { useCallback, useEffect, useState } from "react";

export function useFetch<T>(fn: () => Promise<T>, deps: unknown[]) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tick, setTick] = useState(0);
  const reload = useCallback(() => setTick((t) => t + 1), []);
  useEffect(() => {
    let alive = true;
    setError(null);
    fn().then(
      (d) => alive && setData(d),
      (e) => alive && setError(String(e?.detail ?? e?.message ?? e)),
    );
    return () => { alive = false; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, tick]);
  return { data, error, reload };
}
```

`frontend/src/components/EChart.tsx`:

```tsx
import { useEffect, useRef } from "react";
import * as echarts from "echarts";

export default function EChart({ option, height = 320 }: { option: echarts.EChartsOption; height?: number }) {
  const ref = useRef<HTMLDivElement>(null);
  const chart = useRef<echarts.ECharts | null>(null);
  useEffect(() => {
    chart.current = echarts.init(ref.current!);
    const onResize = () => chart.current?.resize();
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
      chart.current?.dispose();
      chart.current = null;
    };
  }, []);
  useEffect(() => {
    chart.current?.setOption(option, true);
  }, [option]);
  return <div ref={ref} style={{ height, width: "100%" }} />;
}
```

`frontend/src/components/KpiCard.tsx`:

```tsx
export default function KpiCard({ label, value, sub }: { label: string; value: string; sub?: string }) {
  return (
    <div className="card kpi">
      <div className="kpi-label">{label}</div>
      <div className="kpi-value">{value}</div>
      {sub && <div className="kpi-sub">{sub}</div>}
    </div>
  );
}
```

- [ ] **Step 3: Layout, router, placeholder pages, CSS**

`frontend/src/App.tsx`:

```tsx
import { BrowserRouter, NavLink, Route, Routes } from "react-router-dom";
import Overview from "./pages/Overview";
import Performance from "./pages/Performance";
import Risk from "./pages/Risk";
import VarPage from "./pages/VarPage";
import DataPage from "./pages/DataPage";

const links = [
  { to: "/", label: "Overview" },
  { to: "/performance", label: "Performance" },
  { to: "/risk", label: "Risk" },
  { to: "/var", label: "VaR / ES" },
  { to: "/data", label: "Data" },
];

export default function App() {
  return (
    <BrowserRouter>
      <div className="layout">
        <nav className="sidebar">
          <h1>Borobudur<br />Risk</h1>
          {links.map((l) => (
            <NavLink key={l.to} to={l.to} end={l.to === "/"}>{l.label}</NavLink>
          ))}
        </nav>
        <main className="content">
          <Routes>
            <Route path="/" element={<Overview />} />
            <Route path="/performance" element={<Performance />} />
            <Route path="/risk" element={<Risk />} />
            <Route path="/var" element={<VarPage />} />
            <Route path="/data" element={<DataPage />} />
          </Routes>
        </main>
      </div>
    </BrowserRouter>
  );
}
```

Placeholder page (repeat for the 5 pages, adjusting the title; each is replaced by its own later task):

```tsx
export default function Overview() {
  return <h2>Overview</h2>;
}
```

`frontend/src/index.css` (replace entirely):

```css
* { box-sizing: border-box; }
body { margin: 0; font-family: "Segoe UI", system-ui, sans-serif; background: #f4f6f8; color: #1a2330; }
.layout { display: flex; min-height: 100vh; }
.sidebar { width: 200px; background: #12263a; color: #fff; padding: 20px 0; flex-shrink: 0; }
.sidebar h1 { font-size: 18px; padding: 0 20px 16px; margin: 0; border-bottom: 1px solid #2a4562; }
.sidebar a { display: block; color: #b9c9d8; padding: 10px 20px; text-decoration: none; }
.sidebar a.active { color: #fff; background: #1d3952; border-left: 3px solid #4da3ff; }
.content { flex: 1; padding: 24px; max-width: 1280px; }
.card { background: #fff; border-radius: 8px; padding: 16px; box-shadow: 0 1px 3px rgba(16, 30, 54, .08); margin-bottom: 16px; }
.cards-row { display: grid; grid-template-columns: repeat(auto-fill, minmax(170px, 1fr)); gap: 12px; margin-bottom: 16px; }
.kpi-label { font-size: 12px; color: #64748b; text-transform: uppercase; letter-spacing: .04em; }
.kpi-value { font-size: 22px; font-weight: 600; margin-top: 4px; }
.kpi-sub { font-size: 12px; color: #64748b; margin-top: 2px; }
.tbl { width: 100%; border-collapse: collapse; font-size: 13px; }
.tbl th, .tbl td { padding: 6px 10px; text-align: right; border-bottom: 1px solid #e7ecf1; }
.tbl th:first-child, .tbl td:first-child { text-align: left; }
.tbl th { color: #64748b; font-weight: 600; }
.pos { color: #0a7d33; }
.neg { color: #c62828; }
.warn-badge { display: inline-block; background: #fff4e0; color: #925b06; border: 1px solid #f0c36d; border-radius: 4px; padding: 2px 8px; font-size: 12px; margin: 2px 6px 2px 0; }
h2 { margin: 0 0 16px; }
select, input[type="number"], button { font: inherit; padding: 6px 10px; border: 1px solid #cbd5e1; border-radius: 6px; background: #fff; }
button { cursor: pointer; background: #12263a; color: #fff; border: none; }
button:disabled { opacity: .5; cursor: default; }
.controls { display: flex; gap: 12px; align-items: center; margin-bottom: 16px; flex-wrap: wrap; }
.drop { border: 2px dashed #cbd5e1; border-radius: 8px; padding: 32px; text-align: center; color: #64748b; }
.drop.over { border-color: #4da3ff; background: #f0f7ff; }
```

Also update `frontend/src/main.tsx` to import `./index.css` only (remove `App.css` if the template references it) and delete unused template assets (`App.css`, `assets/react.svg` references).

- [ ] **Step 4: Verify build**

```bash
cd frontend && npm run build && cd ..
```

Expected: `tsc` + vite build succeed, `frontend/dist/` produced.

Optional manual check: `cargo run -p server` in one pane, `cd frontend && npm run dev` in another, open http://localhost:5173 — sidebar navigates between placeholder titles.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(frontend): scaffold Vite/React/TS with router, ECharts wrapper, API client"
```

---

### Task 13: Frontend — Data page (upload, imports, settings, positions)

**Files:**
- Modify: `frontend/src/pages/DataPage.tsx` (replace placeholder)

**Interfaces:**
- Consumes: `uploadFile`, `getImports`, `getSettings`, `putSettings`, `getPositions`, `useFetch`, `pct`, `num`, `eur`, `ApiError`.

- [ ] **Step 1: Implement the page**

`frontend/src/pages/DataPage.tsx`:

```tsx
import { useState } from "react";
import {
  ApiError, getImports, getPositions, getSettings, putSettings, uploadFile,
  type ImportOutcome, type Settings,
} from "../api";
import { useFetch } from "../hooks";
import { eur, num, pct } from "../fmt";

export default function DataPage() {
  const [over, setOver] = useState(false);
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<ImportOutcome | null>(null);
  const [uploadErr, setUploadErr] = useState<{ msg: string; rows?: { sheet: string; row: number; message: string }[] } | null>(null);
  const [posDate, setPosDate] = useState<string | undefined>(undefined);

  const imports = useFetch(() => getImports(), []);
  const positions = useFetch(() => getPositions(posDate), [posDate]);
  const settings = useFetch(() => getSettings(), []);

  async function doUpload(f: File) {
    setBusy(true);
    setOutcome(null);
    setUploadErr(null);
    try {
      setOutcome(await uploadFile(f));
      imports.reload();
      positions.reload();
    } catch (e) {
      const ae = e as ApiError;
      setUploadErr({ msg: ae.detail ?? ae.message, rows: ae.rows });
    } finally {
      setBusy(false);
    }
  }

  return (
    <div>
      <h2>Data</h2>

      <div
        className={`card drop ${over ? "over" : ""}`}
        onDragOver={(e) => { e.preventDefault(); setOver(true); }}
        onDragLeave={() => setOver(false)}
        onDrop={(e) => {
          e.preventDefault();
          setOver(false);
          const f = e.dataTransfer.files[0];
          if (f) void doUpload(f);
        }}
      >
        <p>{busy ? "Importing…" : "Drop the NAV Recap .xlsx here, or"}</p>
        <input
          type="file"
          accept=".xlsx"
          disabled={busy}
          onChange={(e) => { const f = e.target.files?.[0]; if (f) void doUpload(f); }}
        />
        {outcome && (
          <p className="pos">
            {outcome.duplicate
              ? "Already imported (identical file) — nothing changed."
              : `Imported: ${outcome.nav_rows} NAV rows, ${outcome.positions} positions, ${outcome.dividends} dividends, ${outcome.operations} operations${outcome.div_ops_replaced ? "" : " (older file: dividends/operations left untouched)"}.`}
          </p>
        )}
        {uploadErr && (
          <div className="neg">
            <p>Import failed: {uploadErr.msg}</p>
            {uploadErr.rows && (
              <table className="tbl"><tbody>
                {uploadErr.rows.slice(0, 20).map((r, i) => (
                  <tr key={i}><td>{r.sheet}</td><td>row {r.row}</td><td>{r.message}</td></tr>
                ))}
              </tbody></table>
            )}
          </div>
        )}
      </div>

      <div className="card">
        <h3>Import history</h3>
        {imports.error && <p className="neg">{imports.error}</p>}
        <table className="tbl">
          <thead><tr><th>File</th><th>NAV date</th><th>Imported at</th><th>Rows</th></tr></thead>
          <tbody>
            {(imports.data ?? []).map((r) => (
              <tr key={r.id}>
                <td>{r.filename}</td>
                <td>{r.nav_date}</td>
                <td>{new Date(r.imported_at).toLocaleString("fr-FR")}</td>
                <td>{Object.entries(r.row_counts).map(([k, v]) => `${k}: ${v}`).join(", ")}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <SettingsCard settings={settings.data} onSaved={settings.reload} />

      <div className="card">
        <h3>Portfolio snapshot</h3>
        <div className="controls">
          <label>Date:{" "}
            <select value={positions.data?.date ?? ""} onChange={(e) => setPosDate(e.target.value || undefined)}>
              {(positions.data?.dates ?? []).map((d) => <option key={d} value={d}>{d}</option>)}
            </select>
          </label>
        </div>
        <table className="tbl">
          <thead><tr><th>Type</th><th>ISIN</th><th>Name</th><th>Ccy</th><th>Qty</th><th>Price</th><th>Valuation €</th><th>Weight</th></tr></thead>
          <tbody>
            {(positions.data?.rows ?? []).map((p, i) => (
              <tr key={i}>
                <td>{p.asset_type}</td><td>{p.isin}</td><td>{p.name ?? ""}</td><td>{p.currency ?? ""}</td>
                <td>{num(p.quantity, 0)}</td><td>{num(p.price)}</td><td>{eur(p.valuation_eur)}</td>
                <td>{pct(p.weight)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function SettingsCard({ settings, onSaved }: { settings: Settings | null; onSaved: () => void }) {
  const [draft, setDraft] = useState<Settings | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const s = draft ?? settings;
  if (!s) return <div className="card"><h3>Settings</h3><p>Loading…</p></div>;
  const set = (patch: Partial<Settings>) => setDraft({ ...s, ...patch });
  return (
    <div className="card">
      <h3>Settings</h3>
      <div className="controls">
        <label>Risk-free %/yr <input type="number" step="0.1" value={(s.risk_free_rate * 100).toFixed(1)}
          onChange={(e) => set({ risk_free_rate: Number(e.target.value) / 100 })} /></label>
        <label>VaR conf % <input type="number" step="0.5" value={(s.var_confidence * 100).toFixed(1)}
          onChange={(e) => set({ var_confidence: Number(e.target.value) / 100 })} /></label>
        <label>Horizon d <input type="number" value={s.var_horizon_days}
          onChange={(e) => set({ var_horizon_days: Number(e.target.value) })} /></label>
        <label>Window d <input type="number" value={s.var_window_days}
          onChange={(e) => set({ var_window_days: Number(e.target.value) })} /></label>
        <label>VaR limit % <input type="number" step="1" value={(s.var_limit * 100).toFixed(0)}
          onChange={(e) => set({ var_limit: Number(e.target.value) / 100 })} /></label>
        <label>Short-DD max days <input type="number" value={s.short_dd_max_days}
          onChange={(e) => set({ short_dd_max_days: Number(e.target.value) })} /></label>
        <button disabled={!draft} onClick={() => {
          putSettings(s).then(() => { setDraft(null); setMsg("Saved."); onSaved(); },
            (e) => setMsg(`Error: ${e.detail ?? e.message}`));
        }}>Save</button>
        {msg && <span>{msg}</span>}
      </div>
    </div>
  );
}
```

- [ ] **Step 2: Verify build**

```bash
cd frontend && npm run build && cd ..
```

Expected: clean build. Manual check with `cargo run -p server` + `npm run dev`: upload the sample file, see the import report, snapshot table and settings round-trip.

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "feat(frontend): data page - upload, import history, settings, snapshot"
```

---

### Task 14: Frontend — Overview page

**Files:**
- Modify: `frontend/src/pages/Overview.tsx` (replace placeholder)

**Interfaces:**
- Consumes: `getSummary`, `getNav`, `getDrawdowns`, `useFetch`, `pct`, `num`, `eur`, `EChart`, `KpiCard`.

- [ ] **Step 1: Implement**

```tsx
import { getDrawdowns, getNav, getSummary } from "../api";
import EChart from "../components/EChart";
import KpiCard from "../components/KpiCard";
import { eur, num, pct } from "../fmt";
import { useFetch } from "../hooks";
import { Link } from "react-router-dom";

export default function Overview() {
  const summary = useFetch(() => getSummary(), []);
  const nav = useFetch(() => getNav(), []);
  const dd = useFetch(() => getDrawdowns(), []);
  const s = summary.data;

  if (s?.empty) {
    return (
      <div>
        <h2>Overview</h2>
        <div className="card">No data yet — <Link to="/data">import a NAV Recap file</Link> to get started.</div>
      </div>
    );
  }

  return (
    <div>
      <h2>Overview {s?.as_of && <small>as of {s.as_of}</small>}</h2>
      {s?.warnings.map((w, i) => <span key={i} className="warn-badge">{w}</span>)}
      <div className="cards-row">
        <KpiCard label="NAV" value={num(s?.nav)} />
        <KpiCard label="AUM" value={eur(s?.aum)} />
        <KpiCard label="YTD" value={pct(s?.ytd)} />
        <KpiCard label="Vol 1Y" value={pct(s?.vol_1y)} sub={`Inception ${pct(s?.vol_inception)}`} />
        <KpiCard label="Sharpe 1Y" value={num(s?.sharpe_1y)} sub={`Yield/Vol ${num(s?.yield_vol_1y)}`} />
        <KpiCard label="Max drawdown" value={pct(s?.max_drawdown)} />
        <KpiCard
          label={`VaR ${pct((s?.var_ucits?.confidence ?? 0.99), 0)}/${s?.var_ucits?.horizon_days ?? 20}d`}
          value={pct(s?.var_ucits?.historical?.var)}
          sub={`Limit ${pct(s?.var_ucits?.limit)} · used ${pct(s?.var_ucits?.utilization, 0)}`}
        />
      </div>

      <div className="card">
        <h3>NAV</h3>
        <EChart option={{
          tooltip: { trigger: "axis" },
          xAxis: { type: "category", data: (nav.data ?? []).map((r) => r.date) },
          yAxis: { type: "value", scale: true },
          series: [{ type: "line", showSymbol: false, data: (nav.data ?? []).map((r) => r.nav), name: "NAV" }],
          dataZoom: [{ type: "inside" }, { type: "slider" }],
          grid: { left: 50, right: 20, top: 20, bottom: 60 },
        }} />
      </div>

      <div className="card">
        <h3>Drawdown</h3>
        <EChart option={{
          tooltip: { trigger: "axis", valueFormatter: (v) => pct(v as number) },
          xAxis: { type: "category", data: (dd.data?.underwater ?? []).map((p) => p.date) },
          yAxis: { type: "value", axisLabel: { formatter: (v: number) => pct(v, 0) } },
          series: [{
            type: "line", showSymbol: false, name: "Drawdown", areaStyle: { opacity: 0.25 },
            color: "#c62828", data: (dd.data?.underwater ?? []).map((p) => p.value),
          }],
          grid: { left: 55, right: 20, top: 20, bottom: 30 },
        }} height={260} />
      </div>
    </div>
  );
}
```

- [ ] **Step 2: Verify build + manual check, commit**

```bash
cd frontend && npm run build && cd ..
git add -A && git commit -m "feat(frontend): overview page with KPIs, NAV and drawdown charts"
```

---

### Task 15: Frontend — Performance page (monthly/quarterly/yearly tables)

**Files:**
- Modify: `frontend/src/pages/Performance.tsx` (replace placeholder)

**Interfaces:**
- Consumes: `getCalendar`, `getDrawdowns`, `useFetch`, `pct`. Spec: show the LAST 3 calendar years in the tables (history shorter than that simply shows fewer rows).

- [ ] **Step 1: Implement**

```tsx
import { getCalendar, getDrawdowns, type PeriodReturn } from "../api";
import { pct } from "../fmt";
import { useFetch } from "../hooks";

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** background heat color: red for losses, green for gains */
function heat(v: number | undefined): string {
  if (v == null) return "transparent";
  const a = Math.min(Math.abs(v) / 0.05, 1) * 0.45;
  return v >= 0 ? `rgba(10, 125, 51, ${a})` : `rgba(198, 40, 40, ${a})`;
}

function byYear(rows: PeriodReturn[]): Map<number, Map<number, number>> {
  const m = new Map<number, Map<number, number>>();
  for (const r of rows) {
    if (!m.has(r.year)) m.set(r.year, new Map());
    m.get(r.year)!.set(r.period, r.value);
  }
  return m;
}

export default function Performance() {
  const cal = useFetch(() => getCalendar(), []);
  const dd = useFetch(() => getDrawdowns(), []);
  const monthly = byYear(cal.data?.monthly ?? []);
  const quarterly = byYear(cal.data?.quarterly ?? []);
  const annual = new Map((cal.data?.annual ?? []).map((r) => [r.year, r.value]));
  const years = [...monthly.keys()].sort((a, b) => b - a).slice(0, 3);

  return (
    <div>
      <h2>Performance</h2>

      <div className="card">
        <h3>Monthly returns</h3>
        <table className="tbl">
          <thead><tr><th>Year</th>{MONTHS.map((m) => <th key={m}>{m}</th>)}<th>Year</th></tr></thead>
          <tbody>
            {years.map((y) => (
              <tr key={y}>
                <td>{y}</td>
                {MONTHS.map((_, i) => {
                  const v = monthly.get(y)?.get(i + 1);
                  return <td key={i} style={{ background: heat(v) }}>{v == null ? "" : pct(v, 1)}</td>;
                })}
                <td className={(annual.get(y) ?? 0) >= 0 ? "pos" : "neg"}>{pct(annual.get(y), 1)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <div className="card">
        <h3>Quarterly returns</h3>
        <table className="tbl">
          <thead><tr><th>Year</th><th>Q1</th><th>Q2</th><th>Q3</th><th>Q4</th></tr></thead>
          <tbody>
            {years.map((y) => (
              <tr key={y}>
                <td>{y}</td>
                {[1, 2, 3, 4].map((q) => {
                  const v = quarterly.get(y)?.get(q);
                  return <td key={q} style={{ background: heat(v) }}>{v == null ? "" : pct(v, 1)}</td>;
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <div className="card">
        <h3>Max drawdown per year</h3>
        <table className="tbl">
          <thead><tr><th>Year</th><th>Max drawdown</th></tr></thead>
          <tbody>
            {(dd.data?.yearly ?? []).map((r) => (
              <tr key={r.year}><td>{r.year}</td><td className="neg">{pct(r.max_drawdown)}</td></tr>
            ))}
            <tr><td><b>Since inception</b></td><td className="neg"><b>{pct(dd.data?.overall_max)}</b></td></tr>
          </tbody>
        </table>
      </div>
    </div>
  );
}
```

- [ ] **Step 2: Verify build + commit**

```bash
cd frontend && npm run build && cd ..
git add -A && git commit -m "feat(frontend): performance page - monthly/quarterly heat tables, yearly drawdowns"
```

---

### Task 16: Frontend — Risk page (rolling charts + short drawdowns)

**Files:**
- Modify: `frontend/src/pages/Risk.tsx` (replace placeholder)

**Interfaces:**
- Consumes: `getRolling`, `getDrawdowns`, `useFetch`, `pct`, `num`, `EChart`. Window selector values: 20 / 60 / 120 / 252, default 60.

- [ ] **Step 1: Implement**

```tsx
import { useState } from "react";
import { getDrawdowns, getRolling, type NavPoint } from "../api";
import EChart from "../components/EChart";
import { num, pct } from "../fmt";
import { useFetch } from "../hooks";

function line(points: NavPoint[], name: string, percent: boolean, color?: string) {
  return {
    tooltip: { trigger: "axis" as const, valueFormatter: (v: unknown) => (percent ? pct(v as number) : num(v as number)) },
    xAxis: { type: "category" as const, data: points.map((p) => p.date) },
    yAxis: { type: "value" as const, scale: true, axisLabel: { formatter: (v: number) => (percent ? pct(v, 0) : num(v, 1)) } },
    series: [{ type: "line" as const, showSymbol: false, name, color, data: points.map((p) => p.value) }],
    grid: { left: 55, right: 20, top: 20, bottom: 30 },
  };
}

export default function Risk() {
  const [window, setWindow] = useState(60);
  const rolling = useFetch(() => getRolling(window), [window]);
  const dd = useFetch(() => getDrawdowns(), []);

  return (
    <div>
      <h2>Risk</h2>
      <div className="controls">
        <label>Rolling window:{" "}
          <select value={window} onChange={(e) => setWindow(Number(e.target.value))}>
            {[20, 60, 120, 252].map((w) => <option key={w} value={w}>{w} days</option>)}
          </select>
        </label>
      </div>

      <div className="card">
        <h3>Annualized volatility ({window}d rolling)</h3>
        <EChart option={line(rolling.data?.vol ?? [], "Vol", true)} height={260} />
      </div>
      <div className="card">
        <h3>Sharpe ratio ({window}d rolling)</h3>
        <EChart option={line(rolling.data?.sharpe ?? [], "Sharpe", false, "#7b1fa2")} height={260} />
      </div>
      <div className="card">
        <h3>Yield / volatility ({window}d rolling)</h3>
        <EChart option={line(rolling.data?.yield_vol ?? [], "Yield/Vol", false, "#00695c")} height={260} />
      </div>

      <div className="card">
        <h3>Top 5 drawdowns over short periods (≤ {dd.data?.max_days ?? 50} days)</h3>
        <table className="tbl">
          <thead><tr><th>#</th><th>Peak</th><th>Trough</th><th>Depth</th><th>Days</th><th>Recovered</th></tr></thead>
          <tbody>
            {(dd.data?.top_short ?? []).map((e, i) => (
              <tr key={i}>
                <td>{i + 1}</td><td>{e.peak_date}</td><td>{e.trough_date}</td>
                <td className="neg">{pct(e.depth)}</td><td>{e.duration_days}</td>
                <td>{e.recovery_date ?? "ongoing"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
```

- [ ] **Step 2: Verify build + commit**

```bash
cd frontend && npm run build && cd ..
git add -A && git commit -m "feat(frontend): risk page - rolling vol/sharpe/yield-vol charts, short drawdowns"
```

---

### Task 17: Frontend — VaR / ES page

**Files:**
- Modify: `frontend/src/pages/VarPage.tsx` (replace placeholder)

**Interfaces:**
- Consumes: `getVar`, `getSettings`, `useFetch`, `pct`, `eur`, `EChart`. Controls: confidence {95, 97.5, 99}%, horizon {1, 10, 20}d, window number input (min 30); initial values from settings.

- [ ] **Step 1: Implement**

```tsx
import { useState } from "react";
import { getSettings, getVar, type VarBlock } from "../api";
import EChart from "../components/EChart";
import { eur, pct } from "../fmt";
import { useFetch } from "../hooks";

function MethodCard({ title, v, varEur }: { title: string; v: { var: number; es: number } | null | undefined; varEur?: number | null }) {
  return (
    <div className="card kpi">
      <div className="kpi-label">{title}</div>
      <div className="kpi-value">{pct(v?.var)}</div>
      <div className="kpi-sub">ES {pct(v?.es)}{varEur != null ? ` · ${eur(varEur)}` : ""}</div>
    </div>
  );
}

export default function VarPage() {
  const settings = useFetch(() => getSettings(), []);
  const [confidence, setConfidence] = useState<number | null>(null);
  const [horizon, setHorizon] = useState<number | null>(null);
  const [window, setWindow] = useState<number | null>(null);

  const c = confidence ?? settings.data?.var_confidence ?? 0.99;
  const h = horizon ?? settings.data?.var_horizon_days ?? 20;
  const w = window ?? settings.data?.var_window_days ?? 252;
  const v = useFetch(() => getVar({ confidence: c, horizon: h, window: w }), [c, h, w, !!settings.data]);
  const m: VarBlock | null = v.data?.methods ?? null;

  return (
    <div>
      <h2>VaR / Expected Shortfall</h2>
      {v.data?.warnings.map((wn, i) => <span key={i} className="warn-badge">{wn}</span>)}
      <div className="controls">
        <label>Confidence:{" "}
          <select value={c} onChange={(e) => setConfidence(Number(e.target.value))}>
            {[0.95, 0.975, 0.99].map((x) => <option key={x} value={x}>{(x * 100).toFixed(1)}%</option>)}
          </select>
        </label>
        <label>Horizon:{" "}
          <select value={h} onChange={(e) => setHorizon(Number(e.target.value))}>
            {[1, 10, 20].map((x) => <option key={x} value={x}>{x}d</option>)}
          </select>
        </label>
        <label>Window:{" "}
          <input type="number" min={30} value={w} onChange={(e) => setWindow(Math.max(30, Number(e.target.value)))} />
        </label>
        <span>UCITS limit: {pct(v.data?.limit)}</span>
      </div>

      <div className="cards-row">
        <MethodCard title="Historical" v={m?.historical} varEur={m?.var_eur} />
        <MethodCard title="Gaussian" v={m?.gaussian} />
        <MethodCard title="Cornish-Fisher" v={m?.cornish_fisher} />
        <div className="card kpi">
          <div className="kpi-label">Limit utilization</div>
          <div className={`kpi-value ${(m?.utilization ?? 0) > 1 ? "neg" : "pos"}`}>{pct(m?.utilization, 0)}</div>
          <div className="kpi-sub">of {pct(m?.limit)} absolute VaR limit</div>
        </div>
      </div>

      <div className="card">
        <h3>Rolling VaR (historical, {(c * 100).toFixed(1)}% / {h}d) vs UCITS limit</h3>
        <EChart option={{
          tooltip: { trigger: "axis", valueFormatter: (x) => pct(x as number) },
          xAxis: { type: "category", data: (v.data?.rolling ?? []).map((p) => p.date) },
          yAxis: { type: "value", axisLabel: { formatter: (x: number) => pct(x, 0) } },
          series: [{
            type: "line", showSymbol: false, name: "VaR", color: "#b26a00",
            data: (v.data?.rolling ?? []).map((p) => p.value),
            markLine: {
              silent: true, symbol: "none",
              lineStyle: { color: "#c62828", type: "dashed" },
              data: [{ yAxis: v.data?.limit ?? 0.2, label: { formatter: "UCITS limit" } }],
            },
          }],
          grid: { left: 55, right: 40, top: 20, bottom: 30 },
        }} />
      </div>

      <div className="card">
        <h3>Limit breaches</h3>
        {(v.data?.breaches?.length ?? 0) === 0 ? <p className="pos">No breaches over the computed history.</p> : (
          <table className="tbl">
            <thead><tr><th>Date</th><th>VaR</th></tr></thead>
            <tbody>
              {(v.data?.breaches ?? []).map((b, i) => (
                <tr key={i}><td>{b.date}</td><td className="neg">{pct(b.value)}</td></tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
```

- [ ] **Step 2: Verify build + commit**

```bash
cd frontend && npm run build && cd ..
git add -A && git commit -m "feat(frontend): VaR/ES page - method cards, rolling VaR vs UCITS limit, breaches"
```

---

### Task 18: Integration — embed SPA in binary, build script, README, smoke test

**Files:**
- Create: `crates/server/src/static_assets.rs`, `build.ps1`, `README.md`
- Modify: `crates/server/Cargo.toml` (add deps), `crates/server/src/lib.rs` (add `pub mod static_assets;`), `crates/server/src/routes.rs` (fallback)

**Interfaces:**
- Consumes: `frontend/dist` (must exist — run the frontend build first).
- Produces: any non-`/api` GET serves the SPA (`index.html` fallback for client-side routes); single production exe.

- [ ] **Step 1: Implement static serving**

Add to `crates/server/Cargo.toml` `[dependencies]`:

```toml
rust-embed = "8"
mime_guess = "2"
```

`crates/server/src/static_assets.rs`:

```rust
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

#[derive(rust_embed::Embed)]
#[folder = "../../frontend/dist"]
struct Assets;

pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match Assets::get(path).or_else(|| Assets::get("index.html")) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref().to_string())], content.data).into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}
```

In `routes.rs`, add `.fallback(crate::static_assets::static_handler)` to the router (after the API routes, before `.with_state`).

- [ ] **Step 2: Build script + README**

`build.ps1`:

```powershell
$ErrorActionPreference = "Stop"
Push-Location frontend
npm ci
npm run build
Pop-Location
cargo build --release -p server
Write-Host "Done. Run: .\target\release\server.exe"
```

`README.md`:

```markdown
# Borobudur Risk

Local risk-monitoring dashboard for the Borobudur UCITS fund. Imports the
periodic "NAV Recap" .xlsx into an embedded PostgreSQL and serves analytics
(YTD, volatility, Sharpe, drawdowns, monthly/quarterly tables, VaR/ES with
UCITS 99%/20d monitoring) at http://127.0.0.1:8787.

## Build

Requires Rust (stable) and Node.js (build-time only).

    .\build.ps1

## Run

    .\target\release\server.exe

First start downloads a portable PostgreSQL 17 into
%LOCALAPPDATA%\borobudur-risk (one-time, needs network). The browser opens
automatically; go to the Data page and upload the NAV Recap workbook.

## Development

    cargo run -p server          # API on :8787
    cd frontend && npm run dev   # UI on :5173, proxies /api

Tests: `cargo test` (embedded-PG tests download binaries on first run) and
`cd frontend && npm run build` (type-check).

Design spec: docs/superpowers/specs/2026-07-30-borobudur-risk-tool-design.md
Plan: docs/superpowers/plans/2026-07-30-borobudur-risk-tool.md
```

- [ ] **Step 3: Full verification**

```bash
cd frontend && npm run build && cd ..
cargo test
cargo build --release -p server
```

Expected: all tests PASS, release build succeeds.

- [ ] **Step 4: End-to-end smoke test**

Run `./target/release/server.exe` (or `cargo run --release -p server`), then in another shell:

```bash
curl -s http://127.0.0.1:8787/api/health
# {"status":"ok"}
curl -s -F "file=@../24-07-2026 - Borobudur - NAV Recap.xlsx" http://127.0.0.1:8787/api/imports
# {"import_id":1,"duplicate":false,"nav_rows":344,"positions":111,...}
curl -s http://127.0.0.1:8787/api/metrics/summary
# {"empty":false,"as_of":"2026-07-24","nav":104.42,...}
curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8787/
# 200  (SPA served)
```

Then in the browser at http://127.0.0.1:8787: walk all 5 pages, confirm charts render, tables fill, VaR parameter changes refresh the numbers. Stop the server (Ctrl-C).

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: embed SPA, build script, README - v1 complete"
```

---

## Self-Review Notes (already applied)

- Spec coverage: YTD (T2/T11), ann. vol + rolling vol chart (T2/T11/T16), yield/vol ratio + chart (T2/T16), Sharpe + chart (T2/T16), max yearly drawdown tab (T3/T15), top-5 ≤50d drawdowns tab (T3/T16), underwater chart (T3/T14), monthly + quarterly tabs 3y (T4/T15), VaR/ES 3 methods + UCITS monitor + breaches (T5/T11/T17), 4-sheet ingest (T6), snapshot/upsert/replace import semantics (T8), embedded PG (T7), settings incl. risk-free (T7/T9/T13), import UI + report + errors (T10/T13), empty states (T11/T14), single exe (T18). No spec gap found.
- The file's own NAV row (nav_date not present in HISTO_NAV) is explicitly handled in T8 — nav_rows = 344, tested.
- Type consistency: `NavPoint {date, value}` serialized shape is relied on by frontend `NavPoint` interface; `VarEs {var, es}`, `PeriodReturn {year, period, value}`, `DrawdownEpisode` field names match the TS interfaces in T12.
- Library-version caveats are flagged where APIs drift (postgresql_embedded settings fields, calamine `Range::end`): implementers must consult docs.rs rather than fight compile errors blindly.




