# Universal ingest + CACEIS adapter — design

Date: 2026-08-11
Phase 2 of the multi-portfolio re-architecture. Builds on Phase 1 (portfolio
dimension, merged 2026-08-11). Subsequent phases renumber: mandate limit
profiles + engine (3), mandate overview dashboard (4), ratings ingest (5).
The formerly planned "batch upload" phase is absorbed into this one and
disappears.

## Context and goals

The tool ingests exactly one format today: the NAV Recap workbook, parsed by
`crates/ingest` straight into the portfolio-scoped tables. The management
company's depositary (CACEIS Bank Luxembourg) issues daily fixed-layout CSV
files per fund, and mandates will arrive as dozens of portfolios fed the
same way. Rather than bolting a second parser onto the pipeline, this phase
formalizes a **universal data contract** that all analytics consume, fed by
**per-source adapter modules** whose column transpositions are declarative
tables in code. Adding a future depositary = one new adapter module written
against sample files; the analytics core never changes.

Decisions taken during brainstorming:

- One depositary today (CACEIS); architecture must make a second easy but
  only CACEIS is built now. Adapters are code modules — no mapping-config
  UI (explicitly declined).
- Files received daily: HISINVLUX (position inventory), HISTOVLLUX
  (NAV/TNA/shares), INVXDVLUX (consolidated inventory with unrealized
  decomposition). JOUROPLUX (transaction journal) is NOT received.
- INVXDVLUX duplicates HISINVLUX for our needs → recognized but rejected
  as "not needed" in v1.
- Operations (subscriptions/redemptions) are **derived** from HISTOVLLUX
  day-over-day share deltas; dividends are **derived** from HISINVLUX CPON
  receivable deltas (user's explicit choice over requesting JOUROPLUX).
- The NAV Recap remains supported as adapter #1 (refactored behind the same
  contract) — not needed in the daily CSV flow, useful for history
  backfill. Daily operation needs only HISINVLUX + HISTOVLLUX.
- Sample files analyzed: `HISINVLUX_165878_20260807_*.csv` (110 rows,
  66 columns), `HISTOVLLUX_165878_20260729_*.csv` (1 row, 22 columns),
  `INVXDVLUX_165878_20260804_*.csv`, plus the depositary's own header
  glossary (`Glossary GP CSV Headers.xlsx`). Fund code 165878 =
  BOROBUDUR GLOBAL OPP.

## Scope

In scope:

1. `UniversalBatch` contract in `crates/ingest`; NAV Recap parser
   refactored behind it (adapter #1) with byte-identical import results.
2. CACEIS adapter (adapter #2): HISINVLUX + HISTOVLLUX parsers,
   INVXDVLUX recognition-with-rejection, declarative transposition tables.
3. Auto-routing of self-identifying files via a new `portfolio_codes`
   mapping table, editable in the Portfolios admin card.
4. Multi-file upload (one request, per-file results).
5. Derived operations and dividends with a `derived` flag; explicit rows
   always win.
6. Reference hints: risk country + Bloomberg ticker from CACEIS lines
   pre-fill `instrument_refs` where NULL.
7. Cross-file TNA consistency check (warning).

Out of scope: any other depositary; a mapping-config UI; multi share
class portfolios (hard error for now); JOUROPLUX/INVXDVLUX parsing;
multi-currency portfolio bases (EUR assumption unchanged); mandate limit
profiles (phase 3).

## The universal data contract

`UniversalBatch`, produced by every adapter, is the only thing the import
pipeline writes to the database:

- `nav_points`: `{date, aum, shares_outstanding, nav_per_share}` —
  0..n per file (NAV Recap HISTO_NAV carries many; HISTOVLLUX carries one).
- `snapshots`: 0..n of `{nav_date, positions: Vec<PositionRow>}`.
  `PositionRow` is the existing struct, unchanged: asset_type, isin/code,
  name, currency, quantity, avg_cost, price, valuation_ccy,
  accrued_interest, fx_rate, valuation_eur, weight, ticker.
- `operations`: `{date, label, amount}`.
- `dividends`: `{date, isin, name, amount}`.
- `ref_hints`: `{isin, country_of_risk?, ticker?}` — optional enrichment
  applied to the shared `instrument_refs` **only where the target field is
  currently NULL**; Bloomberg-sourced data is never overwritten. Bonds
  arrive fully classified via the country-only rule; classified
  instruments drop out of the Bloomberg request workbook.

An adapter may fill any subset of the batch. The pipeline upserts what is
present, scoped to the routed portfolio, under the existing dedupe rule
(one import per file hash per portfolio; latest import for a date wins).

**Asset-type vocabulary is a closed list** — the labels analytics already
dispatch on, exactly as the NAV Recap emits them: `Action`, `Fonds`,
`Obligation`, `Future`, `Cash Acc`, `Margin Acc`, `Dividendes`,
`Frais provisionnés`, `Provisions ordres`. Adapters must map source codes
onto this list; an unmappable code is a per-row import error (signal,
don't hide), never a silent "Other".

## Adapter interface, detection, routing

Each source is a module in `crates/ingest` implementing:

- `detect(filename, bytes) -> Option<FileKind>` — "is this file mine,
  which family?" CACEIS claims `HISINVLUX_*.csv` / `HISTOVLLUX_*.csv`
  (and recognizes `INVXDVLUX_*.csv` to reject it with "not needed"),
  with a content sanity check (semicolon count, `yyyymmdd` dates) so a
  renamed random file cannot slip through. The NAV Recap adapter claims
  `.xlsx` with the `PORTEFEUILLE_NAV`/`HISTO_NAV` sheet set. Unrecognized
  file → error listing supported formats.
- `identify(filename, bytes) -> Identification` — external fund code +
  NAV date extracted before parsing. CACEIS: both sit in the filename
  (`HISINVLUX_{fund}_{navdate}_{timestamp}.csv`) and in every row;
  cross-checked, mismatch is a file-level rejection. The NAV Recap has no
  internal identifier → "no code".
- `parse(bytes) -> (UniversalBatch, Vec<RowError>)` — per-row errors
  collected exactly like today's cell-level errors.

**Routing.** New table `portfolio_codes (portfolio_id, source, code)`,
`UNIQUE (source, code)`, editable in the Portfolios admin card (e.g.
Borobudur ↔ CACEIS `165878`).

- Self-identifying file (CACEIS): routed by code lookup **regardless of
  the selected portfolio** — a CSV cannot be misfiled. Unknown code →
  file-level error naming the code ("unknown CACEIS code 165878 — map it
  in the Portfolios panel"); nothing written. Routed-to archived
  portfolio → 409, like any mutation.
- Non-identifying file (NAV Recap): lands in the selected portfolio,
  current behavior, existing archived-guard applies.

**Upload UX.** The Data page upload panel accepts multiple files per
request (a day's bundle or a backlog; order irrelevant — every file
carries its date). Response: per-file result list — routed portfolio,
family, date, rows written, errors/warnings. This replaces the planned
phase-5 batch upload.

## CACEIS transposition tables

Declarative constants in `crates/ingest/src/caceis.rs` — named column
indices, so a layout change is a one-table edit. Files are
semicolon-delimited, headerless, Latin-1, dates `yyyymmdd`, numbers
space-padded with trailing dots (`8336.23333333`, `-12.`).

### HISINVLUX (66 columns) → one position snapshot

| Universal field | CACEIS column (0-based) |
|---|---|
| nav_date | 0 (cross-checked vs filename) |
| fund code | 3 |
| asset_type | 5 (CATVAL) + 16 (Detail Asset Type GP3) — table below |
| isin / code | 45 (ISIN) if present, else 6 (instrument code, e.g. `CFIN2608`, `BK001CHF`) |
| name | 8 |
| currency | 9 (Asset Ccy) |
| quantity | 25 |
| avg_cost | 30 (Unit cost price) |
| price | 28 (Market price; on TRES rows this column holds a conversion rate → NULL there) |
| valuation_ccy | 51 (Market value, local ccy) |
| valuation_eur | 32 (Market value, fund ccy) |
| accrued_interest | 33 |
| weight | 35 (MV/TNA, already %) |
| fx_rate | derived: col 51 ÷ col 32 (ccy per EUR, NAV Recap convention); 1.0 for EUR |
| ticker | 65 (Bloomberg Code; `-1`/blank → none) |
| ref hint: country_of_risk | 41 (Risk country, ISO alpha-3 → alpha-2) |
| ref hint: ticker | 65 |

### Asset-type mapping (closed; unlisted code → row error)

| CACEIS CATVAL + GP3 | Universal |
|---|---|
| VMOB 111xx | Action |
| VMOB 12xxx | Fonds |
| VMOB 13xxx | Obligation |
| FUTU 18xxx | Future |
| TRES COMPTE | Cash Acc |
| TRES MARGES | Margin Acc |
| TRES FP, TRES PF | Frais provisionnés |
| TRES PS, TRES PU | Provisions ordres |
| CPON (any) | Dividendes |

Verified against the samples: FP+PF row count equals the NAV Recap's
"Frais provisionnés" lines (25); VMOB 13900 is the gold ETC the workbook
classes as Obligation; futures arrive at zero cost value with the
market-value column holding mark-to-market — the same representation the
NAV Recap uses, so derivatives analytics are untouched.

### HISTOVLLUX (22 columns) → one NAV point

fund code 0, NAV date 2, share class 3, NAV 5, AUM = TNA 6, shares =
Outstanding 7. Previous-NAV columns ignored. **Single share class only in
v1**: two classes for one fund in a file → file-level error ("multi share
class not supported yet"), never a silent sum.

## Derived datasets

**Operations (from HISTOVLLUX).** A pure function of the NAV series: for
each pair of consecutive NAV points, a shares-outstanding change beyond
0.001 share produces an operation of `delta_shares × new NAV`, labeled
Souscription (delta > 0) or Rachat (delta < 0), dated at the newer point.
Every HISTOVLLUX import **re-derives the whole derived set** for that
portfolio from the series in the database — deterministic and
order-independent: uploading a backlog in any order converges. Derived
rows carry `derived = true`; a date holding an explicit (file-sourced)
operation is skipped, so sources never double-count.

**Dividends (from CPON deltas).** Same recompute-from-consecutive-
snapshots principle: a CPON line that appears or grows produces a dividend
event for that ISIN, dated at the newer snapshot. Change detection runs on
the **local-currency value** (col 51), so FX moves on a foreign-currency
receivable emit nothing; the recorded amount is the EUR delta.
Disappearance of a receivable (payment settled to cash) emits nothing.
Derived rows flagged and skipped where explicit NAV Recap dividends exist,
same rule as operations.

Schema changes for this phase, in full: `portfolio_codes` table +
`derived BOOLEAN NOT NULL DEFAULT false` on `operations` and `dividends`.

## Error handling

- **File-level — reject, nothing written:** unrecognized format; unknown
  fund code; filename date ≠ row dates; multi share class; wrong column
  count. Response says exactly what is wrong.
- **Row-level — import with visible warnings:** unmappable asset-type
  code, unparsable number/date, missing ISIN where expected. Stored with
  the import, shown in the imports list like today's cell errors.
- **Cross-file consistency (warning):** once both files for a date are
  in, sum of HISINVLUX market values vs HISTOVLLUX TNA; drift beyond
  0.1% flags the import — catches truncated position files and stale NAVs.

## Testing

- **Fixtures:** trimmed copies of the real files (a dozen representative
  rows: every asset category, the JPY future, the GBP CPON line) at
  `crates/ingest/tests/fixtures/caceis_*.csv`, plus second-day variants so
  derivation tests have consecutive dates. Full-size originals stay
  untracked in the repo root.
- **Adapter units:** detection accepts/rejects correctly; every transposed
  column asserted against known fixture values; Latin-1, trailing-dot
  numbers, `yyyymmdd`; unmappable GP3 → row error; multi share class →
  rejection; INVXDVLUX → "not needed" rejection.
- **Derivation units:** share-delta ops incl. out-of-order convergence;
  CPON dividends incl. FX-only move emitting nothing; explicit-beats-
  derived on both.
- **API tests:** one multipart request routing files to two portfolios via
  `portfolio_codes`; unknown code → clean error, nothing written; routed
  file to archived portfolio → 409; per-portfolio hash dedupe; TNA
  cross-check warning surfaced; NAV Recap imports byte-identically after
  the adapter refactor (regression guard).
- **Frontend:** `npm run build` gate; upload panel per-file result list.
