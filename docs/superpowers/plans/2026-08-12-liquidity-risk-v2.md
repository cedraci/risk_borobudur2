# Liquidity Risk v2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the static liquidity-bucket model with a days-to-liquidate engine driven by Bloomberg 30-day average volume, add four redemption scenarios, and load two further CACEIS feeds (JOURSRLUX, INVJCPLUX).

**Architecture:** Days become the stored primitive. `analytics` gains four pure modules (business days, coupon schedules, the liquidity engine, flow statistics) with no database access. `ingest` gains two adapters and an authoritative-facts channel alongside the existing fill-only reference hints. `server` reshapes one endpoint and adds four. The frontend reads the new shapes.

**Tech Stack:** Rust workspace (axum 0.8, sqlx, chrono, rust_xlsxwriter, calamine), embedded PostgreSQL 17, React + TypeScript + ECharts.

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-08-12-liquidity-risk-v2-design.md` as revised in `69fb810`. Where this plan and the spec disagree, the spec wins — stop and ask.
- **Branch:** `feat/liquidity-risk-v2`, already checked out. Do not merge to `main`.
- **Never commit the repo-root sample files.** `HISINVLUX_*.csv`, `HISTOVLLUX_*.csv`, `INVXDVLUX_*.csv`, `Glossary GP CSV Headers.xlsx`, `07-08-2026 - Borobudur - NAV Recap.xlsx`, `*.docx`, `docs/*.png` and any `~$*` lock files stay untracked. Only trimmed fixtures under `crates/ingest/tests/fixtures/` are committed. `git add` specific paths — never `git add -A` or `git add .`.
- **Every commit message ends with** `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>` on its own line.
- **Stop the dev server before `cargo test`.** The embedded PostgreSQL instances collide otherwise.
- **`weight` is a fraction**, never a percentage.
- **Closed asset vocabulary:** `Action`, `Fonds`, `Obligation`, `Future`, `Cash Acc`, `Margin Acc`, `Dividendes`, `Frais provisionnés`, `Provisions ordres`.
- **Days are business days**, Monday to Friday, no holiday calendar.
- **"Signal, don't hide."** A missing input is reported with its reason and named in the coverage block. It is never silently defaulted to something that looks like an answer.
- **Frontend gate:** `cd frontend && npm run build` must be clean. There are no frontend unit tests.
- Test commands: `cargo test -p analytics`, `cargo test -p ingest`, `cargo test -p db`, `cargo test -p server`, `cargo test --workspace`.

---

## File Structure

**Created:**

| Path | Responsibility |
|---|---|
| `crates/db/migrations/0011_liquidity_v2.sql` | New columns, two new tables, retire `liquidity_bucket` |
| `crates/analytics/src/bizdays.rs` | Business-day arithmetic. Nothing else. |
| `crates/analytics/src/coupons.rs` | Coupon frequency resolution and the bond inflow schedule |
| `crates/analytics/src/flows.rs` | Observed subscription/redemption statistics |
| `crates/ingest/tests/fixtures/caceis_joursr.csv` | Synthesised JOURSRLUX fixture |
| `crates/ingest/tests/fixtures/caceis_invjcp.csv` | Synthesised INVJCPLUX fixture |
| `crates/analytics/tests/` — none; analytics tests stay in-module per existing convention | |
| `crates/db/tests/liquidity_v2_repo.rs` | Register, flows, and `RefFact` storage |
| `crates/server/tests/api_liquidity_v2.rs` | Response shape, scenarios, register 422s |
| `crates/server/tests/api_bloomberg_adv.rs` | ADV request scoping and due counts |
| `frontend/src/components/ShareholderRegister.tsx` | Register editor |

**Modified:**

| Path | Change |
|---|---|
| `crates/db/src/repo.rs` | `InstrumentRef` fields; `refs_all`/`refs_upsert`; `RefFact` consumption in `import_batch`; register and flow CRUD |
| `crates/db/src/settings.rs` | New keys; `liquidity_default_days` replacing `liquidity_defaults` |
| `crates/analytics/src/liquidity.rs` | Full rewrite around capacity-per-day |
| `crates/analytics/src/lib.rs` | Register the three new modules |
| `crates/ingest/src/caceis.rs` | New HISINVLUX columns; two new adapters |
| `crates/ingest/src/adapter.rs` | `RefFact`, `ShareClassFlowRow`, two `FileKind` variants, two declines |
| `crates/ingest/src/bloomberg.rs` | `build_adv_request` and ADV parsing in `parse_response` |
| `crates/server/src/handlers/limits.rs` | `liquidity_h` rewrite |
| `crates/server/src/handlers/refs.rs` | `liquidity_days` and `adv_eligible` in place of buckets |
| `crates/server/src/handlers/bloomberg.rs` | `adv_request`, `adv_due`, ADV storage on upload |
| `crates/server/src/handlers/portfolios.rs` | Register and flows handlers |
| `crates/server/src/routes.rs` | Four new routes |
| `frontend/src/api.ts` | New types and calls |
| `frontend/src/pages/LimitsPage.tsx` | New liquidity section |
| `frontend/src/pages/DataPage.tsx` | Days input, ADV columns, register, settings |
| `frontend/src/components/BloombergPanel.tsx` | ADV export button with due count |

**Rewritten wholesale:** `crates/analytics/src/liquidity.rs`. Its current 79 lines implement the bucket model this design replaces; nothing in it survives.

---

## Task 1: Migration and reference-data columns

Introduces every new column and table, and retires `liquidity_bucket`. Because dropping that column breaks three call sites, this task also mechanically converts them to days so the workspace compiles and every existing test still passes. The bucket *chart* survives as a render-time band, which the final design needs anyway.

**Files:**
- Create: `crates/db/migrations/0011_liquidity_v2.sql`
- Modify: `crates/db/src/repo.rs:489-534` (`InstrumentRef`, `refs_all`, `refs_upsert`)
- Modify: `crates/db/src/settings.rs`
- Modify: `crates/server/src/handlers/refs.rs`
- Modify: `crates/server/src/handlers/limits.rs:58-81`
- Modify: `crates/analytics/src/liquidity.rs` (add `band_of_days`, keep the rest for now)
- Test: `crates/db/tests/instrument_refs.rs`

**Interfaces:**
- Produces: `db::repo::InstrumentRef` with `liquidity_days: Option<f64>`, `adv_30d: Option<f64>`, `adv_asof: Option<NaiveDate>`, `market_place: Option<String>`, `market_place_name: Option<String>`, `bond_next_coupon: Option<NaiveDate>`, `bond_nominal: Option<f64>`, `adv_eligible: Option<bool>`; `liquidity_bucket` removed.
- Produces: `analytics::band_of_days(days: f64) -> usize` returning `0..=3`, and `analytics::BUCKET_ORDER: [&str; 4]` unchanged.
- Produces: `db::settings::AppSettings::liquidity_default_days: serde_json::Value` replacing `liquidity_defaults`, plus `participation_rate`, `adv_stress_factor`, `liquidity_horizon_days`, `settlement_deadline_days`, `adv_max_age_days`, `flow_lookback_days`.

- [ ] **Step 1: Write the migration**

Create `crates/db/migrations/0011_liquidity_v2.sql`:

```sql
ALTER TABLE instrument_refs
    ADD COLUMN adv_30d           NUMERIC,
    ADD COLUMN adv_asof          DATE,
    ADD COLUMN liquidity_days    NUMERIC,
    ADD COLUMN market_place      TEXT,
    ADD COLUMN market_place_name TEXT,
    ADD COLUMN bond_next_coupon  DATE,
    ADD COLUMN bond_nominal      NUMERIC,
    ADD COLUMN adv_eligible      BOOLEAN;

-- Carry every existing override forward at its band's conservative upper edge.
UPDATE instrument_refs SET liquidity_days = CASE liquidity_bucket
    WHEN 'd1' THEN 1 WHEN 'd2_7' THEN 7 WHEN 'd8_30' THEN 30 WHEN 'd30p' THEN 60 END
    WHERE liquidity_bucket IS NOT NULL;

ALTER TABLE instrument_refs DROP COLUMN liquidity_bucket;

ALTER TABLE instrument_refs ADD CONSTRAINT instrument_refs_liquidity_days_nonneg
    CHECK (liquidity_days IS NULL OR liquidity_days >= 0);
ALTER TABLE instrument_refs ADD CONSTRAINT instrument_refs_adv_nonneg
    CHECK (adv_30d IS NULL OR adv_30d >= 0);
ALTER TABLE instrument_refs ADD CONSTRAINT instrument_refs_bond_nominal_pos
    CHECK (bond_nominal IS NULL OR bond_nominal > 0);

CREATE TABLE shareholders (
    id           BIGSERIAL PRIMARY KEY,
    portfolio_id BIGINT  NOT NULL REFERENCES portfolios(id),
    label        TEXT    NOT NULL,
    pct_of_nav   NUMERIC NOT NULL CHECK (pct_of_nav > 0 AND pct_of_nav <= 100),
    as_of        DATE    NOT NULL
);
CREATE INDEX shareholders_portfolio_idx ON shareholders (portfolio_id);

CREATE TABLE share_class_flows (
    portfolio_id        BIGINT  NOT NULL REFERENCES portfolios(id),
    flow_date           DATE    NOT NULL,
    share_class         TEXT    NOT NULL,
    outstanding_shares  NUMERIC,
    nav_per_share       NUMERIC,
    subscription_amount NUMERIC NOT NULL,
    redemption_amount   NUMERIC NOT NULL,
    PRIMARY KEY (portfolio_id, flow_date, share_class)
);
```

- [ ] **Step 2: Write the failing repo test**

Append to `crates/db/tests/instrument_refs.rs` (follow the existing harness in that file for pool setup):

```rust
#[tokio::test]
async fn liquidity_days_replaces_bucket_and_new_columns_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let pg = db::embedded::start(dir.path()).await.unwrap();
    let pool = pg.pool.clone();

    let r = db::repo::InstrumentRef {
        code: "FR0000121014".into(),
        issuer_group: Some("LVMH".into()),
        liquidity_days: Some(3.5),
        adv_eligible: Some(false),
        bond_coupon_pct: None,
        bond_maturity: None,
        bond_coupon_freq: None,
        bond_next_coupon: None,
        bond_nominal: None,
        adv_30d: None,
        adv_asof: None,
        market_place: None,
        market_place_name: None,
        country_of_risk: None,
        region: None,
        gics_sector: None,
        gics_industry: None,
        ticker: None,
    };
    db::repo::refs_upsert(&pool, &r).await.unwrap();

    let back = db::repo::refs_all(&pool).await.unwrap();
    let got = back.iter().find(|x| x.code == "FR0000121014").unwrap();
    assert_eq!(got.liquidity_days, Some(3.5));
    assert_eq!(got.adv_eligible, Some(false));
    // refs_upsert never writes depositary- or Bloomberg-owned columns.
    assert_eq!(got.adv_30d, None);
    assert_eq!(got.market_place, None);
}
```

- [ ] **Step 3: Run it to confirm it fails**

Run: `cargo test -p db --test instrument_refs`
Expected: compile error — `InstrumentRef` has no field `liquidity_days`.

- [ ] **Step 4: Update `InstrumentRef`, `refs_all` and `refs_upsert`**

In `crates/db/src/repo.rs`, replace the struct at line 489 and the two functions:

```rust
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct InstrumentRef {
    pub code: String,
    pub issuer_group: Option<String>,
    /// Per-instrument days-to-liquidate override. NULL = asset-type default.
    pub liquidity_days: Option<f64>,
    /// User override of the derived venue rule. NULL = derive.
    pub adv_eligible: Option<bool>,
    pub bond_coupon_pct: Option<f64>,
    pub bond_maturity: Option<NaiveDate>,
    pub bond_coupon_freq: Option<i32>,
    // Depositary-maintained (HISINVLUX / INVJCPLUX), overwritten on import.
    pub bond_next_coupon: Option<NaiveDate>,
    pub bond_nominal: Option<f64>,
    pub market_place: Option<String>,
    pub market_place_name: Option<String>,
    // Bloomberg-maintained, written only by the ADV response upload.
    pub adv_30d: Option<f64>,
    pub adv_asof: Option<NaiveDate>,
    pub country_of_risk: Option<String>,
    pub region: Option<String>,
    pub gics_sector: Option<String>,
    pub gics_industry: Option<String>,
    pub ticker: Option<String>,
}

pub async fn refs_all(pool: &PgPool) -> anyhow::Result<Vec<InstrumentRef>> {
    Ok(sqlx::query_as(
        "SELECT code, issuer_group,
                liquidity_days::float8 AS liquidity_days, adv_eligible,
                bond_coupon_pct::float8 AS bond_coupon_pct, bond_maturity, bond_coupon_freq,
                bond_next_coupon, bond_nominal::float8 AS bond_nominal,
                market_place, market_place_name,
                adv_30d::float8 AS adv_30d, adv_asof,
                country_of_risk, region, gics_sector, gics_industry, ticker
         FROM instrument_refs ORDER BY code",
    )
    .fetch_all(pool)
    .await?)
}

/// User-owned fields only. The depositary columns (`market_place`,
/// `bond_next_coupon`, `bond_nominal`) and the Bloomberg columns (`adv_30d`,
/// `adv_asof`) are deliberately absent: an editor save must never blank data
/// the import or the terminal owns.
pub async fn refs_upsert(pool: &PgPool, r: &InstrumentRef) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO instrument_refs
           (code, issuer_group, liquidity_days, adv_eligible,
            bond_coupon_pct, bond_maturity, bond_coupon_freq, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, now())
         ON CONFLICT (code) DO UPDATE SET
           issuer_group     = EXCLUDED.issuer_group,
           liquidity_days   = EXCLUDED.liquidity_days,
           adv_eligible     = EXCLUDED.adv_eligible,
           bond_coupon_pct  = EXCLUDED.bond_coupon_pct,
           bond_maturity    = EXCLUDED.bond_maturity,
           bond_coupon_freq = EXCLUDED.bond_coupon_freq,
           updated_at = now()",
    )
    .bind(&r.code).bind(&r.issuer_group).bind(r.liquidity_days).bind(r.adv_eligible)
    .bind(r.bond_coupon_pct).bind(r.bond_maturity).bind(r.bond_coupon_freq)
    .execute(pool)
    .await?;
    Ok(())
}
```

- [ ] **Step 5: Add `band_of_days` to analytics**

In `crates/analytics/src/liquidity.rs`, add below `BUCKET_ORDER`:

```rust
/// Index into `BUCKET_ORDER` for a days figure. Bands are closed at the
/// upper edge: 1 day is `d1`, 7 days is `d2_7`, 30 days is `d8_30`.
pub fn band_of_days(days: f64) -> usize {
    if days <= 1.0 { 0 } else if days <= 7.0 { 1 } else if days <= 30.0 { 2 } else { 3 }
}
```

- [ ] **Step 6: Convert the settings map to days**

In `crates/db/src/settings.rs`, replace `liquidity_defaults` and add the new keys:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppSettings {
    pub risk_free_rate: f64,
    pub var_confidence: f64,
    pub var_horizon_days: u32,
    pub var_window_days: u32,
    pub var_limit: f64,
    pub short_dd_max_days: u32,
    #[serde(default = "default_liquidity_default_days")]
    pub liquidity_default_days: serde_json::Value,
    #[serde(default = "default_redemption_shock")]
    pub redemption_shock: f64,
    #[serde(default = "default_participation_rate")]
    pub participation_rate: f64,
    #[serde(default = "default_adv_stress_factor")]
    pub adv_stress_factor: f64,
    #[serde(default = "default_liquidity_horizon_days")]
    pub liquidity_horizon_days: u32,
    #[serde(default = "default_settlement_deadline_days")]
    pub settlement_deadline_days: u32,
    #[serde(default = "default_adv_max_age_days")]
    pub adv_max_age_days: u32,
    #[serde(default = "default_flow_lookback_days")]
    pub flow_lookback_days: u32,
}

pub fn default_liquidity_default_days() -> serde_json::Value {
    serde_json::json!({
        "Action": 1, "Fonds": 7, "Obligation": 30, "Future": 1,
        "Dividendes": 1, "Frais provisionnés": 1, "Provisions ordres": 1
    })
}

fn default_redemption_shock() -> f64 { 0.30 }
fn default_participation_rate() -> f64 { 0.25 }
fn default_adv_stress_factor() -> f64 { 0.30 }
fn default_liquidity_horizon_days() -> u32 { 60 }
fn default_settlement_deadline_days() -> u32 { 3 }
fn default_adv_max_age_days() -> u32 { 7 }
fn default_flow_lookback_days() -> u32 { 250 }

/// A pre-v2 database stores `liquidity_defaults`, a map of asset type to
/// bucket name. Map it forward at each band's upper edge rather than
/// silently reverting a portfolio to code defaults.
fn days_from_legacy_buckets(v: &serde_json::Value) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for (k, b) in v.as_object().into_iter().flatten() {
        let days = match b.as_str() {
            Some("d1") => 1, Some("d2_7") => 7, Some("d8_30") => 30, Some("d30p") => 60,
            _ => continue,
        };
        // Cash and margin accounts are capacity-infinite by engine rule, not
        // by table entry, so they are dropped rather than carried at 1 day.
        if k == "Cash Acc" || k == "Margin Acc" { continue; }
        out.insert(k.clone(), serde_json::json!(days));
    }
    serde_json::Value::Object(out)
}
```

and inside `get_settings`, replace the `liquidity_defaults` lookup:

```rust
    let liquidity_default_days = rows.iter().find(|(key, _)| key == "liquidity_default_days")
        .map(|(_, v)| v.clone())
        .or_else(|| rows.iter().find(|(key, _)| key == "liquidity_defaults")
            .map(|(_, v)| days_from_legacy_buckets(v)))
        .unwrap_or_else(default_liquidity_default_days);
```

and in the returned struct add the six new fields via `get_f` / `get_u` with the defaults above. In `put_settings`, replace the `liquidity_defaults` pair with `("liquidity_default_days", s.liquidity_default_days.clone())` and append the six new pairs.

- [ ] **Step 7: Convert the two call sites**

In `crates/server/src/handlers/refs.rs`, replace `BUCKETS` and `effective_bucket`:

```rust
/// Effective days-to-liquidate: override, else asset-type default, else 1.
pub fn effective_days(defaults: &serde_json::Value, asset_type: &str, override_: Option<f64>) -> f64 {
    override_
        .or_else(|| defaults.get(asset_type).and_then(|v| v.as_f64()))
        .unwrap_or(1.0)
}
```

Change `RefRow`'s `effective_bucket`/`bucket_override` to `effective_days: f64` / `days_override: Option<f64>`, and add the read-only display fields `adv_30d: Option<f64>`, `adv_asof: Option<NaiveDate>`, `adv_eligible: Option<bool>`, `market_place_name: Option<String>`.

Change `RefBody`'s `liquidity_bucket: Option<String>` to `liquidity_days: Option<f64>` plus `adv_eligible: Option<bool>`, and mark it:

```rust
#[derive(serde::Deserialize)]
// The spec requires the depositary and Bloomberg columns to be *rejected* in
// the body, not silently dropped. Serde ignores unknown fields by default,
// which would let a client believe it had written adv_30d.
#[serde(deny_unknown_fields)]
pub struct RefBody { /* ... */ }
```

Replace the bucket validation in `put` with:

```rust
    if let Some(d) = b.liquidity_days {
        if !(0.0..=3650.0).contains(&d) || !d.is_finite() {
            return Err(AppError::Unprocessable("liquidity_days must be in [0, 3650]".into()));
        }
    }
