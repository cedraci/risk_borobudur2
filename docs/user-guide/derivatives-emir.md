# Derivatives / EMIR

The Derivatives tab shows two related things for the selected portfolio: the fund's notional
derivatives exposure (which underlyings and asset classes it is exposed to, long and short), and
its standing against the EMIR clearing-obligation thresholds — the regulatory test that decides
whether the fund must start centrally clearing its OTC derivatives. It also carries the operational
side of EMIR monitoring: margin-account balances, the reconciliation/compression obligations implied
by the OTC book, a place to record the monthly middle-office KPIs the tool cannot derive on its own,
and a one-click evidence export for the audit file.

## Snapshot selector and export

At the top of the page:

- **Snapshot** — a dropdown of every position-snapshot date ever imported for the portfolio, most
  recent first. Everything on the page — the exposure table, the threshold averages, the margin
  view, the OTC-obligation counts — is struck as of the date you pick here (the "anchor"). It
  defaults to the most recent snapshot.
- **Export evidence workbook** — a link that downloads an `.xlsx` file with the full calculation
  behind the page for the selected anchor date. See "Evidence export" below for what it contains
  and when it is refused.

If no position snapshot has ever been imported for the portfolio, the page shows an explanatory
message instead of the panels below (nothing else can be computed without at least one snapshot).

## Derivatives exposure

This panel shows notional exposure by reference to the underlying instrument and by asset-class
category, with long and short each expressed in absolute value as a percentage of net assets — no
netting between long and short, and no netting between instruments.

