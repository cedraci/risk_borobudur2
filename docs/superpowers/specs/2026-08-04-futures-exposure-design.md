# Borobudur Risk — Futures Exposure and Bond-Future DV01 — Design

**Date:** 2026-08-04
**Status:** Approved by user (sections reviewed interactively)
**Baseline:** commit `d53ef63` (v1 + v2 merged)

## Purpose

The fund holds eight futures that the tool currently treats as worthless. Their
`Valorisation` in the NAV Recap is variation margin (unrealized P&L), not market
value, so all eight together register as **+0.10% of AUM** while their notional
exposure to the underlying is **25.5% of AUM**. Futures are excluded from every
limits check, and the rates section describes a single cash bond while the
fund's actual rate exposure sits in four bond futures.

This work adds:

1. **A derivative exposure disclosure** — notional by underlying, by category,
   long and short in absolute value as a percentage of total net assets.
2. **Precise DV01 for bond futures** — from cheapest-to-deliver analytics
   uploaded weekly.

## Findings that motivate the design

Established by analysis of the 2026-07-24 NAV Recap before the design:

1. **Notional is recoverable from the workbook alone.** `Valorisation Dev` for a
   future equals `(Prix − PAM) × Qté × point_value`, so the contract multiplier
   is implied by the file and reproduces the exchange specification exactly for
   all eight contracts (10 CAC, 10 Stoxx 50, 20 Nasdaq, 1000 for each of the
   four bond futures, 125,000 EUR/JPY). The README's claim that the file
   carries no notional data is wrong.
2. **`PORTEFEUILLE_NAV` quotes US Treasury futures in thirty-seconds.** `109.145`
   means `109 + 14.5/32` = 109.453125. Proof: TYU6 has exactly one transaction
   (2026-06-23, sell 6 @ 109.453125), so its PAM must equal that price; the
   sheet shows 109.145. The 32nds reading also reproduces the file's own 6750
   valorisation exactly, where the decimal reading gives 6240 and an
   implausible 1081.73 multiplier. `OPERATIONS` uses true decimal for the same
   contract, so the convention differs **per sheet**.
3. **Duration and DV01 are not in the file** in any form and must be supplied.

Exposure under the rule adopted below, as of 2026-07-24:

| Category | Long | Short | Gross |
|---|---:|---:|---:|
| Equity | — | 7.31% | 7.31% |
| Interest rate | 3.38% | 11.73% | 15.11% |
| FX | — | 3.08% | 3.08% |
| **Total** | **3.38%** | **22.12%** | **25.50%** |

The FX line is the JPY leg converted at the workbook's own FX rate, per the
formula below. Taking the contract's EUR base leg instead (7 × €125,000) gives
3.09%; the difference is futures basis, not an error in either.

## Key decisions (user-confirmed)

1. **Computation rule.** Exposure is measured by reference to the underlying,
   summed as notional by category, with long and short each expressed in
   absolute value as a percentage of total net assets. No netting.
2. **Scope: the exposure table only.** Futures are *not* folded into the 5/10/40
   or connected-group issuer checks — those measure issuer credit exposure and
   are a different computation. The existing "futures excluded" note on the
   concentration page stays, and gains a pointer to the new table.
3. **Categories: the standard six, extensible** — equity, interest rate, foreign
   exchange, credit, commodity, other. Only the first three have holdings today;
   the remainder cost nothing and avoid a migration later.
4. **Weekly CTD analytics arrive as a second file** uploaded on the Data page,
   independent of the administrator's workbook.
5. **Exact-match only, no carry-forward.** DV01 requires analytics for that
   precise NAV date. A date without them shows notional and category exposure
   normally, and marks the rates figures as missing. A stale CTD duration is
   wrong precisely when it matters — at a quarterly roll — so it is never
   presented as current.
6. **Precise DV01** via conversion factor and CTD dirty price, not a duration
   approximation.
7. **Architecture: static contract table + weekly analytics table** (approach A),
   rather than one weekly file carrying everything or specs in `settings`.

## Architecture

The two computations are deliberately decoupled:

- The **exposure table** depends only on the workbook plus permanent contract
  specs. It renders for every NAV date including all existing history, and
  cannot be broken by a missed or late Bloomberg pull.
- **DV01** depends on the weekly file and degrades independently and visibly.

## Data model

### New migration `crates/db/migrations/0003_futures.sql`

