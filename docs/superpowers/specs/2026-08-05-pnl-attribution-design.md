# Borobudur Risk — P&L Attribution — Design

**Date:** 2026-08-05
**Status:** Approved by user (sections reviewed interactively)
**Baseline:** commit `3db3b56` (branch `feat/futures-exposure`)

## Purpose

The tool reports fund-level performance and risk but cannot say *where* the
money was made or lost. This work adds a P&L page that decomposes profit and
loss over a user-chosen period, down to the individual instrument, and groups it
by asset class, geography, industry, currency and issuer group.

The decomposition separates the four things that actually move a multi-currency
book: what the position earned in its own currency, what the currency did, what
was locked in by selling, and what is still on paper.

## Findings that motivate the design

Established by analysis of `crates/ingest/tests/fixtures/sample.xlsx` (NAV date
2026-07-24, AUM €28,332,753.49) and the current schema, before the design:

1. **Position data is weekly; NAV data is daily.** `position_snapshots` is
   written per imported workbook, so instrument-level valuations exist only at
   imported NAV dates. `nav_history` carries 344 daily rows but is fund-level
   only. Per-instrument P&L therefore cannot be struck between arbitrary dates
   without a price source the tool does not have.
2. **The trade history is complete and long.** `OPERATIONS` holds 2,050 rows
   from 2025-03-18 to 2026-07-24, with quantity, price, gross amount, fees and
   net amount. This is enough to roll a cost basis from inception.
3. **Geography and industry are absent from the workbook.** The only instrument
   identifiers are ISIN, name and Bloomberg ticker. ISIN prefix gives country of
   registration, which is misleading for the fund's Irish-domiciled holdings
   (`IE00BYTBXV33`, Ryanair) and its thirteen Luxembourg funds. Country of risk
   and GICS classification must be sourced externally.
4. **The book is genuinely multi-currency.** Positions span EUR (78), USD (13),
   GBP (8), CHF (5), JPY (5), DKK (1) and SEK (1). FX is a first-order P&L
   component, not a rounding detail.
5. **`operations` records amounts in instrument currency only.** Converting a
   trade cash flow to EUR requires an FX rate at *trade date*. The tool holds FX
   rates only at weekly snapshot dates. This gap falls directly on both the
   reconciliation and the FX-effect column, and is the reason the Bloomberg
   round-trip carries a daily FX history sheet.
6. **PAM is computed on net price.** The 2025-03-18 SPIE buy shows
   `net_price 40.907584` against `price 40.76` and `fees 737.92`, and the PAM
   column follows the net basis. A cost-basis engine that includes fees
   reconciles to the administrator's own figure; one that excludes them does not.
7. **`Valorisation` for a future is variation margin, not market value** —
   established in the 2026-08-04 futures spec and unchanged here. Futures P&L is
   the change in that figure, not a mark-to-market difference.
8. **Accrued interest is already inside `Valorisation`.** The column is labelled
   *dont Interet composés* ("of which"), so bond carry needs no separate term.

## Key decisions (user-confirmed)

1. **Full decomposition.** Total P&L splits into realized, unrealized, price
   effect and FX effect, with income and fees handled as described below.
2. **Periods snap to snapshot dates.** The user picks any two dates; the tool
   strikes P&L between the nearest imported NAV dates and states plainly which
   dates it used and how many snapshots the period spans. No interpolation.
3. **Five grouping dimensions beyond the instrument itself:** asset class,
   geography (country and region), industry (GICS sector and industry group),
   currency, and issuer group.
4. **Classification arrives by Bloomberg round-trip file** — the tool exports a
   workbook of `BDP`/`BDH` formulas, the user resolves it in Excel against a
   live Terminal, and uploads it back. The Bloomberg Excel add-in cannot be
   called from a server process, so a round trip is the only mechanism
   available. It reuses the shape of the weekly CTD companion file.
5. **Weighted-average cost basis, matching PAM.** Chosen over FIFO specifically
   so that the engine's average cost can be reconciled against the
   administrator's PAM column on every import.
6. **Full reconciliation to NAV.** Cash, margin, accrued fees and provisions are
   included so the total ties to the AUM change net of subscriptions and
   redemptions.
7. **Compute on demand** in the `analytics` crate (approach B), matching
   `concentration`, `liquidity`, `rates`, `var` and `backtest`. No materialized
   P&L tables. At 2,050 trades and ~60 instruments the cost-basis walk is
   sub-millisecond; a cache would be speculative complexity.

## Deviation from the approved options

The user selected full reconciliation with a zero residual. A zero residual
cannot be honestly guaranteed: rounding in the administrator's workbook, fees
accruing between snapshots, and FX on cash balances all leave a small gap.

The design therefore computes the residual always, hides it while it is within
**0.10% of gross P&L**, and surfaces it as a warning above that. This behaves as
the chosen option in normal operation and as a data-quality alarm when something
is genuinely wrong.

## Architecture

New code follows existing patterns. Analytics modules are pure functions over
inputs with no database access; handlers read the database and call them.

