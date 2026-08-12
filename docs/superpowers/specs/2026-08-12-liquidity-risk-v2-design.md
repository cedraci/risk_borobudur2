# Liquidity Risk v2 — ADV-based days-to-liquidate, redemption scenarios — Design

**Date:** 2026-08-12
**Status:** Approved by user (sections reviewed interactively)
**Baseline:** `main` @ `b96497c` (universal ingest + CACEIS adapters merged)

## Purpose

Replace the static liquidity bucket model with a days-to-liquidate engine that
answers, for each portfolio and snapshot date:

1. **Asset side** — how many days to unwind each position, in normal markets
   and in stressed markets, from Bloomberg 30-day average traded volume.
2. **Liability side** — how long it takes to fund a redemption by the five
   largest shareholders, or a fixed percentage of NAV.
3. **Hybrid** — either liability shock combined with stressed volumes.

Today's model stores a *bucket* (`d1`, `d2_7`, `d8_30`, `d30p`) as the truth.
None of the questions above can be answered from a bucket: they need a daily
sellable **amount** and a **day count** that arithmetic can be done on. This
design makes days the primitive and demotes buckets to chart axis labels.

## Decisions

1. **Days are the stored primitive.** `analytics::liquidity` is rewritten
   around a capacity-per-day function; bucket bands are computed at render
   time.
2. **ADV drives the number where it exists**, with a per-instrument or
   per-asset-type **days** fallback everywhere else. One arithmetic path
   regardless of data quality.
3. **Participation cap plus a stress factor.** Normal capacity is
   `ADV x participation` (default 25%); the stressed scenario additionally
   shrinks ADV to 30% of normal ("cut by 70%"). Both are per-portfolio
   settings.
4. **Both sell orderings reported side by side.** Waterfall is the
   operational answer, vertical slice the fairness answer, and the gap
   between them is itself the signal.
5. **Shareholder concentration is a manually maintained register.**
   JOURSRLUX is share-class level and carries no investor-level holdings, so
   the depositary feed cannot supply the top five.
6. **Bond coupon and maturity inflows are included**, computed from bond
   statics already held in `instrument_refs`. No new depositary file is
   required.
7. **Bloomberg calls are user-initiated only, and scoped.** ADV gets its own
   export, separate from the classification workbook, restricted to
   instruments that need it.
8. **Negative positions become an immediate cash need**, closing a defect in
   the current model where payables were reported as a memo and never counted
   against liquid assets in the pass/fail check.

## Units and conventions

- **Days are business days** (Monday to Friday, no holiday calendar). This is
  a deliberate simplification, stated in the UI parameters strip.
- Amounts are EUR (`valuation_eur`), consistent with the rest of the tool.
- `weight` remains a fraction, per the project-wide constraint.
- The asset vocabulary stays closed: `Action`, `Fonds`, `Obligation`,
  `Future`, `Cash Acc`, `Margin Acc`, `Dividendes`, `Frais provisionnés`,
  `Provisions ordres`.

## Data model

### Migration `crates/db/migrations/0011_liquidity_v2.sql`

```sql
ALTER TABLE instrument_refs
    ADD COLUMN adv_30d        NUMERIC,   -- 30-day average daily volume, in shares/units
    ADD COLUMN adv_asof       DATE,      -- upload date of the Bloomberg response that set adv_30d
    ADD COLUMN liquidity_days NUMERIC;   -- NULL = use the asset-type default

-- Backfill days from the retired bucket at each band's conservative upper edge.
UPDATE instrument_refs SET liquidity_days = CASE liquidity_bucket
    WHEN 'd1' THEN 1 WHEN 'd2_7' THEN 7 WHEN 'd8_30' THEN 30 WHEN 'd30p' THEN 60 END
    WHERE liquidity_bucket IS NOT NULL;

ALTER TABLE instrument_refs DROP COLUMN liquidity_bucket;

ALTER TABLE instrument_refs ADD CONSTRAINT instrument_refs_liquidity_days_nonneg
    CHECK (liquidity_days IS NULL OR liquidity_days >= 0);
ALTER TABLE instrument_refs ADD CONSTRAINT instrument_refs_adv_nonneg
    CHECK (adv_30d IS NULL OR adv_30d >= 0);

CREATE TABLE shareholders (
    id           BIGSERIAL PRIMARY KEY,
    portfolio_id BIGINT  NOT NULL REFERENCES portfolios(id),
    label        TEXT    NOT NULL,
    pct_of_nav   NUMERIC NOT NULL CHECK (pct_of_nav > 0 AND pct_of_nav <= 100),
    as_of        DATE    NOT NULL
);
CREATE INDEX shareholders_portfolio_idx ON shareholders (portfolio_id);
```