```

and widen the frequency check, which currently rejects the quarterly and monthly payers the coupon schedule supports:

```rust
    if let Some(f) = b.bond_coupon_freq {
        if ![1, 2, 4, 12].contains(&f) {
            return Err(AppError::Unprocessable("bond_coupon_freq must be 1, 2, 4 or 12".into()));
        }
    }
```

Build the `InstrumentRef` in `put` with every new field set to `None` except `liquidity_days` and `adv_eligible`, which come from the body.

In `crates/server/src/handlers/limits.rs`, change the import at line 2 from `use crate::handlers::refs::effective_bucket;` to `use crate::handlers::refs::effective_days;`, then change `liquidity_h`'s mapping to produce a bucket from days so the existing response shape and its test survive this task unchanged:

```rust
        let days = effective_days(&settings.liquidity_default_days, &p.asset_type,
                                  by.get(p.isin.as_str()).and_then(|r| r.liquidity_days));
        Some(LiqPosition { weight: w, bucket: analytics::BUCKET_ORDER[analytics::band_of_days(days)].into() })
```

- [ ] **Step 8: Run the full suite**

Run: `cargo test --workspace`
Expected: PASS. `api_limits`, `api_refs` and `api_settings` may need their JSON field names updated from `bucket_override` to `days_override` and from `liquidity_defaults` to `liquidity_default_days` — update the assertions, not the behaviour.

- [ ] **Step 9: Keep the frontend compiling**

In `frontend/src/api.ts` change `RefRow`'s `effective_bucket: Bucket; bucket_override: Bucket | null` to `effective_days: number; days_override: number | null; adv_eligible: boolean | null`, `RefBody`'s `liquidity_bucket` to `liquidity_days: number | null` plus `adv_eligible: boolean | null`, and `Settings.liquidity_defaults` to `liquidity_default_days: Record<string, number>`. In `DataPage.tsx` replace the bucket `<select>` at line 287 with `<input type="number" min={0} step={0.5}>` bound to `days_override`, and the settings map at line 203 likewise. Full editor work lands in Task 18; this step only keeps the build green.

Run: `cd frontend && npm run build`
Expected: clean.

- [ ] **Step 10: Commit**

```bash
git add crates/db/migrations/0011_liquidity_v2.sql crates/db/src/repo.rs crates/db/src/settings.rs crates/db/tests/instrument_refs.rs crates/analytics/src/liquidity.rs crates/server/src/handlers/refs.rs crates/server/src/handlers/limits.rs crates/server/tests frontend/src/api.ts frontend/src/pages/DataPage.tsx
git commit -m "$(cat <<'EOF'
feat(db): liquidity v2 schema — days primitive, ADV columns, register and flow tables

Retires instrument_refs.liquidity_bucket in favour of liquidity_days,
backfilled at each band's conservative upper edge. Adds the depositary-owned
columns (market place, next coupon, nominal), the Bloomberg-owned ADV pair,
and the adv_eligible override. Creates shareholders and share_class_flows.

refs_upsert deliberately writes only user-owned fields, so an editor save
cannot blank what the import or the terminal owns.

Settings gain the six v2 keys and liquidity_default_days, which maps a
pre-v2 liquidity_defaults bucket map forward rather than reverting a
portfolio to code defaults.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Business-day arithmetic

The smallest unit in the design, and everything dated depends on it. One file, no dependencies beyond `chrono`.

**Files:**
- Create: `crates/analytics/src/bizdays.rs`
- Modify: `crates/analytics/src/lib.rs`

**Interfaces:**
- Produces: `analytics::is_business_day(NaiveDate) -> bool`
- Produces: `analytics::business_days_between(from: NaiveDate, to: NaiveDate) -> u32` — counts business days in `(from, to]`, i.e. `from` exclusive and `to` inclusive, returning 0 when `to <= from`. This is the offset at which a cash flow dated `to` is credited relative to a snapshot dated `from`.

- [ ] **Step 1: Write the failing test**

