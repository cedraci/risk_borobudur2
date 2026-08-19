# Borobudur Risk

Local risk-monitoring dashboard for several portfolios — UCITS funds and
mandates, each analyzed independently. Imports the periodic "NAV Recap" .xlsx
workbook, and/or the depositary's own CACEIS Bank Luxembourg CSV exports, into
an embedded PostgreSQL and serves analytics (YTD, volatility, Sharpe,
drawdowns, monthly/quarterly tables, VaR/ES with UCITS 99%/20d monitoring) at
http://127.0.0.1:8787. Existing data lives on the built-in Borobudur portfolio.

A full end-user guide — one chapter per tab, plus access rights and
administration — lives in [docs/user-guide/](docs/user-guide/README.md).

## Build

Requires Rust (stable) and Node.js (build-time only).

    .\build.ps1

## Run

    .\target\release\server.exe

First start downloads a portable PostgreSQL 17 into
%LOCALAPPDATA%\borobudur-risk (one-time, needs network). The browser opens
automatically; go to the Data page and upload the NAV Recap workbook, or drop
CACEIS CSV exports for a custodian-fed portfolio (see "CACEIS CSV feed"
below).

## Weekly workflow

0. Pick the portfolio in the nav. Uploads land in the portfolio you are viewing
   — except CACEIS CSVs, which self-identify their fund and route by the
   Portfolios panel's code mapping regardless of which portfolio is selected
   (see "CACEIS CSV feed" below).
1. Upload the NAV Recap on the Data page. New futures contracts are seeded with a
   point value derived from the file and flagged unconfirmed; confirm each one
   once, setting its category, curve and price convention. US Treasury futures are
   quoted in 32nds on the portfolio sheet — set their convention to `th32`.
   Re-uploading a workbook already on record does not re-import it, but it does
   seed any contract specs that are still missing, so it is the repair for a
   futures contract table that is unexpectedly empty.
2. Upload the CTD companion file for the same NAV date. Re-uploading replaces that
   date's rows, so a corrected pull simply overwrites.
3. On the Data page's Bloomberg classification panel, export the request workbook.
   It lists every equity/fund/bond position still missing a country or sector
   (bonds only need a country — Bloomberg publishes no GICS classification for
   Corp/Govt securities, so they graduate off the list once the country is
   stored) plus every non-EUR currency held, with `BDP`/`BDH` formulas that only resolve on a
   machine with a logged-in Bloomberg Terminal add-in — open the file in Excel
   there, let the formulas fill in, save, and upload the result back. The upload
   classifies instruments by country/region/sector/industry, stores the FX rates,
   and cross-checks each rate against the NAV Recap's own Change column at every
   snapshot date it applies to; a mismatch beyond 1% usually means Bloomberg
   returned the inverse quote and is reported rather than stored silently. Cells
   whose formula never resolved (`#N/A`, blank) are skipped and listed, not
   guessed at. The parser assumes a specific `BDH` spill shape (two columns per
   currency, dates then rates); this was built and tested against a synthetic
   workbook, since no live Bloomberg Terminal was available to confirm the
   add-in's actual spill layout — if a real pull parses differently, the
   symptom will be currency rows landing in `skipped` rather than `fx_rows`.

