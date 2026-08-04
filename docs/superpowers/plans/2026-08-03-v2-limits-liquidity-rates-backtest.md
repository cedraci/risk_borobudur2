# Borobudur Risk v2 — Limits, Liquidity, Rates, Back-testing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add concentration-limit monitoring (5/10/40, group, fund, deposit), offline liquidity bucketing with a redemption stress, bond YTM/duration/DV01, and regulatory VaR back-testing (3 methods, Basel zones, Kupiec) to the v1 app.

**Architecture:** One new DB table `instrument_refs` holds all user-editable reference data (issuer groups, liquidity bucket overrides, bond statics). Pure math goes in the `analytics` crate (new modules `concentration`, `liquidity`, `rates`, `backtest`); bond-name parsing in `ingest`; the server composes positions + refs + settings into four new metrics endpoints and a refs editor API; the frontend gains a "Limits" page, a back-testing section on the VaR page, and a reference-data editor on the Data page.

**Tech Stack:** Rust (edition 2024), axum 0.8, sqlx 0.8 (Postgres, embedded via postgresql_embedded), calamine, regex; Vite + React 19 + TypeScript + Apache ECharts 6.

**Spec:** `docs/superpowers/specs/2026-08-03-v2-limits-liquidity-rates-backtest-design.md`

## Global Constraints

- cargo is NOT on PATH in this environment. In PowerShell run it as `& "$env:USERPROFILE\.cargo\bin\cargo.exe" <args>` from the repo root `C:\Users\Laurent\Desktop\CC\riskborobudur\borobudur-risk`.
- Liquidity bucket enum, exactly these four strings everywhere (DB CHECK, Rust, TS): `d1`, `d2_7`, `d8_30`, `d30p`.
- Check status thresholds: `ok` if weight < 80% of limit; `watch` if ≥ 80% and ≤ limit; `breach` if > limit. Serialized lowercase: `"ok"`, `"watch"`, `"breach"`.
- Back-test is PINNED to horizon 1 day, confidence 99% (`0.99` literal), window = `var_window_days` setting. Basel zones over trailing `min(250, n)` points: green ≤ 4 exceptions, yellow 5–9, red ≥ 10. Kupiec reject at p-value < 0.05.
- VaR convention: positive = loss (as in v1 `analytics::var_es`).
- `PUT /api/refs/{code}` validation failures return **422** problem-details via a new `AppError::Unprocessable(String)` variant. Settings PUT keeps its existing 400 behavior.
- Never edit `crates/db/migrations/0001_init.sql` (its checksum is recorded in live DBs). New DDL goes in `0002_refs.sql`. Migration files are LF (enforced by `.gitattributes`).
- Ground truth after importing the fixture `crates/ingest/tests/fixtures/sample.xlsx`: 111 positions, 344 nav rows (last 2026-07-24), one bond `US105756CL22` named `BRAZILIAN GOVERNMENT INTL BOND 6.625% 15-03-35`, currency USD, weight ≈ 0.066.
- All API DTO fields snake_case; dates as ISO `YYYY-MM-DD` strings.
- Settings JSON value for `liquidity_defaults` maps asset types to buckets; asset types missing from the map default to `d1`.
- `sens`/NAV sensitivity convention: `nav_sensitivity_100bp` is the FRACTION of NAV lost per +100bp, i.e. `Σ(MD_i × w_i) × 0.01`.

## File Structure

- Create: `crates/db/migrations/0002_refs.sql` (Task 1)
- Modify: `crates/db/src/settings.rs` (Task 1)
- Create: `crates/db/tests/settings_v2.rs` (Task 1)
- Modify: `crates/ingest/Cargo.toml`, `crates/ingest/src/lib.rs` (Task 2)
- Modify: `crates/db/src/repo.rs` (Task 3)
- Create: `crates/db/tests/instrument_refs.rs` (Task 3)
- Create: `crates/analytics/src/concentration.rs` (Task 4)
- Create: `crates/analytics/src/liquidity.rs` (Task 5)
- Create: `crates/analytics/src/rates.rs` (Task 6)
- Create: `crates/analytics/src/backtest.rs`; modify `crates/analytics/src/var.rs` (normal_cdf) (Task 7)
- Modify: `crates/analytics/src/lib.rs` (Tasks 4–7)
- Create: `crates/server/src/handlers/refs.rs`; modify `error.rs`, `handlers/mod.rs`, `handlers/settings.rs`, `routes.rs` (Task 8)
- Create: `crates/server/tests/api_refs.rs` (Task 8)
- Create: `crates/server/src/handlers/limits.rs`; modify `handlers/metrics.rs`, `routes.rs` (Task 9)
- Create: `crates/server/tests/api_limits.rs` (Task 9)
- Modify: `frontend/src/api.ts`, `frontend/src/App.tsx`; create `frontend/src/pages/LimitsPage.tsx` (Task 10)
- Modify: `frontend/src/pages/VarPage.tsx`, `frontend/src/pages/DataPage.tsx` (Task 11)
- Modify: `README.md` (Task 12)

---

### Task 1: Migration 0002 + settings extension

**Files:**
- Create: `crates/db/migrations/0002_refs.sql`
- Modify: `crates/db/src/settings.rs`
- Test: `crates/db/tests/settings_v2.rs`

**Interfaces:**
- Produces: `instrument_refs` table; `AppSettings.liquidity_defaults: serde_json::Value`, `AppSettings.redemption_shock: f64`; `db::settings::default_liquidity_defaults()`.

- [ ] **Step 1: Write the failing test** — `crates/db/tests/settings_v2.rs`:

```rust
#[tokio::test]
async fn settings_v2_fields_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    let mut s = db::settings::get_settings(&pool).await.unwrap();
    assert!((s.redemption_shock - 0.30).abs() < 1e-12);
    assert_eq!(s.liquidity_defaults["Fonds"], "d2_7");
    assert_eq!(s.liquidity_defaults["Obligation"], "d8_30");

    s.redemption_shock = 0.25;
    s.liquidity_defaults["Fonds"] = serde_json::json!("d8_30");
    db::settings::put_settings(&pool, &s).await.unwrap();

    let s2 = db::settings::get_settings(&pool).await.unwrap();
    assert!((s2.redemption_shock - 0.25).abs() < 1e-12);
    assert_eq!(s2.liquidity_defaults["Fonds"], "d8_30");

    pool.close().await;
    edb.stop().await;
}
```

- [ ] **Step 2: Run it, expect failure** (missing fields):

Run: `& "$env:USERPROFILE\.cargo\bin\cargo.exe" test -p db --test settings_v2`
Expected: COMPILE ERROR — `AppSettings` has no field `redemption_shock`.

- [ ] **Step 3: Create the migration** — `crates/db/migrations/0002_refs.sql` (LF endings):

```sql
CREATE TABLE instrument_refs (
  code TEXT PRIMARY KEY,
  issuer_group TEXT,
  liquidity_bucket TEXT CHECK (liquidity_bucket IN ('d1','d2_7','d8_30','d30p')),
  bond_coupon_pct NUMERIC CHECK (bond_coupon_pct >= 0),
  bond_maturity DATE,
  bond_coupon_freq INT CHECK (bond_coupon_freq IN (1,2)),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO settings (key, value) VALUES
  ('liquidity_defaults', '{"Action":"d1","Fonds":"d2_7","Future":"d1","Obligation":"d8_30","Cash Acc":"d1","Margin Acc":"d1","Dividendes":"d1","Frais provisionnés":"d1","Provisions ordres":"d1"}'),
  ('redemption_shock', '0.30');
```

- [ ] **Step 4: Extend `crates/db/src/settings.rs`** — full new content:

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
    #[serde(default = "default_liquidity_defaults")]
    pub liquidity_defaults: serde_json::Value,
    #[serde(default = "default_redemption_shock")]
    pub redemption_shock: f64,
}

pub fn default_liquidity_defaults() -> serde_json::Value {
    serde_json::json!({
        "Action": "d1", "Fonds": "d2_7", "Future": "d1", "Obligation": "d8_30",
        "Cash Acc": "d1", "Margin Acc": "d1", "Dividendes": "d1",
        "Frais provisionnés": "d1", "Provisions ordres": "d1"
    })
}

fn default_redemption_shock() -> f64 { 0.30 }