`liquidity_days` carries double duty: it is the per-instrument override, and
for a target fund it expresses the contractual dealing lag (redemption
frequency + notice period + settlement), because that lag *is* the fund's
days-to-liquidate. No separate column is needed.

`pct_of_nav` rather than a share count keeps the register trivial to maintain
by hand and lets it revalue automatically as NAV moves.

### Settings (per portfolio)

Added to `AppSettings` in `crates/db/src/settings.rs`, following the existing
code-default convention (`get_f` / `get_u` with a fallback, no seeding
required):

| key | default | meaning |
|---|---|---|
| `participation_rate` | `0.25` | share of a day's volume the fund can be |
| `adv_stress_factor` | `0.30` | ADV retained in the stressed scenario |
| `liquidity_horizon_days` | `60` | business days the coverage curve runs to |
| `settlement_deadline_days` | `3` | business days by which a redemption must be funded |
| `adv_max_age_days` | `7` | age past which an `adv_asof` is stale and due for refresh |
| `liquidity_default_days` | see below | asset type → fallback days (JSON, editable) |

```json
{"Action": 1, "Fonds": 7, "Obligation": 30, "Future": 1,
 "Dividendes": 1, "Frais provisionnés": 1, "Provisions ordres": 1}
```

`redemption_shock` (existing, default `0.30`) is reused unchanged as the fixed
redemption percentage. `liquidity_defaults` (the bucket map) is retired and
replaced by `liquidity_default_days`, migrated by the same upper-edge mapping
used for the column.

`Cash Acc` and `Margin Acc` are deliberately absent from the map: they are
capacity-infinite by engine rule (zero days), not by table entry.

## The engine

New module `crates/analytics/src/liquidity.rs` (rewrite). Every function is
pure; no database access.

### Per-position capacity

```
unit_price_eur   = valuation_eur / quantity                  (quantity > 0)
capacity_eur_day = adv_30d x participation x stress x unit_price_eur
days             = valuation_eur / capacity_eur_day
```

When `adv_30d` is NULL, zero, or `quantity` is NULL or zero, the position
takes the fallback path instead:

```
fallback_days    = liquidity_days
                   else liquidity_default_days[asset_type]
                   else 1
capacity_eur_day = valuation_eur / fallback_days
```

which yields `days = fallback_days` exactly, so both paths agree by
construction. `Cash Acc` and `Margin Acc` have infinite capacity and zero
days. A `fallback_days` of zero is likewise treated as infinite capacity.

The stress factor applies **only** on the ADV path. A fallback days figure is
already an assumption about how long the position takes to sell and is not
re-stressed; this is stated in the UI so the stressed asset profile is not
misread as covering the whole portfolio.

`Future` positions take the fallback path even when an ADV exists, because a
margined contract's `valuation_eur` is its mark-to-market, not its notional,
so `valuation_eur / quantity` is not a price that volume can be measured
against. Futures keep their one-day default; their exposure is the subject of
the derivatives section, not this one.

### Cumulative availability

For a horizon of `H` business days, and with `d` in `1..=H`:

```
A(d) = SUM over positions with valuation_eur > 0 of min(valuation_eur, capacity_eur_day x d)
     + inflows(d)
     + SUM over positions with valuation_eur < 0 of valuation_eur
```

The third term is negative and applies from day 1: payables and negative cash
are an immediate call on liquidity. They remain reported separately as
`negative_memo` for continuity, but they now also reduce availability, which
the current model does not do.

`inflows(d)` sums bond coupons and redemptions landing on or before day `d`.
For each `Obligation` position holding complete statics
(`bond_coupon_pct`, `bond_maturity`, `bond_coupon_freq`), coupon dates are
walked back from `bond_maturity` in `12 / freq` month steps; each coupon
strictly after the snapshot date and within the horizon contributes
`quantity x bond_coupon_pct / 100 / freq`, and the maturity itself contributes
`quantity` (nominal). Both are converted at the position's `fx_rate` and
credited at the business-day offset of their calendar date from the snapshot
date. Bonds with incomplete statics contribute no inflows and are listed in
the coverage block.

### Sell orderings

With `R` the required amount and `NAV` the snapshot net asset value:

- **Waterfall** — `days = min { d in 1..=H : A(d) >= R }`. If no such `d`
  exists, `days` is null and `unmet_eur = R - A(H)`.