| Component | Location | Responsibility |
|---|---|---|
| `pnl` module | `crates/analytics/src/pnl.rs` | Cost-basis walk, period P&L, decomposition, dimension grouping |
| `bloomberg` module | `crates/ingest/src/bloomberg.rs` | Build the request workbook; parse the returned one |
| `pnl` handler | `crates/server/src/handlers/pnl.rs` | `GET /api/pnl` |
| `bloomberg` handler | `crates/server/src/handlers/bloomberg.rs` | `GET /api/bloomberg/request`, `POST /api/bloomberg/upload` |
| `PnlPage` | `frontend/src/pages/PnlPage.tsx` | New "P&L" nav entry between Performance and Risk |

### Schema — migration `0004_pnl.sql`

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
  rate_to_eur NUMERIC NOT NULL,
  PRIMARY KEY (date, currency)
);
```

`instrument_refs` is extended rather than replaced: it is already keyed by ISIN,
already editable on the Data page, and already carries `issuer_group`, which is
one of the five dimensions. Bloomberg values seed it under the same `COALESCE`
discipline the bond statics use — **a manual edit is never overwritten by a
later import.**

`rate_to_eur` is the multiplier taking one unit of `currency` to EUR, matching
the workbook's `Change` column convention (verified: CHF valuation
−282,343.30 × 1.0751 = −303,529.67 EUR; USD 1,027,330.53 × 0.8788 =
902,830.24 EUR).

## The engine

### Cost basis

The average-cost walk applies to **cash instruments only** — `Action`, `Fonds`
and `Obligation`. Futures trades are excluded from it and handled by the
variation-margin rule below, because a future has no acquisition cost to
average and the fund holds short futures, for which an average cost is
meaningless.

One weighted-average cost per instrument, rolled forward over all trades from
inception, computed on **net price including fees**. A buy moves the average; a
sell never does.

```
on buy  q @ net price p:   avg ← (avg·qty + q·p) / (qty + q);  qty ← qty + q
on sell q @ net price p:   realized += q·(p − avg);             qty ← qty − q
```

Sides are `Achat` and `Vente`. The single uppercase `VENTE` row in the sample
confirms side matching must be case-insensitive.

A sell exceeding the running quantity indicates missing history or a
misclassified row. It is reported as a warning naming instrument and trade date,
and the instrument is marked incomplete for the period rather than producing a
negative-quantity cost basis.

### Core identity (local currency, exact)

```
Total P&L = Realized + ΔUnrealized

  Realized    = Σ qty_sold × (net sell price − avg cost at sale)
  Unrealized  = market value − (avg cost × qty)
```

Proof: with cost basis `B = avg·qty`, `ΔB = purchases − cost_of_sold` and
`U = V − B`, so `Realized + ΔU = ΔV + proceeds − purchases = ΔV + ΣCF`, which is
total P&L by definition.

### Price and FX decomposition

With `F(t)` the currency's EUR rate and `CF` the signed trade cash flows in
local currency — **negative for a buy, positive for a sell**, matching the sign
of `net_amount` in `OPERATIONS` (the 2025-03-18 SPIE buy records
`net_amount −204,537.92`):

```
Price effect = LocalP&L × F(t₀)
FX effect    = V(t₁) × [F(t₁) − F(t₀)]  +  Σ CF × [F(trade) − F(t₀)]
Total EUR    = Price effect + FX effect
```

The identity is exact by construction: expanding gives
`V₁F₁ − V₀F₀ + Σ CF·F(trade)`, which is the EUR total. FX effect is the currency
move on the closing position plus the currency move on each flow since period
start.

**The realized/unrealized split lives on the price axis only.** FX effect is
reported per instrument but not carved between realized and unrealized, because
translating a moving cost basis at differing rates makes that split arbitrary.
The table shows four columns — realized, unrealized, FX, total — rather than six
with two of them fictional.

### Fees

Trade fees are inside the cost basis, which is what makes it tie to PAM.
Presenting them as a separate deduction would count them twice, so they appear
as a **memo line**: displayed, not added.

The 29 `Frais provisionnés` rows (NAV calculation, custody, sub-custody) are
fund-level accruals, not trade costs, and get their own reconciliation line.

### Futures

A future's `Valorisation` is variation margin — accumulated unrealized P&L — so
its P&L over a period is the change in that figure plus anything realized on
contracts closed in between. The point-value derivation and the
32nds-on-`PORTEFEUILLE_NAV` convention from `analytics::futures` are reused, not
reimplemented; `OPERATIONS` quotes the same contracts in true decimal, so the
conversion applies to the snapshot side only.

This is the highest convention risk in the feature and is covered by tests
pinned to the known 2026-07-24 figures.

### Income

Dividend income is recognized from the `DIV` sheet by provision date. The
`Dividendes` position rows are the receivable side of the same accrual and are
treated as balance-sheet, not as a second income line.

### Reconciliation

```
  investment P&L (equities, bonds, funds, futures)
