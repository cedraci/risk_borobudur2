# Liquidity Risk v2 — ADV-based days-to-liquidate, redemption scenarios — Design

**Date:** 2026-08-12
**Status:** Approved by user (sections reviewed interactively)
**Baseline:** `main` @ `b96497c` (universal ingest + CACEIS adapters merged)

**Revision, 2026-08-12.** The depositary can supply JOURSRLUX, REGLMTLUX,
RAPDECLUX and INVJCPLUX daily. Inspecting the glossary against the two files
we hold samples of showed the glossary's column order *is* the file layout —
every column index in both existing adapters lands on the matching header
name, and the HISINVLUX sheet's 66 columns match the parser's minimum — so
the remaining feeds can be mapped from the glossary alone. That inspection
also found two things already sitting unused in HISINVLUX: the bond coupon
schedule, and the market place, which is a far better ADV-eligibility test
than asset type. Sections marked *(rev.)* below changed as a result; the
scenarios and the sell orderings are unchanged.

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
6. **Bond coupon and maturity inflows are included**, read from the coupon
   schedule HISINVLUX already carries *(rev.)*. No reconstruction, and no new
   file required for the default horizon.
7. **Bloomberg calls are user-initiated only, and scoped.** ADV gets its own
   export, separate from the classification workbook, restricted to
   instruments that need it.
8. **Negative positions become an immediate cash need**, closing a defect in
   the current model where payables were reported as a memo and never counted
   against liquid assets in the pass/fail check.
9. **ADV eligibility follows the trading venue, not the asset type**
   *(rev.)*. Listed ETFs and ETCs are among the most volume-measurable things
   the fund holds, and an asset-type rule would have silently assumed days
   for all of them.
10. **The liability shock is calibrated against observed flows** *(rev.)*.
    JOURSRLUX supplies daily subscriptions and redemptions per share class;
    the worst observed net outflow is reported beside the configured
    percentage. It never overwrites the setting on its own.
11. **`days` means days to sell, not days until cash settles.** The
    contractual settlement deadline governs the pass/fail chip, so the
    funding question is still asked, once rather than twice.

## Units and conventions

- **Days are business days** (Monday to Friday, no holiday calendar). This is
  a deliberate simplification, stated in the UI parameters strip.
- Amounts are EUR (`valuation_eur`), consistent with the rest of the tool.
- `weight` remains a fraction, per the project-wide constraint.
- The asset vocabulary stays closed: `Action`, `Fonds`, `Obligation`,
  `Future`, `Cash Acc`, `Margin Acc`, `Dividendes`, `Frais provisionnés`,
  `Provisions ordres`.

## Data model

### Migration `crates/db/migrations/0011_liquidity_v2.sql` *(rev.)*

```sql
ALTER TABLE instrument_refs
    ADD COLUMN adv_30d        NUMERIC,   -- 30-day average daily volume, in shares/units
    ADD COLUMN adv_asof       DATE,      -- upload date of the Bloomberg response that set adv_30d
    ADD COLUMN liquidity_days NUMERIC,   -- NULL = use the asset-type default
    -- depositary-maintained, overwritten on every HISINVLUX import
    ADD COLUMN market_place       TEXT,  -- CACEIS venue code, HISINVLUX col 63
    ADD COLUMN market_place_name  TEXT,  -- col 64, display only
    ADD COLUMN bond_next_coupon   DATE,  -- col 57
    ADD COLUMN bond_nominal       NUMERIC, -- col 56, the denomination prices quote against
    -- user override of the derived venue rule; NULL = derive
    ADD COLUMN adv_eligible    BOOLEAN;

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

-- One JOURSRLUX row per share class per day. History accumulates as files
-- are loaded; the table is the only record of it.
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

Both flow amounts are stored as magnitudes: CACEIS's sign convention for the
redemption column is not observable without a sample, so the parser takes the
absolute value of each and derives direction from which column the amount sat
in. `net_flow = subscription_amount - redemption_amount`.

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
| `flow_lookback_days` | `250` | trailing window of loaded flow history used for the observed-outflow statistics |

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

### ADV eligibility *(rev.)*

An instrument is on the ADV path only if it is eligible, and eligibility
follows the trading venue:

```
eligible = COALESCE(refs.adv_eligible,          -- explicit user override
                    asset_type != 'Future'
                    AND CASE WHEN market_place IS NULL
                             THEN asset_type = 'Action'
                             ELSE market_place NOT IN NON_MARKET_CODES
                        END)