- **Vertical slice** — `f = R / NAV`, then
  `days = max over positions with valuation_eur > 0 of (f x valuation_eur / capacity_eur_day)`.
  Slice ignores inflows, which is conservative and keeps the fairness measure
  a pure property of the holdings.

Slice is always the slower of the two.

### Residual composition

After a waterfall completing at `d*`, each position's realised sale is
`min(valuation_eur, capacity_eur_day x d*)`, allocated in ascending `days`
order and scaled so the total equals `R`. The reported figures are the share
of NAV in positions with `days > 30` before the redemption, and the same share
of the *remaining* fund `(NAV - R)` after it. The increase is the dilution
imposed on investors who stayed.

## Scenarios

The asset profile and the scenario list are separate blocks in the response.

**Asset profile** — no required amount. Positions with `valuation_eur > 0`
are distributed by `days` across the bands `0-1`, `2-7`, `8-30`, `>30` (keys
`d1`, `d2_7`, `d8_30`, `d30p` for continuity), by weight, with the cumulative
curve. Negative positions are excluded here and reported as `negative_memo`,
as in the current model; they enter the arithmetic only through `A(d)`.
Computed twice:
`normal` (stress = 1.0) and `stressed` (stress = `adv_stress_factor`).

**Scenarios** — four entries, each carrying `required_eur`, `required_pct`,
`waterfall`, `slice`, `unmet_eur`, `curve` and `residual`:

| key | required amount `R` | stress |
|---|---|---|
| `top5` | sum of the 5 largest `pct_of_nav` register entries x NAV | 1.0 |
| `fixed` | `redemption_shock` x NAV | 1.0 |
| `hybrid_top5` | as `top5` | `adv_stress_factor` |
| `hybrid_fixed` | as `fixed` | `adv_stress_factor` |

If the register holds fewer than five entries, all of them are used and the
count is reported. If it is empty, `top5` and `hybrid_top5` return
`unavailable` with the reason `"no shareholder register"` — never a zero and
never a pass.

**Status chip.** Per scenario, `ok` when `waterfall.days` is non-null and
`<= settlement_deadline_days`, otherwise `breach`. This replaces the current
"assets liquidatable in ≤ 7 days cover 30%" test with the question that
matters operationally: does the money arrive by the contractual date.

## Bloomberg ADV refresh

The server has no Bloomberg connectivity. It writes `BDP` / `BDH` formula
text into a workbook; formulas resolve only when the user opens the file in
Excel on a terminal machine. Reading the Limits page reads `adv_30d` from
Postgres and cannot emit a call. The following rules keep the *user-initiated*
calls small.

1. **Separate export.** `GET /api/bloomberg/adv-request` is distinct from the
   existing `GET /api/bloomberg/request`. Country and GICS are one-and-done —
   a classified instrument leaves that workbook forever. ADV decays daily and
   never leaves. Bundling them would turn every classification export into a
   fleet-wide volume request.
2. **Scoped rows.** Only instruments held in the latest snapshot of
   non-archived portfolios; only `Action` (bonds, target funds and futures use
   their days fallback by design and never generate a call); deduplicated
   fleet-wide; and by default only those whose `adv_asof` is NULL or older
   than `adv_max_age_days`. A `?all=true` query parameter forces a full
   rebuild.
3. **Cost shown before it is paid.** `GET /api/bloomberg/adv-due` returns
   `{due, held}` from the database alone, so the panel can display "N of M
   held instruments due for refresh" before anything is exported.
4. **One cell per instrument.** ADV is a single
   `BDP({ISIN} {market_sector}, "VOLUME_AVG_30D")` point value, not a `BDH`
   history series. The market sector is written per row in its own cell, so a
   non-resolving row can be corrected in Excel exactly as the existing REFS
   sheet allows.
5. **Staleness never triggers anything.** A stale `adv_asof` renders as a
   warning and the position falls back to its days figure, flagged in the
   coverage block. No part of the UI initiates a refresh on the user's behalf.

The existing upload endpoint accepts either workbook and stores whichever
sheets it finds, so there is one upload path for the user. ADV values are
stored with `adv_asof` set to the upload date; unresolved cells flow through
the existing `#N/A` skipped-cell reporting and are not stored.

## API

All routes follow existing conventions: problem-details on error, `?date=`
semantics identical to `/api/positions`, and `dates` listing available
snapshots.

