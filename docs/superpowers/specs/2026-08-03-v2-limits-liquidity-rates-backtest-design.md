# Borobudur Risk Tool v2 — Limits, Liquidity, Rates, VaR Back-testing — Design

**Date:** 2026-08-03
**Status:** Approved by user (sections reviewed interactively)
**Baseline:** v1 merged main (all 18 tasks of `2026-07-30-borobudur-risk-tool.md`)

## Purpose

Extend the v1 market-risk dashboard with the remaining pillars a UCITS risk
manager monitors daily:

1. **Concentration limits** — 5/10/40 + 20% group on transferable securities,
   20% per target fund, 20% deposits per banking group.
2. **Liquidity bucketing** — time-to-liquidate distribution vs a redemption
   stress, fully offline via static rules + per-position overrides.
3. **Rate risk** — bond YTM, modified duration and DV01 from parsed/entered
   bond reference data.
4. **VaR back-testing** — daily 1-day/99% VaR vs realized returns, all three
   methods, Basel traffic-light zones and Kupiec POF test.

## Key decisions (user-confirmed)

1. **Issuer identification: auto + editable mapping.** Defaults derived from
   position data; a UI table lets the user merge issuers into connected
   groups; mapping persists in DB across imports.
2. **Limits monitored:** 5/10/40 + 20% group; 20% per target fund; 20%
   deposits per bank. OTC-counterparty 5/10% and the 10% control limits are
   out of scope (no OTC positions / no shares-outstanding data).
3. **Liquidity: static rules + overrides, no market-data feed.** Defaults per
   asset type, per-position overrides in the same editor. Offline; a volume
   feed is a possible v3.
4. **Bond statics: parse from position name + override.** Coupon and maturity
   regex-parsed from names like `BRAZILIAN GOVERNMENT INTL BOND 6.625%
   15-03-35`; editable; imports never overwrite user edits.
5. **Back-testing: all 3 VaR methods + Kupiec POF**, pinned to 1-day / 99%
   regardless of the VaR page's user-selected parameters.
6. **Architecture: one shared `instrument_refs` table + one "Limits" page**
   (approach A) rather than per-concern tables or non-persistent heuristics.

## Data model

### New migration `crates/db/migrations/0002_refs.sql`