```

`NON_MARKET_CODES` is a single documented constant in `analytics`:
`FOR` (cours forcé — futures, cash, provisions), `260` (organisme de
placement collectif — unlisted target funds), `999` (internal funds
publication) and `254` (technical quotation place). Everything else in the
sample is a real exchange: Euronext Paris and Amsterdam, XETRA, Frankfurt,
SIX, Athens, Milan, Wiener Börse, Madrid, NYSE, NASDAQ, Irish and London.

This replaces an `asset_type == 'Action'` rule that would have excluded the
portfolio's listed ETFs and its gold and agriculture ETCs — instruments that
trade on exchange and whose volume is exactly the number this design is built
on. The venue rule admits them and still excludes the unlisted target funds,
futures and cash. When `market_place` is NULL, which is the case for
portfolios loaded from a NAV Recap rather than from CACEIS, the rule degrades
to the old asset-type test so behaviour there is unchanged.

The rule deliberately admits the ICSD-quoted bond (venue `186`), whose
reported volume understates the OTC market it actually trades in. That
overstates its days-to-liquidate, which is the conservative direction, and
the coverage block names it as measured so the figure is never mistaken for a
dealer quote. `adv_eligible` forces it back to the days fallback if that is
preferred.

Over-inclusion is the safe direction of error generally: an instrument that
turns out not to have a meaningful volume returns `#N/A`, falls back to its
days figure and appears in the coverage block. Under-inclusion silently
assumes liquidity that was never measured.

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

`inflows(d)` sums bond coupons and redemptions landing on or before day `d`
*(rev.)*. The schedule is read, not reconstructed: HISINVLUX carries
`Next coupon date`, `Maturity Date`, `Coupon Type`, `Coupon rate` and
`Nominal` on every position row and the depositary refreshes them daily.

For a bond, `quantity` is the face amount and `price` quotes per 100 of
`bond_nominal`. The sample confirms it: 2,000,000 face of the Brazil 6.625%
2035 at 101.923 gives a local market value of 2,038,460.

- **Coupon.** When `bond_coupon_pct > 0`, `Coupon Type` is `FIX`, and
  `bond_next_coupon` falls strictly after the snapshot date and within the
  horizon, the position contributes `quantity x bond_coupon_pct / 100 / freq`
  at that date. Further coupons step forward from `bond_next_coupon` in
  `12 / freq` month intervals while they remain within the horizon.
- **Redemption.** When `bond_maturity` falls within the horizon, it
  contributes `quantity`.

Both convert at the position's `fx_rate` and are credited at the business-day
offset of their calendar date from the snapshot date.

#### Resolving the coupon frequency

`freq` divides the coupon, so guessing it wrong scales the inflow directly: a
semi-annual bond treated as annual pays double. There is no safe default, and
the three sources are tried in order.

1. **`bond_coupon_freq`**, where INVJCPLUX has supplied it.
2. **Inferred from accrued interest**, which HISINVLUX already reports per
   position. With `C` the annual coupon `quantity x bond_coupon_pct / 100`,
   `A` the accrued interest, and `g` the calendar days from the snapshot to
   `bond_next_coupon`, the accrual period satisfies
   `P = 365 x A / C + g`. Snapped to the nearest of 365, 182.5, 91.25 and
   30.4 days, and accepted only within a 15% tolerance.
3. **Neither** — no coupon is credited and the position is named in the
   coverage block. A flagged omission is recoverable; a doubled inflow is
   not.

The inference is checked against the one real bond we hold. Brazil 6.625%
2035: `C` = 132,500 USD, `A` = 45,236.41 EUR against a EUR-converted `C` of
114,684, `g` = 39 days, giving `P` = 183.0 days, which snaps to semi-annual
at 0.3% error. Computing the accrual on a 30/360 basis instead gives 181.0
and snaps the same way, so the result is not sensitive to the day-count
convention we cannot see.

This is why INVJCPLUX is worth loading but is not load-bearing: it confirms a
number the position file already implies, removes the manual maintenance of
`bond_coupon_freq`, and matters most when the horizon is lengthened. At the
default 60-business-day horizon — roughly a quarter — only a monthly or
quarterly payer fits more than one coupon in range.

