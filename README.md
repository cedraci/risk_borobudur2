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

## Features

- **Limits page**: UCITS concentration checks (issuer 5/10/40, connected group 20%,
  target fund 20%, deposits 20% per bank) with OK/WATCH/BREACH statuses; liquidity
  bucketing (1d / 2–7d / 8–30d / >30d) with a configurable redemption stress; bond
  YTM / modified duration / DV01 (bond futures excluded — no notional data in the file).
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