The companion file is one row per bond future, `.csv` or `.xlsx` (an `.xlsx` file
is read from a worksheet named `CTD`, falling back to its first sheet if there
isn't one), with these columns in any order:

| Column | Meaning |
| --- | --- |
| `nav_date` | `YYYY-MM-DD`; must be the same on every row and match an already-uploaded NAV Recap date. |
| `ticker` | Bloomberg-style futures ticker, e.g. `TYU6 Comdty`; must match a future held in that NAV date's snapshot. |
| `ctd_isin` | ISIN of the cheapest-to-deliver bond. |
| `ctd_mod_duration` | Modified duration of the CTD bond. |
| `ctd_clean_price` | Clean price of the CTD bond. |
| `ctd_accrued` | Accrued interest of the CTD bond (zero is allowed). |
| `conversion_factor` | CTD conversion factor for the contract. |

Upload it on the Data page, in the "Weekly CTD analytics" panel below the futures
contract table. A successful upload confirms the row count and NAV date; a bad
file is rejected with the offending row and column named so it can be fixed and
re-uploaded — nothing is stored until every row passes.

## CACEIS CSV feed

A portfolio administered by CACEIS Bank Luxembourg can be fed directly from
the depositary's own exports instead of (or alongside) the NAV Recap workbook.
Drop one or more files onto the Data page in one go — `HISINVLUX_<fund
code>_<yyyymmdd>_<timestamp>.csv` (positions) and `HISTOVLLUX_<fund
code>_<yyyymmdd>_<timestamp>.csv` (NAV, TNA, shares outstanding) — and each
file comes back with its own result: format detected, portfolio routed to,
rows imported, or the rejection reason. A CACEIS file self-identifies its fund
from the code embedded in its filename and routes through the code mapping
set once per portfolio on the Portfolios panel (a "CACEIS code" column next
to each portfolio); an unmapped code is rejected with a message pointing back
there. `INVXDVLUX` is recognized and declined as redundant — HISINVLUX already
carries the positions. `JOUROPLUX`, the trade journal, is recognized and
declined pending a sample file to build its parser against; until it flows,
a CSV-fed portfolio has no trade journal, so the P&L page shows price/FX
effects on it without realized-trade attribution.

Every HISINVLUX/HISTOVLLUX import cross-checks the position total against the
NAV file's own AUM for any date where both now exist, and reports a warning
when the two drift apart by more than 0.1%. Dividends for a CACEIS-fed
portfolio have no explicit journal, so they are derived instead from the
growth of CPON receivable positions between consecutive snapshots and stored
flagged as derived; an explicit dividend journal for the same date (e.g. from
a NAV Recap import) always wins and suppresses the derived entry. CACEIS rows
that carry a risk country or Bloomberg ticker pre-fill those fields — plus the
region implied by the country — into the shared reference data, but only
where the field is still empty, so a value already confirmed via Bloomberg is
never overwritten. A bond classified this way needs no GICS sector to count
as done (Bloomberg publishes none for Corp/Govt securities), so it drops out
of the Bloomberg request workbook as soon as its country is known.

## Features

- **Portfolios**: create, rename, and archive/restore portfolios on the Data
  page. Everything under a portfolio — imports, NAV history, positions,
  dividends, operations, CTD analytics, EMIR KPIs, settings including
  redemption stress — is scoped to it; instrument reference data (classifications,
  issuer groups, liquidity, bond statics, futures contract specs, FX rates) is
  shared across all portfolios. The Bloomberg request workbook and its FX
  cross-check on upload cover every active portfolio's latest snapshot in one
  pass. An archived portfolio stays readable but refuses new data, and drops
  out of the nav selector.
- **Limits page**: UCITS concentration checks (issuer 5/10/40, connected group 20%,
  target fund 20%, deposits 20% per bank) with OK/WATCH/BREACH statuses; liquidity
  bucketing (1d / 2–7d / 8–30d / >30d) with a configurable redemption stress; bond
  YTM / modified duration / DV01 (bond futures included via weekly-uploaded
  cheapest-to-deliver analytics).
- **Derivatives exposure** (Derivatives page): notional by reference to the
  underlying, by category (equity / interest rate / FX / credit / commodity /
  other), long and short each shown in absolute value as a percentage of net
  assets. Contract point values are derived from the workbook on import and
  confirmed on the Data page.
- **Derivatives / EMIR page**: the derivatives exposure display (moved here from
  Limits) plus EMIR clearing-threshold monitoring — the average of month-end
  gross notional per asset class over the last 12 months (each month uses the
  latest snapshot inside that month; months without one are reported missing,
  and the average says "N of 12"), with total and of-which-OTC lines. Only OTC
  notional counts against the 1/1/3/3/4 bn EUR thresholds (WATCH at 80%);
  contracts default to non-OTC and are flagged on the Data page's contract
  panel (a contract on a non-equivalent third-country venue is OTC even if
  exchange-listed). Also derives the reconciliation-cadence tier and
  compression trigger from the OTC contract count (conservatively assuming a
  single counterparty), shows margin-account balances, records monthly
  middle-office KPIs (confirmations > 5 days, reconciliation status,
  disputes), and exports the full calculation as an `.xlsx` evidence file
  (`EMIR - seuils - {portfolio name} - {anchor}.xlsx`) for archiving per the
  EMIR procedure.
- **NAV sensitivity per +100bp**: signed as profit and loss, `-100 × Σ DV01 ÷ AUM`
  at the snapshot's own NAV date. Negative means net assets fall if yields rise
  100bp (the book is long rates); positive means they rise. It covers the cash
  bonds plus every bond future for which CTD analytics exist on that exact date,
  and shows `–` rather than a zero when the AUM is unknown.
- **Bond-future DV01**: computed from cheapest-to-deliver analytics uploaded weekly
  as a companion file (`nav_date, ticker, ctd_isin, ctd_mod_duration,
  ctd_clean_price, ctd_accrued, conversion_factor`). A NAV date without analytics
  shows notional exposure normally and marks its DV01 as missing — values are never
  carried forward from a previous week.
- **VaR back-testing**: daily 1-day/99% VaR vs realized returns for all three methods,
  Basel traffic-light zones and Kupiec proportion-of-failures test.
- **Reference data** (Data page): editable issuer groups, liquidity bucket overrides
  and bond statics (coupon/maturity/frequency, auto-parsed from position names on import).
  CACEIS imports additionally pre-fill country of risk, region and Bloomberg ticker
  where those fields are still empty (see "CACEIS CSV feed" above).
- **P&L page**: attributes period P&L per instrument into realized/unrealized price
  and realized/unrealized FX, grouped by asset class, country, region, sector,
  industry, currency or issuer group, with a reconciliation to the fund's own AUM
  change (investment P&L, cash/margin, accrued fees, provisions, dividend income,
  less net subscriptions/redemptions) and the residual flagged once it exceeds
  tolerance. MTD/QTD/YTD/ITD presets and a custom date range are struck between the
  two imported NAV dates nearest the request, never interpolated — the page shows
  the actual dates used when they differ from what was asked for. A partial sale
  following a mid-period purchase cannot split that purchase's FX exactly by
  weighted-average costing; it is flagged per instrument (⚠) rather than silently
  approximated. Futures carry no cost basis — their P&L is the variation-margin
  change — and realized P&L from a bond future closed mid-period is not yet derived
  from `OPERATIONS`, so it reports as zero and any resulting error surfaces in the
  residual instead of being absorbed.

## Server mode (multi-user)

Set `BOROBUDUR_DATABASE_URL` (external PostgreSQL) to run as a multi-user
server instead of the single-user desktop app; `BOROBUDUR_BIND` picks the
address, and on an empty database `BOROBUDUR_ADMIN_EMAIL` creates the first
administrator and prints a one-hour single-use enrolment token to the log.
Users, grants, roles and the audit log are managed on the Administration
page.

**TLS is assumed to terminate in front of the server**: the session cookie
is marked `Secure`, so over plain HTTP browsers drop it and login silently
fails. Put the server behind an HTTPS reverse proxy (or an SSH tunnel to
127.0.0.1) — never expose the plain-HTTP port directly.

## Development

    cargo run -p server          # API on :8787
    cd frontend && npm run dev   # UI on :5173, proxies /api

Tests: `cargo test` (embedded-PG tests download binaries on first run) and
`cd frontend && npm run build` (type-check).

Design spec: docs/superpowers/specs/2026-07-30-borobudur-risk-tool-design.md
Plan: docs/superpowers/plans/2026-07-30-borobudur-risk-tool.md