Zero-coupon instruments contribute no coupon. Instruments carrying CACEIS's
far-dated placeholder maturity (the ETCs in the sample show `20491231`)
contribute no redemption because it lies outside any usable horizon.
Positions with a missing `bond_next_coupon` contribute no coupon and are
named in the coverage block.

`accrued_interest` stays outside the liquidation profile. It is not
separately sellable, and the coupon inflow is what realises it. The sample
confirms `valuation_eur` excludes accrued — the weight column is computed on
the clean value — so nothing is counted twice.

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
2. **Scoped rows** *(rev.)*. Only instruments held in the latest snapshot of
   non-archived portfolios; only those the venue rule above marks eligible
   (unlisted target funds, futures and cash never generate a call);
   deduplicated fleet-wide; and by default only those whose `adv_asof` is
   NULL or older than `adv_max_age_days`. A `?all=true` query parameter
   forces a full rebuild. On the sample portfolio this is roughly fifty
   instruments on a full rebuild and a handful on a daily top-up.
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

## Observed flow history *(new)*

`share_class_flows` accumulates one row per share class per day from
JOURSRLUX. Over the trailing `flow_lookback_days` of *loaded* observations,
the engine reports the worst net outflow at three window lengths:

```
net_flow(t)        = subscription_amount(t) - redemption_amount(t)
window_flow(t, w)  = SUM over the w consecutive observations ending at t
nav(t)             = SUM over share classes of outstanding_shares x nav_per_share
worst_w            = min over t of ( window_flow(t, w) / nav(t - w + 1) )
```

reported as a positive percentage for `w` in `{1, 5, 20}`, alongside
`n_observations` and the covered date range. Windows count *loaded
observations*, not calendar gaps: if a day was never uploaded it is simply
absent, which is honest about what the history actually contains.

Below twenty observations the statistics return `unavailable` with the
observation count rather than a number computed from too little history. The
table is empty on day one and fills as files are loaded; nothing about this
is retrospective.

JOURSRLUX is share-class level and this is the one place multiple classes are
handled rather than rejected: amounts sum across classes and the NAV
denominator sums each class's own `outstanding_shares x nav_per_share`. The
NAV-per-share ambiguity that makes HISTOVLLUX reject multi-class portfolios
does not arise, because nothing here is divided by a fund-level share count.

**These numbers inform the shock; they never set it.** `worst_20d` renders
beside the configured `redemption_shock` on the Limits page with an explicit
"adopt as fixed shock" action that writes the setting. An observed history
that has never seen a stress is not evidence that no stress can happen, and a
model that quietly recalibrated itself downward on a quiet year would be
worse than one that is simply configured.

## CACEIS loader changes *(new)*

The glossary sheet for each report lists its headers in file order. That was
verified against both files we hold: every HISINVLUX index in the existing
adapter and every HISTOVLLUX index lands on the matching header name, and the
sheet's 66 columns match the parser's `H_MIN_FIELDS`. The maps below are
derived the same way. They are an inference from a document that has been
right twice, not an observation — the mitigations at the end of this section
exist for that reason.

### HISINVLUX — existing adapter, extended

Columns already present in every file we receive and currently discarded:

| index | glossary header | stored as |
|---|---|---|
| 49 | Maturity Date | `bond_maturity` |
| 56 | Nominal | `bond_nominal` |
| 57 | Next coupon date | `bond_next_coupon` |
| 59 | Coupon Type | gate: only `FIX` yields coupons |
| 60 | Coupon rate | `bond_coupon_pct` |
| 63 | Market place | `market_place` |
| 64 | Market place Description | `market_place_name` |

`H_MIN_FIELDS` stays 66. Column 46 (`Factor`) is deliberately not read: it is
`0` throughout the sample and amortising-bond factors are out of scope.

### New adapters

`filename_meta`'s existing regex already accepts both new prefixes.

| file | columns | carries |
|---|---|---|
| `JOURSRLUX_<fund>_<date>_<ts>.csv` | 15 | 0 fund, 1 NAV date, 2 share class, 3 outstanding, 4 NAV/share, 6 subscription amount, 8 redemption amount |
| `INVJCPLUX_<fund>_<date>_<ts>.csv` | 36 | 0 fund, 2 NAV date, 3 ISIN, 15 frequency, 16 last coupon date, 17 maturity, 22 rate |

