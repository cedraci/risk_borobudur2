# Borobudur Risk Tool — Design

**Date:** 2026-07-30
**Status:** Approved by user (sections reviewed interactively)

## Purpose

A locally-run risk monitoring tool for the Borobudur UCITS fund. It ingests the
periodic "NAV Recap" Excel workbook into PostgreSQL and computes the
performance and risk analytics a fund risk manager needs: YTD performance,
annualized volatility, yield/volatility ratio, Sharpe ratio, drawdown
analytics, monthly/quarterly performance tables, and VaR / Expected Shortfall
monitoring against the UCITS absolute-VaR limit.

## Source data

Sample file: `24-07-2026 - Borobudur - NAV Recap.xlsx`. Four sheets, all
ingested:

| Sheet | Content | Layout |
|---|---|---|
| `PORTEFEUILLE_NAV` | Portfolio positions (~113 rows): Actions, Fonds, Futures, Obligations, cash/margin accounts, fee provisions | Headers row 7, data rows 8+; NAV date in cell B3; AUM/shares/NAV in F2:F4 |
| `HISTO_NAV` | Daily NAV history since inception (2025-02-28, NAV 100): Date, AUM, Nb parts, NAV | Headers row 1 |
| `DIV` | Dividend records: provision date, payment date, issuer, amount, currency | Headers row 1 |
| `OPERATIONS` | Trade records: date, side, ticker, ISIN, name, ccy, qty, price, gross, fees, net | Headers row 3, data rows 4+ |

Notes:
- Fund history starts 2025-02-28 — "last 3 years" tables fill progressively as
  history accumulates.
- `HISTO_NAV`, `DIV`, `OPERATIONS` are cumulative (full history in every file).
- All risk metrics are computed from the `HISTO_NAV` daily NAV series.

## Key decisions (user-confirmed)

1. **Ingest all 4 sheets.**
2. **Local web app**: Rust backend, browser UI.
3. **Frontend: React SPA** — Vite + React + TypeScript + Apache ECharts.
4. **Dated snapshot import model**: each file adds a position snapshot keyed by
   its NAV date; time series upserted, never duplicated.
5. **VaR: historical + parametric (Gaussian + Cornish-Fisher) side by side**,
   UCITS preset (99% / 20-day / 252-day window / 20% absolute limit),
   adjustable parameters.
6. **Risk-free rate: fixed, configurable in settings** (default 2.0%/yr).
7. **Embedded PostgreSQL** via `postgresql_embedded` — no manual DB install;
   the app manages the server lifecycle; data in `%LOCALAPPDATA%\borobudur-risk\pg`.

## Architecture

```
borobudur-risk/                  (git repo)
├── Cargo.toml                   # workspace
├── crates/
│   ├── analytics/               # pure Rust math: returns, vol, drawdowns,
│   │                            #   VaR/ES, Sharpe — no I/O, no deps on db/http
│   ├── ingest/                  # calamine xlsx parsing → typed records
│   ├── db/                      # sqlx (Postgres): migrations, repositories
│   └── server/                  # axum: REST API, serves SPA, manages
│                                #   embedded Postgres lifecycle
├── frontend/                    # Vite + React + TypeScript + ECharts
└── docs/superpowers/specs/      # this document
```

- Server listens on `127.0.0.1:8787`, opens the browser on startup.
- Production build embeds `frontend/dist` into the binary via `rust-embed` →
  single `.exe` at runtime; Node.js needed only at build time.
- Imports happen through the UI: upload `.xlsx` to `POST /api/imports`; the
  server parses with `calamine` (no Excel required) and writes in one
  transaction per file.
- `analytics` operates on plain `Vec<(NaiveDate, f64)>` so every formula is
  unit-testable against hand-computed fixtures.

## Database schema

Money and NAV values stored as `NUMERIC`; migrations managed by sqlx.

- `imports` — `id, filename, sha256, nav_date, imported_at, row_counts jsonb`.
  Re-uploading a byte-identical file (same sha256) is a no-op.
- `nav_history` — `date PK, aum, shares, nav`. Upsert by date: overlapping
  dates update, old dates are never lost.
- `position_snapshots` — the 13 `PORTEFEUILLE_NAV` columns
  (asset_type, isin, name, currency, quantity, avg_cost, price, valuation_ccy,
  accrued_interest, fx_rate, valuation_eur, weight, ticker) + `nav_date` +
  `import_id`. Re-importing a file with an existing NAV date replaces that
  date's snapshot (delete + insert, same transaction).
- `dividends` — provision_date, payment_date, issuer, amount, currency.
- `operations` — trade_date, side, ticker, isin, name, currency, quantity,
  price, gross_amount, fees, net_price, net_amount.
- `dividends` / `operations` replace-all semantics: replaced wholesale when the
  incoming file's NAV date ≥ the latest imported NAV date; otherwise skipped
  with a warning (an old file never erases newer data).
- `settings` — key/value: `risk_free_rate` (default 0.02),
  `var_confidence` (default 0.99), `var_horizon_days` (default 20),
  `var_window_days` (default 252), `var_limit` (default 0.20),
  `short_dd_max_days` (default 50).

### Parsing rules (ingest crate)