| Endpoint | Returns |
|---|---|
| `GET /api/portfolios/{id}/metrics/liquidity?date=` | `{date, dates, params, coverage, asset:{normal, stressed}, scenarios:[...], negative_memo}` |
| `GET /api/portfolios/{id}/shareholders` | `[{id, label, pct_of_nav, as_of}]` |
| `PUT /api/portfolios/{id}/shareholders` | replaces the register; 422 when any `pct_of_nav` is outside `(0, 100]`, when the total exceeds 100, or when `label` is empty after trimming |
| `GET /api/bloomberg/adv-request` | the ADV request workbook (`?all=true` for a full rebuild) |
| `GET /api/bloomberg/adv-due` | `{due, held}` |
| `PUT /api/refs/{code}` | gains `liquidity_days` in place of `liquidity_bucket`; `null` reverts to the default; `adv_30d` and `adv_asof` are read-only and rejected in the body |

`params` echoes the resolved settings (participation, stress factor, horizon,
deadline, fixed percentage, day-unit note) so no displayed number is
unexplained. `coverage` reports `adv_pct_of_nav`, the list of codes on the
fallback path with a reason (`no adv`, `stale adv`, `no quantity`,
`asset type`), the list of bonds with incomplete statics, and the register's
`as_of` with a staleness flag.

## UI

**Limits page, Liquidity section** — four stacked pieces:

1. A parameters strip showing the resolved settings and the business-day
   convention.
2. A coverage chip: "ADV measured on 61% of NAV, 3 instruments stale".
3. The asset profile chart: normal and stressed bars side by side over the
   four day bands, with the cumulative curves.
4. A scenario table of four rows — waterfall days, slice days, unmet amount,
   status chip. Selecting a row draws its coverage curve below, with the
   required amount as a horizontal line and the settlement deadline as a
   vertical one.

**Data page** — the reference-data editor's bucket dropdown becomes a
`liquidity_days` number input and gains a read-only ADV column with its as-of
date, so measured and assumed instruments are distinguishable at a glance. A
new shareholder register editor sits alongside it (label, percent of NAV,
as-of date, add and remove rows), placed here because it is data maintenance;
its staleness surfaces on Limits.

**Bloomberg panel** — a second export button for ADV, labelled with the due
count from `adv-due`, a full-rebuild checkbox, and text stating that formulas
resolve only in Excel.

## Error handling

Governed by the project's "signal, don't hide" rule.

- A missing or stale ADV is never treated as infinite liquidity; the position
  falls back and is named in `coverage` with its reason.
- An empty shareholder register makes `top5` and `hybrid_top5` return
  `unavailable` with a reason, not a zero and not a pass.
- A bond with incomplete statics contributes no inflows and is listed.
- A missing NAV or an empty snapshot returns the established empty shape
  rather than an error, matching the other metrics endpoints.
- `PUT /api/portfolios/{id}/shareholders` returns 422 on validation failure.

## Testing

- **`analytics`** — pure hand-computed fixtures: the capacity formula on both
  paths, agreement of the two paths at the fallback boundary, both orderings
  against the worked example (500,000 shares against 100,000 ADV at 25%
  participation giving 20.0 days normal and 66.7 days stressed), coupon and
  maturity inflows landing on the correct business-day offsets, negative
  positions reducing availability from day 1, and the boundaries: zero ADV,
  zero quantity, a position that cannot clear within the horizon, an empty
  portfolio.
- **`db`** — register CRUD and its constraints, the `liquidity_days` backfill
  migration over pre-existing bucket values, ADV storage with `adv_asof`, and
  the `adv-due` scoping query.
- **`server`** — one test pinning the full liquidity response shape, one
  end-to-end scenario computed through the real stack, one asserting the ADV
  request workbook contains only due instruments, and 422 coverage on the
  register endpoint.
- **`frontend`** — `npm run build` remains the gate.

Per the project constraint, the dev server must be stopped before
`cargo test`, and no shared server test harness is introduced.

## Out of scope

Deliberately excluded, with the seam left open where noted:

- **JOURSRLUX flow history.** The liability shock is an interface fed by the
  register today; a historical net-flow series calibrating the assumed shock
  against observed redemptions drops in behind that interface later. Blocked
  on a sample file.
- **Settlement-lag derivation** from the trade-date / settlement-date pairs in
  REGLMTLUX, RAPDECLUX and JOUROPLUX. Blocked on sample files.
- **INVJCPLUX refinement** of the inflow side (exact next-coupon dates,
  floater rates). The bond statics already held are sufficient for v2.
- **Price impact and liquidation haircuts.** This design measures elapsed
  time, not execution cost.
- **A holiday calendar.** Business days are Monday to Friday.
- **An exportable stress-test report.** The dashboard is the deliverable.
