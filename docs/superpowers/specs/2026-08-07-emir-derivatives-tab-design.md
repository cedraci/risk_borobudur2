# Derivatives / EMIR tab — design

Date: 2026-08-07
Source document: "EMIR simplified (sans la partie declaration).docx" (CAM EMIR
procedure, excluding the trade-reporting section). Goal: automate as much of the
procedure's monitoring as the tool's data allows, in a new **Derivatives** tab.

## Scope

In scope, computed automatically from existing data:

1. **Clearing-threshold monitoring** (suivi des seuils de compensation): average
   month-end gross notional per asset class over the last 12 months, compared to
   the EMIR clearing thresholds.
2. **Derivatives exposure** display — the existing `DerivativesExposure`
   component, **moved** from the Limits page (not duplicated).
3. **OTC obligation monitors**: reconciliation-cadence tier and compression
   trigger derived from the count of open OTC contracts.
4. **Margin/collateral view**: `Margin Acc` rows from the latest snapshot.
5. **Evidence export**: one-click `.xlsx` of the full threshold calculation, to
   archive on SharePoint as the procedure requires.

In scope, manual by nature:

6. **Monthly KPI entry** for the risk committee: unconfirmed-over-5-days count,
   reconciliation done/not-done/not-applicable, dispute count, free-text note.

Out of scope: trade reporting (excluded from the source document), confirmation
timestamp ingestion, counterparty data, collateral eligibility checks, and any
AMF/ESMA notification workflow (the export supports it; the tool does not send
mail).

## Regulatory decisions

- **OTC vs ETD**: EMIR clearing thresholds count OTC positions. Contracts
  executed on an EU regulated market or an equivalent third-country market are
  not OTC; contracts on non-equivalent venues are, even if exchange-listed.
  The tool therefore shows **two lines per asset class — total gross notional
  and of-which-OTC — and only the OTC average is compared to the threshold.**
  OTC-ness is a per-contract flag, editable, defaulting to false (every
  contract currently on record is listed on an equivalent venue).
- **Gross, no netting**: notional is absolute long + absolute short per class.
- **Thresholds** (EUR notional): credit 1 bn, equity 1 bn, interest-rate 3 bn,
  FX 3 bn, commodity-and-other 4 bn. Category mapping: `equity`→equity,
  `credit`→credit, `interest_rate`→interest-rate, `fx`→FX, `commodity` and
  `other`→commodity-and-other. Constants in `analytics/emir.rs` with the
  regulation reference in a comment (not settings — a threshold change is a
  code change with a test).
- **Month window**: the 12 calendar months ending with the month of the latest
  imported NAV date. No wall clock — deterministic from data. Per month, the
  position is the **latest snapshot on or before the month's last day**; the
  snapshot date used is displayed. A month with no snapshot is reported
  missing; the average divides by months present and is labeled "N of 12".
- **Verdicts**: OK below 80% of threshold, WATCH at ≥ 80%, BREACH at ≥ 100%
  (Limits-page convention).
- **Monitors are conservative**: with no counterparty data, the OTC contract
  count assumes a single counterparty (the strictest tier assignment) and the
  page says so explicitly. Tiers: 0 → not triggered; 1–50 → quarterly;
  51–499 → weekly; ≥ 500 → daily reconciliation. Compression: analysis
  required semiannually at ≥ 500 contracts.

## Data model (migration 0006)

- `ALTER TABLE futures_contracts ADD COLUMN otc BOOLEAN NOT NULL DEFAULT false;`
  Edited on the Data page's contract-confirmation panel as a checkbox alongside
  category/curve/price convention.
- New table `emir_kpis`:
  `month DATE PRIMARY KEY` (constrained to first-of-month),
  `unconfirmed_over_5d INT NOT NULL CHECK (>= 0)`,
  `reconciliation TEXT NOT NULL CHECK IN ('done','not_done','not_applicable')`,
  `disputes INT NOT NULL CHECK (>= 0)`,
  `note TEXT`,
  `updated_at TIMESTAMPTZ NOT NULL DEFAULT now()`.
  One row per calendar month, upserted from the tab's form.

## Analytics (`analytics/emir.rs`, pure functions)

Inputs: per-month resolved snapshots (future positions joined with contract
specs and FX rates), threshold constants. Outputs, serializable:

- Per threshold class: per-month rows (month, snapshot date used or missing,
  total gross notional EUR, OTC gross notional EUR), the two averages, the
  threshold, months-present count, verdict on the OTC average.
- Monitor block: OTC open-contract count at the anchor snapshot,
  reconciliation tier, compression-trigger status.
- Warnings attached to the month they affect: unconfirmed contract, missing
  point value, missing FX rate. Affected notional is never silently zeroed;
  the warning names the contract and the month (house ruling: signal data
  quality, never hide it).

Notional arithmetic reuses the existing futures notional path (qty × point
value × price with `th32` handling × FX to EUR).

## API (server)

- `GET /api/emir?date=` — single payload: threshold table, monitors, margin
  lines (from `Margin Acc` rows: account, currency, local value, EUR value),
  KPI history, warnings. `date` optional, defaults to the latest NAV date,
  snapped to the nearest imported snapshot like other pages. The snapped date
  is the **anchor**: exposure, margin, monitors and the contract inventory are
  struck at that snapshot, and the threshold window is the 12 calendar months
  ending with the anchor's month. The payload echoes the anchor date used.
- `PUT /api/emir/kpis/{month}` — upsert one monthly KPI record; server-side
  validation (counts ≥ 0, month is a first-of-month date).
- `GET /api/emir/export` — streams `EMIR - seuils - {anchor date}.xlsx`, built
  by a new module in the `ingest` crate (which owns `rust_xlsxwriter` and the
  workbook conventions). Sheets: `Seuils` (every month × class with dates used,
  both notional lines, averages, thresholds, verdicts, warnings), `Contrats`
  (contract inventory at the anchor date with OTC flags and point values),
  `KPI` (monthly KPI history).

## Frontend

New route `/derivatives`, nav label **Derivatives**, placed between Limits and
Data. Sections top to bottom:

1. Derivatives exposure (`DerivativesExposure`, moved off the Limits page).
2. EMIR threshold table — one row per class showing the two averages, "N of 12",
   threshold, verdict badge; expandable to the 12 monthly figures (P&L-page
   expandable-row pattern).
3. Obligation monitors with the explicit single-counterparty caveat.
4. Margin table.
5. Monthly KPI form + history table.
6. Evidence-export button.

Existing `index.css` classes only; no new dependencies. Warning lists rendered
exactly as the API returns them, per section.

## Error handling

- No snapshots at all → the page's standard empty state.
- Partial data (missing months, unconfirmed contracts, missing FX or point
  values) → computation proceeds where possible, warnings surfaced per section.
- KPI form errors → 400 with the offending field named.

## Testing

- `emir.rs` unit tests: class mapping (incl. `other`→commodity bucket),
  missing-month averaging and "N of 12" labeling, OTC-only threshold feed
  (mixed flags), verdict boundaries (just below 80%, exactly 80%, exactly
  100%), warning attribution.
- Server tests: `GET /api/emir` payload shape against the sample fixture, KPI
  upsert round-trip and validation failures, date snapping.
- Export: build the workbook and read it back with `calamine`, asserting sheet
  names and key cells (house convention from the Bloomberg round-trip tests).
- Frontend: `npm run build` type-check; TS types field-for-field against the
  serialized Rust structs.