`FileKind` gains `CaceisJoursr` and `CaceisInvjcp`; `detect` sniffs 15 and 36
semicolon-delimited fields respectively and applies the same filename
fund-code and date cross-checks the existing adapters use.

**The frequency encoding is the one field genuinely unknown.** CACEIS may
send `2`, `S`, `SEMI` or a month count. The parser accepts an integer in
`1..=12` directly, maps a small documented set of letter codes, and for
anything else emits a row warning and leaves `bond_coupon_freq` NULL. The
engine then falls to the accrued-interest inference, and if that is also
inconclusive it credits no coupon — it never assumes a frequency. The first
real file settles the encoding, and the warning is how we find out.

### Contract change: authoritative reference facts

`RefHint` fills `instrument_refs` only where the target column is NULL, which
is right for enrichment that the user may override. The new fields are
different: the depositary restates them daily and is authoritative. Rather
than weaken `RefHint`, `UniversalBatch` gains a second, explicit vector:

```rust
pub struct RefFact {            // overwrites; the depositary is authoritative
    pub isin: String,
    pub market_place: Option<String>,
    pub market_place_name: Option<String>,
    pub bond_maturity: Option<NaiveDate>,
    pub bond_next_coupon: Option<NaiveDate>,
    pub bond_coupon_pct: Option<f64>,
    pub bond_nominal: Option<f64>,
    pub bond_coupon_freq: Option<i32>,
}
pub ref_facts: Vec<RefFact>,    // alongside the existing fill-only ref_hints
```

`adv_eligible`, `liquidity_days` and the ADV columns are never touched by an
import: the first two are the user's, the last two are Bloomberg's.
`UniversalBatch` also gains `flows: Option<Vec<ShareClassFlowRow>>`,
following the established `Option` convention where `None` means the file
says nothing about that journal.

### REGLMTLUX and RAPDECLUX — recognised and declined

Both are added to `detect` as `Rejected` with a reason explaining the seam,
matching how JOUROPLUX is handled today. The reason is not merely the missing
sample. Everything they would contribute is *already in the snapshot* under a
different name: the settlement ledger's pending trades appear as the
`Provisions ordres` and `Frais provisionnés` rows (27 of them in the sample),
and the detached dividends RAPDECLUX would date appear as the `CPON`
positions. Those files add *dates* to amounts we already hold, not new
amounts — so consuming them without a de-duplication rule written against
observed transaction codes would double-count the liability side. Their
magnitude also argues for patience: the sample's detached dividends total
about €7,600 against a €28.8m fund.

The seam is `RefFact`'s sibling: a dated-commitment vector on
`UniversalBatch` that `A(d)` would consume in place of the day-1 treatment
of negative positions. Adding it is a contained change once a sample exists.

### Fixtures

`crates/ingest/tests/fixtures/caceis_joursr.csv` and `caceis_invjcp.csv` are
synthesised from the glossary column order, in the established trimmed-fixture
style. They test the parser against our *assumed* layout, so they cannot
prove the assumption. What protects against a wrong assumption is the
existing failure discipline: the column-count sniff, the filename-versus-row
fund-code check and the filename-versus-row date check all reject a
mis-shaped file loudly at upload instead of importing plausible wrong
numbers. The repo-root sample files stay untracked, as always.

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
| `GET /api/portfolios/{id}/flows` | `{n_observations, from, to, worst_1d, worst_5d, worst_20d}`, or `{status:"unavailable", reason, n_observations}` |
| `PUT /api/refs/{code}` | gains `liquidity_days` in place of `liquidity_bucket`; `null` reverts to the default; `adv_eligible` is writable (`null` reverts to the venue rule); `adv_30d`, `adv_asof`, `market_place` and the bond statics are read-only and rejected in the body |

`params` echoes the resolved settings (participation, stress factor, horizon,
deadline, fixed percentage, day-unit note) so no displayed number is
unexplained. `coverage` reports `adv_pct_of_nav`, the list of codes on the
fallback path with a reason (`no adv`, `stale adv`, `no quantity`,
`not eligible`, `future`), the list of bonds contributing no coupon with a
reason (`no next coupon date`, `no resolvable frequency`), and the register's
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
5. An observed-flows line beneath the `fixed` scenario: "worst observed
   20-day outflow 12.4% of NAV over 213 observations, 2025-09-02 to
   2026-08-07", with the adopt action. When history is too short it states
   the observation count instead of a percentage.