pub async fn get_settings(pool: &PgPool) -> anyhow::Result<AppSettings> {
    let rows: Vec<(String, serde_json::Value)> =
        sqlx::query_as("SELECT key, value FROM settings").fetch_all(pool).await?;
    let get_f = |k: &str, d: f64| rows.iter().find(|(key, _)| key == k).and_then(|(_, v)| v.as_f64()).unwrap_or(d);
    let get_u = |k: &str, d: u32| rows.iter().find(|(key, _)| key == k).and_then(|(_, v)| v.as_u64()).map(|v| v as u32).unwrap_or(d);
    let liquidity_defaults = rows.iter().find(|(key, _)| key == "liquidity_defaults")
        .map(|(_, v)| v.clone())
        .unwrap_or_else(default_liquidity_defaults);
    Ok(AppSettings {
        risk_free_rate: get_f("risk_free_rate", 0.02),
        var_confidence: get_f("var_confidence", 0.99),
        var_horizon_days: get_u("var_horizon_days", 20),
        var_window_days: get_u("var_window_days", 252),
        var_limit: get_f("var_limit", 0.20),
        short_dd_max_days: get_u("short_dd_max_days", 50),
        liquidity_defaults,
        redemption_shock: get_f("redemption_shock", 0.30),
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
        ("liquidity_defaults", s.liquidity_defaults.clone()),
        ("redemption_shock", s.redemption_shock.into()),
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

- [ ] **Step 5: Run tests, expect pass**

Run: `& "$env:USERPROFILE\.cargo\bin\cargo.exe" test -p db`
Expected: all db tests PASS (including existing settings_roundtrip and import_workbook).

- [ ] **Step 6: Commit**

```
git add crates/db && git commit -m "feat(db): instrument_refs table, liquidity/redemption settings"
```

---

### Task 2: Bond-statics parser in ingest

**Files:**
- Modify: `crates/ingest/Cargo.toml` (add `regex = "1"` to `[dependencies]`)
- Modify: `crates/ingest/src/lib.rs` (append the new struct + function + tests)

**Interfaces:**
- Produces: `ingest::BondStatics { coupon_pct: f64, maturity: chrono::NaiveDate, coupon_freq: i32 }` and `ingest::parse_bond_statics(name: &str, currency: Option<&str>) -> Option<BondStatics>`. Task 3 calls this during import.

- [ ] **Step 1: Write the failing tests** — append to `crates/ingest/src/lib.rs` (inside the existing `#[cfg(test)] mod tests` if present at lib level, else a new `#[cfg(test)] mod bond_tests`):

```rust
#[cfg(test)]
mod bond_tests {
    use super::*;

    #[test]
    fn parses_standard_us_sovereign() {
        let b = parse_bond_statics("BRAZILIAN GOVERNMENT INTL BOND 6.625% 15-03-35", Some("USD")).unwrap();
        assert!((b.coupon_pct - 6.625).abs() < 1e-12);
        assert_eq!(b.maturity, chrono::NaiveDate::from_ymd_opt(2035, 3, 15).unwrap());
        assert_eq!(b.coupon_freq, 2);
    }

    #[test]
    fn parses_comma_decimal_and_eur_freq() {
        let b = parse_bond_statics("FRANCE OAT 2,50% 25-05-30", Some("EUR")).unwrap();
        assert!((b.coupon_pct - 2.5).abs() < 1e-12);
        assert_eq!(b.maturity, chrono::NaiveDate::from_ymd_opt(2030, 5, 25).unwrap());
        assert_eq!(b.coupon_freq, 1);
    }

    #[test]
    fn parses_four_digit_year() {
        let b = parse_bond_statics("XYZ 5% 01-01-2040", None).unwrap();
        assert_eq!(b.maturity, chrono::NaiveDate::from_ymd_opt(2040, 1, 1).unwrap());
        assert_eq!(b.coupon_freq, 1);
    }

    #[test]
    fn rejects_names_without_both_parts() {
        assert!(parse_bond_statics("NO COUPON HERE 15-03-35", Some("USD")).is_none());
        assert!(parse_bond_statics("COUPON ONLY 5%", Some("USD")).is_none());
        // maturity must come AFTER the coupon
        assert!(parse_bond_statics("15-03-35 THEN 5%", None).is_none());
        // invalid calendar date
        assert!(parse_bond_statics("BAD DATE 5% 32-13-35", None).is_none());
    }
}
```

- [ ] **Step 2: Run, expect compile failure** (`parse_bond_statics` not found):

Run: `& "$env:USERPROFILE\.cargo\bin\cargo.exe" test -p ingest`

- [ ] **Step 3: Implement** — append to `crates/ingest/src/lib.rs`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct BondStatics {
    pub coupon_pct: f64,
    pub maturity: chrono::NaiveDate,
    pub coupon_freq: i32,
}

/// Parse coupon and maturity from a bond position name like
/// "BRAZILIAN GOVERNMENT INTL BOND 6.625% 15-03-35". The maturity must
/// appear after the coupon; 2-digit years are 20YY. Frequency defaults to
/// 2 (semi-annual) for USD bonds, 1 otherwise.
pub fn parse_bond_statics(name: &str, currency: Option<&str>) -> Option<BondStatics> {
    let coupon_re = regex::Regex::new(r"(\d+(?:[.,]\d+)?)\s*%").unwrap();
    let mat_re = regex::Regex::new(r"(\d{2})-(\d{2})-(\d{2,4})").unwrap();
    let cm = coupon_re.captures(name)?;
    let coupon_pct: f64 = cm.get(1)?.as_str().replace(',', ".").parse().ok()?;
    let tail = &name[cm.get(0)?.end()..];
    let mm = mat_re.captures(tail)?;
    let day: u32 = mm.get(1)?.as_str().parse().ok()?;
    let month: u32 = mm.get(2)?.as_str().parse().ok()?;
    let ytxt = mm.get(3)?.as_str();
    let year: i32 = if ytxt.len() == 2 { 2000 + ytxt.parse::<i32>().ok()? } else { ytxt.parse().ok()? };
    let maturity = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
    let coupon_freq = if currency == Some("USD") { 2 } else { 1 };
    Some(BondStatics { coupon_pct, maturity, coupon_freq })
}
```

- [ ] **Step 4: Run tests, expect pass**

Run: `& "$env:USERPROFILE\.cargo\bin\cargo.exe" test -p ingest`
Expected: all ingest tests PASS (new + existing fixture tests).

- [ ] **Step 5: Commit**

```
git add crates/ingest && git commit -m "feat(ingest): parse bond coupon/maturity from position names"
```

---

### Task 3: Refs repository + import-time bond seeding

**Files:**
- Modify: `crates/db/src/repo.rs`
- Test: `crates/db/tests/instrument_refs.rs`

**Interfaces:**
- Consumes: `ingest::parse_bond_statics` (Task 2), `instrument_refs` table (Task 1).
- Produces: `db::repo::InstrumentRef { code: String, issuer_group: Option<String>, liquidity_bucket: Option<String>, bond_coupon_pct: Option<f64>, bond_maturity: Option<NaiveDate>, bond_coupon_freq: Option<i32> }`; `refs_all(pool) -> Vec<InstrumentRef>`; `refs_upsert(pool, &InstrumentRef)` (full-row replace semantics — a None field stores NULL = revert to default); import seeding of bond statics that never overwrites user values.

- [ ] **Step 1: Write the failing test** — `crates/db/tests/instrument_refs.rs`:

```rust
use db::repo::InstrumentRef;

fn fixture_bytes() -> Vec<u8> {
    std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx")).unwrap()
}

#[tokio::test]
async fn refs_upsert_seed_and_no_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    // 1. plain upsert + read-back
    let r = InstrumentRef {
        code: "TEST1".into(),
        issuer_group: Some("GROUP A".into()),
        liquidity_bucket: Some("d8_30".into()),
        bond_coupon_pct: None, bond_maturity: None, bond_coupon_freq: None,
    };
    db::repo::refs_upsert(&pool, &r).await.unwrap();
    let all = db::repo::refs_all(&pool).await.unwrap();
    let got = all.iter().find(|x| x.code == "TEST1").unwrap();
    assert_eq!(got.issuer_group.as_deref(), Some("GROUP A"));
    assert_eq!(got.liquidity_bucket.as_deref(), Some("d8_30"));

    // 2. full-row replace: None reverts to NULL
    let r2 = InstrumentRef { code: "TEST1".into(), issuer_group: None, liquidity_bucket: None,
        bond_coupon_pct: None, bond_maturity: None, bond_coupon_freq: None };
    db::repo::refs_upsert(&pool, &r2).await.unwrap();
    let all = db::repo::refs_all(&pool).await.unwrap();
    let got = all.iter().find(|x| x.code == "TEST1").unwrap();
    assert!(got.issuer_group.is_none() && got.liquidity_bucket.is_none());

    // 3. pre-seed a user override for the fixture bond, then import: the
    // user's coupon must survive; the empty maturity/freq get filled.
    let user = InstrumentRef { code: "US105756CL22".into(), issuer_group: None, liquidity_bucket: None,
        bond_coupon_pct: Some(7.0), bond_maturity: None, bond_coupon_freq: None };
    db::repo::refs_upsert(&pool, &user).await.unwrap();

    let wb = ingest::parse_workbook(&fixture_bytes()).unwrap();
    db::repo::import_workbook(&pool, "sample.xlsx", "sha-refs-test", &wb).await.unwrap();

    let all = db::repo::refs_all(&pool).await.unwrap();
    let bond = all.iter().find(|x| x.code == "US105756CL22").unwrap();
    assert_eq!(bond.bond_coupon_pct, Some(7.0)); // user value kept
    assert_eq!(bond.bond_maturity, Some(chrono::NaiveDate::from_ymd_opt(2035, 3, 15).unwrap()));
    assert_eq!(bond.bond_coupon_freq, Some(2));

    pool.close().await;
    edb.stop().await;
}
```

- [ ] **Step 2: Run, expect compile failure** (`InstrumentRef` not found):

Run: `& "$env:USERPROFILE\.cargo\bin\cargo.exe" test -p db --test instrument_refs`

- [ ] **Step 3: Implement** — append to `crates/db/src/repo.rs`:

```rust
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct InstrumentRef {
    pub code: String,
    pub issuer_group: Option<String>,
    pub liquidity_bucket: Option<String>,
    pub bond_coupon_pct: Option<f64>,
    pub bond_maturity: Option<NaiveDate>,
    pub bond_coupon_freq: Option<i32>,
}

pub async fn refs_all(pool: &PgPool) -> anyhow::Result<Vec<InstrumentRef>> {
    Ok(sqlx::query_as(
        "SELECT code, issuer_group, liquidity_bucket,
                bond_coupon_pct::float8 AS bond_coupon_pct, bond_maturity, bond_coupon_freq
         FROM instrument_refs ORDER BY code",
    )
    .fetch_all(pool)
    .await?)
}

/// Full-row replace: every field is written as given; None stores NULL,
/// which means "use the derived default".
pub async fn refs_upsert(pool: &PgPool, r: &InstrumentRef) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO instrument_refs (code, issuer_group, liquidity_bucket, bond_coupon_pct, bond_maturity, bond_coupon_freq, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, now())
         ON CONFLICT (code) DO UPDATE SET
           issuer_group = EXCLUDED.issuer_group,
           liquidity_bucket = EXCLUDED.liquidity_bucket,
           bond_coupon_pct = EXCLUDED.bond_coupon_pct,
           bond_maturity = EXCLUDED.bond_maturity,
           bond_coupon_freq = EXCLUDED.bond_coupon_freq,
           updated_at = now()",
    )
    .bind(&r.code).bind(&r.issuer_group).bind(&r.liquidity_bucket)
    .bind(r.bond_coupon_pct).bind(r.bond_maturity).bind(r.bond_coupon_freq)
    .execute(pool)
    .await?;
    Ok(())
}
```

And inside `import_workbook`, immediately after the positions insert loop (before the `if replace_div_ops` block), add the seeding loop (runs inside the same transaction):

```rust
    // Seed bond reference data parsed from names; never overwrite user
    // values (COALESCE keeps existing non-NULL columns).
    for p in &wb.positions {
        if p.asset_type != "Obligation" { continue; }
        let Some(name) = &p.name else { continue };
        let Some(b) = ingest::parse_bond_statics(name, p.currency.as_deref()) else { continue };
        sqlx::query(
            "INSERT INTO instrument_refs (code, bond_coupon_pct, bond_maturity, bond_coupon_freq)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (code) DO UPDATE SET
               bond_coupon_pct = COALESCE(instrument_refs.bond_coupon_pct, EXCLUDED.bond_coupon_pct),
               bond_maturity = COALESCE(instrument_refs.bond_maturity, EXCLUDED.bond_maturity),
               bond_coupon_freq = COALESCE(instrument_refs.bond_coupon_freq, EXCLUDED.bond_coupon_freq),
               updated_at = now()",
        )
        .bind(&p.isin).bind(b.coupon_pct).bind(b.maturity).bind(b.coupon_freq)
        .execute(&mut *tx)
        .await?;
    }
```

- [ ] **Step 4: Run tests, expect pass**

Run: `& "$env:USERPROFILE\.cargo\bin\cargo.exe" test -p db`
Expected: all db tests PASS.

- [ ] **Step 5: Commit**

```
git add crates/db && git commit -m "feat(db): instrument_refs repo + bond-statics seeding on import"
```

---

### Task 4: Analytics — concentration checks

**Files:**
- Create: `crates/analytics/src/concentration.rs`
- Modify: `crates/analytics/src/lib.rs` (add `pub mod concentration;` and `pub use concentration::*;`)

**Interfaces:**
- Produces: `ConPosition { asset_type: String, group: String, weight: f64 }`, `CheckStatus` (`Ok`/`Watch`/`Breach`, serialized `"ok"`/`"watch"`/`"breach"`), `CheckRow { group, weight, status }`, `Check { check, scope_label, limit, rows, status }`, `default_issuer_group(asset_type, name) -> String`, `concentration(&[ConPosition]) -> Vec<Check>`. The server (Task 9) resolves the effective group per position (override or default) BEFORE calling; for `Fonds` rows it always uses the default name-group (fund_20 is per target fund, overrides don't apply).

- [ ] **Step 1: Write the module with its tests** — `crates/analytics/src/concentration.rs`:

```rust
use serde::Serialize;
use std::collections::BTreeMap;

pub const WATCH_FRAC: f64 = 0.8;

#[derive(Debug, Clone)]
pub struct ConPosition {
    pub asset_type: String,
    /// Effective issuer group (override already applied by the caller).
    pub group: String,
    pub weight: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus { Ok, Watch, Breach }

#[derive(Debug, Clone, Serialize)]
pub struct CheckRow { pub group: String, pub weight: f64, pub status: CheckStatus }

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub check: String,
    pub scope_label: String,
    pub limit: f64,
    pub rows: Vec<CheckRow>,
    pub status: CheckStatus,
}

/// Default issuer group: normalized name; for cash/margin accounts the bank
/// code after the last "- " (e.g. "Depositary Bk- CBLU" -> "CBLU").
pub fn default_issuer_group(asset_type: &str, name: &str) -> String {
    let n = name.split_whitespace().collect::<Vec<_>>().join(" ").to_uppercase();
    match asset_type {
        "Cash Acc" | "Margin Acc" => n.rsplit_once("- ")
            .map(|(_, b)| b.trim().to_string())
            .filter(|b| !b.is_empty())
            .unwrap_or(n),
        _ => n,
    }
}

fn status_for(weight: f64, limit: f64) -> CheckStatus {
    if weight > limit { CheckStatus::Breach }
    else if weight >= WATCH_FRAC * limit { CheckStatus::Watch }
    else { CheckStatus::Ok }
}