Create `crates/analytics/src/bizdays.rs` containing only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, day).unwrap() }

    #[test]
    fn weekends_are_not_business_days() {
        assert!(is_business_day(d(2026, 8, 7)));   // Friday
        assert!(!is_business_day(d(2026, 8, 8)));  // Saturday
        assert!(!is_business_day(d(2026, 8, 9)));  // Sunday
        assert!(is_business_day(d(2026, 8, 10)));  // Monday
    }

    #[test]
    fn offset_skips_the_weekend() {
        // From Friday, the next business day is Monday: offset 1, not 3.
        assert_eq!(business_days_between(d(2026, 8, 7), d(2026, 8, 10)), 1);
        assert_eq!(business_days_between(d(2026, 8, 7), d(2026, 8, 14)), 5);
    }

    #[test]
    fn same_day_and_past_dates_are_zero() {
        assert_eq!(business_days_between(d(2026, 8, 7), d(2026, 8, 7)), 0);
        assert_eq!(business_days_between(d(2026, 8, 7), d(2026, 8, 1)), 0);
    }

    #[test]
    fn the_real_bond_coupon_offset() {
        // Brazil 6.625% 2035 pays 2026-09-15; the sample snapshot is
        // 2026-08-07. Inside the default 60-business-day horizon.
        assert_eq!(business_days_between(d(2026, 8, 7), d(2026, 9, 15)), 27);
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Add `pub mod bizdays;` and `pub use bizdays::*;` to `crates/analytics/src/lib.rs`, then run:
`cargo test -p analytics bizdays`
Expected: compile error — `is_business_day` not found.

- [ ] **Step 3: Write the implementation**

Prepend to `crates/analytics/src/bizdays.rs`:

```rust
//! Business days, Monday to Friday. No holiday calendar — a deliberate
//! simplification stated in the UI parameters strip.

use chrono::{Datelike, Duration, NaiveDate, Weekday};

pub fn is_business_day(d: NaiveDate) -> bool {
    !matches!(d.weekday(), Weekday::Sat | Weekday::Sun)
}

/// Business days in `(from, to]`. Zero when `to <= from`, so a cash flow
/// dated on or before the snapshot is never credited to a future day.
pub fn business_days_between(from: NaiveDate, to: NaiveDate) -> u32 {
    let mut n = 0;
    let mut d = from;
    while d < to {
        d += Duration::days(1);
        if is_business_day(d) { n += 1; }
    }
    n
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p analytics bizdays`
Expected: PASS, 4 tests.

- [ ] **Step 5: Commit**

Commit message subject: `feat(analytics): business-day arithmetic for the liquidity horizon`

Body: Monday to Friday, no holiday calendar. `business_days_between` counts the half-open range `(from, to]` so a flow dated on the snapshot itself is treated as already held rather than credited on a future day.

```bash
git add crates/analytics/src/bizdays.rs crates/analytics/src/lib.rs
git commit   # message as above, with the Co-Authored-By trailer
```

---

## Task 3: Coupon frequency and the bond inflow schedule

The frequency divides the coupon, so guessing it wrong scales the inflow directly. Three sources are tried in order and none of them is a default.

**Files:**
- Create: `crates/analytics/src/coupons.rs`
- Modify: `crates/analytics/src/lib.rs`

**Interfaces:**
- Consumes: `analytics::business_days_between` from Task 2.
- Produces: `analytics::infer_coupon_freq(annual_coupon: f64, accrued: f64, days_to_next_coupon: i64) -> Option<i32>`
- Produces: `analytics::CouponInput { code: String, quantity: f64, coupon_pct: Option<f64>, coupon_type: Option<String>, next_coupon: Option<NaiveDate>, maturity: Option<NaiveDate>, freq: Option<i32>, accrued_eur: Option<f64>, fx_rate: f64 }`
- Produces: `analytics::Inflow { day: u32, amount_eur: f64 }`
- Produces: `analytics::CouponGap { code: String, reason: &'static str }`
- Produces: `analytics::CouponResult { inflows: Vec<Inflow>, gaps: Vec<CouponGap> }`
- Produces: `analytics::bond_inflows(inputs: &[CouponInput], snapshot: NaiveDate, horizon_days: u32) -> CouponResult`

- [ ] **Step 1: Write the failing tests**

Create `crates/analytics/src/coupons.rs` containing only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, day).unwrap() }

    // Brazil 6.625% 15-03-35, from the real HISINVLUX sample: 2,000,000 face,
    // accrued 45,236.41 EUR, next coupon 2026-09-15, snapshot 2026-08-07,
    // market value 1,764,365.78 EUR against 2,038,460 USD local.
    const FX: f64 = 1_764_365.78 / 2_038_460.0;

    fn brazil() -> CouponInput {
        CouponInput {
            code: "US105756CL22".into(),
            quantity: 2_000_000.0,
            coupon_pct: Some(6.625),
            coupon_type: Some("FIX".into()),
            next_coupon: Some(d(2026, 9, 15)),
            maturity: Some(d(2035, 3, 15)),
            freq: None,
            accrued_eur: Some(45_236.41),
            fx_rate: FX,
        }
    }

    #[test]
    fn infers_semi_annual_from_accrued_interest() {
        let annual_eur = 2_000_000.0 * 6.625 / 100.0 * FX;
        assert_eq!(infer_coupon_freq(annual_eur, 45_236.41, 39), Some(2));
    }

    #[test]
    fn inference_survives_a_30_360_accrual() {
        // The day-count convention is not visible in the file. Recomputing the
        // same position on a 30/360 basis must not change the answer.
        let annual_eur = 2_000_000.0 * 6.625 / 100.0 * FX;
        let accrued_30_360 = annual_eur * (142.0 / 360.0);
        assert_eq!(infer_coupon_freq(annual_eur, accrued_30_360, 39), Some(2));
    }

    #[test]
    fn an_out_of_tolerance_accrual_infers_nothing() {
        // An accrual implying a 240-day period matches no standard frequency.
        let annual = 100_000.0;
        assert_eq!(infer_coupon_freq(annual, annual * (200.0 / 365.0), 40), None);
        // Degenerate inputs never guess.
        assert_eq!(infer_coupon_freq(0.0, 100.0, 40), None);
        assert_eq!(infer_coupon_freq(100_000.0, -1.0, 40), None);
    }

    #[test]
    fn credits_one_coupon_at_its_business_day_offset() {
        let r = bond_inflows(&[brazil()], d(2026, 8, 7), 60);
        assert!(r.gaps.is_empty(), "{:?}", r.gaps);
        assert_eq!(r.inflows.len(), 1);
        assert_eq!(r.inflows[0].day, 27);
        // 2,000,000 x 6.625% / 2 = 66,250 USD, converted at the position rate.
        let expected = 2_000_000.0 * 6.625 / 100.0 / 2.0 * FX;
        assert!((r.inflows[0].amount_eur - expected).abs() < 1e-6);
    }

    #[test]
    fn an_explicit_frequency_beats_the_inference() {
        let mut b = brazil();
        b.freq = Some(4);
        let r = bond_inflows(&[b], d(2026, 8, 7), 60);
        let expected = 2_000_000.0 * 6.625 / 100.0 / 4.0 * FX;
        assert!((r.inflows[0].amount_eur - expected).abs() < 1e-6);
    }

    #[test]
    fn an_unresolvable_frequency_credits_nothing_and_reports_why() {
        let mut b = brazil();
        b.accrued_eur = None;
        let r = bond_inflows(&[b], d(2026, 8, 7), 60);
        assert!(r.inflows.is_empty());
        assert_eq!(r.gaps.len(), 1);
        assert_eq!(r.gaps[0].reason, "no resolvable frequency");
    }

    #[test]
    fn a_missing_next_coupon_date_is_reported_not_reconstructed() {
        let mut b = brazil();
        b.next_coupon = None;
        let r = bond_inflows(&[b], d(2026, 8, 7), 60);
        assert!(r.inflows.is_empty());
        assert_eq!(r.gaps[0].reason, "no next coupon date");
    }

    #[test]
    fn zero_coupon_and_far_placeholder_maturity_contribute_nothing() {
        // The sample's ETCs: 0.00% coupon, CACEIS placeholder maturity 2049-12-31.
        let etc = CouponInput {
            code: "GB00B00FHZ82".into(),
            quantity: 1_000.0,
            coupon_pct: Some(0.0),
            coupon_type: Some("FIX".into()),
            next_coupon: Some(d(2049, 12, 31)),
            maturity: Some(d(2049, 12, 31)),
            freq: None,
            accrued_eur: Some(0.0),
            fx_rate: 1.0,
        };
        let r = bond_inflows(&[etc], d(2026, 8, 7), 60);
        assert!(r.inflows.is_empty());
        assert!(r.gaps.is_empty(), "a zero-coupon instrument is not a gap");
    }

    #[test]
    fn a_maturity_inside_the_horizon_redeems_the_face() {
        let b = CouponInput {
            code: "XS0000000001".into(),
            quantity: 500_000.0,
            coupon_pct: Some(0.0),
            coupon_type: Some("FIX".into()),
            next_coupon: None,
            maturity: Some(d(2026, 8, 21)),
            freq: Some(1),
            accrued_eur: Some(0.0),
            fx_rate: 1.0,
        };
        let r = bond_inflows(&[b], d(2026, 8, 7), 60);
        assert_eq!(r.inflows.len(), 1);
        assert_eq!(r.inflows[0].day, 10);
        assert!((r.inflows[0].amount_eur - 500_000.0).abs() < 1e-9);
    }

    #[test]
    fn a_monthly_payer_fits_several_coupons_in_the_horizon() {
        let b = CouponInput {
            code: "XS0000000002".into(),
            quantity: 1_200_000.0,
            coupon_pct: Some(12.0),
            coupon_type: Some("FIX".into()),
            next_coupon: Some(d(2026, 8, 20)),
            maturity: Some(d(2030, 8, 20)),
            freq: Some(12),
            accrued_eur: Some(0.0),
            fx_rate: 1.0,
        };
        let r = bond_inflows(&[b], d(2026, 8, 7), 60);
        // 2026-08-20, 2026-09-20 and 2026-10-20 all land inside 60 business days.
        assert_eq!(r.inflows.len(), 3);
        let each = 1_200_000.0 * 12.0 / 100.0 / 12.0;
        assert!(r.inflows.iter().all(|i| (i.amount_eur - each).abs() < 1e-9));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Add `pub mod coupons;` and `pub use coupons::*;` to `crates/analytics/src/lib.rs`, then run:
`cargo test -p analytics coupons`
Expected: compile error — `CouponInput` not found.

- [ ] **Step 3: Write the implementation**

Prepend to `crates/analytics/src/coupons.rs`:

```rust
//! Bond coupon and redemption inflows, built from the schedule the
//! depositary sends daily in HISINVLUX rather than reconstructed.
//!
//! The frequency divides the coupon, so a wrong guess scales the inflow
//! directly: a semi-annual bond treated as annual pays double. There is no
//! safe default, and the three sources below are tried in order.

use crate::bizdays::business_days_between;
use chrono::{Datelike, NaiveDate};

#[derive(Debug, Clone)]
pub struct CouponInput {
    pub code: String,
    /// Face amount. CACEIS quotes bond prices per 100 of the nominal, so the
    /// coupon is a percentage of this quantity directly.
    pub quantity: f64,
    pub coupon_pct: Option<f64>,
    pub coupon_type: Option<String>,
    pub next_coupon: Option<NaiveDate>,
    pub maturity: Option<NaiveDate>,
    pub freq: Option<i32>,
    pub accrued_eur: Option<f64>,
    pub fx_rate: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Inflow {
    /// Business-day offset from the snapshot date.
    pub day: u32,
    pub amount_eur: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CouponGap {
    pub code: String,
    pub reason: &'static str,
}

#[derive(Debug, Default)]
pub struct CouponResult {
    pub inflows: Vec<Inflow>,
    pub gaps: Vec<CouponGap>,
}

/// Standard coupon periods in days, paired with their frequency.
const PERIODS: [(i32, f64); 4] = [(1, 365.0), (2, 182.5), (4, 91.25), (12, 30.4167)];
const PERIOD_TOLERANCE: f64 = 0.15;

/// Infer the coupon frequency from accrued interest.
///
/// With `C` the annual coupon, `A` the accrued interest in the same currency,
/// and `g` the calendar days to the next coupon, the elapsed accrual is
/// `365 A / C` and the full period is `elapsed + g`. Snapped to the nearest
/// standard period and accepted only within 15% — wide enough to absorb the
/// day-count convention the file does not disclose, narrow enough to reject a
/// period matching nothing.
pub fn infer_coupon_freq(annual_coupon: f64, accrued: f64, days_to_next_coupon: i64) -> Option<i32> {
    if !annual_coupon.is_finite() || annual_coupon <= 0.0 { return None; }
    if !accrued.is_finite() || accrued < 0.0 { return None; }
    if days_to_next_coupon < 0 { return None; }
    let period = 365.0 * accrued / annual_coupon + days_to_next_coupon as f64;
    if !period.is_finite() || period <= 0.0 { return None; }
    PERIODS.iter()
        .map(|&(f, p)| (f, ((period - p) / p).abs()))
        .filter(|&(_, err)| err <= PERIOD_TOLERANCE)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(f, _)| f)
}

/// Add whole months, clamping the day to the target month's length.
fn add_months(d: NaiveDate, months: u32) -> Option<NaiveDate> {
    let total = d.month0() + months;
    let year = d.year() + (total / 12) as i32;
    let month = total % 12 + 1;
    let last = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 { 29 } else { 28 },
    };
    NaiveDate::from_ymd_opt(year, month, d.day().min(last))
}

fn coupon_schedule(b: &CouponInput, snapshot: NaiveDate, horizon: u32, out: &mut CouponResult) {
    let pct = b.coupon_pct.unwrap_or(0.0);
    let is_fixed = b.coupon_type.as_deref().is_some_and(|t| t.eq_ignore_ascii_case("FIX"));
    if pct <= 0.0 || !is_fixed || b.quantity <= 0.0 || !(b.fx_rate.is_finite() && b.fx_rate > 0.0) {
        return; // A zero-coupon instrument is not a gap; it simply pays nothing.
    }
    let Some(first) = b.next_coupon else {
        out.gaps.push(CouponGap { code: b.code.clone(), reason: "no next coupon date" });
        return;
    };
    let annual_eur = b.quantity * pct / 100.0 * b.fx_rate;
    let freq = b.freq.filter(|f| *f > 0).or_else(|| {
        infer_coupon_freq(annual_eur, b.accrued_eur?, (first - snapshot).num_days())
    });
    let Some(f) = freq else {
        out.gaps.push(CouponGap { code: b.code.clone(), reason: "no resolvable frequency" });
        return;
    };
    let amount = annual_eur / f as f64;
    let step = (12 / f).max(1) as u32;
    let mut date = first;
    loop {
        // A past or same-day coupon yields offset 0 and is already in the
        // position; a coupon past the horizon ends the walk.
        let day = business_days_between(snapshot, date);
        if day == 0 || day > horizon { break; }
        if b.maturity.is_some_and(|m| date > m) { break; }
        out.inflows.push(Inflow { day, amount_eur: amount });
        match add_months(date, step) {
            Some(next) if next > date => date = next,
            _ => break,
        }
    }
}

pub fn bond_inflows(inputs: &[CouponInput], snapshot: NaiveDate, horizon_days: u32) -> CouponResult {
    let mut out = CouponResult::default();
    for b in inputs {
        coupon_schedule(b, snapshot, horizon_days, &mut out);
        if let Some(m) = b.maturity {
            let day = business_days_between(snapshot, m);
            if day > 0 && day <= horizon_days && b.quantity > 0.0
                && b.fx_rate.is_finite() && b.fx_rate > 0.0
            {
                out.inflows.push(Inflow { day, amount_eur: b.quantity * b.fx_rate });
            }
        }
    }
    out
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p analytics coupons`
Expected: PASS, 9 tests.

- [ ] **Step 5: Commit**

Commit message subject: `feat(analytics): bond inflows from the depositary's own coupon schedule`

Body: HISINVLUX carries next coupon date, maturity, coupon type, rate and nominal on every position row, so the schedule is read rather than reconstructed. Coupon frequency resolves in three steps and never defaults: INVJCPLUX first, then inference from accrued interest, then no coupon and a named gap. The inference validates against the one real bond held, resolving to semi-annual within 0.3% under both ACT/365 and 30/360 accruals.

```bash
git add crates/analytics/src/coupons.rs crates/analytics/src/lib.rs
git commit   # message as above, with the Co-Authored-By trailer
```

---

## Task 4: The liquidity engine

A full rewrite of `crates/analytics/src/liquidity.rs`. The current 79 lines implement the bucket model this design replaces; only `BUCKET_ORDER`, `BucketWeight` and `band_of_days` survive.

**Files:**
- Modify (rewrite): `crates/analytics/src/liquidity.rs`

**Interfaces:**
- Consumes: `analytics::Inflow` from Task 3.
- Produces: `analytics::NON_MARKET_CODES: [&str; 4]`
- Produces: `analytics::LiqPosition { code, asset_type, valuation_eur, quantity, adv_30d, adv_stale, adv_eligible, market_place, liquidity_days, default_days }`
- Produces: `analytics::adv_eligible(p: &LiqPosition) -> bool`
- Produces: `analytics::Capacity { code, valuation_eur, capacity_eur_day: Option<f64>, days, measured, reason: Option<&'static str> }`
- Produces: `analytics::capacity(p: &LiqPosition, participation: f64, stress: f64) -> Capacity`
- Produces: `analytics::available(caps: &[Capacity], inflows: &[Inflow], negatives_eur: f64, d: u32) -> f64`
- Produces: `analytics::Waterfall { days: Option<u32>, unmet_eur: f64 }` and `analytics::waterfall(...) -> Waterfall`
- Produces: `analytics::slice_days(caps: &[Capacity], required: f64, nav: f64) -> Option<f64>`
- Produces: `analytics::AssetProfile { buckets, cumulative }` and `analytics::asset_profile(caps: &[Capacity], nav: f64) -> AssetProfile`
- Produces: `analytics::Residual { slow_share_before: f64, slow_share_after: f64 }` and `analytics::residual(...) -> Residual`

- [ ] **Step 1: Write the failing tests**

Replace the `mod tests` block in `crates/analytics/src/liquidity.rs` entirely with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn pos(code: &str, atype: &str, val: f64) -> LiqPosition {
        LiqPosition {
            code: code.into(), asset_type: atype.into(), valuation_eur: val,
            quantity: None, adv_30d: None, adv_stale: false, adv_eligible: None,
            market_place: None, liquidity_days: None, default_days: 1.0,
        }
    }

    fn measured(code: &str, val: f64, qty: f64, adv: f64) -> LiqPosition {
        LiqPosition {
            quantity: Some(qty), adv_30d: Some(adv), market_place: Some("025".into()),
            ..pos(code, "Action", val)
        }
    }

    // ---- eligibility ----

    #[test]
    fn the_venue_rule_admits_listed_etfs_and_etcs() {
        // Amundi MSCI EM Latin America, Euronext Paris, mapped Fonds.
        let mut etf = pos("LU1681045024", "Fonds", 1.0);
        etf.market_place = Some("025".into());
        assert!(adv_eligible(&etf));
        // Gold Bullion Securities, LSE, mapped Obligation by the 13x rule.
        let mut etc = pos("GB00B00FHZ82", "Obligation", 1.0);
        etc.market_place = Some("361".into());
        assert!(adv_eligible(&etc));
    }

    #[test]
    fn the_venue_rule_excludes_unlisted_funds_cash_and_futures() {
        let mut uci = pos("FR0010599399", "Fonds", 1.0);
        uci.market_place = Some("260".into());
        assert!(!adv_eligible(&uci));
        let mut internal = pos("LU1995653893", "Fonds", 1.0);
        internal.market_place = Some("999".into());
        assert!(!adv_eligible(&internal));
        let mut fut = pos("FVSU6", "Future", 1.0);
        fut.market_place = Some("FOR".into());
        assert!(!adv_eligible(&fut));
    }

    #[test]
    fn a_null_venue_degrades_to_the_asset_type_rule() {
        // NAV Recap portfolios carry no market place; behaviour is unchanged there.
        assert!(adv_eligible(&pos("FR0000121014", "Action", 1.0)));
        assert!(!adv_eligible(&pos("LU1112771255", "Fonds", 1.0)));
    }

    #[test]
    fn the_override_forces_both_directions_but_never_enables_a_future() {
        let mut uci = pos("FR0010599399", "Fonds", 1.0);
        uci.market_place = Some("260".into());
        uci.adv_eligible = Some(true);
        assert!(adv_eligible(&uci));

        let mut eq = pos("FR0000121014", "Action", 1.0);
        eq.adv_eligible = Some(false);
        assert!(!adv_eligible(&eq));

        // A margined contract's valuation is mark-to-market, not notional, so
        // volume cannot be measured against it. That is structural, not a
        // data-quality opinion, and the override does not reach it.
        let mut fut = pos("FVSU6", "Future", 1.0);
        fut.adv_eligible = Some(true);
        assert!(!adv_eligible(&fut));
    }

    // ---- capacity ----

    #[test]
    fn the_worked_example_normal_and_stressed() {
        // 500,000 shares against 100,000 ADV at 25% participation.
        let p = measured("X", 5_000_000.0, 500_000.0, 100_000.0);
        let normal = capacity(&p, 0.25, 1.0);
        assert!(normal.measured);
        assert!((normal.days - 20.0).abs() < 1e-9);
        let stressed = capacity(&p, 0.25, 0.30);
        assert!((stressed.days - 500_000.0 / (100_000.0 * 0.25 * 0.30)).abs() < 1e-9);
        assert!((stressed.days - 66.666_666_666).abs() < 1e-6);
    }

    #[test]
    fn both_paths_agree_at_the_fallback_boundary() {
        // The fallback sets capacity so that days equals the assumed figure
        // exactly, so the two paths share one arithmetic.
        let mut p = pos("Y", "Fonds", 700_000.0);
        p.default_days = 7.0;
        let c = capacity(&p, 0.25, 1.0);
        assert!(!c.measured);
        assert!((c.days - 7.0).abs() < 1e-12);
        assert!((c.capacity_eur_day.unwrap() - 100_000.0).abs() < 1e-9);
    }

    #[test]
    fn the_stress_factor_does_not_touch_the_fallback_path() {
        // A fallback days figure is already an assumption; re-stressing it
        // would stack a guess on a guess.
        let mut p = pos("Y", "Fonds", 700_000.0);
        p.default_days = 7.0;
        assert!((capacity(&p, 0.25, 0.30).days - 7.0).abs() < 1e-12);
    }

    #[test]
    fn the_instrument_override_beats_the_asset_type_default() {
        let mut p = pos("Y", "Fonds", 700_000.0);
        p.default_days = 7.0;
        p.liquidity_days = Some(35.0);
        assert!((capacity(&p, 0.25, 1.0).days - 35.0).abs() < 1e-12);
    }

    #[test]
    fn cash_is_infinite_capacity_at_zero_days() {
        for t in ["Cash Acc", "Margin Acc"] {
            let c = capacity(&pos("C", t, 1_000_000.0), 0.25, 1.0);
            assert_eq!(c.capacity_eur_day, None);
            assert_eq!(c.days, 0.0);
            assert_eq!(c.reason, None, "cash is not on the fallback path");
        }
    }

    #[test]
    fn every_fallback_names_its_reason() {
        let mut stale = measured("A", 1_000.0, 100.0, 50.0);
        stale.adv_stale = true;
        assert_eq!(capacity(&stale, 0.25, 1.0).reason, Some("stale adv"));

        let mut no_adv = measured("B", 1_000.0, 100.0, 0.0);
        no_adv.adv_30d = None;
        assert_eq!(capacity(&no_adv, 0.25, 1.0).reason, Some("no adv"));

        let mut zero_adv = measured("C", 1_000.0, 100.0, 0.0);
        zero_adv.adv_30d = Some(0.0);
        assert_eq!(capacity(&zero_adv, 0.25, 1.0).reason, Some("no adv"));

        let mut no_qty = measured("D", 1_000.0, 0.0, 50.0);
        no_qty.quantity = None;
        assert_eq!(capacity(&no_qty, 0.25, 1.0).reason, Some("no quantity"));

        let mut fut = pos("E", "Future", 1_000.0);
        fut.quantity = Some(10.0);
        fut.adv_30d = Some(5_000.0);
        assert_eq!(capacity(&fut, 0.25, 1.0).reason, Some("future"));

        let mut uci = pos("F", "Fonds", 1_000.0);
        uci.market_place = Some("260".into());
        uci.quantity = Some(10.0);
        uci.adv_30d = Some(5_000.0);
        assert_eq!(capacity(&uci, 0.25, 1.0).reason, Some("not eligible"));
    }

    // ---- availability and orderings ----

    fn book() -> Vec<Capacity> {
        // 1m of cash, 2m at 100k/day (20 days), 1m at 25k/day (40 days).
        vec![
            capacity(&pos("CASH", "Cash Acc", 1_000_000.0), 0.25, 1.0),
            capacity(&{ let mut p = pos("FAST", "Fonds", 2_000_000.0); p.default_days = 20.0; p }, 0.25, 1.0),
            capacity(&{ let mut p = pos("SLOW", "Fonds", 1_000_000.0); p.default_days = 40.0; p }, 0.25, 1.0),
        ]
    }

    #[test]
    fn availability_accumulates_and_caps_at_position_value() {
        let c = book();
        // Day 1: 1,000,000 cash + 100,000 + 25,000
        assert!((available(&c, &[], 0.0, 1) - 1_125_000.0).abs() < 1e-6);
        // Day 40: everything, capped at each position's own value.
        assert!((available(&c, &[], 0.0, 40) - 4_000_000.0).abs() < 1e-6);
        // Beyond the last position's days nothing more appears.
        assert!((available(&c, &[], 0.0, 90) - 4_000_000.0).abs() < 1e-6);
    }

    #[test]
    fn negative_positions_reduce_availability_from_day_one() {
        // The defect this design closes: payables were a memo that never
        // counted against the pass/fail test.
        let c = book();
        assert!((available(&c, &[], -500_000.0, 1) - 625_000.0).abs() < 1e-6);
        assert!((available(&c, &[], -500_000.0, 40) - 3_500_000.0).abs() < 1e-6);
    }

    #[test]
    fn inflows_land_on_their_own_day_and_not_before() {
        let c = book();
        let inf = vec![Inflow { day: 10, amount_eur: 60_000.0 }];
        assert!((available(&c, &inf, 0.0, 9) - available(&c, &[], 0.0, 9)).abs() < 1e-9);
        assert!((available(&c, &inf, 0.0, 10) - available(&c, &[], 0.0, 10) - 60_000.0).abs() < 1e-6);
    }

    #[test]
    fn waterfall_sells_the_liquid_names_hardest() {
        let c = book();
        // Need 1,500,000: cash covers 1m, the rest at 125,000/day -> day 4.
        let w = waterfall(&c, &[], 0.0, 1_500_000.0, 60);
        assert_eq!(w.days, Some(4));
        assert_eq!(w.unmet_eur, 0.0);
    }

    #[test]
    fn an_unreachable_requirement_reports_the_shortfall_not_a_pass() {
        let c = book();
        let w = waterfall(&c, &[], 0.0, 6_000_000.0, 60);
        assert_eq!(w.days, None);
        assert!((w.unmet_eur - 2_000_000.0).abs() < 1e-6);
    }

    #[test]
    fn slice_is_the_slower_ordering() {
        let c = book();
        let nav = 4_000_000.0;
        let required = 1_500_000.0;
        // f = 0.375; the 40-day position needs 0.375 x 40 = 15 days.
        let s = slice_days(&c, required, nav).unwrap();
        assert!((s - 15.0).abs() < 1e-9);
        assert!(s >= waterfall(&c, &[], 0.0, required, 60).days.unwrap() as f64);
    }

    // ---- profile and residual ----

    #[test]
    fn the_profile_distributes_weight_across_the_day_bands() {
        let c = book();
        let p = asset_profile(&c, 4_000_000.0);
        assert!((p.buckets[0].weight - 0.25).abs() < 1e-12); // cash, 0 days
        assert!((p.buckets[2].weight - 0.50).abs() < 1e-12); // 20 days
        assert!((p.buckets[3].weight - 0.25).abs() < 1e-12); // 40 days
        assert!((p.cumulative[3].weight - 1.0).abs() < 1e-12);
    }

    #[test]
    fn residual_shows_the_dilution_left_to_the_investors_who_stayed() {
        let c = book();
        // A 1,500,000 waterfall completing at day 4 sells cash first, then the
        // fast name; the slow 1m barely moves, so its share of a smaller fund
        // rises.
        let r = residual(&c, 1_500_000.0, 4_000_000.0, 4);
        assert!((r.slow_share_before - 0.25).abs() < 1e-12);
        assert!(r.slow_share_after > r.slow_share_before);
    }

    #[test]
    fn an_empty_portfolio_is_not_a_pass() {
        let w = waterfall(&[], &[], 0.0, 1_000.0, 60);
        assert_eq!(w.days, None);
        assert!((w.unmet_eur - 1_000.0).abs() < 1e-12);
        assert_eq!(slice_days(&[], 1_000.0, 0.0), None);
        let p = asset_profile(&[], 0.0);
        assert!(p.buckets.iter().all(|b| b.weight == 0.0));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p analytics liquidity`
Expected: compile errors — `LiqPosition` has no field `code`, `capacity` not found.

- [ ] **Step 3: Write the implementation**

Replace everything above the test module in `crates/analytics/src/liquidity.rs` with:

```rust
//! Days-to-liquidate. Every function here is pure and takes no database.
//!
//! Days are the primitive: bucket bands exist only as a chart axis. A
//! position's capacity per day comes from Bloomberg 30-day volume where the
//! instrument is exchange-traded and measurable, and from an assumed days
//! figure everywhere else. Both paths produce the same two numbers, so one
//! arithmetic serves the whole portfolio regardless of data quality.

use crate::coupons::Inflow;
use serde::Serialize;

pub const BUCKET_ORDER: [&str; 4] = ["d1", "d2_7", "d8_30", "d30p"];

/// CACEIS market-place codes that are not a trading venue: `FOR` is a forced
/// price (futures, cash, provisions), `260` an unlisted collective investment
/// undertaking, `999` an internal funds publication and `254` a technical
/// quotation place. Every other code in the sample is a real exchange.
pub const NON_MARKET_CODES: [&str; 4] = ["FOR", "260", "999", "254"];

/// Index into `BUCKET_ORDER`. Bands close at their upper edge: 1 day is
/// `d1`, 7 days is `d2_7`, 30 days is `d8_30`.
pub fn band_of_days(days: f64) -> usize {
    if days <= 1.0 { 0 } else if days <= 7.0 { 1 } else if days <= 30.0 { 2 } else { 3 }
}

#[derive(Debug, Clone, Serialize)]
pub struct BucketWeight { pub bucket: String, pub weight: f64 }

#[derive(Debug, Clone)]
pub struct LiqPosition {
    pub code: String,
    pub asset_type: String,
    pub valuation_eur: f64,
    pub quantity: Option<f64>,
    pub adv_30d: Option<f64>,
    /// `adv_asof` is older than `adv_max_age_days`.
    pub adv_stale: bool,
    /// User override of the venue rule. `None` = derive.
    pub adv_eligible: Option<bool>,
    pub market_place: Option<String>,
    /// Per-instrument days override.
    pub liquidity_days: Option<f64>,
    /// Asset-type default, already resolved from settings.
    pub default_days: f64,
}

fn is_cash(asset_type: &str) -> bool {
    matches!(asset_type, "Cash Acc" | "Margin Acc")
}

/// Whether Bloomberg volume is meaningful for this instrument.
///
/// Futures are excluded unconditionally, ahead of the override: a margined
/// contract's `valuation_eur` is its mark-to-market rather than its notional,
/// so `valuation_eur / quantity` is not a price that volume can be measured
/// against. That is a structural fact, not a data-quality judgement, so the
/// override does not reach it.
pub fn adv_eligible(p: &LiqPosition) -> bool {
    if p.asset_type == "Future" || is_cash(&p.asset_type) { return false; }
    if let Some(forced) = p.adv_eligible { return forced; }
    match p.market_place.as_deref() {
        // No venue data (a NAV Recap portfolio): the pre-v2 asset-type rule.
        None => p.asset_type == "Action",
        Some(m) => !NON_MARKET_CODES.contains(&m),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Capacity {
    pub code: String,
    pub valuation_eur: f64,
    /// `None` = infinite: cash and margin accounts, and any position whose
    /// resolved days figure is zero.
    pub capacity_eur_day: Option<f64>,
    pub days: f64,
    pub measured: bool,
    /// Why this position is on the fallback path. `None` when measured, and
    /// also `None` for cash, which is not a fallback but a rule.
    pub reason: Option<&'static str>,
}

/// Why an eligible-looking position still cannot be measured.
fn fallback_reason(p: &LiqPosition) -> Option<&'static str> {
    if p.asset_type == "Future" { return Some("future"); }
    if !adv_eligible(p) { return Some("not eligible"); }
    if p.adv_stale { return Some("stale adv"); }
    if !p.adv_30d.is_some_and(|a| a.is_finite() && a > 0.0) { return Some("no adv"); }
    if !p.quantity.is_some_and(|q| q.is_finite() && q > 0.0) { return Some("no quantity"); }
    None
}

/// Days-to-liquidate and euros-per-day for one position.
///
/// Only meaningful for `valuation_eur > 0`; negative positions are an
/// immediate cash need rather than something to be sold, and are handled as a
/// separate term in `available`.
pub fn capacity(p: &LiqPosition, participation: f64, stress: f64) -> Capacity {
    let infinite = |reason| Capacity {
        code: p.code.clone(), valuation_eur: p.valuation_eur,
        capacity_eur_day: None, days: 0.0, measured: false, reason,
    };
    if is_cash(&p.asset_type) { return infinite(None); }

    match fallback_reason(p) {
        None => {
            // Both non-None and positive, checked above.
            let unit_price = p.valuation_eur / p.quantity.unwrap();
            let cap = p.adv_30d.unwrap() * participation * stress * unit_price;
            if cap.is_finite() && cap > 0.0 {
                return Capacity {
                    code: p.code.clone(), valuation_eur: p.valuation_eur,
                    capacity_eur_day: Some(cap), days: p.valuation_eur / cap,
                    measured: true, reason: None,
                };
            }
            // A non-positive capacity from positive inputs means the
            // participation or stress setting is degenerate. Report it rather
            // than dividing by it.
            fallback(p, Some("no adv"))
        }
        Some(reason) => fallback(p, Some(reason)),
    }
}

fn fallback(p: &LiqPosition, reason: Option<&'static str>) -> Capacity {
    let days = p.liquidity_days.filter(|d| d.is_finite() && *d >= 0.0).unwrap_or(p.default_days);
    if !(days.is_finite() && days > 0.0) {
        // Zero assumed days is same-day liquidity, i.e. infinite capacity.
        return Capacity {
            code: p.code.clone(), valuation_eur: p.valuation_eur,
            capacity_eur_day: None, days: 0.0, measured: false, reason,
        };
    }
    Capacity {
        code: p.code.clone(), valuation_eur: p.valuation_eur,
        // Chosen so that `days` comes back out exactly: the two paths agree
        // by construction rather than by coincidence.
        capacity_eur_day: Some(p.valuation_eur / days),
        days, measured: false, reason,
    }
}

/// Cumulative euros available by business day `d`.
///
/// `negatives_eur` is the sum of negative position values (payables, negative
/// cash) and is expected to be <= 0. It applies from day 1: those are an
/// immediate call on liquidity, not a memo.
pub fn available(caps: &[Capacity], inflows: &[Inflow], negatives_eur: f64, d: u32) -> f64 {
    let sellable: f64 = caps.iter().map(|c| match c.capacity_eur_day {
        None => c.valuation_eur,
        Some(cap) => c.valuation_eur.min(cap * d as f64),
    }).sum();
    let inflow: f64 = inflows.iter().filter(|i| i.day <= d).map(|i| i.amount_eur).sum();
    sellable + inflow + negatives_eur
}

#[derive(Debug, Clone, Serialize)]
pub struct Waterfall {
    /// First business day on which `available` reaches the requirement.
    /// `None` when the horizon is never enough.
    pub days: Option<u32>,
    pub unmet_eur: f64,
}

/// Sell the liquid names hardest: the fastest the money could arrive.
pub fn waterfall(
    caps: &[Capacity], inflows: &[Inflow], negatives_eur: f64,
    required: f64, horizon: u32,
) -> Waterfall {
    for d in 1..=horizon {
        if available(caps, inflows, negatives_eur, d) >= required {
            return Waterfall { days: Some(d), unmet_eur: 0.0 };
        }
    }
    let short = required - available(caps, inflows, negatives_eur, horizon);
    Waterfall { days: None, unmet_eur: short.max(0.0) }
}

/// Every position contributes its own proportion, so composition is
/// unchanged. Always the slower of the two orderings, and deliberately blind
/// to inflows so it stays a pure property of the holdings.
pub fn slice_days(caps: &[Capacity], required: f64, nav: f64) -> Option<f64> {
    if !(nav.is_finite() && nav > 0.0) { return None; }
    let f = required / nav;
    let mut worst: Option<f64> = None;
    for c in caps.iter().filter(|c| c.valuation_eur > 0.0) {
        let d = match c.capacity_eur_day {
            None => 0.0,
            Some(cap) => f * c.valuation_eur / cap,
        };
        worst = Some(worst.map_or(d, |w: f64| w.max(d)));
    }
    worst
}

#[derive(Debug, Clone, Serialize)]
pub struct AssetProfile {
    pub buckets: Vec<BucketWeight>,
    pub cumulative: Vec<BucketWeight>,
}

/// Distribution of positive positions across the day bands, by weight.
/// Negative positions are excluded here and reported as `negative_memo`.
pub fn asset_profile(caps: &[Capacity], nav: f64) -> AssetProfile {
    let mut sums = [0.0f64; 4];
    if nav.is_finite() && nav > 0.0 {
        for c in caps.iter().filter(|c| c.valuation_eur > 0.0) {
            sums[band_of_days(c.days)] += c.valuation_eur / nav;
        }
    }
    let buckets: Vec<BucketWeight> = BUCKET_ORDER.iter().zip(sums)
        .map(|(b, w)| BucketWeight { bucket: (*b).into(), weight: w })
        .collect();
    let mut acc = 0.0;
    let cumulative = buckets.iter()
        .map(|b| { acc += b.weight; BucketWeight { bucket: b.bucket.clone(), weight: acc } })
        .collect();
    AssetProfile { buckets, cumulative }
}

#[derive(Debug, Clone, Serialize)]
pub struct Residual {
    pub slow_share_before: f64,
    pub slow_share_after: f64,
}

/// What a waterfall completing at `d_star` leaves behind.
///
/// Sales are allocated in ascending days order and capped at each position's
/// own realisable amount by that day. The reported figures are the share of
/// the fund held in positions slower than 30 days, before the redemption and
/// again against the smaller fund that remains — the dilution imposed on the
/// investors who stayed.
pub fn residual(caps: &[Capacity], required: f64, nav: f64, d_star: u32) -> Residual {
    const SLOW: f64 = 30.0;
    if !(nav.is_finite() && nav > 0.0) {
        return Residual { slow_share_before: 0.0, slow_share_after: 0.0 };
    }
    let slow_before: f64 = caps.iter()
        .filter(|c| c.valuation_eur > 0.0 && c.days > SLOW)
        .map(|c| c.valuation_eur).sum();

    let mut order: Vec<&Capacity> = caps.iter().filter(|c| c.valuation_eur > 0.0).collect();
    order.sort_by(|a, b| a.days.total_cmp(&b.days));

    let mut remaining = required;
    let mut slow_left = slow_before;
    for c in order {
        if remaining <= 0.0 { break; }
        let realisable = match c.capacity_eur_day {
            None => c.valuation_eur,
            Some(cap) => c.valuation_eur.min(cap * d_star as f64),
        };
        let sold = realisable.min(remaining).max(0.0);
        remaining -= sold;
        if c.days > SLOW { slow_left -= sold; }
    }

    let after_nav = nav - required;
    Residual {
        slow_share_before: slow_before / nav,
        slow_share_after: if after_nav > 0.0 { (slow_left.max(0.0)) / after_nav } else { 0.0 },
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p analytics liquidity`
Expected: PASS, 17 tests.

- [ ] **Step 5: Repair the temporary shim**

Task 1 left `liquidity_h` constructing the old `LiqPosition { weight, bucket }`, which no longer exists. Until Task 12 rewrites the handler properly, make it compile against the new engine by building `Capacity` values and calling `asset_profile`:

```rust
    let nav = db::repo::aum_for(&st.pool, pid, date.unwrap_or_default()).await?.unwrap_or(0.0);
    let caps: Vec<analytics::Capacity> = rows.iter().filter_map(|p| {
        let v = p.valuation_eur?;
        if v <= 0.0 { return None; }
        let r = by.get(p.isin.as_str());
        Some(analytics::capacity(&analytics::LiqPosition {
            code: p.isin.clone(), asset_type: p.asset_type.clone(), valuation_eur: v,
            quantity: p.quantity, adv_30d: None, adv_stale: false,
            adv_eligible: r.and_then(|r| r.adv_eligible), market_place: None,
            liquidity_days: r.and_then(|r| r.liquidity_days),
            default_days: effective_days(&settings.liquidity_default_days, &p.asset_type, None),
        }, settings.participation_rate, 1.0))
    }).collect();
    let profile = analytics::asset_profile(&caps, nav);
    let negative_memo: f64 = rows.iter().filter_map(|p| p.weight).filter(|w| *w < 0.0).sum();
```

and return `buckets`/`cumulative` from `profile`, keeping the existing `negative_memo`, `shock` and `stress_status` keys so `api_limits` still passes. The full response lands in Task 12.

- [ ] **Step 6: Run the workspace suite**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 7: Commit**

Commit message subject: `feat(analytics): days-to-liquidate engine replacing the bucket model`

Body: Capacity per day comes from ADV where the instrument is exchange-traded and from an assumed days figure everywhere else, with the fallback chosen so both paths yield the same days by construction. Eligibility follows the trading venue rather than the asset type, which admits the listed ETFs and ETCs an `Action`-only rule would have silently assumed days for. Futures are excluded ahead of the override because their valuation is mark-to-market rather than notional. Negative positions now reduce availability from day one, closing the defect where payables were a memo that never counted against the pass/fail test.

```bash
git add crates/analytics/src/liquidity.rs crates/server/src/handlers/limits.rs
git commit   # message as above, with the Co-Authored-By trailer
```

---

## Task 5: Observed flow statistics

**Files:**
- Create: `crates/analytics/src/flows.rs`
- Modify: `crates/analytics/src/lib.rs`

**Interfaces:**
- Produces: `analytics::FlowObs { date: NaiveDate, net_eur: f64, nav_eur: f64 }`
- Produces: `analytics::WorstOutflow { window: u32, pct_of_nav: f64 }`
- Produces: `analytics::FlowStats { n_observations: usize, from: NaiveDate, to: NaiveDate, worst: Vec<WorstOutflow> }`
- Produces: `analytics::FLOW_WINDOWS: [u32; 3]`, `analytics::MIN_FLOW_OBSERVATIONS: usize`
- Produces: `analytics::flow_stats(obs: &[FlowObs]) -> Option<FlowStats>` — `None` below the minimum observation count.

- [ ] **Step 1: Write the failing tests**

Create `crates/analytics/src/flows.rs` containing only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, NaiveDate};

    /// `n` daily observations at a constant 100m NAV, all zero net flow.
    fn series(n: usize) -> Vec<FlowObs> {
        let start = NaiveDate::from_ymd_opt(2026, 1, 5).unwrap();
        (0..n).map(|i| FlowObs {
            date: start + Duration::days(i as i64),
            net_eur: 0.0,
            nav_eur: 100_000_000.0,
        }).collect()
    }

    #[test]
    fn too_little_history_is_unavailable_not_a_number() {
        assert!(flow_stats(&series(19)).is_none());
        assert!(flow_stats(&[]).is_none());
        assert!(flow_stats(&series(20)).is_some());
    }

    #[test]
    fn the_worst_single_day_outflow_is_reported_as_a_positive_percentage() {
        let mut s = series(40);
        s[10].net_eur = -5_000_000.0;
        let st = flow_stats(&s).unwrap();
        let w1 = st.worst.iter().find(|w| w.window == 1).unwrap();
        assert!((w1.pct_of_nav - 0.05).abs() < 1e-12);
    }

    #[test]
    fn a_run_of_outflows_shows_up_in_the_longer_window() {
        let mut s = series(40);
        for i in 10..15 { s[i].net_eur = -2_000_000.0; }
        let st = flow_stats(&s).unwrap();
        let w1 = st.worst.iter().find(|w| w.window == 1).unwrap();
        let w5 = st.worst.iter().find(|w| w.window == 5).unwrap();
        assert!((w1.pct_of_nav - 0.02).abs() < 1e-12);
        assert!((w5.pct_of_nav - 0.10).abs() < 1e-12);
    }

    #[test]
    fn subscriptions_never_produce_a_negative_worst_outflow() {
        let mut s = series(40);
        for o in s.iter_mut() { o.net_eur = 1_000_000.0; }
        let st = flow_stats(&s).unwrap();
        assert!(st.worst.iter().all(|w| w.pct_of_nav == 0.0));
    }

    #[test]
    fn the_range_and_count_describe_the_history_actually_loaded() {
        let s = series(25);
        let st = flow_stats(&s).unwrap();
        assert_eq!(st.n_observations, 25);
        assert_eq!(st.from, NaiveDate::from_ymd_opt(2026, 1, 5).unwrap());
        assert_eq!(st.to, NaiveDate::from_ymd_opt(2026, 1, 29).unwrap());
    }

    #[test]
    fn observations_are_ordered_by_date_regardless_of_input_order() {
        let mut s = series(30);
        s[3].net_eur = -9_000_000.0;
        s.reverse();
        let st = flow_stats(&s).unwrap();
        assert_eq!(st.from, NaiveDate::from_ymd_opt(2026, 1, 5).unwrap());
        assert!((st.worst.iter().find(|w| w.window == 1).unwrap().pct_of_nav - 0.09).abs() < 1e-12);
    }

    #[test]
    fn a_non_positive_nav_observation_is_skipped_not_divided_by() {
        let mut s = series(30);
        s[5].nav_eur = 0.0;
        s[5].net_eur = -1_000_000.0;
        let st = flow_stats(&s).unwrap();
        assert!(st.worst.iter().all(|w| w.pct_of_nav.is_finite()));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Add `pub mod flows;` and `pub use flows::*;` to `crates/analytics/src/lib.rs`, then run:
`cargo test -p analytics flows`
Expected: compile error — `FlowObs` not found.

- [ ] **Step 3: Write the implementation**

Prepend to `crates/analytics/src/flows.rs`:

```rust
//! Observed subscription and redemption history, from JOURSRLUX.
//!
//! Windows count *loaded observations*, not calendar days: a day that was
//! never uploaded is simply absent, which is honest about what the history
//! actually contains. These numbers inform the configured shock; they never
//! set it.

use chrono::NaiveDate;
use serde::Serialize;

pub const FLOW_WINDOWS: [u32; 3] = [1, 5, 20];
/// Below this, the statistics are reported as unavailable with the count
/// rather than computed from too little history.
pub const MIN_FLOW_OBSERVATIONS: usize = 20;

#[derive(Debug, Clone)]
pub struct FlowObs {
    pub date: NaiveDate,
    /// Subscriptions less redemptions, both as magnitudes.
    pub net_eur: f64,
    /// Fund net assets on that date.
    pub nav_eur: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorstOutflow {
    pub window: u32,
    /// Positive fraction of NAV. Zero when the window never saw a net outflow.
    pub pct_of_nav: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FlowStats {
    pub n_observations: usize,
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub worst: Vec<WorstOutflow>,
}

pub fn flow_stats(obs: &[FlowObs]) -> Option<FlowStats> {
    if obs.len() < MIN_FLOW_OBSERVATIONS { return None; }
    let mut s: Vec<&FlowObs> = obs.iter().collect();
    s.sort_by_key(|o| o.date);

    let worst = FLOW_WINDOWS.iter().map(|&w| {
        let w_us = w as usize;
        let mut worst_ratio = 0.0f64;
        if s.len() >= w_us {
            for end in (w_us - 1)..s.len() {
                let start = end + 1 - w_us;
                // The window is measured against the NAV it opened with.
                let denom = s[start].nav_eur;
                if !(denom.is_finite() && denom > 0.0) { continue; }
                let sum: f64 = s[start..=end].iter().map(|o| o.net_eur).sum();
                let ratio = sum / denom;
                if ratio < worst_ratio { worst_ratio = ratio; }
            }
        }
        WorstOutflow { window: w, pct_of_nav: -worst_ratio }
    }).collect();

    Some(FlowStats {
        n_observations: s.len(),
        from: s.first().unwrap().date,
        to: s.last().unwrap().date,
        worst,
    })
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p analytics flows`
Expected: PASS, 7 tests.

- [ ] **Step 5: Commit**

Commit message subject: `feat(analytics): worst observed outflow over 1, 5 and 20 day windows`

Body: Windows count loaded observations rather than calendar days, so a day never uploaded is absent instead of silently interpolated. Below twenty observations the statistics are unavailable with their count, not a number computed from too little history.

```bash
git add crates/analytics/src/flows.rs crates/analytics/src/lib.rs
git commit   # message as above, with the Co-Authored-By trailer
```

---

## Task 6: HISINVLUX carries the statics we were discarding

Seven columns already present in every file we receive and currently dropped on the floor. `RefHint` fills NULLs only, which is right for enrichment the user may override; these are different, so they get their own explicit channel.

**Files:**
- Modify: `crates/ingest/src/adapter.rs`
- Modify: `crates/ingest/src/caceis.rs`
- Modify: `crates/db/src/repo.rs` (`import_batch`)
- Modify: `crates/ingest/tests/fixtures/caceis_hisinv.csv`
- Test: `crates/ingest/tests/caceis.rs`
- Test: `crates/db/tests/import_batch.rs`

**Interfaces:**
- Produces: `ingest::adapter::RefFact { isin: String, market_place: Option<String>, market_place_name: Option<String>, bond_maturity: Option<NaiveDate>, bond_next_coupon: Option<NaiveDate>, bond_coupon_pct: Option<f64>, bond_nominal: Option<f64>, bond_coupon_freq: Option<i32> }`
- Produces: `ingest::adapter::UniversalBatch::ref_facts: Vec<RefFact>` alongside the existing fill-only `ref_hints`.

- [ ] **Step 1: Extend the fixture**

Open `crates/ingest/tests/fixtures/caceis_hisinv.csv`. Confirm it contains a `VMOB;13101` bond row and a `VMOB;12100` listed-ETF row; if either is absent, copy a row from the real file at the repo root, trimming nothing that changes the column count. Every row must keep its existing field count. Populate on the bond row: index 49 `20350315`, 56 `100.`, 57 `20260915`, 59 `FIX`, 60 `6.625`, 63 `186`, 64 `INTERNATIONAL SECURITIES`. On the ETF row: 63 `025`, 64 `EURONEXT PARIS`. On the STRABAG equity row: 63 `050`, 64 `WIENER BOERSE`.

**The repo-root sample CSVs are never committed.** Only this fixture is.

- [ ] **Step 2: Write the failing tests**

Append to `crates/ingest/tests/caceis.rs`:

```rust
#[test]
fn hisinv_emits_depositary_statics_as_authoritative_facts() {
    let b = batch();
    let bond = b.ref_facts.iter().find(|f| f.isin == "US105756CL22").expect("bond fact");
    assert_eq!(bond.bond_maturity, chrono::NaiveDate::from_ymd_opt(2035, 3, 15));
    assert_eq!(bond.bond_next_coupon, chrono::NaiveDate::from_ymd_opt(2026, 9, 15));
    assert_eq!(bond.bond_coupon_pct, Some(6.625));
    assert_eq!(bond.bond_nominal, Some(100.0));
    // Frequency is not in HISINVLUX; it comes from INVJCPLUX or the inference.
    assert_eq!(bond.bond_coupon_freq, None);
    assert_eq!(bond.market_place.as_deref(), Some("186"));
}

#[test]
fn market_place_distinguishes_listed_from_unlisted() {
    let b = batch();
    let by = |isin: &str| b.ref_facts.iter().find(|f| f.isin == isin).cloned();
    let eq = by("AT000000STR1").expect("equity fact");
    assert_eq!(eq.market_place.as_deref(), Some("050"));
    assert_eq!(eq.market_place_name.as_deref(), Some("WIENER BOERSE"));
    // Cash, provisions and futures quote at a forced price and are not listed.
    let forced = b.ref_facts.iter().filter(|f| f.market_place.as_deref() == Some("FOR")).count();
    assert!(forced > 0, "the sample holds cash and futures rows");
}

#[test]
fn an_absent_static_is_none_not_zero() {
    let b = batch();
    let eq = b.ref_facts.iter().find(|f| f.isin == "AT000000STR1").unwrap();
    assert_eq!(eq.bond_maturity, None);
    assert_eq!(eq.bond_next_coupon, None);
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p ingest caceis`
Expected: compile error — no field `ref_facts`.

- [ ] **Step 4: Add the contract**

In `crates/ingest/src/adapter.rs`, after `RefHint`:

```rust
/// Reference data the depositary restates on every file and is authoritative
/// for. Unlike `RefHint`, these OVERWRITE: the daily feed is the source of
/// truth, and a value the user typed for an instrument CACEIS also reports is
/// superseded by the depositary's own.
///
/// Deliberately absent: `liquidity_days` and `adv_eligible` (the user's), and
/// `adv_30d` / `adv_asof` (Bloomberg's). An import never touches those.
#[derive(Debug, Clone, Default)]
pub struct RefFact {
    pub isin: String,
    pub market_place: Option<String>,
    pub market_place_name: Option<String>,
    pub bond_maturity: Option<NaiveDate>,
    pub bond_next_coupon: Option<NaiveDate>,
    pub bond_coupon_pct: Option<f64>,
    pub bond_nominal: Option<f64>,
    pub bond_coupon_freq: Option<i32>,
}
```

and add `pub ref_facts: Vec<RefFact>,` to `UniversalBatch`. Add `ref_facts: Vec::new()` to every existing construction site: `to_batch` in this file, both returns in `caceis::parse_hisinv` and `caceis::parse_histovl`.

- [ ] **Step 5: Read the columns**

In `crates/ingest/src/caceis.rs`, add the constants beside the existing ones:

```rust
const H_MATURITY: usize = 49;      // "Maturity Date"
const H_NOMINAL: usize = 56;       // "Nominal" — the denomination prices quote against
const H_NEXT_COUPON: usize = 57;   // "Next coupon date"
const H_COUPON_TYPE: usize = 59;   // "Coupon Type" — only FIX yields coupons
const H_COUPON_RATE: usize = 60;   // "Coupon rate"
const H_MARKET_PLACE: usize = 63;  // "Market place"
const H_MARKET_NAME: usize = 64;   // "Market place Description"
```

Column 46 (`Factor`) is deliberately not read: it is `0` throughout the sample and amortising-bond factors are out of scope.

Add a date helper next to `num`/`text`:

```rust
fn date(fields: &[&str], i: usize) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(field(fields, i), "%Y%m%d").ok()
}
```

and inside the row loop in `parse_hisinv`, after the `ref_hints` block:

```rust
        let coupon_type = text(&fields, H_COUPON_TYPE);
        let fixed = coupon_type.as_deref().is_some_and(|t| t.eq_ignore_ascii_case("FIX"));
        let fact = RefFact {
            isin: isin.clone(),
            market_place: text(&fields, H_MARKET_PLACE),
            market_place_name: text(&fields, H_MARKET_NAME),
            // Coupon statics only where the instrument actually carries a
            // fixed coupon; an equity row's blank columns must not write NULLs
            // over a bond's data if the same code ever appears twice.
            bond_maturity: if fixed { date(&fields, H_MATURITY) } else { None },
            bond_next_coupon: if fixed { date(&fields, H_NEXT_COUPON) } else { None },
            bond_coupon_pct: if fixed { num(&fields, H_COUPON_RATE) } else { None },
            bond_nominal: if fixed { num(&fields, H_NOMINAL).filter(|n| *n > 0.0) } else { None },
            bond_coupon_freq: None, // HISINVLUX does not carry it
        };
        if fact.market_place.is_some() || fact.bond_maturity.is_some() {
            ref_facts.push(fact);
        }
```

Declare `let mut ref_facts: Vec<RefFact> = Vec::new();` beside `ref_hints`, import `RefFact` in the `use crate::adapter::{...}` line, and return it in the `UniversalBatch`.

- [ ] **Step 6: Store them**

In `crates/db/src/repo.rs`, immediately after the existing `ref_hints` loop in `import_batch`:

```rust
    // Authoritative depositary facts: overwrite where present, leave alone
    // where this file says nothing. COALESCE(EXCLUDED, existing) rather than
    // COALESCE(existing, EXCLUDED) — the inverse of the hint loop above.
    for f in &b.ref_facts {
        sqlx::query(
            "INSERT INTO instrument_refs
               (code, market_place, market_place_name, bond_maturity,
                bond_next_coupon, bond_coupon_pct, bond_nominal, bond_coupon_freq)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (code) DO UPDATE SET
               market_place      = COALESCE(EXCLUDED.market_place,      instrument_refs.market_place),
               market_place_name = COALESCE(EXCLUDED.market_place_name, instrument_refs.market_place_name),
               bond_maturity     = COALESCE(EXCLUDED.bond_maturity,     instrument_refs.bond_maturity),
               bond_next_coupon  = COALESCE(EXCLUDED.bond_next_coupon,  instrument_refs.bond_next_coupon),
               bond_coupon_pct   = COALESCE(EXCLUDED.bond_coupon_pct,   instrument_refs.bond_coupon_pct),
               bond_nominal      = COALESCE(EXCLUDED.bond_nominal,      instrument_refs.bond_nominal),
               bond_coupon_freq  = COALESCE(EXCLUDED.bond_coupon_freq,  instrument_refs.bond_coupon_freq),
               updated_at = now()",
        )
        .bind(&f.isin).bind(&f.market_place).bind(&f.market_place_name).bind(f.bond_maturity)
        .bind(f.bond_next_coupon).bind(f.bond_coupon_pct).bind(f.bond_nominal).bind(f.bond_coupon_freq)
        .execute(&mut *tx).await?;
    }
```

This must run **after** the name-parsed bond statics block at line 126, so a depositary value wins over one guessed from an instrument name.

- [ ] **Step 7: Write the db test**

Append to `crates/db/tests/import_batch.rs` a test that imports the HISINVLUX fixture and asserts `refs_all` returns `market_place == Some("186")` and `bond_next_coupon == Some(2026-09-15)` for `US105756CL22`, and that a subsequent `refs_upsert` setting only `liquidity_days` leaves both intact.

- [ ] **Step 8: Run**

Run: `cargo test -p ingest && cargo test -p db`
Expected: PASS.

- [ ] **Step 9: Commit**

Commit message subject: `feat(ingest): read the bond schedule and market place HISINVLUX already sends`

Body: Seven columns present in every file and previously discarded. `RefFact` is a second, explicit channel alongside `RefHint`: hints fill NULLs because the user may override them, facts overwrite because the depositary restates them daily and is authoritative. Neither touches `liquidity_days`, `adv_eligible` or the ADV pair.

```bash
git add crates/ingest/src/adapter.rs crates/ingest/src/caceis.rs crates/ingest/tests crates/db/src/repo.rs crates/db/tests/import_batch.rs
git commit   # message as above, with the Co-Authored-By trailer
```

---

## Task 7: The JOURSRLUX adapter

Daily subscriptions and redemptions per share class — the one feed that turns a guessed shock into an observed one.

**Files:**
- Create: `crates/ingest/tests/fixtures/caceis_joursr.csv`
- Modify: `crates/ingest/src/adapter.rs`
- Modify: `crates/ingest/src/caceis.rs`
- Modify: `crates/db/src/repo.rs`
- Test: `crates/ingest/tests/caceis.rs`
- Test: `crates/db/tests/liquidity_v2_repo.rs` (create)

**Interfaces:**
- Consumes: `ingest::caceis::filename_meta` (its existing regex already accepts the prefix).
- Produces: `ingest::ShareClassFlowRow { flow_date: NaiveDate, share_class: String, outstanding_shares: Option<f64>, nav_per_share: Option<f64>, subscription_amount: f64, redemption_amount: f64 }` in `crates/ingest/src/lib.rs`.
- Produces: `ingest::adapter::UniversalBatch::flows: Option<Vec<ShareClassFlowRow>>` — `None` means the file says nothing about the flow journal, matching the existing `dividends`/`operations` convention.
- Produces: `ingest::caceis::parse_joursr(filename: &str, bytes: &[u8]) -> Result<UniversalBatch, ParseFailure>`
- Produces: `ingest::adapter::FileKind::CaceisJoursr`
- Produces: `db::repo::flows_upsert(pool, portfolio_id, rows: &[ShareClassFlowRow]) -> anyhow::Result<u64>` and `db::repo::flows_for(pool, portfolio_id, lookback: u32) -> anyhow::Result<Vec<FlowRecord>>`

- [ ] **Step 1: Write the fixture**

Create `crates/ingest/tests/fixtures/caceis_joursr.csv`, semicolon-delimited, headerless, 15 fields per line, in the depositary's glossary column order: fund code, NAV date, share class code, outstanding shares, NAV per share, subscription quantity, subscription amount, redemption quantity, redemption amount, balance quantity, subscription/redemption quantity, retrocession, fund currency, internal share class code, transaction nominal.

```
165878;20260807;C1;271295.542;104.04;0.;0.;1922.15;200000.;269373.392;-1922.15;0.;EUR;165878C1    ;0.
```

Add a second line for a second share class `C2` on the same date, with a subscription of `350000.` and a redemption of `0.`, so the multi-class aggregation is exercised. Match the real file's space-padding and trailing-dot number style, which the existing `num` helper already trims and parses.

- [ ] **Step 2: Write the failing tests**

Append to `crates/ingest/tests/caceis.rs`:

```rust
const JOURSR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/caceis_joursr.csv");
const JOURSR_FNAME: &str = "JOURSRLUX_165878_20260807_20260810130151.csv";

#[test]
fn joursr_reads_both_share_classes() {
    let bytes = std::fs::read(JOURSR).unwrap();
    let b = caceis::parse_joursr(JOURSR_FNAME, &bytes).expect("fixture parses");
    assert_eq!(b.primary_date, chrono::NaiveDate::from_ymd_opt(2026, 8, 7).unwrap());
    assert!(b.snapshots.is_empty(), "a flow file carries no positions");
    assert!(b.nav_points.is_empty(), "NAV history stays HISTOVLLUX's job");
    let flows = b.flows.as_ref().expect("flow journal present");
    assert_eq!(flows.len(), 2);
    let c1 = flows.iter().find(|f| f.share_class == "C1").unwrap();
    assert_eq!(c1.outstanding_shares, Some(271_295.542));
    assert_eq!(c1.nav_per_share, Some(104.04));
    assert_eq!(c1.subscription_amount, 0.0);
    assert_eq!(c1.redemption_amount, 200_000.0);
}

#[test]
fn joursr_stores_both_amounts_as_magnitudes() {
    // The depositary's sign convention for the redemption column is not
    // observable without a real file, so direction comes from which column
    // the amount sat in and never from its sign. The same file with the
    // redemption written negative must parse to the same magnitude.
    let text = String::from_utf8(std::fs::read(JOURSR).unwrap()).unwrap();
    let flipped = text.replace(";1922.15;200000.;", ";1922.15;-200000.;");
    assert_ne!(flipped, text, "the fixture's redemption amount must be present to flip");
    let b = caceis::parse_joursr(JOURSR_FNAME, flipped.as_bytes()).unwrap();
    let c1 = b.flows.unwrap().into_iter().find(|f| f.share_class == "C1").unwrap();
    assert_eq!(c1.redemption_amount, 200_000.0);
}

#[test]
fn joursr_rejects_a_mis_shaped_or_mislabelled_file() {
    let short = b"165878;20260807;C1\n";
    assert!(caceis::parse_joursr(JOURSR_FNAME, short).is_err());

    let bytes = std::fs::read(JOURSR).unwrap();
    // Filename fund code disagreeing with the rows is a routing accident, not
    // a row-level anomaly: reject the file rather than import it elsewhere.
    assert!(caceis::parse_joursr("JOURSRLUX_999999_20260807_1.csv", &bytes).is_err());
    assert!(caceis::parse_joursr("JOURSRLUX_165878_20260806_1.csv", &bytes).is_err());
}
```

Replace the `replace_redemption_with_negative()` call with an inline byte edit — read the fixture to a `String`, swap `;200000.;` for `;-200000.;`, and pass `s.as_bytes()`.

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p ingest caceis`
Expected: compile error — `parse_joursr` not found.

- [ ] **Step 4: Implement**

In `crates/ingest/src/lib.rs`, beside `NavHistoryRow`:

```rust
#[derive(Debug, Clone)]
pub struct ShareClassFlowRow {
    pub flow_date: NaiveDate,
    pub share_class: String,
    pub outstanding_shares: Option<f64>,
    pub nav_per_share: Option<f64>,
    /// Magnitudes, both non-negative. Direction comes from the column, not
    /// the sign: the depositary's convention is not observable from the
    /// glossary alone.
    pub subscription_amount: f64,
    pub redemption_amount: f64,
}
```

Add `pub flows: Option<Vec<ShareClassFlowRow>>` to `UniversalBatch` and `flows: None` at every existing construction site.

In `crates/ingest/src/caceis.rs`:

```rust
// JOURSRLUX columns (0-based), from the depositary glossary.
const R_FUND_CODE: usize = 0;
const R_NAV_DATE: usize = 1;
const R_SHARE_CLASS: usize = 2;
const R_OUTSTANDING: usize = 3;
const R_NAV_PER_SHARE: usize = 4;
const R_SUB_AMOUNT: usize = 6;
const R_RED_AMOUNT: usize = 8;
const R_MIN_FIELDS: usize = 15;

pub fn parse_joursr(filename: &str, bytes: &[u8]) -> Result<UniversalBatch, ParseFailure> {
    let (fund_code, file_date) = filename_meta(filename)
        .ok_or_else(|| ParseFailure::Workbook(format!(
            "filename {filename:?} does not match JOURSRLUX_<fund>_<yyyymmdd>_<ts>.csv")))?;

    let textual = decode_latin1(bytes);
    let mut rows: Vec<crate::ShareClassFlowRow> = Vec::new();
    for (i, line) in textual.lines().enumerate() {
        let lineno = i + 1;
        if line.trim().is_empty() { continue; }
        let fields: Vec<&str> = line.split(';').collect();
        if fields.len() < R_MIN_FIELDS {
            return Err(ParseFailure::Workbook(format!(
                "line {lineno}: {} columns, expected at least {R_MIN_FIELDS} — not a JOURSRLUX layout",
                fields.len())));
        }
        if field(&fields, R_FUND_CODE) != fund_code {
            return Err(ParseFailure::Workbook(format!(
                "line {lineno}: fund code {:?} differs from filename code {fund_code:?}",
                field(&fields, R_FUND_CODE))));
        }
        let row_date = NaiveDate::parse_from_str(field(&fields, R_NAV_DATE), "%Y%m%d")
            .map_err(|_| ParseFailure::Workbook(format!(
                "line {lineno}: bad NAV date {:?}", field(&fields, R_NAV_DATE))))?;
        if row_date != file_date {
            return Err(ParseFailure::Workbook(format!(
                "line {lineno}: row date {row_date} differs from filename date {file_date}")));
        }
        let share_class = field(&fields, R_SHARE_CLASS).to_string();
        if share_class.is_empty() {
            return Err(ParseFailure::Workbook(format!("line {lineno}: blank share class code")));
        }
        rows.push(crate::ShareClassFlowRow {
            flow_date: row_date,
            share_class,
            outstanding_shares: num(&fields, R_OUTSTANDING),
            nav_per_share: num(&fields, R_NAV_PER_SHARE),
            subscription_amount: num(&fields, R_SUB_AMOUNT).unwrap_or(0.0).abs(),
            redemption_amount: num(&fields, R_RED_AMOUNT).unwrap_or(0.0).abs(),
        });
    }
    if rows.is_empty() {
        return Err(ParseFailure::Workbook("no flow rows found".into()));
    }
    Ok(UniversalBatch {
        primary_date: file_date,
        nav_points: Vec::new(),
        snapshots: Vec::new(),
        dividends: None,
        operations: None,
        flows: Some(rows),
        ref_hints: Vec::new(),
        ref_facts: Vec::new(),
        warnings: Vec::new(),
    })
}
```

- [ ] **Step 5: Store the flows**

In `crates/db/src/repo.rs`:

```rust
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
/// its own transaction, following `seed_futures_contracts`. Callers outside a
/// transaction pass `&mut *pool.acquire().await?`.
pub async fn flows_upsert(
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

/// The most recent `lookback` distinct dates, oldest first.
pub async fn flows_for(pool: &PgPool, portfolio_id: i64, lookback: u32) -> anyhow::Result<Vec<FlowRecord>> {
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
    .bind(portfolio_id).bind(lookback as i64)
    .fetch_all(pool).await?)
}
```

Call it inside `import_batch`'s transaction body, mirroring the `dividends` block:

```rust
    if let Some(rows) = &b.flows {
        flows_upsert(&mut tx, portfolio_id, rows).await?;
        row_counts["flows"] = serde_json::json!(rows.len());
    }
```

`row_counts` is built before the `imports` INSERT, so add the `flows` count there rather than mutating it afterwards — place this block above the INSERT and bind the finished value.

- [ ] **Step 6: Write the db test**

Create `crates/db/tests/liquidity_v2_repo.rs` with a test that upserts two days of flows, re-upserts the same day with changed amounts, and asserts `flows_for` returns three rows total with the updated values — proving the same day loaded twice does not double-count.

- [ ] **Step 7: Run**

Run: `cargo test -p ingest && cargo test -p db`
Expected: PASS.

- [ ] **Step 8: Commit**

Commit message subject: `feat(ingest): JOURSRLUX subscription and redemption history`

Body: Amounts are stored as magnitudes with direction taken from the column, because the depositary's sign convention is not observable from the glossary alone. Storage is idempotent per portfolio, date and share class, so re-loading a day corrects it rather than double-counting. This is the feed that turns the configured shock into a number with an observed comparison beside it.

```bash
git add crates/ingest/src crates/ingest/tests crates/db/src/repo.rs crates/db/tests/liquidity_v2_repo.rs
git commit   # message as above, with the Co-Authored-By trailer
```

---

## Task 8: The INVJCPLUX adapter, and two files recognised but declined

INVJCPLUX confirms a number the position file already implies. It is worth loading — it removes the manual maintenance of `bond_coupon_freq` — but it is not load-bearing, and the frequency encoding is the one field we genuinely cannot predict.

**Files:**
- Create: `crates/ingest/tests/fixtures/caceis_invjcp.csv`
- Modify: `crates/ingest/src/caceis.rs`
- Modify: `crates/ingest/src/adapter.rs`
- Test: `crates/ingest/tests/caceis.rs`
- Test: `crates/server/tests/api_ingest_routing.rs`

**Interfaces:**
- Produces: `ingest::caceis::parse_invjcp(filename: &str, bytes: &[u8]) -> Result<UniversalBatch, ParseFailure>` returning only `ref_facts`.
- Produces: `ingest::adapter::FileKind::CaceisInvjcp`.

- [ ] **Step 1: Write the fixture**

Create `crates/ingest/tests/fixtures/caceis_invjcp.csv`: 36 semicolon-delimited fields in glossary order. Index 0 fund code, 2 NAV date, 3 ISIN, 15 frequency, 16 last coupon date, 17 maturity, 22 rate. Two rows: the Brazil bond with frequency `2`, and a second bond `XS9999999999` with an unrecognised frequency token `SEMI` so the warning path is covered.

- [ ] **Step 2: Write the failing tests**

Append to `crates/ingest/tests/caceis.rs`:

```rust
const INVJCP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/caceis_invjcp.csv");
const INVJCP_FNAME: &str = "INVJCPLUX_165878_20260807_20260810130151.csv";

#[test]
fn invjcp_supplies_the_coupon_frequency() {
    let bytes = std::fs::read(INVJCP).unwrap();
    let b = caceis::parse_invjcp(INVJCP_FNAME, &bytes).expect("fixture parses");
    assert!(b.snapshots.is_empty() && b.flows.is_none());
    let f = b.ref_facts.iter().find(|f| f.isin == "US105756CL22").unwrap();
    assert_eq!(f.bond_coupon_freq, Some(2));
    assert_eq!(f.bond_maturity, chrono::NaiveDate::from_ymd_opt(2035, 3, 15));
}

#[test]
fn an_unrecognised_frequency_warns_and_stays_null() {
    // Never a guess: the engine falls to the accrued-interest inference, and
    // if that is inconclusive it credits no coupon at all.
    let bytes = std::fs::read(INVJCP).unwrap();
    let b = caceis::parse_invjcp(INVJCP_FNAME, &bytes).unwrap();
    let f = b.ref_facts.iter().find(|f| f.isin == "XS9999999999").unwrap();
    assert_eq!(f.bond_coupon_freq, None);
    assert!(b.warnings.iter().any(|w| w.contains("frequency")),
            "the first real file settles the encoding; the warning is how we find out: {:?}", b.warnings);
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p ingest caceis`
Expected: compile error — `parse_invjcp` not found.

- [ ] **Step 4: Implement**

In `crates/ingest/src/caceis.rs`:

```rust
// INVJCPLUX columns (0-based), from the depositary glossary.
const J_FUND_CODE: usize = 0;
const J_NAV_DATE: usize = 2;
const J_ISIN: usize = 3;
const J_FREQ: usize = 15;
const J_MATURITY: usize = 17;
const J_RATE: usize = 22;
const J_MIN_FIELDS: usize = 36;

/// CACEIS's frequency encoding is not visible in the glossary. An integer in
/// 1..=12 is taken as given, a small set of letter codes is mapped, and
/// anything else warns and yields NULL. Nothing is ever guessed: a wrong
/// frequency scales the coupon directly.
fn coupon_freq(token: &str) -> Result<Option<i32>, ()> {
    let t = token.trim();
    if t.is_empty() { return Ok(None); }
    if let Ok(n) = t.parse::<i32>() {
        return if (1..=12).contains(&n) { Ok(Some(n)) } else { Err(()) };
    }
    match t.to_ascii_uppercase().as_str() {
        "A" | "ANNUEL" | "ANNUAL" => Ok(Some(1)),
        "T" | "TRIMESTRIEL" | "QUARTERLY" => Ok(Some(4)),
        "M" | "MENSUEL" | "MONTHLY" => Ok(Some(12)),
        _ => Err(()),
    }
}
```

`S` is deliberately absent from that map: it is ambiguous between *semestriel* (2) and *semaine*, and the two differ by a factor of 26. It falls through to the warning.

`parse_invjcp` follows `parse_joursr`'s structure — same filename, fund-code and date cross-checks, same minimum-field rejection — pushing one `RefFact` per row with `bond_coupon_freq`, `bond_maturity` (index 17) and `bond_coupon_pct` (index 22), and appending `format!("line {lineno}: unrecognised coupon frequency {token:?} for {isin} — left unset")` to `warnings` on `Err(())`.

- [ ] **Step 5: Wire the router**

In `crates/ingest/src/adapter.rs`, extend the enum and `detect`:

```rust
pub enum FileKind { NavRecap, CaceisHisinv, CaceisHistovl, CaceisJoursr, CaceisInvjcp }
```

```rust
    if lower.starts_with("joursrlux_") {
        let Some(fund_code) = caceis_meta() else {
            return Err(DetectError::Unrecognized(filename.to_string()));
        };
        if !sniff_semicolons(bytes, 15) { return Err(DetectError::Unrecognized(filename.to_string())); }
        return Ok(Identification { kind: FileKind::CaceisJoursr, fund_code: Some(fund_code) });
    }
    if lower.starts_with("invjcplux_") {
        let Some(fund_code) = caceis_meta() else {
            return Err(DetectError::Unrecognized(filename.to_string()));
        };
        if !sniff_semicolons(bytes, 36) { return Err(DetectError::Unrecognized(filename.to_string())); }
        return Ok(Identification { kind: FileKind::CaceisInvjcp, fund_code: Some(fund_code) });
    }
    if lower.starts_with("reglmtlux_") || lower.starts_with("rapdeclux_") {
        return Err(DetectError::Rejected(
            "REGLMTLUX and RAPDECLUX are recognized but not consumed yet. Everything they carry is \
             already in the snapshot under a different name — the settlement ledger's pending trades \
             are the Provisions ordres and Frais provisionnés rows, and the detached dividends are the \
             CPON positions. They add dates to amounts we already hold, so importing them without a \
             de-duplication rule written against real transaction codes would double-count the \
             liability side. Provide one sample of each and the rule can be written.".into()));
    }
```

Extend `parse` with the two new arms, and update `DetectError::Unrecognized`'s message to list the supported set: `NAV Recap (.xlsx), CACEIS HISINVLUX / HISTOVLLUX / JOURSRLUX / INVJCPLUX (.csv)`.

- [ ] **Step 6: Assert the routing**

Append to `crates/server/tests/api_ingest_routing.rs` a test uploading a two-line `REGLMTLUX_165878_20260807_1.csv` and asserting the response carries the decline text, plus one asserting a JOURSRLUX upload routes by fund code to the mapped portfolio exactly as HISINVLUX does.

- [ ] **Step 7: Run**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 8: Commit**

Commit message subject: `feat(ingest): INVJCPLUX coupon frequency; REGLMTLUX and RAPDECLUX declined with a reason`

Body: INVJCPLUX confirms a number HISINVLUX already implies and removes the manual maintenance of `bond_coupon_freq`. The frequency encoding is the one field the glossary does not disclose: integers and a small set of letter codes are mapped, `S` is deliberately unmapped because *semestriel* and *semaine* differ by a factor of 26, and anything else warns and stays NULL rather than guessing. REGLMTLUX and RAPDECLUX are recognised and declined with the double-counting reason, not merely unrecognised.

```bash
git add crates/ingest/src crates/ingest/tests crates/server/tests/api_ingest_routing.rs
git commit   # message as above, with the Co-Authored-By trailer
```

---

## Task 9: The shareholder register

The depositary feed is share-class level and carries no investor-level holdings, so the top five cannot come from it. `pct_of_nav` rather than a share count keeps the register trivial to maintain by hand and lets it revalue as NAV moves.

**Files:**
- Modify: `crates/db/src/repo.rs`
- Modify: `crates/server/src/handlers/portfolios.rs`
- Modify: `crates/server/src/routes.rs`
- Test: `crates/db/tests/liquidity_v2_repo.rs`
- Test: `crates/server/tests/api_liquidity_v2.rs` (create)

**Interfaces:**
- Produces: `db::repo::Shareholder { id: i64, label: String, pct_of_nav: f64, as_of: NaiveDate }`
- Produces: `db::repo::shareholders_for(pool, portfolio_id) -> anyhow::Result<Vec<Shareholder>>` ordered by `pct_of_nav` descending.
- Produces: `db::repo::shareholders_replace(pool, portfolio_id, rows: &[(String, f64, NaiveDate)]) -> anyhow::Result<()>`
- Produces: `GET /api/portfolios/{id}/shareholders`, `PUT /api/portfolios/{id}/shareholders`

- [ ] **Step 1: Write the failing API test**

Create `crates/server/tests/api_liquidity_v2.rs`, copying the harness helpers (`get_json`, `put_json`, `upload_req`, the temp-dir server construction) from `crates/server/tests/api_limits.rs` — the repo deliberately has no shared server test harness, so duplication here is the established pattern.

```rust
#[tokio::test]
async fn shareholder_register_crud_and_validation() {
    // ... build `app` exactly as api_limits.rs does, importing the sample ...

    let (s, _) = put_json(&app, "/api/portfolios/1/shareholders", serde_json::json!([
        {"label": "Founder family", "pct_of_nav": 18.0, "as_of": "2026-08-07"},
        {"label": "Pension fund A", "pct_of_nav": 12.5, "as_of": "2026-08-07"}
    ])).await;
    assert_eq!(s, StatusCode::OK);

    let (s, body) = get_json(&app, "/api/portfolios/1/shareholders").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 2);
    // Largest first: the top-five scenario reads straight off this order.
    assert_eq!(body[0]["label"], "Founder family");

    // A PUT replaces the register rather than appending to it.
    let (_, _) = put_json(&app, "/api/portfolios/1/shareholders", serde_json::json!([
        {"label": "Founder family", "pct_of_nav": 20.0, "as_of": "2026-08-10"}
    ])).await;
    let (_, body) = get_json(&app, "/api/portfolios/1/shareholders").await;
    assert_eq!(body.as_array().unwrap().len(), 1);

    for bad in [
        serde_json::json!([{"label": "X", "pct_of_nav": 0.0, "as_of": "2026-08-07"}]),
        serde_json::json!([{"label": "X", "pct_of_nav": 101.0, "as_of": "2026-08-07"}]),
        serde_json::json!([{"label": "  ", "pct_of_nav": 10.0, "as_of": "2026-08-07"}]),
        // A register summing past the whole fund is a typo, not a portfolio.
        serde_json::json!([
            {"label": "A", "pct_of_nav": 60.0, "as_of": "2026-08-07"},
            {"label": "B", "pct_of_nav": 60.0, "as_of": "2026-08-07"}
        ]),
    ] {
        let (s, _) = put_json(&app, "/api/portfolios/1/shareholders", bad).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    }

    // The rejected payloads left the stored register untouched.
    let (_, body) = get_json(&app, "/api/portfolios/1/shareholders").await;
    assert_eq!(body.as_array().unwrap().len(), 1);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p server --test api_liquidity_v2`
Expected: FAIL — 404, the route does not exist.

- [ ] **Step 3: Add the repo functions**

In `crates/db/src/repo.rs`:

```rust
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct Shareholder {
    pub id: i64,
    pub label: String,
    pub pct_of_nav: f64,
    pub as_of: NaiveDate,
}

/// Largest first: the top-five scenario reads straight off this order.
pub async fn shareholders_for(pool: &PgPool, portfolio_id: i64) -> anyhow::Result<Vec<Shareholder>> {
    Ok(sqlx::query_as(
        "SELECT id, label, pct_of_nav::float8 AS pct_of_nav, as_of
         FROM shareholders WHERE portfolio_id = $1 ORDER BY pct_of_nav DESC, id",
    )
    .bind(portfolio_id).fetch_all(pool).await?)
}

pub async fn shareholders_replace(
    pool: &PgPool, portfolio_id: i64, rows: &[(String, f64, NaiveDate)],
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
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
```

The delete and the inserts share one transaction so a mid-list failure cannot leave a half-replaced register.

- [ ] **Step 4: Add the handlers**

In `crates/server/src/handlers/portfolios.rs`:

```rust
#[derive(serde::Deserialize)]
pub struct ShareholderBody {
    pub label: String,
    pub pct_of_nav: f64,
    pub as_of: chrono::NaiveDate,
}

pub async fn shareholders_list(
    State(st): State<AppState>, Path(pid): Path<i64>,
) -> Result<Json<Vec<db::repo::Shareholder>>, AppError> {
    ensure(&st.pool, pid, false).await?;
    Ok(Json(db::repo::shareholders_for(&st.pool, pid).await?))
}

pub async fn shareholders_put(
    State(st): State<AppState>, Path(pid): Path<i64>, Json(body): Json<Vec<ShareholderBody>>,
) -> Result<Json<Vec<db::repo::Shareholder>>, AppError> {
    ensure(&st.pool, pid, false).await?;
    let mut total = 0.0;
    let mut rows = Vec::with_capacity(body.len());
    for b in &body {
        let label = b.label.trim();
        if label.is_empty() {
            return Err(AppError::Unprocessable("label must not be blank".into()));
        }
        if !(b.pct_of_nav.is_finite() && b.pct_of_nav > 0.0 && b.pct_of_nav <= 100.0) {
            return Err(AppError::Unprocessable(format!(
                "{label}: pct_of_nav must be in (0, 100]")));
        }
        total += b.pct_of_nav;
        rows.push((label.to_string(), b.pct_of_nav, b.as_of));
    }
    // A register summing past the whole fund is a typo, not a portfolio.
    if total > 100.0 {
        return Err(AppError::Unprocessable(format!(
            "register totals {total:.2}% of NAV, which exceeds 100%")));
    }
    db::repo::shareholders_replace(&st.pool, pid, &rows).await?;
    Ok(Json(db::repo::shareholders_for(&st.pool, pid).await?))
}
```

Every check runs before any write, so a rejected payload leaves the stored register untouched.

- [ ] **Step 5: Route it**

In `crates/server/src/routes.rs`, after the `codes` route:

```rust
        .route("/api/portfolios/{id}/shareholders",
            get(handlers::portfolios::shareholders_list)
                .put(handlers::portfolios::shareholders_put))
```

- [ ] **Step 6: Run**

Run: `cargo test -p server --test api_liquidity_v2 && cargo test -p db`
Expected: PASS.

- [ ] **Step 7: Commit**

Commit message subject: `feat(server): shareholder register for the top-five redemption scenario`

Body: Stored as percent of NAV rather than share counts, so it revalues automatically and stays maintainable by hand. Replace-in-one-transaction, with every validation running before any write. The depositary feed is share-class level and carries no investor-level holdings, which is why this is manual.

```bash
git add crates/db/src/repo.rs crates/db/tests/liquidity_v2_repo.rs crates/server/src/handlers/portfolios.rs crates/server/src/routes.rs crates/server/tests/api_liquidity_v2.rs
git commit   # message as above, with the Co-Authored-By trailer
```

---

## Task 10: The liquidity endpoint

Assembles the pure engine, the register and the coupon schedule into the response the Limits page reads. Replaces the shim Task 4 left behind.

**Files:**
- Modify: `crates/server/src/handlers/limits.rs`
- Test: `crates/server/tests/api_liquidity_v2.rs`

**Interfaces:**
- Consumes: everything from Tasks 2 through 5, `db::repo::shareholders_for`, `db::repo::aum_for`.
- Produces: `GET /api/portfolios/{id}/metrics/liquidity?date=` returning `{date, dates, nav, params, coverage, asset:{normal, stressed}, scenarios, negative_memo, negative_memo_eur}`.

- [ ] **Step 1: Write the failing test**

Append to `crates/server/tests/api_liquidity_v2.rs`:

```rust
#[tokio::test]
async fn liquidity_response_shape_and_scenarios() {
    // ... build `app` and import the sample as api_limits.rs does ...

    let (s, b) = get_json(&app, "/api/portfolios/1/metrics/liquidity").await;
    assert_eq!(s, StatusCode::OK);

    // Every displayed number is explained by an echoed parameter.
    for k in ["participation_rate", "adv_stress_factor", "liquidity_horizon_days",
              "settlement_deadline_days", "redemption_shock", "day_unit"] {
        assert!(!b["params"][k].is_null(), "params.{k} missing");
    }
    assert_eq!(b["params"]["day_unit"], "business days (Mon-Fri, no holiday calendar)");

    // Two asset profiles over the same four bands.
    for view in ["normal", "stressed"] {
        let buckets = b["asset"][view]["buckets"].as_array().unwrap();
        assert_eq!(buckets.len(), 4);
        assert_eq!(buckets[0]["bucket"], "d1");
        assert_eq!(b["asset"][view]["cumulative"].as_array().unwrap().len(), 4);
    }

    // Four scenarios, always present, always keyed.
    let keys: Vec<&str> = b["scenarios"].as_array().unwrap().iter()
        .map(|s| s["key"].as_str().unwrap()).collect();
    assert_eq!(keys, vec!["top5", "fixed", "hybrid_top5", "hybrid_fixed"]);

    // With no register loaded, the top-five scenarios are explicitly
    // unavailable — never a zero and never a pass.
    let top5 = &b["scenarios"][0];
    assert_eq!(top5["status"], "unavailable");
    assert_eq!(top5["reason"], "no shareholder register");
    assert!(top5["waterfall"].is_null());

    // The fixed scenario computes against the configured shock.
    let fixed = &b["scenarios"][1];
    assert!(fixed["required_eur"].as_f64().unwrap() > 0.0);
    assert!((fixed["required_pct"].as_f64().unwrap() - 0.30).abs() < 1e-9);
    assert!(matches!(fixed["status"].as_str().unwrap(), "ok" | "breach"));

    assert!(!b["coverage"]["adv_pct_of_nav"].is_null());
    assert!(b["coverage"]["fallbacks"].is_array());
}

#[tokio::test]
async fn a_loaded_register_drives_the_top_five_scenarios() {
    // ... build `app`, import the sample ...
    put_json(&app, "/api/portfolios/1/shareholders", serde_json::json!([
        {"label": "A", "pct_of_nav": 10.0, "as_of": "2026-08-07"},
        {"label": "B", "pct_of_nav": 8.0,  "as_of": "2026-08-07"},
        {"label": "C", "pct_of_nav": 6.0,  "as_of": "2026-08-07"},
        {"label": "D", "pct_of_nav": 4.0,  "as_of": "2026-08-07"},
        {"label": "E", "pct_of_nav": 2.0,  "as_of": "2026-08-07"},
        {"label": "F", "pct_of_nav": 1.0,  "as_of": "2026-08-07"}
    ])).await;

    let (_, b) = get_json(&app, "/api/portfolios/1/metrics/liquidity").await;
    let top5 = &b["scenarios"][0];
    assert_ne!(top5["status"], "unavailable");
    // The five largest only: 10 + 8 + 6 + 4 + 2 = 30%, not 31%.
    assert!((top5["required_pct"].as_f64().unwrap() - 0.30).abs() < 1e-9);
    assert_eq!(top5["register_count"], 5);

    // The hybrid runs the same requirement against stressed volumes, so it is
    // never faster than its unstressed twin.
    let hy = &b["scenarios"][2];
    let (a, h) = (top5["waterfall"]["days"].as_u64(), hy["waterfall"]["days"].as_u64());
    if let (Some(a), Some(h)) = (a, h) { assert!(h >= a); }

    // Slice is always the slower ordering.
    assert!(top5["slice_days"].as_f64().unwrap() >= top5["waterfall"]["days"].as_f64().unwrap_or(0.0));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p server --test api_liquidity_v2`
Expected: FAIL — `params` is null; the shim response has no such key.

- [ ] **Step 3: Rewrite the handler**

Replace `liquidity_h` in `crates/server/src/handlers/limits.rs`:

```rust
/// A register whose as-of date is older than this is flagged stale. It is not
/// a setting: the register is a compliance artefact with no per-portfolio
/// cadence to calibrate against, and a quarter is the interval at which one
/// would normally be refreshed.
const REGISTER_MAX_AGE_DAYS: i64 = 90;

fn build_positions(
    rows: &[db::repo::PositionRecord],
    by: &HashMap<&str, &db::repo::InstrumentRef>,
    settings: &db::settings::AppSettings,
    asof: chrono::NaiveDate,
) -> Vec<analytics::LiqPosition> {
    rows.iter().filter_map(|p| {
        let v = p.valuation_eur?;
        if v <= 0.0 { return None; }  // negatives are a cash need, not a sale
        let r = by.get(p.isin.as_str());
        Some(analytics::LiqPosition {
            code: p.isin.clone(),
            asset_type: p.asset_type.clone(),
            valuation_eur: v,
            quantity: p.quantity,
            adv_30d: r.and_then(|r| r.adv_30d),
            adv_stale: r.and_then(|r| r.adv_asof)
                .map(|d| (asof - d).num_days() > settings.adv_max_age_days as i64)
                // No as-of at all is "no adv", reported by its own reason.
                .unwrap_or(false),
            adv_eligible: r.and_then(|r| r.adv_eligible),
            market_place: r.and_then(|r| r.market_place.clone()),
            liquidity_days: r.and_then(|r| r.liquidity_days),
            default_days: super::refs::effective_days(
                &settings.liquidity_default_days, &p.asset_type, None),
        })
    }).collect()
}

pub async fn liquidity_h(
    State(st): State<AppState>, Path(pid): Path<i64>, Query(q): Query<DateQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    super::portfolios::ensure(&st.pool, pid, false).await?;
    let (dates, date, rows, refs) = snapshot(&st, pid, &q).await?;
    let settings = db::settings::get_settings(&st.pool, pid).await?;
    let by = ref_map(&refs);
    let horizon = settings.liquidity_horizon_days;

    let params = serde_json::json!({
        "participation_rate": settings.participation_rate,
        "adv_stress_factor": settings.adv_stress_factor,
        "liquidity_horizon_days": horizon,
        "settlement_deadline_days": settings.settlement_deadline_days,
        "adv_max_age_days": settings.adv_max_age_days,
        "redemption_shock": settings.redemption_shock,
        "day_unit": "business days (Mon-Fri, no holiday calendar)",
    });

    // An absent snapshot or NAV returns the established empty shape rather
    // than an error, matching every other metrics endpoint.
    let (Some(asof), Some(nav)) = (date, match date {
        Some(d) => db::repo::aum_for(&st.pool, pid, d).await?,
        None => None,
    }) else {
        return Ok(Json(serde_json::json!({
            "dates": dates, "date": date, "nav": null, "params": params,
            "coverage": serde_json::Value::Null, "asset": serde_json::Value::Null,
            "scenarios": [], "negative_memo": 0.0, "negative_memo_eur": 0.0,
        })));
    };

    let positions = build_positions(&rows, &by, &settings, asof);
    let cap_at = |stress: f64| -> Vec<analytics::Capacity> {
        positions.iter().map(|p| analytics::capacity(p, settings.participation_rate, stress)).collect()
    };
    let normal = cap_at(1.0);
    let stressed = cap_at(settings.adv_stress_factor);

    let negative_eur: f64 = rows.iter().filter_map(|p| p.valuation_eur).filter(|v| *v < 0.0).sum();
    let negative_memo: f64 = rows.iter().filter_map(|p| p.weight).filter(|w| *w < 0.0).sum();

    // Coupon and redemption inflows, from the depositary's own schedule.
    let coupon_inputs: Vec<analytics::CouponInput> = rows.iter()
        .filter(|p| p.asset_type == "Obligation")
        .filter_map(|p| {
            let r = by.get(p.isin.as_str())?;
            Some(analytics::CouponInput {
                code: p.isin.clone(),
                quantity: p.quantity.unwrap_or(0.0),
                coupon_pct: r.bond_coupon_pct,
                // Only a fixed coupon reaches instrument_refs at all, so its
                // presence is the FIX gate the parser already applied.
                coupon_type: r.bond_coupon_pct.map(|_| "FIX".to_string()),
                next_coupon: r.bond_next_coupon,
                maturity: r.bond_maturity,
                freq: r.bond_coupon_freq,
                accrued_eur: p.accrued_interest,
                fx_rate: p.fx_rate.unwrap_or(1.0),
            })
        }).collect();
    let coupons = analytics::bond_inflows(&coupon_inputs, asof, horizon);

    let register = db::repo::shareholders_for(&st.pool, pid).await?;
    let top5_pct: f64 = register.iter().take(5).map(|s| s.pct_of_nav).sum::<f64>() / 100.0;

    let scenario = |key: &str, required_pct: Option<f64>, caps: &[analytics::Capacity]| -> serde_json::Value {
        let Some(pct) = required_pct else {
            return serde_json::json!({
                "key": key, "status": "unavailable", "reason": "no shareholder register",
            });
        };
        let required = pct * nav;
        let w = analytics::waterfall(caps, &coupons.inflows, negative_eur, required, horizon);
        let status = match w.days {
            Some(d) if d <= settings.settlement_deadline_days => "ok",
            _ => "breach",
        };
        let curve: Vec<serde_json::Value> = (1..=horizon).map(|d| serde_json::json!({
            "day": d,
            "available_eur": analytics::available(caps, &coupons.inflows, negative_eur, d),
        })).collect();
        serde_json::json!({
            "key": key,
            "required_eur": required,
            "required_pct": pct,
            "register_count": register.len().min(5),
            "status": status,
            "waterfall": w,
            "slice_days": analytics::slice_days(caps, required, nav),
            "residual": analytics::residual(caps, required, nav, w.days.unwrap_or(horizon)),
            "curve": curve,
        })
    };

    let top5 = (!register.is_empty()).then_some(top5_pct);
    let fixed = Some(settings.redemption_shock);
    let scenarios = vec![
        scenario("top5", top5, &normal),
        scenario("fixed", fixed, &normal),
        scenario("hybrid_top5", top5, &stressed),
        scenario("hybrid_fixed", fixed, &stressed),
    ];

    let measured_eur: f64 = normal.iter().filter(|c| c.measured).map(|c| c.valuation_eur).sum();
    let fallbacks: Vec<serde_json::Value> = normal.iter()
        .filter_map(|c| c.reason.map(|r| serde_json::json!({"code": c.code, "reason": r})))
        .collect();

    Ok(Json(serde_json::json!({
        "dates": dates,
        "date": date,
        "nav": nav,
        "params": params,
        "coverage": {
            "adv_pct_of_nav": if nav > 0.0 { measured_eur / nav } else { 0.0 },
            "fallbacks": fallbacks,
            "coupon_gaps": coupons.gaps,
            "register": {
                "count": register.len(),
                "as_of": register.iter().map(|s| s.as_of).min(),
                "stale": register.iter().any(|s| (asof - s.as_of).num_days() > REGISTER_MAX_AGE_DAYS),
            },
        },
        "asset": {
            "normal": analytics::asset_profile(&normal, nav),
            "stressed": analytics::asset_profile(&stressed, nav),
        },
        "scenarios": scenarios,
        "negative_memo": negative_memo,
        "negative_memo_eur": negative_eur,
    })))
}
```

- [ ] **Step 4: Update the legacy assertions**

`crates/server/tests/api_limits.rs` asserts the old `buckets` / `shock` / `stress_status` keys. Move its liquidity assertions to the new shape — `asset.normal.buckets` and the `fixed` scenario's status — rather than keeping a parallel old response alive.

- [ ] **Step 5: Run**

Run: `cargo test -p server`
Expected: PASS.

- [ ] **Step 6: Commit**

Commit message subject: `feat(server): liquidity endpoint with asset profiles, four scenarios and coverage`

Body: The pass/fail chip now asks whether the money arrives by the contractual settlement date rather than whether assets liquidatable within seven days cover thirty percent. An empty register makes the top-five scenarios explicitly unavailable with a reason rather than a misleading zero or a pass. `params` echoes every resolved setting so no displayed number is unexplained, and `coverage` names every position on the fallback path with its reason.

```bash
git add crates/server/src/handlers/limits.rs crates/server/tests
git commit   # message as above, with the Co-Authored-By trailer
```

---

## Task 11: The flows endpoint

**Files:**
- Modify: `crates/server/src/handlers/portfolios.rs`
- Modify: `crates/server/src/routes.rs`
- Test: `crates/server/tests/api_liquidity_v2.rs`

**Interfaces:**
- Consumes: `db::repo::flows_for` (Task 7), `analytics::flow_stats` (Task 5).
- Produces: `GET /api/portfolios/{id}/flows`.

- [ ] **Step 1: Write the failing test**

Append to `crates/server/tests/api_liquidity_v2.rs`:

```rust
#[tokio::test]
async fn flows_are_unavailable_until_enough_history_is_loaded() {
    // ... build `app`, import the sample (which carries no flow file) ...
    let (s, b) = get_json(&app, "/api/portfolios/1/flows").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["status"], "unavailable");
    assert_eq!(b["n_observations"], 0);
    // Never a percentage computed from too little history.
    assert!(b["worst"].is_null());
    assert!(b["reason"].as_str().unwrap().contains("observation"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p server --test api_liquidity_v2 flows`
Expected: FAIL — 404.

- [ ] **Step 3: Implement**

In `crates/server/src/handlers/portfolios.rs`:

```rust
pub async fn flows(
    State(st): State<AppState>, Path(pid): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    ensure(&st.pool, pid, false).await?;
    let settings = db::settings::get_settings(&st.pool, pid).await?;
    let records = db::repo::flows_for(&st.pool, pid, settings.flow_lookback_days).await?;

    // Aggregate the share classes into one fund-level series. Each class
    // contributes its own net amount and its own net assets, so no
    // NAV-per-share ambiguity arises and multi-class portfolios need no
    // special case here.
    let mut by_date: std::collections::BTreeMap<chrono::NaiveDate, (f64, f64)> = Default::default();
    for r in &records {
        let e = by_date.entry(r.flow_date).or_insert((0.0, 0.0));
        e.0 += r.subscription_amount - r.redemption_amount;
        e.1 += r.outstanding_shares.unwrap_or(0.0) * r.nav_per_share.unwrap_or(0.0);
    }
    let obs: Vec<analytics::FlowObs> = by_date.into_iter()
        .map(|(date, (net_eur, nav_eur))| analytics::FlowObs { date, net_eur, nav_eur })
        .collect();

    Ok(Json(match analytics::flow_stats(&obs) {
        Some(s) => serde_json::json!(s),
        None => serde_json::json!({
            "status": "unavailable",
            "n_observations": obs.len(),
            "reason": format!(
                "{} observation(s) loaded; {} are needed before an observed outflow means anything",
                obs.len(), analytics::MIN_FLOW_OBSERVATIONS),
        }),
    }))
}
```

Route it as `.route("/api/portfolios/{id}/flows", get(handlers::portfolios::flows))`.

- [ ] **Step 4: Run**

Run: `cargo test -p server`
Expected: PASS.

- [ ] **Step 5: Commit**

Commit message subject: `feat(server): observed flow statistics endpoint`

Body: Share classes aggregate into one fund-level series, each contributing its own net amount and its own net assets, so multi-class portfolios need no special case. Below the minimum observation count the response says unavailable with its count rather than returning a percentage computed from too little history.

```bash
git add crates/server/src/handlers/portfolios.rs crates/server/src/routes.rs crates/server/tests/api_liquidity_v2.rs
git commit   # message as above, with the Co-Authored-By trailer
```

---

## Task 12: The Bloomberg ADV round trip

Country and GICS are one-and-done, so that workbook shrinks toward empty. ADV decays daily and would never drop out. Bundling them would turn every classification export into a fleet-wide volume request, so they get separate endpoints and separate buttons.

**Files:**
- Modify: `crates/ingest/src/bloomberg.rs`
- Modify: `crates/server/src/handlers/bloomberg.rs`
- Modify: `crates/db/src/repo.rs`
- Modify: `crates/server/src/routes.rs`
- Test: `crates/ingest/tests/bloomberg.rs`
- Test: `crates/server/tests/api_bloomberg_adv.rs` (create)

**Interfaces:**
- Produces: `ingest::bloomberg::build_adv_request(items: &[RequestItem], asof: NaiveDate) -> anyhow::Result<Vec<u8>>` writing an `ADV` sheet with columns `isin`, `adv_30d`, `market_sector`.
- Produces: `ingest::bloomberg::ParsedResponse::adv: Vec<AdvRow>` where `AdvRow { isin: String, adv_30d: f64 }`.
- Produces: `db::repo::adv_upsert_many(pool, rows: &[(String, f64)], asof: NaiveDate) -> anyhow::Result<u64>`
- Produces: `GET /api/bloomberg/adv-request?all=true`, `GET /api/bloomberg/adv-due`.

- [ ] **Step 1: Write the failing tests**

Create `crates/server/tests/api_bloomberg_adv.rs`:

```rust
#[tokio::test]
async fn adv_request_is_scoped_to_listed_instruments_that_are_due() {
    // ... build `app`, import the CACEIS HISINVLUX fixture so market places exist ...

    let (s, b) = get_json(&app, "/api/bloomberg/adv-due").await;
    assert_eq!(s, StatusCode::OK);
    let due = b["due"].as_u64().unwrap();
    let held = b["held"].as_u64().unwrap();
    assert!(due > 0 && due <= held, "you see the cost before you pay it: {due} of {held}");

    let res = app.clone().oneshot(
        Request::get("/api/bloomberg/adv-request").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let isins = adv_sheet_isins(&bytes);   // helper: read the ADV sheet's column A

    // The listed ETF is in; the unlisted target fund, the futures and the
    // cash accounts are not.
    assert!(isins.iter().any(|i| i == "AT000000STR1"), "a listed equity is requested");
    assert!(!isins.iter().any(|i| i == "FR0010599399"), "an unlisted target fund is not");
    assert!(!isins.iter().any(|i| i.starts_with("FVS")), "futures are never requested");
}

#[tokio::test]
async fn a_fresh_adv_drops_out_until_it_goes_stale() {
    // ... import, then upload an ADV response for one instrument ...
    // Assert adv-due falls by exactly one, and that ?all=true still includes it.
}
```

Write `adv_sheet_isins` inline in the test file using `calamine`, mirroring how `crates/ingest/tests/bloomberg.rs` already reads a generated workbook.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p server --test api_bloomberg_adv`
Expected: FAIL — 404 on `/api/bloomberg/adv-due`.

- [ ] **Step 3: Build the request workbook**

In `crates/ingest/src/bloomberg.rs`:

```rust
/// The ADV request workbook. Separate from `build_request` on purpose:
/// country and GICS are one-and-done, so that sheet shrinks toward empty,
/// while ADV decays daily and never drops out. Bundling them would turn every
/// classification export into a fleet-wide volume request.
///
/// One `BDP` cell per instrument — a point value, not a `BDH` history series.
/// That is the smallest possible footprint per instrument, and it makes a
/// typical daily refresh a handful of formulas rather than a sweep.
pub fn build_adv_request(items: &[RequestItem], asof: NaiveDate) -> anyhow::Result<Vec<u8>> {
    let mut wb = Workbook::new();
    let bold = Format::new().set_bold();
    let s = wb.add_worksheet();
    s.set_name("ADV")?;
    for (c, h) in ["isin", "adv_30d", "market_sector"].iter().enumerate() {
        s.write_string_with_format(0, c as u16, *h, &bold)?;
    }
    s.set_column_width(0, 16)?;
    s.set_column_width(2, 14)?;
    for (i, it) in items.iter().enumerate() {
        let r = (i + 1) as u32;
        s.write_string(r, 0, &it.isin)?;
        s.write_string(r, 2, &it.market_sector)?;
        let row = r + 1;
        s.write_formula(r, 1, Formula::new(
            format!("=BDP(A{row}&\" \"&C{row},\"VOLUME_AVG_30D\")")))?;
    }

    let r = wb.add_worksheet();
    r.set_name("README")?;
    r.set_column_width(0, 100)?;
    for (i, l) in [
        "Borobudur Risk - Bloomberg 30-day average volume request".to_string(),
        format!("Exported {asof}. {} instrument(s).", items.len()),
        String::new(),
        "1. Open in Excel on a machine with a logged-in Bloomberg Terminal.".into(),
        "2. Wait for every formula to resolve. #N/A cells are reported on upload and not stored.".into(),
        "3. Save as .xlsx and upload on the Data page, Bloomberg panel.".into(),
        String::new(),
        "Volumes are stored with the upload date as their as-of. A volume older than the".into(),
        "configured maximum age is treated as stale: the position falls back to its assumed".into(),
        "days figure and is flagged, and nothing in the tool ever refreshes it on your behalf.".into(),
    ].iter().enumerate() {
        r.write_string(i as u32, 0, l)?;
    }
    Ok(wb.save_to_buffer()?)
}
```

Extend `ParsedResponse` with `pub adv: Vec<AdvRow>` and, in `parse_response`, read an `ADV` sheet when present: column A the ISIN, column B the volume, unresolved cells pushed to `skipped` with `sheet: "ADV"` exactly as the REFS loop does. Relax the "neither REFS nor FX" guard to also accept a workbook carrying only `ADV`, so one upload path serves both files.

- [ ] **Step 4: Scope and serve**

In `crates/server/src/handlers/bloomberg.rs`:

```rust
/// Every eligible instrument held in the fleet's latest snapshots, split into
/// those whose stored volume has gone stale and the full held set.
/// Deduplicated by ISIN: an instrument held by three portfolios is requested
/// once.
async fn adv_scope(st: &AppState) -> Result<(Vec<RequestItem>, Vec<RequestItem>), AppError> {
    let refs = db::repo::refs_all(&st.pool).await?;
    let by: std::collections::HashMap<&str, &db::repo::InstrumentRef> =
        refs.iter().map(|r| (r.code.as_str(), r)).collect();

    let mut seen: BTreeSet<String> = BTreeSet::new();
    let (mut due, mut held) = (Vec::new(), Vec::new());
    for pf in db::repo::portfolios_list(&st.pool).await?.iter().filter(|p| !p.archived) {
        let dates = db::repo::position_dates(&st.pool, pf.id).await?;
        let Some(latest) = dates.first().copied() else { continue };
        let settings = db::settings::get_settings(&st.pool, pf.id).await?;
        for p in db::repo::positions_for(&st.pool, pf.id, latest).await? {
            let r = by.get(p.isin.as_str());
            let probe = analytics::LiqPosition {
                code: p.isin.clone(), asset_type: p.asset_type.clone(),
                valuation_eur: p.valuation_eur.unwrap_or(0.0), quantity: p.quantity,
                adv_30d: None, adv_stale: false,
                adv_eligible: r.and_then(|r| r.adv_eligible),
                market_place: r.and_then(|r| r.market_place.clone()),
                liquidity_days: None, default_days: 1.0,
            };
            if !analytics::adv_eligible(&probe) { continue; }
            if !seen.insert(p.isin.clone()) { continue; }
            let item = RequestItem {
                isin: p.isin.clone(),
                market_sector: market_sector_for(asset_class_of(&p.asset_type)).to_string(),
            };
            let stale = r.and_then(|r| r.adv_asof)
                .map(|d| (latest - d).num_days() > settings.adv_max_age_days as i64)
                .unwrap_or(true);   // never fetched is always due
            if stale { due.push(item.clone()); }
            held.push(item);
        }
    }
    Ok((due, held))
}
```

`adv_request` reads `?all=true` from a `Query<AdvQuery>` extractor, calls `adv_scope`, passes `held` when `all` is set and `due` otherwise to `build_adv_request`, and returns it with the same content-type and `attachment; filename="bloomberg_adv_request_{date}.xlsx"` headers the existing `request` handler uses. `adv_due` calls `adv_scope` and returns `{"due": due.len(), "held": held.len()}` without building a workbook, so the panel can show the cost before it is paid.

In `upload`, after the classification block, store any ADV rows:

```rust
    let adv_rows: Vec<(String, f64)> = parsed.adv.iter()
        .filter(|a| a.adv_30d.is_finite() && a.adv_30d >= 0.0)
        .map(|a| (a.isin.clone(), a.adv_30d)).collect();
    let adv_stored = db::repo::adv_upsert_many(
        &st.pool, &adv_rows, chrono::Utc::now().date_naive()).await?;
```

and add `"adv_rows": adv_stored` to the response JSON. `adv_upsert_many` writes `adv_30d` and `adv_asof` only, touching no other column.

Route `/api/bloomberg/adv-request` and `/api/bloomberg/adv-due`.

- [ ] **Step 5: Run**

Run: `cargo test -p ingest && cargo test -p server`
Expected: PASS.

- [ ] **Step 6: Commit**

Commit message subject: `feat(bloomberg): scoped, user-initiated ADV request separate from classification`

Body: The server has no Bloomberg connectivity — it writes formula text that resolves only in Excel — so reading the Limits page cannot emit a call. What this keeps small is the user-initiated request: one `BDP` point value per instrument, scoped to listed instruments actually held, deduplicated fleet-wide, and by default only those whose stored volume has gone stale. A due-count endpoint shows the size of the request before it is exported. Staleness renders as a warning and falls the position back to its assumed days; nothing in the UI ever initiates a refresh.

```bash
git add crates/ingest/src/bloomberg.rs crates/ingest/tests/bloomberg.rs crates/server/src/handlers/bloomberg.rs crates/server/src/routes.rs crates/db/src/repo.rs crates/server/tests/api_bloomberg_adv.rs
git commit   # message as above, with the Co-Authored-By trailer
```

---

## Task 13: The Limits page liquidity section

Four stacked pieces, each answering a question the previous one raises. There are no frontend unit tests; `npm run build` is the gate, and the acceptance check is reading the page against a real snapshot.

**Files:**
- Modify: `frontend/src/api.ts`
- Modify: `frontend/src/pages/LimitsPage.tsx`

**Interfaces:**
- Consumes: `GET /api/portfolios/{id}/metrics/liquidity` (Task 10), `GET /api/portfolios/{id}/flows` (Task 11).
- Produces: `Liquidity`, `LiquidityParams`, `LiquidityCoverage`, `AssetProfile`, `Scenario`, `FlowStats` in `api.ts`; `getFlows(pid)`.

- [ ] **Step 1: Replace the API types**

In `frontend/src/api.ts`, replace the `Liquidity` interface and add the rest:

```ts
export interface BucketWeight { bucket: string; weight: number }
export interface AssetProfile { buckets: BucketWeight[]; cumulative: BucketWeight[] }

export interface LiquidityParams {
  participation_rate: number; adv_stress_factor: number;
  liquidity_horizon_days: number; settlement_deadline_days: number;
  adv_max_age_days: number; redemption_shock: number; day_unit: string;
}

export interface LiquidityCoverage {
  adv_pct_of_nav: number;
  fallbacks: { code: string; reason: string }[];
  coupon_gaps: { code: string; reason: string }[];
  register: { count: number; as_of: string | null; stale: boolean };
}

export interface Scenario {
  key: "top5" | "fixed" | "hybrid_top5" | "hybrid_fixed";
  status: "ok" | "breach" | "unavailable";
  reason?: string;
  required_eur?: number;
  required_pct?: number;
  register_count?: number;
  waterfall?: { days: number | null; unmet_eur: number };
  slice_days?: number | null;
  residual?: { slow_share_before: number; slow_share_after: number };
  curve?: { day: number; available_eur: number }[];
}

export interface Liquidity {
  dates: string[]; date: string | null; nav: number | null;
  params: LiquidityParams;
  coverage: LiquidityCoverage | null;
  asset: { normal: AssetProfile; stressed: AssetProfile } | null;
  scenarios: Scenario[];
  negative_memo: number; negative_memo_eur: number;
}

export interface FlowStats {
  status?: "unavailable"; reason?: string;
  n_observations: number; from?: string; to?: string;
  worst?: { window: number; pct_of_nav: number }[];
}

export const getFlows = (pid: number) => req<FlowStats>(`/api/portfolios/${pid}/flows`);
```

- [ ] **Step 2: Rewrite the liquidity section**

In `frontend/src/pages/LimitsPage.tsx`, replace the whole `<h3>Liquidity</h3>` block. Add `const flows = useFetch(() => getFlows(portfolio.id), [portfolio.id]);` beside the existing fetches and `const [scenario, setScenario] = useState<string>("fixed");`.

The section renders, in order:

1. **Parameters strip** — a `kpi-sub` paragraph reading the echoed `params`: participation, stress factor, horizon, settlement deadline, and `day_unit` verbatim so the Monday-to-Friday simplification is stated where the numbers are.

2. **Coverage chip** — `ADV measured on {pct(coverage.adv_pct_of_nav)} of NAV`, followed by the fallback count and, when non-zero, a `<details>` listing each code with its reason. The stressed profile moves only the measured part of the portfolio, so state that here rather than leaving the reader to infer it:

```tsx
<p className="kpi-sub">
  The stress factor applies only to positions measured from traded volume.
  A position on the assumed-days fallback is already an assumption and is not
  re-stressed, so the stressed profile moves {pct(cov.adv_pct_of_nav)} of the fund.
</p>
```

3. **Asset profile chart** — one `EChart` with two bar series (`Normal`, `Stressed`) over `BUCKET_LABELS`, plus the two cumulative lines. Keep the existing `BUCKET_LABELS` map; the keys are unchanged.

4. **Scenario table** — one row per scenario with columns Scenario, Required, Waterfall days, Slice days, Unmet, Status. An `unavailable` row spans the numeric columns with its `reason` text, styled `warn-badge`, never a dash that could read as zero. Clicking a row sets `scenario`; the selected row's `curve` renders below as a line chart with two `markLine` entries — `required_eur` horizontal, `settlement_deadline_days` vertical.

5. **Observed flows line**, beneath the table:

```tsx
{flows.data && (flows.data.status === "unavailable" ? (
  <p className="kpi-sub">
    Observed outflows: {flows.data.reason}. Load JOURSRLUX files on the Data page.
  </p>
) : (
  <p className="kpi-sub">
    Worst observed 20-day outflow{" "}
    <strong>{pct(flows.data.worst?.find((w) => w.window === 20)?.pct_of_nav ?? 0)}</strong>{" "}
    of NAV over {flows.data.n_observations} observations, {flows.data.from} to {flows.data.to}.
    Configured shock is {pct(liq.data.params.redemption_shock, 0)}.
  </p>
))}
```

Do **not** add an automatic adjustment of the shock from this figure. The adopt action belongs on the Data page beside the setting it writes (Task 14).

6. **Negative memo** — keep the existing line, amended to say the amount now reduces availability from day one rather than being a memo only.

- [ ] **Step 3: Build**

Run: `cd frontend && npm run build`
Expected: clean.

- [ ] **Step 4: Read the page against real data**

Start the dev server, open Limits for the CACEIS portfolio, and check: the parameters strip states business days; the coverage percentage is plausible against the portfolio's equity share; the stressed bars sit to the right of the normal bars; the four scenario rows are present with the two top-five rows unavailable until a register is entered.

- [ ] **Step 5: Commit**

Commit message subject: `feat(frontend): liquidity section with asset profiles, scenarios and coverage`

Body: Reads the parameters the server echoes rather than restating them, so no number on the page is unexplained. An unavailable scenario shows its reason in place of the numeric columns rather than a dash that could be read as zero, and the coverage note states what share of the fund the stress factor actually moves.

```bash
git add frontend/src/api.ts frontend/src/pages/LimitsPage.tsx
git commit   # message as above, with the Co-Authored-By trailer
```

---

## Task 14: The Data page editors

Three pieces of data maintenance, all in the place maintenance already lives.

**Files:**
- Create: `frontend/src/components/ShareholderRegister.tsx`
- Modify: `frontend/src/pages/DataPage.tsx`
- Modify: `frontend/src/components/BloombergPanel.tsx`
- Modify: `frontend/src/api.ts`

**Interfaces:**
- Consumes: `GET`/`PUT /api/portfolios/{id}/shareholders` (Task 9), `GET /api/bloomberg/adv-due` and `GET /api/bloomberg/adv-request` (Task 12), the `RefRow` fields from Task 1.
- Produces: `getShareholders(pid)`, `putShareholders(pid, rows)`, `getAdvDue()`, `advRequestUrl` in `api.ts`.

- [ ] **Step 1: Add the API calls**

```ts
export interface Shareholder { id?: number; label: string; pct_of_nav: number; as_of: string }
export const getShareholders = (pid: number) => req<Shareholder[]>(`/api/portfolios/${pid}/shareholders`);
export const putShareholders = (pid: number, rows: Shareholder[]) =>
  req<Shareholder[]>(`/api/portfolios/${pid}/shareholders`, {
    method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(rows),
  });

export const advRequestUrl = "/api/bloomberg/adv-request";
export const getAdvDue = () => req<{ due: number; held: number }>("/api/bloomberg/adv-due");
```

- [ ] **Step 2: Finish the reference editor**

Task 1 left a bare number input. Complete the row: `liquidity_days` as `<input type="number" min={0} step={0.5}>` with placeholder `default (${r.effective_days})`; read-only `adv_30d` and `adv_asof` columns, the as-of rendered `warn-badge` when older than the configured maximum age; a read-only market place column showing `market_place_name`; and `adv_eligible` as a three-state `<select>` — `derived` (null), `always` (true), `never` (false).

Update the card's help text, which currently says buckets drive the liquidity view:

```tsx
<p className="kpi-sub">
  Days-to-liquidate drives the liquidity view; bond fields drive YTM and duration.
  Blank days = the asset-type default. ADV, market place and the bond schedule are
  maintained by the depositary feed and Bloomberg and cannot be edited here.
</p>
```

- [ ] **Step 3: Add the register editor**

Create `frontend/src/components/ShareholderRegister.tsx`: a table of label / percent of NAV / as-of with add and remove buttons and one Save that PUTs the whole list. Show the running total beside the Save button and disable Save above 100%, so the server's 422 is the backstop rather than the first feedback. Render the server's error text on failure — it names the offending entry.

Add a sentence stating the limit honestly:

```tsx
<p className="kpi-sub">
  Maintained by hand: the depositary feed is share-class level and carries no
  investor-level holdings. Nothing reconciles these percentages against the
  fund's outstanding shares, so a stale register moves the top-five scenarios
  without warning beyond the as-of date shown on Limits.
</p>
```

Mount it in `DataPage.tsx` beside `<RefsCard>`.

- [ ] **Step 4: Add the settings fields**

In the settings card, add number inputs for `participation_rate`, `adv_stress_factor`, `liquidity_horizon_days`, `settlement_deadline_days`, `adv_max_age_days` and `flow_lookback_days`, and convert the `liquidity_defaults` bucket dropdowns to `liquidity_default_days` number inputs. Beside `redemption_shock`, render the observed worst 20-day outflow from `getFlows` with an **Adopt as fixed shock** button that writes the setting. It never applies on its own: an observed history that has never seen a stress is not evidence that no stress can happen.

- [ ] **Step 5: Add the ADV export button**

In `BloombergPanel.tsx`, add a second export beside the existing classification link:

```tsx
<a href={all ? `${advRequestUrl}?all=true` : advRequestUrl} download>
  Export ADV request{due != null ? ` (${due.due} of ${due.held} due)` : ""}
</a>
<label><input type="checkbox" checked={all} onChange={(e) => setAll(e.target.checked)} /> full rebuild</label>
<p className="kpi-sub">
  Formulas resolve only when you open the file in Excel on a machine with a
  logged-in Bloomberg Terminal. Nothing here queries Bloomberg on its own.
</p>
```

The due count comes from `getAdvDue()` on mount, so the size of the request is visible before it is exported rather than after Excel opens.

- [ ] **Step 6: Build**

Run: `cd frontend && npm run build`
Expected: clean.

- [ ] **Step 7: Exercise it**

With the dev server running: enter six register entries and confirm Limits switches the top-five scenarios from unavailable to computed using only the largest five; set an instrument's `adv_eligible` to `never` and confirm it appears in the Limits coverage fallback list; export the ADV request and confirm the workbook contains the due count the button advertised.

- [ ] **Step 8: Commit**

Commit message subject: `feat(frontend): reference days, shareholder register, ADV export and v2 settings`

Body: The register editor states plainly that nothing reconciles it against outstanding shares. The observed worst outflow sits beside the configured shock with an explicit adopt action and never applies itself. The ADV export shows its due count before it is exported, and the panel states that formulas resolve only in Excel.

```bash
git add frontend/src
git commit   # message as above, with the Co-Authored-By trailer
```

---

## Task 15: Full verification

**Files:** none modified unless a failure is found.

- [ ] **Step 1: Stop the dev server**

The embedded PostgreSQL instances collide with a running server. Confirm nothing is listening on the dev port before continuing.

- [ ] **Step 2: Run the whole suite**

Run: `cargo test --workspace`
Expected: PASS across `analytics`, `ingest`, `db` and `server`. Read the output to the Doc-tests lines for all four crates — piping through `tail` returns *tail's* exit code, not cargo's, so a green tail proves nothing on its own.

- [ ] **Step 3: Build the frontend**

Run: `cd frontend && npm run build`
Expected: clean.

- [ ] **Step 4: Confirm no sample file was staged**

```bash
git status --short
git log --stat feat/liquidity-risk-v2 --not main -- . | grep -iE "HISINVLUX_|HISTOVLLUX_|INVXDVLUX_|Glossary GP|NAV Recap|\.docx|\.png"
```

Expected: no matches. The untracked repo-root files must still be untracked, and no commit on this branch may contain one. If a match appears, stop and rewrite the offending commit before going further.

- [ ] **Step 5: Walk the spec**

Open `docs/superpowers/specs/2026-08-12-liquidity-risk-v2-design.md` and check each section against what was built. Note any deviation in the final report rather than silently accepting it — particularly the Out of Scope list, which must still be true.

---

## Notes for the implementer

**Where this plan is inferring rather than observing.** The JOURSRLUX and INVJCPLUX column maps come from the depositary's glossary, which proved to be the exact file layout for both files we hold samples of. That is strong evidence, not proof. The fixtures encode the assumption and therefore cannot test it. What protects against a wrong assumption is the failure discipline every adapter shares: the column-count sniff, the filename-versus-row fund-code check and the filename-versus-row date check all reject a mis-shaped file loudly at upload instead of importing plausible wrong numbers. Do not weaken any of the three to make a fixture pass — fix the fixture.

**The coupon frequency is the single riskiest number.** It divides the coupon, so a wrong value scales the inflow directly. Task 3's three-step resolution ends in *no coupon and a named gap*, never a default. If a later task makes that inconvenient, the answer is to surface the gap better, not to pick a frequency.

**`RefHint` and `RefFact` differ deliberately.** Hints fill NULLs because the user may override them. Facts overwrite because the depositary restates them daily. Neither ever touches `liquidity_days`, `adv_eligible`, `adv_30d` or `adv_asof`. If a future change needs an import to write one of those four, that is a design decision to raise, not an implementation detail.