```sql
CREATE TABLE instrument_refs (
    code            TEXT PRIMARY KEY,      -- position code: ISIN or account code
    issuer_group    TEXT,                  -- NULL = use derived default
    liquidity_bucket TEXT,                 -- NULL = use asset-type default; else 'd1'|'d2_7'|'d8_30'|'d30p'
    bond_coupon_pct NUMERIC,               -- annual coupon in %, e.g. 6.625
    bond_maturity   DATE,
    bond_coupon_freq INT,                  -- coupons per year (1 or 2)
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

Semantics: a NULL field means "use the default"; only overrides are stored.
`liquidity_bucket` values are constrained by a CHECK to
`('d1','d2_7','d8_30','d30p')`.

### New settings rows (seeded in the same migration)

| key | default | meaning |
|---|---|---|
| `liquidity_defaults` | `{"Action":"d1","Fonds":"d2_7","Future":"d1","Obligation":"d8_30","Cash Acc":"d1","Margin Acc":"d1","Dividendes":"d1","Frais provisionnés":"d1","Provisions ordres":"d1"}` | asset type → default bucket (JSON, editable) |
| `redemption_shock` | `0.30` | stress outflow as fraction of NAV, checked against ≤7d liquid assets |

Asset types not present in the map default to `d1`.

## Derived defaults (zero-editing behavior)

- **Issuer group default:**
  - `Action`, `Obligation`, `Fonds`, `Dividendes`: the position `name`,
    uppercased and whitespace-normalized.
  - `Cash Acc`, `Margin Acc`: the bank code parsed as the token after the
    last `- ` in the name (`Depositary Bk- CBLU` → `CBLU`,
    `Managed acc - CABK` → `CABK`). If no `- ` present, the full name.
  - Other types: the position name (they never enter issuer checks anyway).
- **Liquidity bucket default:** `liquidity_defaults[asset_type]`, else `d1`.
- **Bond statics (import-time seeding):** during import of each `Obligation`
  row, parse the name with:
  - coupon: first match of `(\d+(?:[.,]\d+)?)\s*%` → `bond_coupon_pct`
  - maturity: first match of `(\d{2})-(\d{2})-(\d{2,4})` after the coupon
    match, read as DD-MM-YY(YY); 2-digit years are 20YY.
  - frequency: 2 if position currency is USD, else 1.
  Parsed values are written to `instrument_refs` **only where the column is
  currently NULL** (COALESCE-style upsert); a failed parse leaves NULLs.
  Import behavior is otherwise unchanged; parse failures never fail the
  import.

## Analytics definitions

All computed in the `analytics` crate on plain inputs (no I/O), server
assembles inputs from DB.

### Concentration (per snapshot date; default latest)

Let `w_i` = position weight (valuation_eur / NAV of that date's snapshot,
i.e. the stored `weight`).

- **Issuer exposures (5/10/40 scope):** rows with asset_type `Action` or
  `Obligation`, plus `Dividendes` rows folded into the same issuer group.
  Exposure per issuer group = sum of positive `w_i` (long only; negative
  lines within a group offset down to a floor of 0 for the group).
  - Check `issuer_10`: every issuer group ≤ 10%.
  - Check `forty`: sum of issuer-group exposures > 5% must be ≤ 40%.
  - Check `group_20`: every issuer group ≤ 20% (binds only when the user
    merges issuers into a wider connected group; with defaults it is
    dominated by `issuer_10`).
- **Funds:** each `Fonds` row vs 20% NAV (`fund_20`); aggregate of all
  `Fonds` shown as memo (no limit attached; all targets assumed UCITS).
- **Deposits:** net aggregate of `Cash Acc` + `Margin Acc` weights per bank
  issuer group; checked ≤ 20% only when the net aggregate is positive
  (`deposit_20`).
- **Excluded:** `Future` rows (not issuer exposure under 5/10/40; noted in
  UI), fee/provision rows.
- **Status per check row:** `ok` (< 80% of limit), `watch` (≥ 80% and ≤
  limit), `breach` (> limit).

### Liquidity (per snapshot date)

- Effective bucket per position: override else default.
- Aggregates over **long** rows only (`w_i > 0`): % NAV per bucket, plus
  cumulative curve in bucket order `d1 → d2_7 → d8_30 → d30p`.
- Memo: sum of negative weights (payables, negative cash) reported
  separately, not netted.
- **Stress check:** `ok` if `cum(d1)+... through d2_7 ≥ redemption_shock`,
  i.e. assets liquidatable in ≤ 7 days cover the shock; else `breach`.

### Rates (per snapshot date)

For each `Obligation` row with complete statics (coupon, maturity,
frequency) from `instrument_refs`:

- Clean price `P` = position `price` (per 100). Accrued interest is carried
  in the file separately and is not used in YTM.
- **YTM**: solve `P = Σ (c/f)·100 / (1+y/f)^k + 100 / (1+y/f)^n` by
  bisection on `y ∈ [-0.5, 1.0]`, tolerance 1e-8, where `c` = coupon/100,
  `f` = frequency, `n` = number of remaining coupon periods (ceil of years
  to maturity × f, from the snapshot date, ACT/365.25 year fraction), and
  the first coupon at the fractional period to next coupon date
  (approximation: periods spaced 1/f years back from maturity).
- **Modified duration** `MD = MacaulayDuration / (1 + y/f)`.
- **DV01 €** `= MD × market_value_eur × 0.0001` where market_value_eur =
  the position `valuation_eur`.
- Portfolio totals: Σ DV01 €, and rate sensitivity `% NAV per 100 bp =
  Σ (MD_i × w_i)`.
- Bonds with missing statics render "missing reference data" with a link to
  the editor; they are excluded from totals with a warning badge.
- **Caveat displayed:** bond futures (`Future` rows whose name matches
  OAT/BUND/BONO/US 10YR/EURO BOND patterns — in practice: all `Future`
  rows are simply listed) are NOT included; their notional/CTD duration is
  not derivable from the file.

### VaR back-testing

- Parameters pinned: **horizon 1 day, confidence 99%**. Window `W` =
  `var_window_days` setting (default 252). A back-test point requires a
  full `W` prior returns; if no date qualifies, `insufficient: true` and
  the section shows an "insufficient history" badge.
- For each date `t` with `≥ W` prior returns: compute 1d/99% VaR by
  historical, Gaussian, Cornish-Fisher on returns `(t−W, t]`. **Exception**
  for method m at `t` when `r_{t+1} < −VaR_m(t)`.
- Per method, over the trailing `min(250, available)` back-test points:
  - exception count `x` and sample size `n` (badge "partial: n/250" when
    n < 250);
  - **Basel zone** (scaled thresholds when n < 250 are NOT applied — zones
    use the raw count as if n were 250, with the partial badge as the
    caveat): green `x ≤ 4`, yellow `5 ≤ x ≤ 9`, red `x ≥ 10`;
  - **Kupiec POF**: LR = −2·ln[ (1−p)^{n−x} p^x / ((1−x/n)^{n−x} (x/n)^x) ]
    with p = 0.01; p-value = 1 − χ²₁(LR) (survival function of chi-squared,
    1 df). x = 0 uses the limit form LR = −2(n−x)·ln(1−p). Reject (flag) at
    p-value < 0.05.
- Chart series: daily return per date, the three −VaR_m(t) lines, and
  exception markers per method.

## API

New endpoints (all JSON, problem-details on error, same conventions as v1):

| Endpoint | Returns |
|---|---|
| `GET /api/metrics/concentration?date=` | `{date, dates, checks:[{check, scope_label, limit, rows:[{group, weight, status}], status}], excluded_note}` — checks: issuer_10, forty (single-row aggregate), group_20, fund_20, deposit_20 |
| `GET /api/metrics/liquidity?date=` | `{date, dates, buckets:[{bucket, weight}], cumulative:[{bucket, weight}], negative_memo, shock, stress_status}` |
| `GET /api/metrics/rates?date=` | `{date, dates, bonds:[{code, name, coupon_pct, maturity, freq, price, ytm, mod_duration, dv01_eur, weight} | {code, name, missing: true}], total_dv01_eur, nav_sensitivity_100bp, futures_note:[names]}` |
| `GET /api/metrics/backtest` | `{window, confidence: 0.99, horizon_days: 1, n_points, methods:{historical|gaussian|cornish_fisher: {exceptions, n, zone, kupiec_lr, kupiec_p, reject}}, series:[{date, ret_next, var_hist, var_gauss, var_cf, exc_hist, exc_gauss, exc_cf}], insufficient: bool}` |
| `GET /api/refs` | rows = latest-snapshot positions LEFT JOIN instrument_refs: `{code, name, asset_type, effective_issuer_group, issuer_group_override, effective_bucket, bucket_override, bond_coupon_pct, bond_maturity, bond_coupon_freq, is_bond}` |
| `PUT /api/refs/{code}` | body with any of `{issuer_group, liquidity_bucket, bond_coupon_pct, bond_maturity, bond_coupon_freq}`; `null` field ⇒ revert to default (store NULL). Validates bucket enum, coupon ≥ 0, freq ∈ {1,2}, maturity a valid date. |
| `GET/PUT /api/settings` | extended with `liquidity_defaults` (JSON object string) and `redemption_shock` (0 < x < 1) |

`?date=` semantics identical to `/api/positions` (default latest snapshot;
`dates` lists available snapshot dates).

## UI

- **New sidebar page "Limits"** (route `/limits`), three sections:
  1. *Concentration* — status cards (worst status per check), then tables
     per check with weight bars vs limit, status chips; note about excluded
     futures.
  2. *Liquidity* — bucket bar chart, cumulative curve with the
     `redemption_shock` line, stress status chip, negative-weights memo.
  3. *Rates* — bond table (coupon, maturity, YTM, MD, DV01), totals, bond
     futures caveat listing the future positions.
  Snapshot date selector shared by the three sections (same pattern as the
  Data page positions table).
- **VaR page**: new "Back-testing" section below existing content — three
  method cards (exceptions x/n, zone chip colored green/yellow/red, Kupiec
  p-value with reject flag) + the exceptions chart. Partial/insufficient
  badges per the analytics rules.
- **Data page**: new "Reference data" section — table from `GET /api/refs`,
  inline-editable cells for issuer group, bucket (select), and the three
  bond fields (only enabled on `Obligation` rows); overridden cells are
  visually marked with a "reset" affordance (sends `null`).
- **Settings editor** gains `redemption_shock` (percent input) and the
  liquidity defaults (one select per asset type).

## Error handling

- Same problem-details conventions as v1. `PUT /api/refs/{code}` →
  422 on validation failure.
- Metrics endpoints return their `empty`/insufficient shapes rather than
  errors when data is missing (consistent with v1 `MIN_OBS` behavior).

## Testing

- `analytics`: hand-computed fixtures — concentration ratios on a toy
  portfolio incl. group merge and 40% rule; liquidity aggregation +
  stress; YTM/MD/DV01 against a worked bond example (e.g. 6.625%
  semi-annual, verify round-trip price(ytm)=P); Kupiec LR/p against
  published values (e.g. n=250, x=5 ⇒ LR≈1.9, p≈0.17); back-test exception
  counting on a constructed series.
- `ingest`: bond-name parsing cases (standard, comma decimal, 4-digit year,
  no match).
- `db`/API: integration tests on temp embedded Postgres — refs upsert +
  revert-to-default, import seeding does not overwrite overrides,
  concentration/liquidity/rates/backtest endpoints on fixture data,
  settings validation.
- `frontend`: type-check + production build.

## Out of scope (v2)

- Market-data/volume feeds for liquidity (candidate v3).
- Bond-futures DV01 (needs contract multipliers + CTD data).
- OTC counterparty limits, 10% control limits, non-UCITS 30% aggregate.
- Stored breach history for concentration (computed per snapshot on read).