fn severity(s: CheckStatus) -> u8 {
    match s { CheckStatus::Ok => 0, CheckStatus::Watch => 1, CheckStatus::Breach => 2 }
}

/// Sum weights per group; negatives offset within a group, floored at 0;
/// sorted descending by weight.
fn group_sums<'a>(rows: impl Iterator<Item = &'a ConPosition>) -> Vec<(String, f64)> {
    let mut m: BTreeMap<String, f64> = BTreeMap::new();
    for p in rows { *m.entry(p.group.clone()).or_default() += p.weight; }
    let mut v: Vec<(String, f64)> = m.into_iter().map(|(g, w)| (g, w.max(0.0))).collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1));
    v
}

/// The five v2 concentration checks: issuer_10, forty, group_20 on
/// transferable securities (+ dividend receivables), fund_20 per target
/// fund, deposit_20 per bank on net-positive cash+margin.
pub fn concentration(positions: &[ConPosition]) -> Vec<Check> {
    let sec_groups = group_sums(positions.iter()
        .filter(|p| matches!(p.asset_type.as_str(), "Action" | "Obligation" | "Dividendes")));
    let issuer_rows: Vec<CheckRow> = sec_groups.iter()
        .map(|(g, w)| CheckRow { group: g.clone(), weight: *w, status: status_for(*w, 0.10) })
        .collect();
    let over5: f64 = sec_groups.iter().filter(|(_, w)| *w > 0.05).map(|(_, w)| w).sum();
    let forty_rows = vec![CheckRow {
        group: "Sum of issuer exposures > 5%".into(), weight: over5, status: status_for(over5, 0.40),
    }];
    let group_rows: Vec<CheckRow> = sec_groups.iter()
        .map(|(g, w)| CheckRow { group: g.clone(), weight: *w, status: status_for(*w, 0.20) })
        .collect();
    let fund_rows: Vec<CheckRow> = group_sums(positions.iter().filter(|p| p.asset_type == "Fonds"))
        .iter()
        .map(|(g, w)| CheckRow { group: g.clone(), weight: *w, status: status_for(*w, 0.20) })
        .collect();
    let dep_rows: Vec<CheckRow> = group_sums(positions.iter()
        .filter(|p| matches!(p.asset_type.as_str(), "Cash Acc" | "Margin Acc")))
        .iter()
        .filter(|(_, w)| *w > 0.0)
        .map(|(g, w)| CheckRow { group: g.clone(), weight: *w, status: status_for(*w, 0.20) })
        .collect();

    let mk = |check: &str, scope_label: &str, limit: f64, rows: Vec<CheckRow>| {
        let status = rows.iter().map(|r| r.status).max_by_key(|s| severity(*s)).unwrap_or(CheckStatus::Ok);
        Check { check: check.into(), scope_label: scope_label.into(), limit, rows, status }
    };
    vec![
        mk("issuer_10", "Issuer <= 10% NAV (equities + bonds)", 0.10, issuer_rows),
        mk("forty", "Sum of issuers > 5% <= 40% NAV", 0.40, forty_rows),
        mk("group_20", "Connected group <= 20% NAV", 0.20, group_rows),
        mk("fund_20", "Target fund <= 20% NAV", 0.20, fund_rows),
        mk("deposit_20", "Deposits per bank <= 20% NAV", 0.20, dep_rows),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(t: &str, g: &str, w: f64) -> ConPosition {
        ConPosition { asset_type: t.into(), group: g.into(), weight: w }
    }

    #[test]
    fn default_groups() {
        assert_eq!(default_issuer_group("Action", "  Kering  SA "), "KERING SA");
        assert_eq!(default_issuer_group("Cash Acc", "Depositary Bk- CBLU"), "CBLU");
        assert_eq!(default_issuer_group("Margin Acc", "Managed acc - CABK"), "CABK");
        assert_eq!(default_issuer_group("Cash Acc", "NO SEPARATOR"), "NO SEPARATOR");
    }

    #[test]
    fn five_checks_toy_portfolio() {
        let positions = vec![
            pos("Action", "ALPHA", 0.09),        // watch on issuer_10 (>= 0.08)
            pos("Action", "BETA", 0.11),          // breach on issuer_10
            pos("Action", "GAMMA", 0.04),
            pos("Dividendes", "GAMMA", 0.02),     // folds into GAMMA -> 0.06
            pos("Future", "IGNORED", 0.50),       // excluded from all checks
            pos("Fonds", "F1", 0.19),             // watch on fund_20
            pos("Fonds", "F2", 0.05),             // ok
            pos("Cash Acc", "CBLU", 0.05),
            pos("Margin Acc", "CBLU", -0.01),     // nets to 0.04
            pos("Cash Acc", "NEGBANK", -0.02),    // floored to 0, dropped from rows
        ];
        let checks = concentration(&positions);
        assert_eq!(checks.len(), 5);

        let issuer = &checks[0];
        assert_eq!(issuer.check, "issuer_10");
        assert_eq!(issuer.status, CheckStatus::Breach);
        assert_eq!(issuer.rows[0].group, "BETA"); // sorted desc
        assert_eq!(issuer.rows[0].status, CheckStatus::Breach);
        assert_eq!(issuer.rows[1].group, "ALPHA");
        assert_eq!(issuer.rows[1].status, CheckStatus::Watch);
        let gamma = issuer.rows.iter().find(|r| r.group == "GAMMA").unwrap();
        assert!((gamma.weight - 0.06).abs() < 1e-12);
        assert!(!issuer.rows.iter().any(|r| r.group == "IGNORED"));

        let forty = &checks[1];
        assert!((forty.rows[0].weight - 0.26).abs() < 1e-12); // 0.09 + 0.11 + 0.06
        assert_eq!(forty.status, CheckStatus::Ok);

        let group = &checks[2];
        assert_eq!(group.check, "group_20");
        assert_eq!(group.status, CheckStatus::Ok); // 0.11 < 0.16 watch threshold

        let fund = &checks[3];
        assert_eq!(fund.rows[0].group, "F1");
        assert_eq!(fund.rows[0].status, CheckStatus::Watch);
        assert_eq!(fund.rows.len(), 2);

        let dep = &checks[4];
        assert_eq!(dep.rows.len(), 1); // NEGBANK floored to 0 and dropped
        assert_eq!(dep.rows[0].group, "CBLU");
        assert!((dep.rows[0].weight - 0.04).abs() < 1e-12);
        assert_eq!(dep.status, CheckStatus::Ok);
    }

    #[test]
    fn empty_input_yields_five_ok_checks() {
        let checks = concentration(&[]);
        assert_eq!(checks.len(), 5);
        assert!(checks.iter().all(|c| c.status == CheckStatus::Ok));
    }
}
```

- [ ] **Step 2: Wire into lib.rs and run** — add to `crates/analytics/src/lib.rs`:

```rust
pub mod concentration;
pub use concentration::*;
```

Run: `& "$env:USERPROFILE\.cargo\bin\cargo.exe" test -p analytics`
Expected: PASS (all analytics tests, old and new).

- [ ] **Step 3: Commit**

```
git add crates/analytics && git commit -m "feat(analytics): concentration checks (5/10/40, group, fund, deposit)"
```

---

### Task 5: Analytics — liquidity bucketing

**Files:**
- Create: `crates/analytics/src/liquidity.rs`
- Modify: `crates/analytics/src/lib.rs` (add `pub mod liquidity;` and `pub use liquidity::*;`)

**Interfaces:**
- Produces: `BUCKET_ORDER: [&str; 4]` (`["d1","d2_7","d8_30","d30p"]`), `LiqPosition { weight: f64, bucket: String }`, `BucketWeight { bucket: String, weight: f64 }`, `LiquidityReport { buckets, cumulative, negative_memo, stress_ok }`, `liquidity(&[LiqPosition], shock: f64) -> LiquidityReport`.

- [ ] **Step 1: Write the module** — `crates/analytics/src/liquidity.rs`:

```rust
use serde::Serialize;

pub const BUCKET_ORDER: [&str; 4] = ["d1", "d2_7", "d8_30", "d30p"];

#[derive(Debug, Clone)]
pub struct LiqPosition {
    pub weight: f64,
    pub bucket: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BucketWeight { pub bucket: String, pub weight: f64 }

#[derive(Debug, Clone, Serialize)]
pub struct LiquidityReport {
    pub buckets: Vec<BucketWeight>,
    pub cumulative: Vec<BucketWeight>,
    /// Sum of negative weights (payables, negative cash) — reported, not netted.
    pub negative_memo: f64,
    /// True when assets liquidatable in <= 7 days (d1 + d2_7) cover `shock`.
    pub stress_ok: bool,
}

/// Aggregate long weights per bucket. Unknown bucket names count as d30p
/// (conservative).
pub fn liquidity(rows: &[LiqPosition], shock: f64) -> LiquidityReport {
    let mut sums = [0.0f64; 4];
    let mut neg = 0.0;
    for r in rows {
        if r.weight < 0.0 { neg += r.weight; continue; }
        let idx = BUCKET_ORDER.iter().position(|b| *b == r.bucket).unwrap_or(3);
        sums[idx] += r.weight;
    }
    let buckets: Vec<BucketWeight> = BUCKET_ORDER.iter().zip(sums)
        .map(|(b, w)| BucketWeight { bucket: (*b).into(), weight: w })
        .collect();
    let mut acc = 0.0;
    let cumulative: Vec<BucketWeight> = buckets.iter()
        .map(|b| { acc += b.weight; BucketWeight { bucket: b.bucket.clone(), weight: acc } })
        .collect();
    let stress_ok = cumulative[1].weight >= shock;
    LiquidityReport { buckets, cumulative, negative_memo: neg, stress_ok }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lp(w: f64, b: &str) -> LiqPosition { LiqPosition { weight: w, bucket: b.into() } }

    #[test]
    fn buckets_cumulative_and_stress() {
        let rows = vec![
            lp(0.40, "d1"), lp(0.10, "d1"),
            lp(0.25, "d2_7"),
            lp(0.15, "d8_30"),
            lp(0.05, "d30p"),
            lp(0.02, "bogus"),   // unknown -> d30p
            lp(-0.03, "d1"),     // negative -> memo only
        ];
        let r = liquidity(&rows, 0.30);
        assert!((r.buckets[0].weight - 0.50).abs() < 1e-12);
        assert!((r.buckets[1].weight - 0.25).abs() < 1e-12);
        assert!((r.buckets[2].weight - 0.15).abs() < 1e-12);
        assert!((r.buckets[3].weight - 0.07).abs() < 1e-12);
        assert!((r.cumulative[3].weight - 0.97).abs() < 1e-12);
        assert!((r.negative_memo - (-0.03)).abs() < 1e-12);
        assert!(r.stress_ok); // 0.75 >= 0.30
        assert!(!liquidity(&rows, 0.80).stress_ok); // 0.75 < 0.80
    }

    #[test]
    fn empty_is_all_zero() {
        let r = liquidity(&[], 0.30);
        assert!(r.buckets.iter().all(|b| b.weight == 0.0));
        assert!(!r.stress_ok);
        assert!(liquidity(&[], 0.0).stress_ok);
    }
}
```

- [ ] **Step 2: Wire into lib.rs and run**

Add `pub mod liquidity;` / `pub use liquidity::*;` to `crates/analytics/src/lib.rs`.
Run: `& "$env:USERPROFILE\.cargo\bin\cargo.exe" test -p analytics`
Expected: PASS.

- [ ] **Step 3: Commit**

```
git add crates/analytics && git commit -m "feat(analytics): liquidity bucketing with redemption stress"
```

---

### Task 6: Analytics — bond YTM / duration

**Files:**
- Create: `crates/analytics/src/rates.rs`
- Modify: `crates/analytics/src/lib.rs` (add `pub mod rates;` and `pub use rates::*;`)

**Interfaces:**
- Consumes: `chrono::NaiveDate` (already a dependency).
- Produces: `BondMetrics { ytm: f64, macaulay: f64, modified: f64 }`, `bond_metrics(clean_price: f64, coupon_pct: f64, freq: u32, asof: NaiveDate, maturity: NaiveDate) -> Option<BondMetrics>`. DV01 is computed by the SERVER as `modified * valuation_eur * 1e-4`.

- [ ] **Step 1: Write the module** — `crates/analytics/src/rates.rs`:

```rust
use chrono::NaiveDate;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct BondMetrics {
    pub ytm: f64,
    pub macaulay: f64,
    pub modified: f64,
}

/// YTM (nominal, compounded `freq` times/yr), Macaulay and modified duration
/// from a clean price per 100 face. Coupons are laid out backwards from
/// maturity every 1/freq years (ACT/365.25 year fractions). Bisection on
/// y in [-0.5, 1.0]. None if maturity <= asof, freq not in {1, 2},
/// price <= 0, coupon < 0, or the price is outside the bracketed range.
pub fn bond_metrics(clean_price: f64, coupon_pct: f64, freq: u32, asof: NaiveDate, maturity: NaiveDate) -> Option<BondMetrics> {
    if !(freq == 1 || freq == 2) || clean_price <= 0.0 || coupon_pct < 0.0 {
        return None;
    }
    let t_mat = (maturity - asof).num_days() as f64 / 365.25;
    if t_mat <= 0.0 { return None; }
    let f = freq as f64;
    let n = (t_mat * f).ceil() as usize;
    let times: Vec<f64> = (0..n).map(|k| t_mat - (n - 1 - k) as f64 / f).collect();
    let cpn = coupon_pct / f; // per-period coupon per 100 face

    let price_at = |y: f64| -> f64 {
        let per = 1.0 + y / f;
        times.iter().enumerate().map(|(k, t)| {
            let cf = if k == n - 1 { cpn + 100.0 } else { cpn };
            cf / per.powf(f * t)
        }).sum()
    };

    let (mut lo, mut hi) = (-0.5f64, 1.0f64);
    if price_at(lo) < clean_price || price_at(hi) > clean_price { return None; }
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if price_at(mid) > clean_price { lo = mid; } else { hi = mid; }
    }
    let y = 0.5 * (lo + hi);
    let per = 1.0 + y / f;
    let p = price_at(y);
    let macaulay = times.iter().enumerate().map(|(k, t)| {
        let cf = if k == n - 1 { cpn + 100.0 } else { cpn };
        t * cf / per.powf(f * t)
    }).sum::<f64>() / p;
    Some(BondMetrics { ytm: y, macaulay, modified: macaulay / per })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, dd: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, dd).unwrap() }

    /// Independent PV for round-trip tests (same schedule convention).
    fn pv(coupon_pct: f64, f: f64, t_mat: f64, y: f64) -> f64 {
        let n = (t_mat * f).ceil() as usize;
        let cpn = coupon_pct / f;
        let per = 1.0 + y / f;
        (0..n).map(|k| {
            let t = t_mat - (n - 1 - k) as f64 / f;
            let cf = if k == n - 1 { cpn + 100.0 } else { cpn };
            cf / per.powf(f * t)
        }).sum()
    }

    #[test]
    fn round_trip_recovers_yield_annual() {
        // ~3y annual 5% bond priced at y = 4%
        let (asof, mat) = (d(2026, 8, 1), d(2029, 8, 1));
        let t_mat = (mat - asof).num_days() as f64 / 365.25;
        let price = pv(5.0, 1.0, t_mat, 0.04);
        let m = bond_metrics(price, 5.0, 1, asof, mat).unwrap();
        assert!((m.ytm - 0.04).abs() < 1e-6);
    }

    #[test]
    fn round_trip_recovers_yield_semiannual() {
        let (asof, mat) = (d(2026, 8, 1), d(2035, 3, 15));
        let t_mat = (mat - asof).num_days() as f64 / 365.25;
        let price = pv(6.625, 2.0, t_mat, 0.07);
        let m = bond_metrics(price, 6.625, 2, asof, mat).unwrap();
        assert!((m.ytm - 0.07).abs() < 1e-6);
    }

    #[test]
    fn par_bond_duration_close_to_textbook() {
        // 3-year 5% annual bond at par: Macaulay ~ 2.859, modified ~ 2.723.
        // Dates give t_mat ~ 2.9952 years, so allow a small tolerance.
        let (asof, mat) = (d(2026, 8, 3), d(2029, 8, 1));
        let m = bond_metrics(100.0, 5.0, 1, asof, mat).unwrap();
        assert!((m.ytm - 0.05).abs() < 5e-3);
        assert!((m.macaulay - 2.859).abs() < 0.02);
        assert!((m.modified - 2.723).abs() < 0.02);
        assert!(m.modified < m.macaulay);
    }

    #[test]
    fn rejects_invalid_inputs() {
        let (asof, mat) = (d(2026, 8, 1), d(2029, 8, 1));
        assert!(bond_metrics(100.0, 5.0, 4, asof, mat).is_none());   // bad freq
        assert!(bond_metrics(0.0, 5.0, 1, asof, mat).is_none());     // bad price
        assert!(bond_metrics(100.0, -1.0, 1, asof, mat).is_none());  // bad coupon
        assert!(bond_metrics(100.0, 5.0, 1, mat, asof).is_none());   // matured
        assert!(bond_metrics(1e-3, 0.0, 1, asof, mat).is_none());    // out of bracket
    }
}
```

- [ ] **Step 2: Wire into lib.rs and run**

Add `pub mod rates;` / `pub use rates::*;` to `crates/analytics/src/lib.rs`.
Run: `& "$env:USERPROFILE\.cargo\bin\cargo.exe" test -p analytics`
Expected: PASS.

- [ ] **Step 3: Commit**

```
git add crates/analytics && git commit -m "feat(analytics): bond YTM and duration via bisection"
```

---

### Task 7: Analytics — VaR back-testing + Kupiec

**Files:**
- Create: `crates/analytics/src/backtest.rs`
- Modify: `crates/analytics/src/var.rs` (add `normal_cdf` next to `inverse_normal_cdf`)
- Modify: `crates/analytics/src/lib.rs` (add `pub mod backtest;` and `pub use backtest::*;`)

**Interfaces:**
- Consumes: `daily_returns`, `var_es`, `VarMethod`, `NavPoint` (v1).
- Produces: `normal_cdf(z: f64) -> f64` (in `var.rs`); `BacktestPoint { date, ret, var_hist, var_gauss, var_cf, exc_hist, exc_gauss, exc_cf }`, `MethodSummary { exceptions, n, zone, kupiec_lr, kupiec_p, reject }`, `BacktestReport { points, historical, gaussian, cornish_fisher }`, `backtest(nav: &[NavPoint], window: usize, confidence: f64) -> BacktestReport`, `kupiec_pof(n: u32, x: u32, p: f64) -> Option<(f64, f64)>`.

- [ ] **Step 1: Add `normal_cdf` to `crates/analytics/src/var.rs`** (below `inverse_normal_cdf`):

```rust
/// Standard normal CDF via the Abramowitz–Stegun 7.1.26 erf approximation
/// (|error| < 1.5e-7).
pub fn normal_cdf(z: f64) -> f64 {
    let x = z / std::f64::consts::SQRT_2;
    let (sign, x) = if x < 0.0 { (-1.0, -x) } else { (1.0, x) };
    let t = 1.0 / (1.0 + 0.3275911 * x);
    let y = 1.0
        - (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736) * t
            + 0.254829592)
            * t
            * (-x * x).exp();
    0.5 * (1.0 + sign * y)
}
```

- [ ] **Step 2: Write the module** — `crates/analytics/src/backtest.rs`:

```rust
use crate::{daily_returns, normal_cdf, var_es, NavPoint, VarMethod};
use chrono::NaiveDate;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct BacktestPoint {
    /// Date of the realized return being compared.
    pub date: NaiveDate,
    pub ret: f64,
    pub var_hist: Option<f64>,
    pub var_gauss: Option<f64>,
    pub var_cf: Option<f64>,
    pub exc_hist: bool,
    pub exc_gauss: bool,
    pub exc_cf: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MethodSummary {
    pub exceptions: u32,
    pub n: u32,
    /// "green" (<=4), "yellow" (5-9), "red" (>=10) over trailing min(250, n).
    pub zone: String,
    pub kupiec_lr: Option<f64>,
    pub kupiec_p: Option<f64>,
    pub reject: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct BacktestReport {
    pub points: Vec<BacktestPoint>,
    pub historical: MethodSummary,
    pub gaussian: MethodSummary,
    pub cornish_fisher: MethodSummary,
}

/// Kupiec proportion-of-failures test: LR statistic and chi²(1 df) p-value.
/// None if n == 0, x > n, or p outside (0, 1).
pub fn kupiec_pof(n: u32, x: u32, p: f64) -> Option<(f64, f64)> {
    if n == 0 || x > n || p <= 0.0 || p >= 1.0 { return None; }
    let (nf, xf) = (n as f64, x as f64);
    let ln_null = (nf - xf) * (1.0 - p).ln() + xf * p.ln();
    let phat = xf / nf;
    let ln_alt = if x == 0 {
        0.0
    } else if x == n {
        xf * phat.ln() // = 0; kept explicit to avoid 0 * ln(0) in the general form
    } else {
        (nf - xf) * (1.0 - phat).ln() + xf * phat.ln()
    };
    let lr = (-2.0 * (ln_null - ln_alt)).max(0.0);
    // chi²(1) survival: P(X > lr) = 2 * (1 - Phi(sqrt(lr)))
    let pval = 2.0 * (1.0 - normal_cdf(lr.sqrt()));
    Some((lr, pval))
}

/// Regulatory back-test: for each date with `window` prior returns, 1-day
/// VaR at `confidence` from the trailing window vs that date's realized
/// return. Empty `points` when history is insufficient.
pub fn backtest(nav: &[NavPoint], window: usize, confidence: f64) -> BacktestReport {
    let rets = daily_returns(nav);
    let mut points: Vec<BacktestPoint> = Vec::new();
    if window >= 2 && rets.len() > window {
        for i in window..rets.len() {
            let w: Vec<f64> = rets[i - window..i].iter().map(|p| p.value).collect();
            let r = rets[i].value;
            let vh = var_es(&w, VarMethod::Historical, confidence, 1.0).map(|v| v.var);
            let vg = var_es(&w, VarMethod::Gaussian, confidence, 1.0).map(|v| v.var);
            let vc = var_es(&w, VarMethod::CornishFisher, confidence, 1.0).map(|v| v.var);
            let exc = |v: Option<f64>| v.map(|v| r < -v).unwrap_or(false);
            points.push(BacktestPoint {
                date: rets[i].date,
                ret: r,
                exc_hist: exc(vh), exc_gauss: exc(vg), exc_cf: exc(vc),
                var_hist: vh, var_gauss: vg, var_cf: vc,
            });
        }
    }
    let tail: &[BacktestPoint] = if points.len() > 250 { &points[points.len() - 250..] } else { &points };
    let p_tail = 1.0 - confidence;
    let summarize = |get: fn(&BacktestPoint) -> bool| -> MethodSummary {
        let n = tail.len() as u32;
        let x = tail.iter().filter(|pt| get(pt)).count() as u32;
        let zone = if x <= 4 { "green" } else if x <= 9 { "yellow" } else { "red" };
        let kp = kupiec_pof(n, x, p_tail);
        MethodSummary {
            exceptions: x,
            n,
            zone: zone.into(),
            kupiec_lr: kp.map(|(lr, _)| lr),
            kupiec_p: kp.map(|(_, p)| p),
            reject: kp.map(|(_, p)| p < 0.05).unwrap_or(false),
        }
    };
    let historical = summarize(|p| p.exc_hist);
    let gaussian = summarize(|p| p.exc_gauss);
    let cornish_fisher = summarize(|p| p.exc_cf);
    BacktestReport { points, historical, gaussian, cornish_fisher }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn normal_cdf_known_values() {
        assert!((normal_cdf(0.0) - 0.5).abs() < 1e-7);
        assert!((normal_cdf(1.96) - 0.9750).abs() < 1e-4);
        assert!((normal_cdf(-1.96) - 0.0250).abs() < 1e-4);
    }

    #[test]
    fn kupiec_published_values() {
        // n=250, x=5, p=1%: LR ~ 1.9569, p-value ~ 0.1618
        let (lr, p) = kupiec_pof(250, 5, 0.01).unwrap();
        assert!((lr - 1.9569).abs() < 1e-3);
        assert!((p - 0.1618).abs() < 1e-3);
        // n=250, x=0: LR = -2 * 250 * ln(0.99) ~ 5.0252, p ~ 0.0250 -> reject
        let (lr0, p0) = kupiec_pof(250, 0, 0.01).unwrap();
        assert!((lr0 - 5.0252).abs() < 1e-3);
        assert!((p0 - 0.0250).abs() < 1e-3);
        assert!(kupiec_pof(0, 0, 0.01).is_none());
        assert!(kupiec_pof(10, 11, 0.01).is_none());
    }

    #[test]
    fn counts_engineered_exceptions() {
        // 31 nav points -> 30 returns: +0.001 everywhere except two -5%
        // spikes at return indices 15 and 20; window = 10.
        let mut nav = Vec::new();
        let mut v = 100.0;
        nav.push(NavPoint { date: NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(), value: v });
        for i in 0..30u32 {
            let r = if i == 15 || i == 20 { -0.05 } else { 0.001 };
            v *= 1.0 + r;
            nav.push(NavPoint {
                date: NaiveDate::from_ymd_opt(2025, 1, 1).unwrap() + chrono::Days::new(i as u64 + 1),
                value: v,
            });
        }
        let report = backtest(&nav, 10, 0.99);
        assert_eq!(report.points.len(), 20); // 30 returns - window 10
        assert_eq!(report.historical.exceptions, 2);
        assert_eq!(report.historical.n, 20);
        assert_eq!(report.historical.zone, "green");
        let exc_dates: Vec<NaiveDate> = report.points.iter().filter(|p| p.exc_hist).map(|p| p.date).collect();
        assert_eq!(exc_dates.len(), 2);
        // insufficient history -> empty points, n = 0, no kupiec
        let short = backtest(&nav[..5], 10, 0.99);
        assert!(short.points.is_empty());
        assert_eq!(short.historical.n, 0);
        assert!(short.historical.kupiec_p.is_none());
    }
}
```

- [ ] **Step 3: Wire into lib.rs and run**

Add `pub mod backtest;` / `pub use backtest::*;` to `crates/analytics/src/lib.rs`.
Run: `& "$env:USERPROFILE\.cargo\bin\cargo.exe" test -p analytics`
Expected: PASS (all analytics tests, old and new).

- [ ] **Step 4: Commit**

```
git add crates/analytics && git commit -m "feat(analytics): VaR back-testing with Basel zones and Kupiec POF"
```

---

### Task 8: Server — refs API + settings validation + 422 variant

**Files:**
- Create: `crates/server/src/handlers/refs.rs`
- Modify: `crates/server/src/error.rs` (add `Unprocessable` variant)
- Modify: `crates/server/src/handlers/mod.rs` (add `pub mod refs;` — check the existing module declarations; they may live in `handlers.rs` or `handlers/mod.rs`)
- Modify: `crates/server/src/handlers/settings.rs` (validate new fields)
- Modify: `crates/server/src/routes.rs`
- Test: `crates/server/tests/api_refs.rs`

**Interfaces:**
- Consumes: `db::repo::{refs_all, refs_upsert, InstrumentRef, position_dates, positions_for}` (Task 3), `analytics::default_issuer_group` (Task 4), `AppSettings.liquidity_defaults`/`redemption_shock` (Task 1).
- Produces: `GET /api/refs` → `Vec<RefRow>`; `PUT /api/refs/{code}` (body `RefBody`, full-row replace, `null` field = revert to default) → stored `InstrumentRef`; `handlers::refs::effective_bucket(defaults, asset_type, override) -> String` (reused by Task 9); extended settings validation.

- [ ] **Step 1: Add the 422 variant** — in `crates/server/src/error.rs`, extend the enum and match:

```rust
// add to enum AppError:
    Unprocessable(String),
```

```rust
// add to the match in into_response():
            AppError::Unprocessable(msg) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({"title": "Unprocessable Entity", "status": 422, "detail": msg})),
            )
                .into_response(),
