# Portfolio dimension — design

Date: 2026-08-10
Phase 1 of the multi-portfolio re-architecture (Approach A: one database, a
portfolio dimension threaded through schema → ingest → API → UI). Later
phases build on this one: mandate limit profiles + engine (2), mandate
overview dashboard (3), ratings ingest (4), batch upload (5).

## Context and goals

The tool is single-fund today: `imports`, `position_snapshots`,
`nav_history` are keyed by date alone, every analytics call assumes "the
fund", and the UI has no selector. The management company needs the same
analysis for several UCITS funds and, later, dozens of mandates. Phase 1
makes every existing feature portfolio-scoped without changing what any
feature computes.

Decisions taken during brainstorming:

- Each UCITS fund is analyzed independently (the current full dashboard,
  per fund). Mandates will get a cross-portfolio overview with drill-down
  into the same full dashboard (phase 3); a drilled-into mandate shows
  **full analytics**, not a reduced view — which is why one uniform
  portfolio dimension is the right shape.
- Upload attribution: the app has a current-portfolio context and uploads
  land in the portfolio being viewed. No file-content sniffing — the NAV
  Recap carries no fund name anywhere in the workbook (verified against
  the sample: the `PORTEFEUILLE_NAV` header block is date/AUM/shares/NAV
  only). Filenames do carry the portfolio name consistently, which
  phase 5 (batch upload) will use for auto-matching with confirmation;
  phase 1 does not.
- Mandate source-file format is unknown until a real file is in hand; the
  portfolio dimension must not care where rows came from.

## Scope

In scope:

1. `portfolios` entity: name, kind `ucits` | `mandate`, soft archive.
2. `portfolio_id` on every time-series table; existing data migrated onto
   portfolio #1 "Borobudur" (ucits) in place — nothing re-imported.
3. Every portfolio-scoped API route moves under `/api/portfolios/{id}/…`;
   repo functions gain a `portfolio_id` parameter.
4. Portfolio selector in the nav; routes gain a `/p/{id}/` prefix.
5. Portfolio admin panel (create / rename / archive) on the Data page.
6. Uploads (NAV Recap, CTD, EMIR KPI edits, settings) land in the selected
   portfolio.
7. The Bloomberg request workbook collects unclassified instruments across
   the union of every non-archived portfolio's latest snapshot.

Out of scope (later phases): mandate limit profiles and evaluation engine,
mandate overview dashboard, ratings ingest, batch upload, any parser other
than the NAV Recap format. Also out: multi-currency portfolio bases — the
tool assumes an EUR-based portfolio throughout and phase 1 keeps that
assumption (documented here, not configurable).

## Data model (migration 0008)

New table:

```sql
CREATE TABLE portfolios (
  id         BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  name       TEXT NOT NULL UNIQUE,
  kind       TEXT NOT NULL CHECK (kind IN ('ucits','mandate')),
  archived   BOOLEAN NOT NULL DEFAULT false,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

Seeded in the migration itself with `('Borobudur', 'ucits')` so the
backfill below always has its target (it gets id 1 on a fresh database and
on the live one alike).

`portfolio_id BIGINT NOT NULL REFERENCES portfolios(id)` is added to:
`imports`, `nav_history`, `position_snapshots`, `dividends`, `operations`,
`futures_analytics`, `emir_kpis`, `settings`. The migration adds each
column with `DEFAULT 1` (backfilling existing rows), then drops the
default — new writes must name their portfolio explicitly.

Key changes:

- `nav_history`: PK `date` → `(portfolio_id, date)`.
- `imports`: `sha256 UNIQUE` → `UNIQUE (portfolio_id, sha256)`. The same
  file cannot be double-imported *within* a portfolio, but dedupe does not
  block another portfolio — and a file uploaded to the wrong portfolio can
  simply be uploaded again to the right one.
- `futures_analytics`: PK `(nav_date, ticker)` →
  `(portfolio_id, nav_date, ticker)`.
- `emir_kpis`: PK `month` → `(portfolio_id, month)`.
- `settings`: PK `key` → `(portfolio_id, key)` — redemption stress is a
  per-portfolio choice.

Stays shared, untouched — facts about instruments and markets, not
portfolios: `instrument_refs` (classifications, issuer groups, liquidity
overrides, bond statics, tickers), `futures_contracts` (specs + OTC flag),
`fx_history`. Classify an instrument once and every portfolio benefits.

Archiving is soft: an archived portfolio disappears from the selector's
default list and refuses new data, but its history stays readable. No
delete.

The whole migration runs in one transaction; the live database migrates in
place on next server start.

## API

New portfolio management endpoints:

- `GET /api/portfolios` — list: id, name, kind, archived, and the latest
  NAV date on record per portfolio (freshness signal for the selector now
  and the phase-3 overview later).
- `POST /api/portfolios` — create `{name, kind}`. Name must be non-empty
  after trimming and unique; kind must be `ucits` or `mandate`; 422
  otherwise.
- `PUT /api/portfolios/{id}` — rename / archive / unarchive
  `{name, archived}`. Same validation. No delete endpoint.

Moved under `/api/portfolios/{id}/…`, same handler logic with the id
threaded through: `settings`, `imports`, `nav`, `positions`,
`metrics/summary`, `metrics/rolling`, `metrics/drawdowns`,
`metrics/calendar`, `metrics/var`, `metrics/concentration`,
`metrics/liquidity`, `metrics/rates`, `metrics/derivatives`,
`metrics/backtest`, `pnl`, `emir`, `emir/kpis/{month}`, `emir/export`,
`futures-analytics`. Every repo function these call gains a
`portfolio_id` parameter — wide but mechanical; the compiler enforces
completeness. No un-scoped aliases are kept: an old-path request is a 404,
and the frontend migrates in the same phase.

Staying global (shared reference data): `refs`, `refs/{code}`,
`futures-contracts`, `futures-contracts/{root}`, `bloomberg/request`,
`bloomberg/upload`, `health`.

Behavioral change to `bloomberg/request`: instead of "the latest
snapshot", it walks every non-archived portfolio, takes each one's own
latest snapshot date, unions the positions, and lists every
equity/fund/bond still missing classification (bonds: country only, per
the 2026-08-10 rule) plus every non-EUR currency held anywhere. One
Terminal round-trip serves the whole fleet. The FX `BDH` date range spans
from the earliest NAV-history date across portfolios to the latest
snapshot date across portfolios.

Errors: unknown portfolio id → 404 on every scoped route. Archived
portfolio → reads succeed, mutations (imports, CTD upload, KPI puts,
settings puts) → 409 with "portfolio is archived". EMIR export filename
becomes `EMIR - seuils - {portfolio name} - {anchor}.xlsx`.

## Frontend

Routes gain a portfolio prefix — `/p/{id}/` in front of each of the eight
existing pages (`/` overview, `/performance`, `/pnl`, `/risk`, `/var`,
`/limits`, `/derivatives`, `/data`) — bookmarkable,
and the phase-3 overview will deep-link straight into a mandate's pages.

- Nav gets a portfolio dropdown: active portfolios, name + kind badge;
  switching navigates to the same page under the new prefix.
- `/` redirects to the last-used portfolio (localStorage), falling back to
  the first active portfolio. If the stored id no longer exists or is
  archived, fall back the same way. If no portfolios exist (impossible
  after migration, which seeds Borobudur) the selector shows only the
  admin panel link.
- The Data page gains a **Portfolios** admin card: create (name + kind),
  rename, archive/unarchive. Its upload panels (NAV Recap, CTD) operate on
  the current portfolio and say so. The shared panels — issuer groups /
  liquidity / bond statics editor, Bloomberg classification, futures
  contracts — stay on the page, labeled "shared across all portfolios".
- `api.ts` functions gain a portfolio-id argument; a React context
  provides the current id from the route. No new dependencies; existing
  `index.css` classes only.

## Testing

- Two-portfolio isolation: create a second portfolio via the API, upload
  the same sample workbook to both (legal now that dedupe is
  per-portfolio), then assert limits, P&L and EMIR respond independently
  and identically for each; assert per-portfolio settings don't leak.
- Import dedupe: same file twice into one portfolio → duplicate skip;
  into a second portfolio → fresh import.
- Bloomberg union: instrument held only by portfolio 2 appears in the
  request workbook; classifying it (shared refs) removes it for everyone.
- Validation: 404 unknown portfolio; 422 bad kind/empty name/duplicate
  name; 409 mutations on an archived portfolio.
- Migration on existing data is exercised implicitly: every embedded-PG
  test runs the full migration chain, and the seeded-Borobudur backfill
  path is asserted by checking the sample upload lands on portfolio 1.
- Frontend: `npm run build` type-check; TS types field-for-field against
  the serialized Rust structs.