A caption line ("Notional by reference to the underlying; long and short each in absolute value as
a percentage of net assets. No netting.") sits at the top of the panel as a standing reminder of
this convention.

### Data-quality banners

Above the tables, the panel can show:

- **Category breakdown not computed** — appears when you don't have view access to shared reference
  data (contract specs). Without it, every future's asset-class category and OTC flag cannot be
  resolved reliably, so the category table is withheld entirely rather than shown as a misleading
  "all other" result. See [Access rights](access-rights.md).
- **% NAV not computed** — appears when net assets for the anchor date are unavailable (denied
  access to NAV data, or no NAV has been imported for that date). The euro notional figures in the
  contracts table still show, but the "% NAV" column and every percentage-of-net-assets figure on
  the page do not.
- **Unconfirmed contract specs** — lists the contract roots (e.g. `RX`, `TY`) whose specification has
  not yet been confirmed on the Data page. Notionals computed from an unconfirmed spec are
  provisional; each affected row in the contracts table also carries its own "unconfirmed" badge.
- **N contract(s) excluded from the totals** — lists the tickers left out of every total on this
  page because their percentage of net assets could not be computed: no contract spec, no quantity
  or price in the snapshot, no FX rate for the trading currency, or — since the percentage is
  notional ÷ net assets — an unknown net-asset value for the date. When this banner is showing, the
  category table and the "Total notional" row are explicitly labelled **partial** and every figure
  in them is prefixed with "≥" (e.g. "≥ 12.40%") to flag that it is a lower bound, not a complete
  total.

### Notional by category

If the snapshot holds no derivative positions at all, the panel says so and stops there. Otherwise
a table lists one row per asset-class category that has any exposure, in this order: **Equity**,
**Interest rate**, **Foreign exchange**, **Credit**, **Commodity**, **Other**. A category with zero
exposure in the snapshot is left out of the table (not shown as a zero row). Columns:

| Column | Meaning |
|---|---|
| Category | The asset-class label. |
| Long | Sum of every long position's notional in that category, as a percentage of net assets (positive). Shows "—" if there is no long exposure in the category. |
| Short | Sum of every short position's notional in that category, in absolute value, as a percentage of net assets. Shows "—" if there is no short exposure. |
| Gross | Long + Short (never netted against each other). |

A final **Total notional** row sums Long, Short and Gross across every category (labelled "Total
notional (partial)" with "≥" values when contracts were excluded, as described above).

### Contracts

Below the category table, a second table lists every individual derivative position in the
snapshot (not just the ones with a resolvable notional), one row per contract:

| Column | Meaning |
|---|---|
| Ticker | The position's Bloomberg-style ticker. Carries an "unconfirmed" badge when its contract spec has not yet been confirmed. |
| Name | The instrument name from the position snapshot. |
| Category | The asset-class category the ticker's contract spec is classified under. |
| Qty | The position's quantity from the snapshot. Shown in red as "missing" if the snapshot row had no quantity. |
| Price | The position's price. Shown in red as "missing" if the snapshot row had no price. |
| Point value | The contract's point value, from its confirmed (or seeded) spec. Shown in red as "spec missing" if no contract spec resolves for this ticker's root at all. |
| Notional € | Quantity × point value × price, converted to euros at the workbook's own FX rate for the position's currency. "–" if any of the inputs is missing. |
| % NAV | Notional € ÷ net assets on the anchor date, signed — a negative percentage marks a short position. Unlike the category totals above (which sum long and short separately as positive magnitudes), this column shows the position's true sign. "–" if it could not be computed. |

Contract point values themselves are not entered here: they come from the "NAV Recap" workbook on
import and are reviewed and confirmed on the Data page's contract panel — see
[Reference data](data.md) and "Contract specs and the OTC flag" below.

## EMIR clearing thresholds

This panel compares the fund's OTC derivatives exposure against the EU clearing-obligation
thresholds of Delegated Regulation (EU) No 149/2013 as amended.

A caption line explains the method: **"Average of month-end gross notional over the last 12 months
(N of 12 months have a snapshot)."**, followed by a short note on what counts as OTC (see "The OTC
flag" below).

### How the average is built

For each of the 12 calendar months ending with the anchor snapshot's month, the calculation picks
the most recent snapshot date that falls inside that month — for the anchor's own month, it never
looks past the anchor date itself. A calendar month with no snapshot inside it is left out of the
average entirely (never estimated or carried forward from an earlier month), and is counted against
the "N of 12" figure in the caption. The averages below are therefore an average over the months
that actually have a snapshot, not always a divide-by-12.

Within each month and category, gross notional adds every position's absolute notional in euros
(shorts included, never netted against longs); the OTC line adds only the positions whose contract
is flagged OTC.

### Threshold table

One row per regulatory asset class, in this fixed order and with these EUR thresholds:

| Class | Threshold |
|---|---|
| Credit derivatives | 1,000,000,000 € |
| Equity derivatives | 1,000,000,000 € |
| Interest-rate derivatives | 3,000,000,000 € |
| FX derivatives | 3,000,000,000 € |
| Commodity and other derivatives | 4,000,000,000 € |

(The regulation's own fifth bucket lumps commodity derivatives together with anything that doesn't
fit the other four categories, hence "Commodity and other".)

Columns:

| Column | Meaning |
|---|---|
| Class | The asset-class label, from the table above. Click a row to expand/collapse its month-by-month detail (▸/▾). |
| Avg OTC notional | The 12-month average of month-end OTC gross notional for the class — the figure that is actually tested against the threshold. |
| Avg total notional | The same average including non-OTC notional too, shown alongside for context (it is never itself compared to the threshold). |
| Threshold | The fixed EUR threshold for the class. |
| % of threshold | Avg OTC notional ÷ Threshold. |
| Verdict | **OK**, **WATCH**, or **BREACH** — see below. |

Expanding a class row reveals one line per month in the 12-month window, oldest first, showing the
month, the snapshot date used inside it (or a "no snapshot this month" flag if the month has none),
and that month's OTC and total notional for the class.

### Verdict thresholds

Only OTC notional counts toward a threshold. The verdict per class is:

- **OK** — average OTC notional is below 80% of the threshold.
- **WATCH** — average OTC notional is at or above 80% of the threshold but below 100%.
- **BREACH** — average OTC notional is at or above 100% of the threshold.

Any data-quality warnings collected while building the report (a month with no snapshot, a
position excluded from the sums for want of a spec/quantity/price/FX rate, or a position whose
notional is provisional because its contract spec is unconfirmed) are listed as badges under the
table.

### The OTC flag

A contract defaults to **not OTC** when its spec is first seeded from the workbook. Whether a
contract actually counts as OTC for EMIR purposes is a manual flag you set on the Data page's
futures-contract panel: tick "OTC" for any contract executed on a non-equivalent third-country venue
or bilaterally — including a contract that is nominally exchange-listed but trades on a venue the EU
does not recognise as equivalent, which is OTC despite being "listed". A contract on an EU regulated
market or a recognised equivalent third-country market is not OTC. See
[Reference data](data.md) for the contract panel itself.

If you don't have view access to shared reference data, the contract specs (and therefore every
OTC flag) cannot be resolved. Rather than silently defaulting every position to "not OTC" and
showing a false clean pass, every class in the threshold table degrades: the verdict shows as
unavailable and every computed figure (average OTC/total notional, % of threshold, and the month
detail figures) is blanked out — only the class name, label and fixed threshold remain visible. See
[Access rights](access-rights.md).

## OTC obligations

A small panel derived from the OTC-flagged positions in the anchor snapshot:

| Row | Meaning |
|---|---|
| Open OTC contracts | Count of positions in the anchor snapshot whose contract is flagged OTC. |
| Portfolio reconciliation | The reconciliation cadence implied by that count: **"Not triggered — no OTC contracts outstanding"** at 0, **Quarterly** for 1–50, **Weekly** for 51–499, **Daily** for 500 or more. |
| Compression analysis (≥ 500 contracts) | **"required semiannually"** once the OTC count reaches 500; otherwise **"not required (N < 500)"**. |

A note underneath explains the conservative assumption behind both figures: the tool has no
counterparty breakdown, so the reconciliation tier and the compression trigger assume every OTC
contract faces a single counterparty — the strictest possible reading of the RTS 149/2013 tiers.

## Margin accounts

Lists the margin-account balances recorded in the anchor snapshot, with a caption noting how many
futures positions they collateralize ("Margin balances from the snapshot, collateralizing N futures
position(s)."). If the snapshot has no margin accounts, the panel says so instead of showing an
empty table. Otherwise, one row per account:

| Column | Meaning |
|---|---|
| Account | The margin account's name from the snapshot, or "—" if unnamed. |
| Currency | The account's currency. |
| Local value | The balance in its own currency. |
| EUR value | The balance converted to euros. |

These are read-only figures taken straight from the position snapshot — there is nothing to enter
here.

## Monthly EMIR KPIs

Some EMIR obligations are middle-office facts the tool has no way to derive on its own: whether
trade confirmations went out and came back within the regulatory window, whether portfolio
reconciliation with counterparties was actually performed, and whether any disputes were open. This
panel is where those facts are recorded, one entry per calendar month, for review at the risk
committee.

### Entering a month

A small form lets you record or update one month at a time:

| Field | Meaning |
|---|---|
| Month | A month picker, defaulting to the current month. |
| Unconfirmed > 5 days | Number of trade confirmations still outstanding after 5 business days (whole number, ≥ 0). |
| Reconciliation | **Done**, **Not done**, or **N/A**, for whether portfolio reconciliation with counterparties was carried out that month. |
| Disputes | Number of open disputes with counterparties that month (whole number, ≥ 0). |
| Note | Free-text note, optional. |

Clicking **Save month** stores the entry for that calendar month; saving the same month again
overwrites the previous entry for it (there is exactly one record per month). A confirmation or
error message appears next to the button.

Recording a KPI needs its own permission (Reference/Configure), distinct from the permission needed
merely to view the page — see [Access rights](access-rights.md).

### KPI history

Below the form, a table lists every recorded month, most recent entries as saved:

| Column | Meaning |
|---|---|
| Month | The calendar month (YYYY-MM). |
| Unconfirmed > 5 days | The recorded count. |
| Reconciliation | Done / Not done / N/A. |
| Disputes | The recorded count. |
| Note | The free-text note, or "—" if none was entered. |

If you lack view access to shared reference data, this table shows no rows even if KPI entries
exist for the portfolio — the same access grant that governs contract specs also governs the KPI
history read.

## Evidence export

The **Export evidence workbook** link at the top of the page downloads an `.xlsx` file named:

```
EMIR - seuils - {portfolio name} - {anchor date}.xlsx
```

It is the audit-file evidence for the EMIR clearing-threshold procedure: everything the on-screen
verdict is built from, archived in one file, for the snapshot date currently selected. It has three
worksheets:

- **Seuils** — the title, anchor date, "months with a snapshot" count, and the OTC/no-netting
  method note, followed by the summary table (one row per class: label, threshold, average OTC
  notional, % of threshold, verdict, average total notional), then the month-by-month detail table
  behind it (class, month, snapshot date used or "missing", total EUR, OTC EUR for that month), and
  finally the full list of data-quality warnings.
- **Contrats** — the shared contract-spec inventory: root, label, category, OTC flag (true/false),
  confirmed flag (true/false), point value, and currency for every futures contract on record (not
  only the ones held by this portfolio).
- **KPI** — the full monthly KPI history recorded for the portfolio: month, unconfirmed-over-5-days
  count, reconciliation status, disputes count, and note.

Downloading the export requires its own Export permission on portfolio positions, on top of the
view access needed to see the page. Unlike the on-screen panels, the export never degrades quietly:
if the contract specs behind the OTC flagging, or the KPI history, are not accessible to you, the
export is refused outright with an error rather than producing a file that would look like a clean,
complete evidence pack while actually being built on incomplete data. See
[Access rights](access-rights.md).

## Access rights

Viewing the Derivatives tab at all requires view access to the portfolio's positions; without it,
the whole page shows an access-denied message instead of the controls and panels. Two other grants
feed into the page and degrade independently rather than blocking it outright:

- **Reference data** (shared contract specs and the recorded KPI history) — without it, the
  category/OTC breakdown on the exposure panel is withheld, every class in the EMIR threshold table
  shows an unavailable verdict with its computed figures blanked out (never a false "OK"), and the
  KPI history table shows no rows.
- **NAV data** — without it, net assets for the anchor date are unknown, so the "% NAV" column and
  every percentage-of-net-assets figure on the exposure panel is withheld (the euro notional figures
  themselves are unaffected).

Recording a monthly KPI needs a further, separate grant (Reference/Configure) from the one needed to
view the page, and downloading the evidence export needs its own Export grant on portfolio
positions. See [Access rights](access-rights.md) for how permissions, domains and grants are
structured across the tool.