+ cash and margin accounts (FX revaluation)
+ accrued fees movement
+ provisions movement
+ dividend income
─────────────────────────────────────────────────
= total P&L    vs    ΔAUM − Σ(Δshares × NAV)
```

Subscriptions and redemptions are not recorded directly. They are derived from
`nav_history` as `Δshares × NAV` per day, exact for a daily-dealing fund priced
at that day's NAV. The derivation is shown in the reconciliation panel rather
than applied silently.

Residual is hidden within **0.10% of gross P&L**, where gross P&L is the sum of
the absolute values of the reconciliation lines above. Absolute values, not the
net total, so that a period in which large offsetting gains and losses net to
near zero does not make every residual look like a breach.

### Missing data

Matching the CTD precedent — never carry forward, never guess:

- An instrument with no classification groups under `Unclassified`, with the
  count shown next to the dimension selector.
- A trade whose currency has no FX rate for its trade date is reported by row,
  naming instrument and date, and the affected period is marked incomplete
  rather than silently approximated.
- A period whose endpoints have no snapshot returns the snapped dates actually
  used, never an interpolation.

## Bloomberg round-trip

`GET /api/bloomberg/request` returns a workbook containing only instruments the
tool cannot already classify, so it shrinks toward empty in steady state.

| Sheet | Content |
|---|---|
| `REFS` | Per unclassified instrument: ISIN, ticker, then `=BDP(B2,"CNTRY_OF_RISK")`, `=BDP(B2,"GICS_SECTOR_NAME")`, `=BDP(B2,"GICS_INDUSTRY_GROUP_NAME")` |
| `FX` | Start and end dates in `$A$2` and `$A$3`, then one column per held currency: `=BDH("EURUSD Curncy","PX_LAST",$A$2,$A$3)`, spanning the fund's full history |
| `README` | Fixed instructions plus the export date, so a stale file is obvious |

`POST /api/bloomberg/upload` reads **values only**, never formulas. `#N/A` and
`#N/A N/A` are reported per row and not stored. Nothing is written unless the
file parses; partial rows are rejected individually and named.

Two cross-checks follow from this, both consistent with the futures precedent:

1. **FX inversion check.** Bloomberg quotes `EURUSD` as dollars per euro; the
   tool needs euros per dollar and inverts. The inverted series is compared
   against the workbook's own `Change` column at every snapshot date; a
   disagreement beyond tolerance flags the upload rather than trusting it.
2. **PAM reconciliation.** The engine's average cost is compared against the
   workbook's PAM per position on every import, warning above €0.01 drift. This
   validates the entire trade walk against an independent calculation, weekly.

Region is derived from country by a fixed lookup table in the code, not fetched.

## API

```
GET /api/pnl?from=2026-06-30&to=2026-07-24&dimension=sector

{ "period": { "requested_from", "requested_to",
              "actual_from", "actual_to", "snapshots": 4 },
  "groups": [ { "key": "Consumer Discretionary",
                "realized", "unrealized", "fx", "total", "fees_memo",
                "instruments": [ { "isin", "name", "realized",
                                   "unrealized", "fx", "total" } ] } ],
  "reconciliation": { "investment_pnl", "cash_and_margin", "accrued_fees",
                      "provisions", "dividend_income", "total_pnl",
                      "aum_change", "net_flows", "residual",
                      "within_tolerance" },
  "warnings": [ "8 instruments unclassified" ] }
```

`dimension` is one of `asset_class | country | region | sector | industry |
currency | issuer_group`. Instrument rows nest inside their group so the table
expands without a second request. Omitting `dimension` returns a flat
instrument-level list.

## UI

A new `P&L` entry in the sidebar between Performance and Risk:

- Period presets (MTD, QTD, YTD, ITD) plus custom dates, with the snapping
  notice stating which snapshot dates were actually used
- Dimension selector, with the unclassified count beside it
- Expandable table: group rows showing realized, unrealized, FX and total, each
  expanding to its instruments
- Contribution bar chart via the existing `EChart` component
- Reconciliation panel, collapsed while the residual is within tolerance

The Data page gains a "Bloomberg classification" panel below the CTD panel:
export button, upload control, and a count of unclassified instruments.

## Testing

- `analytics/pnl.rs`: cost-basis walk (buy/buy/sell sequences), the
  `Total = Realized + ΔUnrealized` identity, and a property test asserting
  `price effect + FX effect == total` exactly across generated inputs
- Case-insensitive side matching (`Vente` / `VENTE`)
- Futures P&L pinned to known 2026-07-24 figures, including a 32nds contract
- Reconciliation residual on `sample.xlsx` asserted within tolerance
- `ingest/bloomberg.rs`: request generation, valid parse, `#N/A` rejection,
  unknown-ticker rejection — fixtures alongside the existing `ctd_sample.*`
- FX inversion check against the workbook's `Change` column
- `cargo test` and `npm run build` both clean

## Out of scope

No daily P&L, no benchmark-relative attribution, no Brinson decomposition, no
tax lots, no realized/unrealized split of the FX effect. Each is a separate spec
if wanted.

Daily P&L is the most likely follow-on. The engine reads prices and FX through a
narrow interface, so a future daily price history can replace the snapshot
source without reworking the decomposition.