```

- [ ] **Step 2: Write the refs handler** — `crates/server/src/handlers/refs.rs`:

```rust
use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::Json;
use chrono::NaiveDate;
use std::collections::{HashMap, HashSet};

pub const BUCKETS: [&str; 4] = ["d1", "d2_7", "d8_30", "d30p"];

#[derive(serde::Serialize)]
pub struct RefRow {
    pub code: String,
    pub name: String,
    pub asset_type: String,
    pub effective_issuer_group: String,
    pub issuer_group_override: Option<String>,
    pub effective_bucket: String,
    pub bucket_override: Option<String>,
    pub bond_coupon_pct: Option<f64>,
    pub bond_maturity: Option<NaiveDate>,
    pub bond_coupon_freq: Option<i32>,
    pub is_bond: bool,
}

/// Effective liquidity bucket: override, else asset-type default, else d1.
pub fn effective_bucket(defaults: &serde_json::Value, asset_type: &str, override_: Option<&str>) -> String {
    override_
        .map(str::to_string)
        .or_else(|| defaults.get(asset_type).and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_else(|| "d1".into())
}

/// Latest-snapshot positions merged with their instrument_refs rows,
/// de-duplicated by code (e.g. an equity plus its dividend receivable).
pub async fn list(State(st): State<AppState>) -> Result<Json<Vec<RefRow>>, AppError> {
    let dates = db::repo::position_dates(&st.pool).await?;
    let Some(latest) = dates.first().copied() else { return Ok(Json(Vec::new())); };
    let positions = db::repo::positions_for(&st.pool, latest).await?;
    let refs = db::repo::refs_all(&st.pool).await?;
    let settings = db::settings::get_settings(&st.pool).await?;
    let by_code: HashMap<&str, &db::repo::InstrumentRef> =
        refs.iter().map(|r| (r.code.as_str(), r)).collect();

    let mut seen: HashSet<&str> = HashSet::new();
    let mut rows = Vec::new();
    for p in &positions {
        if !seen.insert(p.isin.as_str()) { continue; }
        let name = p.name.clone().unwrap_or_default();
        let r = by_code.get(p.isin.as_str());
        let issuer_group_override = r.and_then(|r| r.issuer_group.clone());
        let bucket_override = r.and_then(|r| r.liquidity_bucket.clone());
        rows.push(RefRow {
            code: p.isin.clone(),
            effective_issuer_group: issuer_group_override
                .clone()
                .unwrap_or_else(|| analytics::default_issuer_group(&p.asset_type, &name)),
            issuer_group_override,
            effective_bucket: effective_bucket(&settings.liquidity_defaults, &p.asset_type, bucket_override.as_deref()),
            bucket_override,
            bond_coupon_pct: r.and_then(|r| r.bond_coupon_pct),
            bond_maturity: r.and_then(|r| r.bond_maturity),
            bond_coupon_freq: r.and_then(|r| r.bond_coupon_freq),
            is_bond: p.asset_type == "Obligation",
            asset_type: p.asset_type.clone(),
            name,
        });
    }
    Ok(Json(rows))
}

#[derive(serde::Deserialize)]
pub struct RefBody {
    pub issuer_group: Option<String>,
    pub liquidity_bucket: Option<String>,
    pub bond_coupon_pct: Option<f64>,
    pub bond_maturity: Option<NaiveDate>,
    pub bond_coupon_freq: Option<i32>,
}

pub async fn put(
    State(st): State<AppState>,
    Path(code): Path<String>,
    Json(b): Json<RefBody>,
) -> Result<Json<db::repo::InstrumentRef>, AppError> {
    if let Some(bkt) = &b.liquidity_bucket {
        if !BUCKETS.contains(&bkt.as_str()) {
            return Err(AppError::Unprocessable(format!("liquidity_bucket must be one of {BUCKETS:?}")));
        }
    }
    if let Some(c) = b.bond_coupon_pct {
        if !(0.0..=100.0).contains(&c) {
            return Err(AppError::Unprocessable("bond_coupon_pct must be in [0, 100]".into()));
        }
    }
    if let Some(f) = b.bond_coupon_freq {
        if f != 1 && f != 2 {
            return Err(AppError::Unprocessable("bond_coupon_freq must be 1 or 2".into()));
        }
    }
    if let Some(g) = &b.issuer_group {
        if g.trim().is_empty() {
            return Err(AppError::Unprocessable("issuer_group must not be blank (send null to revert)".into()));
        }
    }
    let r = db::repo::InstrumentRef {
        code,
        issuer_group: b.issuer_group.map(|g| g.trim().to_string()),
        liquidity_bucket: b.liquidity_bucket,
        bond_coupon_pct: b.bond_coupon_pct,
        bond_maturity: b.bond_maturity,
        bond_coupon_freq: b.bond_coupon_freq,
    };
    db::repo::refs_upsert(&st.pool, &r).await?;
    Ok(Json(r))
}
```

- [ ] **Step 3: Extend settings validation** — in `crates/server/src/handlers/settings.rs`, add to `validate`:

```rust
    if !(s.redemption_shock > 0.0 && s.redemption_shock < 1.0) {
        return Err("redemption_shock must be in (0, 1)".into());
    }
    let Some(obj) = s.liquidity_defaults.as_object() else {
        return Err("liquidity_defaults must be a JSON object".into());
    };
    for (k, v) in obj {
        let ok = v.as_str().map(|b| ["d1", "d2_7", "d8_30", "d30p"].contains(&b)).unwrap_or(false);
        if !ok {
            return Err(format!("liquidity_defaults[{k}] must be one of d1, d2_7, d8_30, d30p"));
        }
    }
```

- [ ] **Step 4: Register module and routes** — add `pub mod refs;` to the handlers module list; in `crates/server/src/routes.rs` add (before the `.layer` line):

```rust
        .route("/api/refs", get(handlers::refs::list))
        .route("/api/refs/{code}", axum::routing::put(handlers::refs::put))
```

- [ ] **Step 5: Write the integration test** — `crates/server/tests/api_refs.rs` (copy the `upload_req`/`get_json` helpers from `api_metrics.rs`; add a `put_json` helper):

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

async fn put_json(app: &axum::Router, uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let res = app.clone().oneshot(
        Request::put(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    ).await.unwrap();
    let status = res.status();
    let body = serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    (status, body)
}

#[tokio::test]
async fn refs_editor_flow() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    // empty DB -> empty list
    let (st, body) = get_json(&app, "/api/refs").await;
    assert_eq!(st, StatusCode::OK);
    assert!(body.as_array().unwrap().is_empty());

    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx")).unwrap();
    let res = app.clone().oneshot(upload_req(&bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let (_, rows) = get_json(&app, "/api/refs").await;
    let rows = rows.as_array().unwrap().clone();
    assert!(rows.len() >= 100); // 111 positions minus duplicate codes
    let bond = rows.iter().find(|r| r["code"] == "US105756CL22").unwrap();
    assert_eq!(bond["is_bond"], true);
    assert_eq!(bond["bond_coupon_pct"].as_f64().unwrap(), 6.625);
    assert_eq!(bond["bond_maturity"], "2035-03-15");
    assert_eq!(bond["bond_coupon_freq"], 2);
    assert_eq!(bond["effective_bucket"], "d8_30"); // Obligation default
    let cash = rows.iter().find(|r| r["asset_type"] == "Cash Acc").unwrap();
    assert_eq!(cash["effective_issuer_group"], "CBLU");

    // set an issuer-group override on a fund code
    let helium = rows.iter().find(|r| r["code"] == "LU1112771255").unwrap();
    assert_eq!(helium["issuer_group_override"], serde_json::Value::Null);
    let (st, _) = put_json(&app, "/api/refs/LU1112771255", serde_json::json!({
        "issuer_group": "HELIUM GROUP", "liquidity_bucket": "d8_30",
        "bond_coupon_pct": null, "bond_maturity": null, "bond_coupon_freq": null
    })).await;
    assert_eq!(st, StatusCode::OK);
    let (_, rows2) = get_json(&app, "/api/refs").await;
    let helium2 = rows2.as_array().unwrap().iter().find(|r| r["code"] == "LU1112771255").unwrap();
    assert_eq!(helium2["effective_issuer_group"], "HELIUM GROUP");
    assert_eq!(helium2["effective_bucket"], "d8_30");

    // revert with nulls
    let (st, _) = put_json(&app, "/api/refs/LU1112771255", serde_json::json!({
        "issuer_group": null, "liquidity_bucket": null,
        "bond_coupon_pct": null, "bond_maturity": null, "bond_coupon_freq": null
    })).await;
    assert_eq!(st, StatusCode::OK);
    let (_, rows3) = get_json(&app, "/api/refs").await;
    let helium3 = rows3.as_array().unwrap().iter().find(|r| r["code"] == "LU1112771255").unwrap();
    assert_eq!(helium3["issuer_group_override"], serde_json::Value::Null);
    assert_eq!(helium3["effective_bucket"], "d2_7"); // back to Fonds default

    // invalid bucket -> 422
    let (st, err) = put_json(&app, "/api/refs/LU1112771255", serde_json::json!({
        "issuer_group": null, "liquidity_bucket": "weekly",
        "bond_coupon_pct": null, "bond_maturity": null, "bond_coupon_freq": null
    })).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(err["status"], 422);

    // settings validation: bad redemption_shock -> 400
    let (_, mut s) = get_json(&app, "/api/settings").await;
    s["redemption_shock"] = serde_json::json!(1.5);
    let (st, _) = put_json(&app, "/api/settings", s).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    pool.close().await;
    edb.stop().await;
}
```

- [ ] **Step 6: Run tests, expect pass**

Run: `& "$env:USERPROFILE\.cargo\bin\cargo.exe" test -p server`
Expected: all server tests PASS.

- [ ] **Step 7: Commit**

```
git add crates/server && git commit -m "feat(server): reference-data API, 422 variant, settings validation"
```

---

### Task 9: Server — concentration/liquidity/rates/backtest endpoints

**Files:**
- Create: `crates/server/src/handlers/limits.rs`
- Modify: `crates/server/src/handlers/metrics.rs` (add `backtest` handler)
- Modify: handlers module list (add `pub mod limits;`)
- Modify: `crates/server/src/routes.rs`
- Test: `crates/server/tests/api_limits.rs`

**Interfaces:**
- Consumes: analytics Tasks 4–7, `handlers::refs::effective_bucket` (Task 8), `db::repo` (Task 3).
- Produces: `GET /api/metrics/concentration?date=`, `GET /api/metrics/liquidity?date=`, `GET /api/metrics/rates?date=`, `GET /api/metrics/backtest` with the JSON shapes below (Task 10 consumes them).

- [ ] **Step 1: Write the handlers** — `crates/server/src/handlers/limits.rs`:

```rust
use crate::error::AppError;
use crate::handlers::refs::effective_bucket;
use crate::state::AppState;
use analytics::{concentration, default_issuer_group, liquidity, ConPosition, LiqPosition};
use axum::extract::{Query, State};
use axum::Json;
use chrono::NaiveDate;
use std::collections::HashMap;

#[derive(serde::Deserialize)]
pub struct DateQuery { date: Option<String> }

type Snapshot = (Vec<NaiveDate>, Option<NaiveDate>, Vec<db::repo::PositionRecord>, Vec<db::repo::InstrumentRef>);

async fn snapshot(st: &AppState, q: &DateQuery) -> Result<Snapshot, AppError> {
    let dates = db::repo::position_dates(&st.pool).await?;
    let date = match &q.date {
        Some(s) => Some(s.parse::<NaiveDate>().map_err(|_| AppError::BadRequest(format!("bad date: {s}")))?),
        None => dates.first().copied(),
    };
    let rows = match date {
        Some(d) => db::repo::positions_for(&st.pool, d).await?,
        None => Vec::new(),
    };
    let refs = db::repo::refs_all(&st.pool).await?;
    Ok((dates, date, rows, refs))
}

fn ref_map(refs: &[db::repo::InstrumentRef]) -> HashMap<&str, &db::repo::InstrumentRef> {
    refs.iter().map(|r| (r.code.as_str(), r)).collect()
}

pub async fn concentration_h(State(st): State<AppState>, Query(q): Query<DateQuery>) -> Result<Json<serde_json::Value>, AppError> {
    let (dates, date, rows, refs) = snapshot(&st, &q).await?;
    let by = ref_map(&refs);
    let cons: Vec<ConPosition> = rows.iter().filter_map(|p| {
        let w = p.weight?;
        let name = p.name.clone().unwrap_or_default();
        // fund_20 is per target fund: overrides don't regroup Fonds rows
        let group = if p.asset_type == "Fonds" {
            default_issuer_group(&p.asset_type, &name)
        } else {
            by.get(p.isin.as_str())
                .and_then(|r| r.issuer_group.clone())
                .unwrap_or_else(|| default_issuer_group(&p.asset_type, &name))
        };
        Some(ConPosition { asset_type: p.asset_type.clone(), group, weight: w })
    }).collect();
    Ok(Json(serde_json::json!({
        "dates": dates,
        "date": date,
        "checks": concentration(&cons),
        "excluded_note": "Futures are excluded from issuer limits (not issuer exposure under 5/10/40); fee and order provisions are excluded.",
    })))
}

pub async fn liquidity_h(State(st): State<AppState>, Query(q): Query<DateQuery>) -> Result<Json<serde_json::Value>, AppError> {
    let (dates, date, rows, refs) = snapshot(&st, &q).await?;
    let settings = db::settings::get_settings(&st.pool).await?;
    let by = ref_map(&refs);
    let liq: Vec<LiqPosition> = rows.iter().filter_map(|p| {
        let w = p.weight?;
        let override_ = by.get(p.isin.as_str()).and_then(|r| r.liquidity_bucket.as_deref());
        Some(LiqPosition {
            weight: w,
            bucket: effective_bucket(&settings.liquidity_defaults, &p.asset_type, override_),
        })
    }).collect();
    let report = liquidity(&liq, settings.redemption_shock);
    Ok(Json(serde_json::json!({
        "dates": dates,
        "date": date,
        "buckets": report.buckets,
        "cumulative": report.cumulative,
        "negative_memo": report.negative_memo,
        "shock": settings.redemption_shock,
        "stress_status": if report.stress_ok { "ok" } else { "breach" },
    })))
}

pub async fn rates_h(State(st): State<AppState>, Query(q): Query<DateQuery>) -> Result<Json<serde_json::Value>, AppError> {
    let (dates, date, rows, refs) = snapshot(&st, &q).await?;
    let by = ref_map(&refs);
    let mut bonds = Vec::new();
    let mut total_dv01 = 0.0f64;
    let mut md_weight_sum = 0.0f64;
    let mut missing_any = false;
    for p in rows.iter().filter(|p| p.asset_type == "Obligation") {
        let r = by.get(p.isin.as_str());
        let complete = r.map(|r| r.bond_coupon_pct.is_some() && r.bond_maturity.is_some() && r.bond_coupon_freq.is_some()).unwrap_or(false);
        let metrics = match (complete, p.price, p.valuation_eur, p.weight, date) {
            (true, Some(price), Some(mv), Some(w), Some(d)) => {
                let r = r.unwrap();
                analytics::bond_metrics(price, r.bond_coupon_pct.unwrap(), r.bond_coupon_freq.unwrap() as u32, d, r.bond_maturity.unwrap())
                    .map(|m| (m, price, mv, w, r))
            }
            _ => None,
        };
        match metrics {
            Some((m, price, mv, w, r)) => {
                let dv01 = m.modified * mv * 1e-4;
                total_dv01 += dv01;
                md_weight_sum += m.modified * w;
                bonds.push(serde_json::json!({
                    "code": p.isin, "name": p.name, "missing": false,
                    "coupon_pct": r.bond_coupon_pct, "maturity": r.bond_maturity, "freq": r.bond_coupon_freq,
                    "price": price, "ytm": m.ytm, "mod_duration": m.modified, "dv01_eur": dv01, "weight": w,
                }));
            }
            None => {
                missing_any = true;
                bonds.push(serde_json::json!({ "code": p.isin, "name": p.name, "missing": true }));
            }
        }
    }
    let futures_note: Vec<String> = rows.iter()
        .filter(|p| p.asset_type == "Future")
        .map(|p| p.name.clone().unwrap_or_else(|| p.isin.clone()))
        .collect();
    Ok(Json(serde_json::json!({
        "dates": dates,
        "date": date,
        "bonds": bonds,
        "total_dv01_eur": total_dv01,
        "nav_sensitivity_100bp": md_weight_sum * 0.01,
        "futures_note": futures_note,
        "missing_any": missing_any,
    })))
}
```

- [ ] **Step 2: Add the backtest handler** — append to `crates/server/src/handlers/metrics.rs`:

```rust
pub async fn backtest(State(st): State<AppState>) -> Result<Json<serde_json::Value>, AppError> {
    let rows = db::repo::nav_rows(&st.pool).await?;
    let settings = db::settings::get_settings(&st.pool).await?;
    let nav = to_points(&rows);
    let window = settings.var_window_days as usize;
    let report = analytics::backtest(&nav, window, 0.99);
    Ok(Json(serde_json::json!({
        "window": window,
        "confidence": 0.99,
        "horizon_days": 1,
        "n_points": report.points.len(),
        "insufficient": report.points.is_empty(),
        "methods": {
            "historical": report.historical,
            "gaussian": report.gaussian,
            "cornish_fisher": report.cornish_fisher,
        },
        "series": report.points,
    })))
}
```

(Also add `analytics::backtest` to the `use analytics::{...}` list — or call it fully qualified as shown.)

- [ ] **Step 3: Register module and routes** — add `pub mod limits;` beside the other handler modules; in `routes.rs` add:

```rust
        .route("/api/metrics/concentration", get(handlers::limits::concentration_h))
        .route("/api/metrics/liquidity", get(handlers::limits::liquidity_h))
        .route("/api/metrics/rates", get(handlers::limits::rates_h))
        .route("/api/metrics/backtest", get(handlers::metrics::backtest))
```

- [ ] **Step 4: Write the integration test** — `crates/server/tests/api_limits.rs` (same `upload_req`/`get_json` helpers as `api_metrics.rs`):

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
async fn limits_and_backtest_on_sample() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx")).unwrap();
    let res = app.clone().oneshot(upload_req(&bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // concentration
    let (st, c) = get_json(&app, "/api/metrics/concentration").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(c["date"], "2026-07-24");
    let checks = c["checks"].as_array().unwrap();
    assert_eq!(checks.len(), 5);
    assert_eq!(checks[0]["check"], "issuer_10");
    // no equity is near 10% in the sample; the bond is 6.6% -> forty > 5% picks it up
    assert_eq!(checks[0]["status"], "ok");
    let forty = &checks[1];
    assert!(forty["rows"][0]["weight"].as_f64().unwrap() > 0.05);
    let fund = &checks[3];
    assert!(fund["rows"].as_array().unwrap().iter().any(|r| r["weight"].as_f64().unwrap() > 0.07)); // Eleva ~7.4%
    let dep = &checks[4];
    assert!(dep["rows"].as_array().unwrap().iter().any(|r| r["group"] == "CBLU"));

    // liquidity
    let (_, l) = get_json(&app, "/api/metrics/liquidity").await;
    let buckets = l["buckets"].as_array().unwrap();
    assert_eq!(buckets.len(), 4);
    assert_eq!(buckets[0]["bucket"], "d1");
    assert!(buckets[0]["weight"].as_f64().unwrap() > 0.5); // equities dominate
    assert!(l["stress_status"] == "ok" || l["stress_status"] == "breach");
    let cum = l["cumulative"].as_array().unwrap();
    assert!(cum[3]["weight"].as_f64().unwrap() >= cum[0]["weight"].as_f64().unwrap());

    // rates: one bond with parsed statics
    let (_, r) = get_json(&app, "/api/metrics/rates").await;
    let bonds = r["bonds"].as_array().unwrap();
    assert_eq!(bonds.len(), 1);
    assert_eq!(bonds[0]["missing"], false);
    assert!(bonds[0]["ytm"].as_f64().unwrap() > 0.0);
    let md = bonds[0]["mod_duration"].as_f64().unwrap();
    assert!(md > 4.0 && md < 9.0); // 2035 bullet
    assert!(r["total_dv01_eur"].as_f64().unwrap() > 0.0);
    assert!(!r["futures_note"].as_array().unwrap().is_empty());

    // backtest: 343 returns, window 252 -> 91 points
    let (_, b) = get_json(&app, "/api/metrics/backtest").await;
    assert_eq!(b["confidence"].as_f64().unwrap(), 0.99);
    assert_eq!(b["horizon_days"], 1);
    assert_eq!(b["insufficient"], false);
    assert_eq!(b["n_points"], 91);
    let hist = &b["methods"]["historical"];
    assert!(hist["n"].as_u64().unwrap() == 91);
    let zone = hist["zone"].as_str().unwrap();
    assert!(["green", "yellow", "red"].contains(&zone));
    assert_eq!(b["series"].as_array().unwrap().len(), 91);

    // bad date -> 400
    let (st, _) = get_json(&app, "/api/metrics/concentration?date=notadate").await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    pool.close().await;
    edb.stop().await;
}
```

- [ ] **Step 5: Run tests, expect pass**

Run: `& "$env:USERPROFILE\.cargo\bin\cargo.exe" test -p server`
Expected: all server tests PASS.

- [ ] **Step 6: Commit**

```
git add crates/server && git commit -m "feat(server): concentration, liquidity, rates and backtest endpoints"
```

---

### Task 10: Frontend — API types + Limits page

**Files:**
- Modify: `frontend/src/api.ts`
- Create: `frontend/src/pages/LimitsPage.tsx`
- Modify: `frontend/src/App.tsx`

**Interfaces:**
- Consumes: the four metrics endpoints (Task 9) and `/api/refs` (Task 8).
- Produces: TS types + fetchers used by Task 11 (`Backtest`, `RefRow`, `RefBody`, `putRef`, `getBacktest`, extended `Settings`).

- [ ] **Step 1: Extend `frontend/src/api.ts`** — append the new types and fetchers, and extend `Settings`:

```ts
export type Bucket = "d1" | "d2_7" | "d8_30" | "d30p";
export type CheckStatus = "ok" | "watch" | "breach";
export interface CheckRow { group: string; weight: number; status: CheckStatus }
export interface Check { check: string; scope_label: string; limit: number; rows: CheckRow[]; status: CheckStatus }
export interface Concentration { dates: string[]; date: string | null; checks: Check[]; excluded_note: string }
export interface BucketWeight { bucket: Bucket; weight: number }
export interface Liquidity {
  dates: string[]; date: string | null; buckets: BucketWeight[]; cumulative: BucketWeight[];
  negative_memo: number; shock: number; stress_status: "ok" | "breach";
}
export interface BondRow {
  code: string; name: string | null; missing: boolean;
  coupon_pct?: number; maturity?: string; freq?: number; price?: number;
  ytm?: number; mod_duration?: number; dv01_eur?: number; weight?: number;
}
export interface Rates {
  dates: string[]; date: string | null; bonds: BondRow[];
  total_dv01_eur: number; nav_sensitivity_100bp: number; futures_note: string[]; missing_any: boolean;
}
export interface MethodSummary {
  exceptions: number; n: number; zone: "green" | "yellow" | "red";
  kupiec_lr: number | null; kupiec_p: number | null; reject: boolean;
}
export interface BacktestPoint {
  date: string; ret: number; var_hist: number | null; var_gauss: number | null; var_cf: number | null;
  exc_hist: boolean; exc_gauss: boolean; exc_cf: boolean;
}
export interface Backtest {
  window: number; confidence: number; horizon_days: number; n_points: number; insufficient: boolean;
  methods: { historical: MethodSummary; gaussian: MethodSummary; cornish_fisher: MethodSummary };
  series: BacktestPoint[];
}
export interface RefRow {
  code: string; name: string; asset_type: string;
  effective_issuer_group: string; issuer_group_override: string | null;
  effective_bucket: Bucket; bucket_override: Bucket | null;
  bond_coupon_pct: number | null; bond_maturity: string | null; bond_coupon_freq: number | null;
  is_bond: boolean;
}
export interface RefBody {
  issuer_group: string | null; liquidity_bucket: Bucket | null;
  bond_coupon_pct: number | null; bond_maturity: string | null; bond_coupon_freq: number | null;
}

export const getConcentration = (date?: string) =>
  req<Concentration>(`/api/metrics/concentration${date ? `?date=${date}` : ""}`);
export const getLiquidity = (date?: string) =>
  req<Liquidity>(`/api/metrics/liquidity${date ? `?date=${date}` : ""}`);
export const getRates = (date?: string) =>
  req<Rates>(`/api/metrics/rates${date ? `?date=${date}` : ""}`);
export const getBacktest = () => req<Backtest>("/api/metrics/backtest");
export const getRefs = () => req<RefRow[]>("/api/refs");
export const putRef = (code: string, body: RefBody) =>
  req<unknown>(`/api/refs/${code}`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
```

And change the `Settings` interface to:

```ts
export interface Settings {
  risk_free_rate: number; var_confidence: number; var_horizon_days: number;
  var_window_days: number; var_limit: number; short_dd_max_days: number;
  liquidity_defaults: Record<string, Bucket>; redemption_shock: number;
}
```

- [ ] **Step 2: Create `frontend/src/pages/LimitsPage.tsx`:**

```tsx
import { useState } from "react";
import { getConcentration, getLiquidity, getRates, type Check, type CheckStatus } from "../api";
import EChart from "../components/EChart";
import { eur, num, pct } from "../fmt";
import { useFetch } from "../hooks";

const STATUS_LABEL: Record<CheckStatus, string> = { ok: "OK", watch: "WATCH", breach: "BREACH" };

function StatusChip({ s }: { s: CheckStatus }) {
  const cls = s === "ok" ? "pos" : s === "watch" ? "warn-badge" : "neg";
  return <span className={cls}>{STATUS_LABEL[s]}</span>;
}

function CheckCard({ c }: { c: Check }) {
  return (
    <div className="card">
      <h3>{c.scope_label} <StatusChip s={c.status} /></h3>
      {c.rows.length === 0 ? <p>No positions in scope.</p> : (
        <table className="tbl">
          <thead><tr><th>Group</th><th>Weight</th><th>vs limit {pct(c.limit, 0)}</th><th>Status</th></tr></thead>
          <tbody>
            {c.rows.map((r, i) => (
              <tr key={i}>
                <td>{r.group}</td>
                <td>{pct(r.weight)}</td>
                <td>
                  <div style={{ background: "#eee", height: 8, width: 120 }}>
                    <div style={{
                      background: r.status === "breach" ? "#c62828" : r.status === "watch" ? "#b26a00" : "#2e7d32",
                      height: 8,
                      width: Math.min(120, (r.weight / c.limit) * 120),
                    }} />
                  </div>
                </td>
                <td><StatusChip s={r.status} /></td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

const BUCKET_LABELS: Record<string, string> = {
  d1: "1 day", d2_7: "2-7 days", d8_30: "8-30 days", d30p: "> 30 days",
};

export default function LimitsPage() {
  const [date, setDate] = useState<string | undefined>(undefined);
  const conc = useFetch(() => getConcentration(date), [date]);
  const liq = useFetch(() => getLiquidity(date), [date]);
  const rates = useFetch(() => getRates(date), [date]);

  return (
    <div>
      <h2>Limits</h2>
      <div className="controls">
        <label>Snapshot:{" "}
          <select value={conc.data?.date ?? ""} onChange={(e) => setDate(e.target.value || undefined)}>
            {(conc.data?.dates ?? []).map((d) => <option key={d} value={d}>{d}</option>)}
          </select>
        </label>
      </div>

      <h3>Concentration</h3>
      {conc.error && <p className="neg">{conc.error}</p>}
      {(conc.data?.checks ?? []).map((c) => <CheckCard key={c.check} c={c} />)}
      {conc.data && <p className="kpi-sub">{conc.data.excluded_note}</p>}

      <h3>Liquidity</h3>
      {liq.error && <p className="neg">{liq.error}</p>}
      {liq.data && (
        <div className="card">
          <p>
            Redemption stress {pct(liq.data.shock, 0)} vs assets liquidatable in ≤ 7 days:{" "}
            <StatusChip s={liq.data.stress_status === "ok" ? "ok" : "breach"} />
          </p>
          <EChart option={{
            tooltip: { trigger: "axis", valueFormatter: (x) => pct(x as number) },
            legend: { data: ["Bucket", "Cumulative"] },
            xAxis: { type: "category", data: liq.data.buckets.map((b) => BUCKET_LABELS[b.bucket] ?? b.bucket) },
            yAxis: { type: "value", axisLabel: { formatter: (x: number) => pct(x, 0) } },
            series: [
              { type: "bar", name: "Bucket", color: "#1d64c2", data: liq.data.buckets.map((b) => b.weight) },
              {
                type: "line", name: "Cumulative", color: "#2e7d32",
                data: liq.data.cumulative.map((b) => b.weight),
                markLine: {
                  silent: true, symbol: "none",
                  lineStyle: { color: "#c62828", type: "dashed" },
                  data: [{ yAxis: liq.data.shock, label: { formatter: "Stress" } }],
                },
              },
            ],
            grid: { left: 55, right: 40, top: 40, bottom: 30 },
          }} />
          <p className="kpi-sub">Negative positions (payables, short cash): {pct(liq.data.negative_memo)} — shown as memo, not netted.</p>
        </div>
      )}

      <h3>Rates</h3>
      {rates.error && <p className="neg">{rates.error}</p>}
      {rates.data && (
        <div className="card">
          {rates.data.missing_any && (
            <p className="warn-badge">Some bonds lack reference data — fill coupon/maturity/frequency on the Data page.</p>
          )}
          <table className="tbl">
            <thead><tr><th>Bond</th><th>Coupon</th><th>Maturity</th><th>Price</th><th>YTM</th><th>Mod. duration</th><th>DV01 €</th><th>Weight</th></tr></thead>
            <tbody>
              {rates.data.bonds.map((b, i) => b.missing ? (
                <tr key={i}><td>{b.name ?? b.code}</td><td colSpan={7} className="neg">missing reference data</td></tr>
              ) : (
                <tr key={i}>
                  <td>{b.name ?? b.code}</td>
                  <td>{num(b.coupon_pct)}%</td>
                  <td>{b.maturity}</td>
                  <td>{num(b.price)}</td>
                  <td>{pct(b.ytm)}</td>
                  <td>{num(b.mod_duration)}</td>
                  <td>{eur(b.dv01_eur ?? null)}</td>
                  <td>{pct(b.weight)}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <p>
            Portfolio DV01: <strong>{eur(rates.data.total_dv01_eur)}</strong> · NAV sensitivity per +100bp:{" "}
            <strong>{pct(rates.data.nav_sensitivity_100bp)}</strong>
          </p>
          <p className="kpi-sub">
            Not included (no notional/CTD data in the source file): {rates.data.futures_note.join(", ") || "—"}
          </p>
        </div>
      )}
    </div>
  );
}
```

- [ ] **Step 3: Register the route** — in `frontend/src/App.tsx`, import `LimitsPage`, add `{ to: "/limits", label: "Limits" }` to `links` between "VaR / ES" and "Data", and add `<Route path="/limits" element={<LimitsPage />} />`.

- [ ] **Step 4: Type-check and build**

Run (from `frontend/`): `npm run build`
Expected: tsc + vite build succeed.

- [ ] **Step 5: Commit**

```
git add frontend && git commit -m "feat(frontend): Limits page (concentration, liquidity, rates)"
```

---

### Task 11: Frontend — back-testing section, refs editor, settings

**Files:**
- Modify: `frontend/src/pages/VarPage.tsx` (back-testing section)
- Modify: `frontend/src/pages/DataPage.tsx` (reference-data editor + settings additions)

**Interfaces:**
- Consumes: `getBacktest`, `getRefs`, `putRef`, `RefRow`, `RefBody`, `Bucket`, extended `Settings` (Task 10).

- [ ] **Step 1: Add the back-testing section to `frontend/src/pages/VarPage.tsx`** — import `getBacktest` and `num` (`import { eur, num, pct } from "../fmt";`), fetch `const bt = useFetch(() => getBacktest(), []);` inside the component, and render after the "Limit breaches" card:

```tsx
      <h3>Back-testing (1-day / 99%, window {bt.data?.window ?? "…"})</h3>
      {bt.data?.insufficient ? (
        <div className="card"><p className="warn-badge">Insufficient history for back-testing (needs more than {bt.data.window} daily returns).</p></div>
      ) : bt.data && (
        <>
          <div className="cards-row">
            {([
              ["Historical", bt.data.methods.historical],
              ["Gaussian", bt.data.methods.gaussian],
              ["Cornish-Fisher", bt.data.methods.cornish_fisher],
            ] as const).map(([title, m]) => (
              <div className="card kpi" key={title}>
                <div className="kpi-label">{title}</div>
                <div className={`kpi-value ${m.zone === "green" ? "pos" : "neg"}`}>
                  {m.exceptions}/{m.n} · {m.zone.toUpperCase()}
                </div>
                <div className="kpi-sub">
                  Kupiec p {m.kupiec_p == null ? "n/a" : num(m.kupiec_p, 3)}{m.reject ? " · model rejected" : ""}
                  {m.n < 250 ? ` · partial: ${m.n}/250` : ""}
                </div>
              </div>
            ))}
          </div>
          <div className="card">
            <h3>Daily returns vs −VaR (exceptions marked)</h3>
            <EChart option={{
              tooltip: { trigger: "axis", valueFormatter: (x) => pct(x as number) },
              legend: { data: ["Return", "−VaR hist", "−VaR gauss", "−VaR CF"] },
              xAxis: { type: "category", data: bt.data.series.map((p) => p.date) },
              yAxis: { type: "value", axisLabel: { formatter: (x: number) => pct(x, 1) } },
              series: [
                { type: "line", name: "Return", showSymbol: false, color: "#607d8b", data: bt.data.series.map((p) => p.ret) },
                { type: "line", name: "−VaR hist", showSymbol: false, color: "#b26a00", data: bt.data.series.map((p) => p.var_hist == null ? null : -p.var_hist) },
                { type: "line", name: "−VaR gauss", showSymbol: false, color: "#1d64c2", data: bt.data.series.map((p) => p.var_gauss == null ? null : -p.var_gauss) },
                { type: "line", name: "−VaR CF", showSymbol: false, color: "#6a1b9a", data: bt.data.series.map((p) => p.var_cf == null ? null : -p.var_cf) },
                {
                  type: "scatter", name: "Exception", color: "#c62828", symbolSize: 8,
                  data: bt.data.series.filter((p) => p.exc_hist || p.exc_gauss || p.exc_cf).map((p) => [p.date, p.ret]),
                },
              ],
              grid: { left: 55, right: 40, top: 40, bottom: 30 },
            }} />
          </div>
        </>
      )}
```

- [ ] **Step 2: Add the reference-data editor to `frontend/src/pages/DataPage.tsx`** — import `getRefs`, `putRef`, and types; add `const refs = useFetch(() => getRefs(), []);` in `DataPage` and render `<RefsCard rows={refs.data} onSaved={refs.reload} />` between the settings card and the portfolio snapshot; append the component:

```tsx
function RefsCard({ rows, onSaved }: { rows: import("../api").RefRow[] | null; onSaved: () => void }) {
  const [msg, setMsg] = useState<string | null>(null);
  const [drafts, setDrafts] = useState<Record<string, Partial<import("../api").RefBody>>>({});
  if (!rows) return <div className="card"><h3>Reference data</h3><p>Loading…</p></div>;

  const draftFor = (code: string) => drafts[code] ?? {};
  const setDraft = (code: string, patch: Partial<import("../api").RefBody>) =>
    setDrafts((d) => ({ ...d, [code]: { ...draftFor(code), ...patch } }));

  async function save(r: import("../api").RefRow) {
    const d = draftFor(r.code);
    const body: import("../api").RefBody = {
      issuer_group: d.issuer_group !== undefined ? d.issuer_group : r.issuer_group_override,
      liquidity_bucket: d.liquidity_bucket !== undefined ? d.liquidity_bucket : r.bucket_override,
      bond_coupon_pct: d.bond_coupon_pct !== undefined ? d.bond_coupon_pct : r.bond_coupon_pct,
      bond_maturity: d.bond_maturity !== undefined ? d.bond_maturity : r.bond_maturity,
      bond_coupon_freq: d.bond_coupon_freq !== undefined ? d.bond_coupon_freq : r.bond_coupon_freq,
    };
    try {
      await putRef(r.code, body);
      setDrafts((prev) => { const rest = { ...prev }; delete rest[r.code]; return rest; });
      setMsg(`Saved ${r.code}.`);
      onSaved();
    } catch (e) {
      const ae = e as ApiError;
      setMsg(`Error: ${ae.detail ?? ae.message}`);
    }
  }

  async function reset(r: import("../api").RefRow) {
    try {
      await putRef(r.code, { issuer_group: null, liquidity_bucket: null, bond_coupon_pct: null, bond_maturity: null, bond_coupon_freq: null });
      setDrafts((prev) => { const rest = { ...prev }; delete rest[r.code]; return rest; });
      setMsg(`Reset ${r.code} to defaults.`);
      onSaved();
    } catch (e) {
      const ae = e as ApiError;
      setMsg(`Error: ${ae.detail ?? ae.message}`);
    }
  }

  return (
    <div className="card">
      <h3>Reference data</h3>
      <p className="kpi-sub">
        Issuer groups drive the concentration checks (merge connected issuers by giving them the same group);
        buckets drive the liquidity view; bond fields drive YTM/duration. Blank override = default.
      </p>
      {msg && <p>{msg}</p>}
      <table className="tbl">
        <thead><tr><th>Code</th><th>Name</th><th>Type</th><th>Issuer group</th><th>Bucket</th><th>Coupon %</th><th>Maturity</th><th>Freq</th><th></th></tr></thead>
        <tbody>
          {rows.map((r) => {
            const d = draftFor(r.code);
            const dirty = Object.keys(d).length > 0;
            const overridden = r.issuer_group_override != null || r.bucket_override != null;
            return (
              <tr key={r.code}>
                <td>{r.code}</td>
                <td>{r.name}</td>
                <td>{r.asset_type}</td>
                <td>
                  <input
                    value={d.issuer_group !== undefined ? (d.issuer_group ?? "") : (r.issuer_group_override ?? "")}
                    placeholder={r.effective_issuer_group}
                    onChange={(e) => setDraft(r.code, { issuer_group: e.target.value || null })}
                  />
                </td>
                <td>
                  <select
                    value={d.liquidity_bucket !== undefined ? (d.liquidity_bucket ?? "") : (r.bucket_override ?? "")}
                    onChange={(e) => setDraft(r.code, { liquidity_bucket: (e.target.value || null) as import("../api").Bucket | null })}
                  >
                    <option value="">default ({r.effective_bucket})</option>
                    {["d1", "d2_7", "d8_30", "d30p"].map((b) => <option key={b} value={b}>{b}</option>)}
                  </select>
                </td>
                {r.is_bond ? (
                  <>
                    <td><input type="number" step="0.001" style={{ width: 70 }}
                      value={d.bond_coupon_pct !== undefined ? (d.bond_coupon_pct ?? "") : (r.bond_coupon_pct ?? "")}
                      onChange={(e) => setDraft(r.code, { bond_coupon_pct: e.target.value === "" ? null : Number(e.target.value) })} /></td>
                    <td><input type="date"
                      value={d.bond_maturity !== undefined ? (d.bond_maturity ?? "") : (r.bond_maturity ?? "")}
                      onChange={(e) => setDraft(r.code, { bond_maturity: e.target.value || null })} /></td>
                    <td>
                      <select
                        value={d.bond_coupon_freq !== undefined ? (d.bond_coupon_freq ?? "") : (r.bond_coupon_freq ?? "")}
                        onChange={(e) => setDraft(r.code, { bond_coupon_freq: e.target.value === "" ? null : Number(e.target.value) })}
                      >
                        <option value="">—</option>
                        <option value="1">annual</option>
                        <option value="2">semi-annual</option>
                      </select>
                    </td>
                  </>
                ) : (
                  <><td>—</td><td>—</td><td>—</td></>
                )}
                <td>
                  <button disabled={!dirty} onClick={() => void save(r)}>Save</button>
                  {(overridden || r.bond_coupon_pct != null) && (
                    <button onClick={() => void reset(r)}>Reset</button>
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
```

Note: `reset` clears bond statics too; after a reset, re-importing the file re-seeds parsed bond values (by design).

- [ ] **Step 3: Extend the SettingsCard** in `DataPage.tsx` — add inside the `.controls` div, before the Save button:

```tsx
        <label>Redemption stress % <input type="number" step="5" value={(s.redemption_shock * 100).toFixed(0)}
          onChange={(e) => set({ redemption_shock: Number(e.target.value) / 100 })} /></label>
```

And after the `.controls` div (still inside the card), the liquidity defaults editor:

```tsx
      <h4>Liquidity defaults by asset type</h4>
      <div className="controls">
        {Object.entries(s.liquidity_defaults).map(([atype, bucket]) => (
          <label key={atype}>{atype}{" "}
            <select value={bucket} onChange={(e) =>
              set({ liquidity_defaults: { ...s.liquidity_defaults, [atype]: e.target.value as import("../api").Bucket } })}>
              {["d1", "d2_7", "d8_30", "d30p"].map((b) => <option key={b} value={b}>{b}</option>)}
            </select>
          </label>
        ))}
      </div>
```

- [ ] **Step 4: Type-check and build**

Run (from `frontend/`): `npm run build`
Expected: tsc + vite build succeed.

- [ ] **Step 5: Commit**

```
git add frontend && git commit -m "feat(frontend): backtest section, reference-data editor, v2 settings"
```

---

### Task 12: README + full verification

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update README** — in the features section, add:

```markdown
- **Limits page**: UCITS concentration checks (issuer 5/10/40, connected group 20%,
  target fund 20%, deposits 20% per bank) with OK/WATCH/BREACH statuses; liquidity
  bucketing (1d / 2–7d / 8–30d / >30d) with a configurable redemption stress; bond
  YTM / modified duration / DV01 (bond futures excluded — no notional data in the file).
- **VaR back-testing**: daily 1-day/99% VaR vs realized returns for all three methods,
  Basel traffic-light zones and Kupiec proportion-of-failures test.
- **Reference data** (Data page): editable issuer groups, liquidity bucket overrides
  and bond statics (coupon/maturity/frequency, auto-parsed from position names on import).
```

- [ ] **Step 2: Run the full workspace test suite**

Run: `& "$env:USERPROFILE\.cargo\bin\cargo.exe" test --workspace`
Expected: ALL tests pass (v1's 31 + all new ones).

- [ ] **Step 3: Full production build**

Run (from repo root): `.\build.ps1`
Expected: npm build + release cargo build succeed (frontend embedded in exe).

- [ ] **Step 4: Commit**

```
git add README.md && git commit -m "docs: v2 features in README"
```