- `PORTEFEUILLE_NAV`: keep a row only if it has an asset type and a parseable
  valuation; skip separator/sub-header rows (e.g. the stray "Type" line in the
  cash section). NAV date from B3, cross-checked against `HISTO_NAV` last date.
- Strict number/date parsing; any malformed cell rejects the whole file with a
  per-row error report. No partial imports — one transaction per file.
- The UI receives an import report: rows inserted / updated / skipped, with
  reasons.

## Analytics definitions

Daily return `r_t = NAV_t / NAV_{t-1} − 1` on the business-day series as
stored. Annualization uses 252 trading days (factor √252 for vol).

| Metric | Definition |
|---|---|
| YTD performance | `NAV_last / NAV_(last date ≤ 31 Dec prior year) − 1`; falls back to inception NAV if fund is younger than the year |
| Annualized volatility | `σ(daily returns) × √252`; headline (since inception and trailing 1Y) + rolling series |
| Yield/vol ratio | annualized return ÷ annualized volatility over the window, **no** risk-free deduction; headline + rolling series |
| Sharpe ratio | `(annualized return − rf) / annualized volatility`, rf from settings; headline + rolling series |
| Annualized return (window) | `(NAV_end / NAV_start)^(252 / n_obs) − 1` |
| Drawdown series | `NAV_t / max(NAV_0..t) − 1` → underwater chart |
| Max yearly drawdown | deepest drawdown within each calendar year (running peak resets at year start); table one row per year, plus overall since-inception max DD |
| Top-5 short drawdowns | distinct peak→trough episodes with peak-to-trough duration ≤ 50 **calendar** days (configurable), ranked by depth; columns: peak date, trough date, depth %, duration, recovery date or "ongoing" |
| Monthly performance | month-end NAV / prior month-end NAV − 1 (first month: vs inception NAV); matrix years × 12 months + annual total; last 3 calendar years |
| Quarterly performance | same at quarter ends; last 3 calendar years |
| VaR (1−c) | three methods side by side: **historical** (empirical quantile of daily returns over window W), **Gaussian** (μ + σ·z_c), **Cornish-Fisher** (z adjusted for skew/kurtosis). Horizon scaling √h. Confidence ∈ {95, 97.5, 99}%, horizon ∈ {1, 10, 20}d, window default 252d. Displayed as % of NAV and € on latest AUM |
| Expected Shortfall | mean loss beyond VaR, same three methods and parameters |
| UCITS monitor | preset 99% / 20d / 252d vs **20% absolute VaR limit**; rolling VaR-utilization chart with limit line; breach log |

**Rolling windows** for through-time charts: selectable 20 / 60 / 120 / 252
trading days, default 60.

**Data sufficiency:** a metric with fewer than 30 usable observations renders
"n/a" with an explanatory badge; a window longer than available history shrinks
to available data with a warning badge.

## API

JSON under `/api`:

| Endpoint | Purpose |
|---|---|
| `POST /api/imports` | multipart xlsx upload → import report |
| `GET /api/imports` | import history |
| `GET /api/nav` | NAV/AUM series |
| `GET /api/positions?date=` | snapshot for a date (default: latest); list of available dates |
| `GET /api/metrics/summary` | headline KPIs |
| `GET /api/metrics/rolling?window=` | rolling vol / Sharpe / yield-vol series |
| `GET /api/metrics/drawdowns` | underwater series, yearly max DD table, top-5 short episodes |
| `GET /api/metrics/calendar` | monthly + quarterly tables |
| `GET /api/metrics/var?confidence=&horizon=&window=` | VaR/ES all methods + rolling UCITS series + breaches |
| `GET/PUT /api/settings` | settings |

## UI pages (sidebar navigation)

1. **Overview** — KPI cards (NAV, AUM, YTD, vol 1Y, Sharpe 1Y, max DD,
   VaR 99%/20d vs limit), NAV line chart, underwater drawdown chart.
2. **Performance** — monthly table (heat-colored), quarterly table, yearly max
   drawdown table.
3. **Risk** — rolling volatility, rolling Sharpe, rolling yield/vol charts
   (shared window selector), top-5 short drawdowns table.
4. **VaR / ES** — method comparison cards, parameter controls, rolling VaR vs
   UCITS limit chart, breach log.
5. **Data** — drag-and-drop upload, import history + reports, portfolio
   snapshot table by date, settings editor.

## Error handling

- Typed error enum in `server` → RFC-7807-style problem-details JSON.
- Import failures return per-row diagnostics; nothing is written on failure.
- Embedded Postgres startup failure → clear console + UI message.
- Empty DB → UI empty-states pointing to the Data page.

## Testing

- `analytics`: unit tests against hand-computed fixtures (known small series →
  known vol/DD/VaR/Sharpe values, including Cornish-Fisher).
- `ingest`: tests against a fixture copy of the real sample workbook.
- `db` + API: integration tests against a temporary embedded Postgres.
- `frontend`: type-check + production build in CI; no E2E in v1.

## Out of scope (v1)

- Position-level risk decomposition (factor models, per-line VaR).
- Benchmark-relative metrics (tracking error, beta) — no benchmark series in
  the source file.
- Multi-fund support; automated file watching; authentication (localhost only).
- €STR/external market data feeds.
