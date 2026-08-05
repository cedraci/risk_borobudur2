# Borobudur Risk

Local risk-monitoring dashboard for the Borobudur UCITS fund. Imports the
periodic "NAV Recap" .xlsx into an embedded PostgreSQL and serves analytics
(YTD, volatility, Sharpe, drawdowns, monthly/quarterly tables, VaR/ES with
UCITS 99%/20d monitoring) at http://127.0.0.1:8787.

## Build

Requires Rust (stable) and Node.js (build-time only).

    .\build.ps1

## Run

    .\target\release\server.exe

First start downloads a portable PostgreSQL 17 into
%LOCALAPPDATA%\borobudur-risk (one-time, needs network). The browser opens
automatically; go to the Data page and upload the NAV Recap workbook.

## Weekly workflow

1. Upload the NAV Recap on the Data page. New futures contracts are seeded with a
   point value derived from the file and flagged unconfirmed; confirm each one
   once, setting its category, curve and price convention. US Treasury futures are
   quoted in 32nds on the portfolio sheet — set their convention to `th32`.
2. Upload the CTD companion file for the same NAV date. Re-uploading replaces that
   date's rows, so a corrected pull simply overwrites.

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

## Features

- **Limits page**: UCITS concentration checks (issuer 5/10/40, connected group 20%,
  target fund 20%, deposits 20% per bank) with OK/WATCH/BREACH statuses; liquidity
  bucketing (1d / 2–7d / 8–30d / >30d) with a configurable redemption stress; bond
  YTM / modified duration / DV01 (bond futures included via weekly-uploaded
  cheapest-to-deliver analytics).
- **Derivatives exposure**: notional by reference to the underlying, by category
  (equity / interest rate / FX / credit / commodity / other), long and short each
  shown in absolute value as a percentage of net assets. Contract point values are
  derived from the workbook on import and confirmed on the Data page.
- **Bond-future DV01**: computed from cheapest-to-deliver analytics uploaded weekly
  as a companion file (`nav_date, ticker, ctd_isin, ctd_mod_duration,
  ctd_clean_price, ctd_accrued, conversion_factor`). A NAV date without analytics
  shows notional exposure normally and marks its DV01 as missing — values are never
  carried forward from a previous week.
- **VaR back-testing**: daily 1-day/99% VaR vs realized returns for all three methods,
  Basel traffic-light zones and Kupiec proportion-of-failures test.
- **Reference data** (Data page): editable issuer groups, liquidity bucket overrides
  and bond statics (coupon/maturity/frequency, auto-parsed from position names on import).

## Development

    cargo run -p server          # API on :8787
    cd frontend && npm run dev   # UI on :5173, proxies /api

Tests: `cargo test` (embedded-PG tests download binaries on first run) and
`cd frontend && npm run build` (type-check).

Design spec: docs/superpowers/specs/2026-07-30-borobudur-risk-tool-design.md
Plan: docs/superpowers/plans/2026-07-30-borobudur-risk-tool.md