**Data page** — the reference-data editor's bucket dropdown becomes a
`liquidity_days` number input and gains read-only ADV, as-of, and market
place columns plus an `adv_eligible` tri-state (derived / forced on / forced
off), so measured and assumed instruments are distinguishable at a glance and
the venue rule can be corrected per instrument. A new shareholder register
editor sits alongside it (label, percent of NAV, as-of date, add and remove
rows), placed here because it is data maintenance; its staleness surfaces on
Limits.

**Bloomberg panel** — a second export button for ADV, labelled with the due
count from `adv-due`, a full-rebuild checkbox, and text stating that formulas
resolve only in Excel.

## Error handling

Governed by the project's "signal, don't hide" rule.

- A missing or stale ADV is never treated as infinite liquidity; the position
  falls back and is named in `coverage` with its reason.
- An empty shareholder register makes `top5` and `hybrid_top5` return
  `unavailable` with a reason, not a zero and not a pass.
- A bond whose coupon frequency resolves from neither INVJCPLUX nor the
  accrued-interest inference contributes no coupon and is listed with that
  reason. A frequency is never assumed, because guessing it scales the
  inflow directly.
- A JOURSRLUX history too short to support the outflow statistics reports
  `unavailable` with its observation count, not a percentage.
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
- **`analytics`, coupon frequency** — the accrued-interest inference over the
  real Brazil 6.625% 2035 figures resolving to semi-annual under both ACT/365
  and 30/360 accruals; an explicit `bond_coupon_freq` taking precedence over
  the inference; and an out-of-tolerance accrual crediting no coupon and
  reporting a reason rather than assuming one.
- **`analytics`, ADV eligibility** — a listed ETF and an ETC eligible, an
  unlisted target fund and a future not, a NULL market place falling back to
  the asset-type rule, and `adv_eligible` overriding in both directions.
- **`analytics`, flows** — worst-outflow windows over a hand-built series,
  the below-minimum `unavailable` path, and multi-class aggregation.
- **`ingest`** — the two new adapters against synthesised fixtures; a
  wrong-column-count file rejected by the sniff; a fund-code and a date
  mismatch each rejected; an unrecognised frequency code warned and left
  NULL rather than guessed; HISINVLUX yielding the bond statics and market
  place for the Brazil bond and an ETC; REGLMTLUX and RAPDECLUX returning
  their declined message rather than being silently unrecognised.
- **`db`** — register CRUD and its constraints, the `liquidity_days` backfill
  migration over pre-existing bucket values, ADV storage with `adv_asof`,
  the `adv-due` scoping query, `share_class_flows` upsert idempotency when
  the same day is loaded twice, and `RefFact` overwriting where `RefHint`
  would not while leaving `liquidity_days` and `adv_eligible` untouched.
- **`server`** — one test pinning the full liquidity response shape, one
  end-to-end scenario computed through the real stack, one asserting the ADV
  request workbook contains the listed ETF and excludes the unlisted target
  fund and the futures, and 422 coverage on the register endpoint.
- **`frontend`** — `npm run build` remains the gate.

Per the project constraint, the dev server must be stopped before
`cargo test`, and no shared server test harness is introduced.

## Out of scope

Deliberately excluded, with the seam left open where noted:

- **Dated commitments from REGLMTLUX and RAPDECLUX.** Both are recognised and
  declined with a reason. The seam is described under CACEIS loader changes:
  a dated-commitment vector on `UniversalBatch` that `A(d)` consumes instead
  of treating negative positions as a day-1 call. Blocked on samples, and on
  the de-duplication rule those samples would let us write.
- **JOUROPLUX.** Unchanged from the current branch: recognised, declined,
  pending a sample.
- **Settlement lag on sale proceeds.** `days` means days to sell. The
  contractual deadline is asked once, through `settlement_deadline_days`.
- **Floating-rate coupon projection.** INVJCPLUX carries `Index`,
  `Base Index` and `Margin`; only `Rate` is read. Projecting a floater past
  its current period needs a forward curve this tool does not have.
- **Amortising-bond factors.** HISINVLUX column 46 is read by neither the
  valuation nor the inflow path.
- **Price impact and liquidation haircuts.** This design measures elapsed
  time, not execution cost.
- **A holiday calendar.** Business days are Monday to Friday.
- **An exportable stress-test report.** The dashboard is the deliverable.