```sql
CREATE TABLE futures_contracts (
  contract_root    TEXT PRIMARY KEY,           -- RX, OAT, KOA, TY, CF, VG, NQ, RY
  label            TEXT NOT NULL,
  category         TEXT NOT NULL CHECK (category IN
                   ('equity','interest_rate','fx','credit','commodity','other')),
  point_value      NUMERIC CHECK (point_value > 0),   -- currency units per price point
  currency         TEXT NOT NULL,
  curve            TEXT,                       -- rates only: DE-10y, FR-10y, ES-10y, US-10y
  price_convention TEXT NOT NULL DEFAULT 'decimal'
                   CHECK (price_convention IN ('decimal','th32')),
  confirmed        BOOLEAN NOT NULL DEFAULT false,
  updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE futures_analytics (
  nav_date          DATE NOT NULL,
  ticker            TEXT NOT NULL,             -- 'RXU6 Comdty'; matches position_snapshots.ticker
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

**Why `point_value` and not a contract size.** `notional = qty × point_value ×
price` holds for every contract type in the book — index, bond and FX — so one
field serves all three. For bond futures the deliverable face is
`point_value × 100`, which is what DV01 needs, so there is no second size field
that could drift out of step with the first.

**Why `futures_analytics` is keyed on the full ticker.** It joins directly to
`position_snapshots.ticker` with no derivation, and is automatically correct
across rolls because each week's file names whichever contract is live. The
contract root is derived only to locate the static spec.

**Root derivation.** Take the symbol before the space and drop its final two
characters, the month letter and year digit: `RXU6`→`RX`, `OATU6`→`OAT`,
`KOAU6`→`KOA`, `TYU6`→`TY`, `CFQ6`→`CF`, `VGU6`→`VG`, `NQU6`→`NQ`,
`RYU6`→`RY`. A ticker that will not parse produces an import warning and the
contract is listed as spec-missing.

## Companion file format

`.xlsx` or `.csv`, one row per bond future. The header row is matched by name,
case-insensitively and trimmed, so column order is free.

| nav_date | ticker | ctd_isin | ctd_mod_duration | ctd_clean_price | ctd_accrued | conversion_factor |
|---|---|---|---|---|---|---|
| 2026-07-24 | RXU6 Comdty | DE0001102580 | 8.41 | 98.72 | 0.63 | 0.782145 |

`nav_date` repeats on every row — it fills down naturally from a Bloomberg BDP
sheet — and all rows must agree. Equity and FX contracts never appear: they
need nothing weekly.

Re-uploading for a NAV date replaces that date's rows wholesale, in one
transaction. There is deliberately **no SHA dedupe** here, unlike the workbook
import: the expected reason to re-upload is a corrected pull.

No `dv01_override` column. Precise DV01 was chosen, and an override would be a
second code path producing a different number.

## Computation

### Price decoding

One function driven by `price_convention`. `th32` reads `108.105` as
`108 + 105/320` = 108.328125, the `108-10.5` thirty-seconds quote. Applied to
both `Prix` and `PAM` before any other use.

### Notional and categories

```
notional_ccy = qty × point_value × price_decoded
notional_eur = notional_ccy × fx_rate          (fx_rate from the workbook row)
```

Per category: `long` = Σ positive `notional_eur`, `short` = Σ |negative
`notional_eur`|, each as a percentage of that date's AUM in absolute value;
`gross` = long + short.

### DV01

For bond futures with analytics for that exact NAV date:

```
dirty         = ctd_clean_price + ctd_accrued
dv01_contract = ctd_mod_duration × dirty × point_value × 1e-4 / conversion_factor
dv01_position = qty × dv01_contract × fx_rate
```

`total_dv01_eur` becomes cash bonds plus futures.

### Restating `nav_sensitivity_100bp`

Today it is `Σ(modified × weight) × 0.01`, which requires a market-value weight
that futures do not have. `100 × total_dv01_eur / aum` is algebraically
identical for the existing bonds and extends to futures unchanged, so that line
is restated rather than special-cased. **Existing bond figures must not move**;
a regression test pins them.

### Import cross-check

For each futures row, recompute `point_value` from
`valorisation / ((price − pam) × qty)` and compare with the stored spec:

- **No stored row** → seed one with the derived value, category guessed from the
  ticker suffix (`Index`→equity, `Curncy`→fx, `Comdty`→**other**, because
  `Comdty` covers both bond and commodity futures), `confirmed = false`. The
  two remaining NOT NULL columns are taken from the position row: `label` from
  `Intitulé`, `currency` from `Dev`. `curve` is left NULL — it cannot be
  inferred and is entered when the contract is confirmed.
- **Matches within 0.5%** → silent.
- **Mismatch** → retry under the opposite price convention. If that matches, the
  warning names the cause: *"RX: point value implies convention th32, stored
  decimal."* Otherwise: *"stored 1000, implied 1081.7."*
- **`price == pam`** → undeterminable this week; skipped, no warning.

These are **warnings on the import outcome, never failures.** Blocking a weekly
NAV import because a new contract appeared would be worse than importing it
flagged. Seeded rows follow the existing `COALESCE` convention in
`import_workbook`: user edits are never overwritten.

## Degradation

Missing data never raises an error at read time. The API returns flags the UI
renders explicitly:

| Condition | Behaviour |
|---|---|
| `point_value` absent | Contract listed, notional blank, excluded from totals; totals carry a caveat flag (`spec_missing`) |
| `confirmed = false` | Included, badged `unconfirmed` |
| No analytics for the date | Notional and category exposure normal; rates row marked `duration_missing` |
| No FX rate on the row | `notional_ccy` computed, `notional_eur` null, excluded from percentage totals with a flag |

Stating what is absent is the point of the work: the present failure mode is a
silent zero.

## API

| Endpoint | Purpose |
|---|---|
| `GET /api/limits/derivatives?date=` | Category table + per-contract detail + flags |
| `GET /api/limits/rates?date=` | Extended: `futures[]` added, `total_dv01_eur` includes them |
| `GET /api/futures-contracts` | Static specs for the grid |
| `PUT /api/futures-contracts/:root` | Edit a spec; validation mirrors `refs.rs::put` |
| `POST /api/futures-analytics` | Multipart companion-file upload (field `file`) |
| `GET /api/futures-analytics?date=` | Rows held for a date |

## Code layout

| Path | Contents |
|---|---|
| `crates/analytics/src/futures.rs` | Pure: price decoding, point-value derivation, notional, category aggregation, DV01 |
| `crates/ingest/src/futures_file.rs` | Companion-file parser, reusing `RowError` |
| `crates/db/migrations/0003_futures.sql` | Both tables |
| `crates/db/src/repo.rs` | Contract and analytics accessors |
| `crates/server/src/handlers/futures.rs` | Contracts CRUD + analytics upload |
| `crates/server/src/handlers/limits.rs` | Extended: derivatives handler, rates gains futures |
| `frontend/src/pages/LimitsPage.tsx` | "Derivatives exposure" section |
| `frontend/src/pages/DataPage.tsx` | Contracts grid + second upload control |

The rates section shows total DV01 and per-contract rows carrying a curve
label. No per-curve subtotals: the short-OAT / long-Bono spread stays visible
row by row without an aggregation that was not asked for.

## Error handling

Upload errors follow the existing path — `ParseFailure::Workbook` → 400,
`ParseFailure::Rows` → 422 via `AppError::UnprocessableRows`, with 1-based row
numbers pointing at a line in the sheet.

- **File-level (400):** missing required header, no data rows, rows disagreeing
  on `nav_date`.
- **Row-level (422, all collected before failing):** unparseable number,
  duration / price / conversion factor ≤ 0, negative accrued, blank ticker or
  ISIN, the same ticker twice.
- **Semantic (422):** `nav_date` has no position snapshot — *"no NAV snapshot
  for 2026-07-24; upload the NAV Recap first"* — or a ticker that is not a
  Future in that snapshot, named explicitly.

## Testing

| Crate | Coverage |
|---|---|
| `analytics` | Price decoding both conventions, including `108.105 → 108.328125`; point-value recovery for the 10 / 20 / 1000 / 125,000 shapes; notional sign and magnitude; category aggregation with a category holding both signs; DV01 against a hand-computed value; degenerate cases — `price == pam`, zero quantity, absent `point_value` |
| `ingest` | Companion-file parser, valid and malformed, against a committed fixture |
| `db` | Upload and read back; re-upload replaces; unknown NAV date and unknown ticker rejected |
| `server` | Endpoint shape; degradation when analytics are absent; `PUT` validation, in the style of `api_limits.rs` |
| regression | Existing bond DV01 and `nav_sensitivity_100bp` values pinned across the restatement |

**Fixtures are synthetic.** The real NAV Recap is confidential fund data and
does not go into the repository. `sample.xlsx` is already a made-up workbook and
gains futures rows of the same shape. The arithmetic in this document was
validated against the real file during analysis; nothing derived from it is
committed.

## Out of scope

- Futures in the 5/10/40 and connected-group issuer checks (decision 2).
- Any direct Bloomberg integration — MARS Web API, B-PIPE, or a `blpapi`
  sidecar. The weekly file is the interface; a future automated pull would
  write to the same `futures_analytics` table and change nothing else.
- Carry-forward of stale CTD analytics (decision 5).
- Per-curve DV01 subtotals.
- Commodity and credit futures beyond the category enum accepting them.
