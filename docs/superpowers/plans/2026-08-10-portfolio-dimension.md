# Portfolio Dimension Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Thread a portfolio dimension (UCITS funds + mandates) through schema, ingest, API and UI so every existing feature works per portfolio, with existing data migrated onto portfolio #1 "Borobudur".

**Architecture:** One database, one new `portfolios` table, `portfolio_id` on every time-series table (instrument/market reference data stays shared). Scoped API routes move under `/api/portfolios/{id}/…`; the frontend gains a `/p/{id}/` route prefix, a selector, and an admin card. Spec: `docs/superpowers/specs/2026-08-10-portfolio-dimension-design.md`.

**Tech Stack:** Rust (axum 0.8, sqlx, embedded PostgreSQL 17), React + TypeScript (vite, react-router).

## Global Constraints

- axum 0.8 path syntax: `{name}` braces, e.g. `/api/portfolios/{id}/pnl`.
- Migrations use `sqlx::migrate!("./migrations")` glob — new file `0008_portfolios.sql`, no registration anywhere. If a stale build ignores it: `cargo clean -p db`.
- Portfolio kinds: exactly `'ucits'` and `'mandate'`. Seed row: `('Borobudur', 'ucits')`, id 1.
- Shared (NO portfolio_id, endpoints stay global): `instrument_refs`, `futures_contracts`, `fx_history`; routes `refs`, `refs/{code}`, `futures-contracts`, `futures-contracts/{root}`, `bloomberg/request`, `bloomberg/upload`, `health`.
- Scoped (gain portfolio_id): `imports`, `nav_history`, `position_snapshots`, `dividends`, `operations`, `futures_analytics`, `emir_kpis`, `settings`.
- Errors: unknown portfolio → 404 on every scoped route; mutation on archived portfolio → 409; invalid kind / empty or duplicate name → 422. House ruling: signal data quality, never silently zero or hide.
- No old-path aliases: `/api/imports` etc. cease to exist (frontend migrates in the same branch).
- Frontend: no new dependencies, existing `index.css` classes only, `verbatimModuleSyntax` (type-only imports as `import type`). No test runner — `cd frontend && npm run build` is the type-check gate.
- Tests: no shared server test harness — each `api_*.rs` inlines its own helpers (house ruling).
- Windows/PowerShell environment; embedded-PG tests download binaries on first run. Stale postmaster after a killed run: `& "$env:LOCALAPPDATA\borobudur-risk\pg-install\17.10.0\bin\pg_ctl.exe" -D "$env:LOCALAPPDATA\borobudur-risk\pg-data" -m fast stop`.
- Commit trailer: `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

---

### Task 1: Migration 0008, portfolio repo functions, `/api/portfolios` CRUD

**Files:**
- Create: `crates/db/migrations/0008_portfolios.sql`
- Modify: `crates/db/src/repo.rs` (append portfolio section)
- Create: `crates/server/src/handlers/portfolios.rs`
- Modify: `crates/server/src/handlers/mod.rs` (add `pub mod portfolios;`)
- Modify: `crates/server/src/routes.rs` (three routes)
- Modify: `crates/server/src/error.rs` (add `NotFound`, `Conflict` variants)
- Test: `crates/server/tests/api_portfolios.rs`

**Interfaces:**
- Produces (later tasks rely on these exact signatures):
  - `db::repo::Portfolio { pub id: i64, pub name: String, pub kind: String, pub archived: bool, pub latest_nav_date: Option<chrono::NaiveDate> }` (serde::Serialize, sqlx::FromRow)
  - `db::repo::portfolios_list(pool) -> anyhow::Result<Vec<Portfolio>>` (ordered by id; includes archived)
  - `db::repo::portfolio_get(pool, id: i64) -> anyhow::Result<Option<Portfolio>>`
  - `db::repo::portfolio_create(pool, name: &str, kind: &str) -> anyhow::Result<Portfolio>`
  - `db::repo::portfolio_update(pool, id: i64, name: &str, archived: bool) -> anyhow::Result<Option<Portfolio>>`
  - `AppError::NotFound(String)` → 404, `AppError::Conflict(String)` → 409

- [ ] **Step 1: Write the migration**

`crates/db/migrations/0008_portfolios.sql` — exactly this content:

```sql
-- Phase 1 of the multi-portfolio re-architecture: a portfolio dimension.
-- Existing data belongs to the Borobudur UCITS fund, seeded here as id 1
-- (fresh table => identity starts at 1, on the live DB and in tests alike).
-- Instrument/market reference data (instrument_refs, futures_contracts,
-- fx_history) stays shared: facts about instruments, not portfolios.

CREATE TABLE portfolios (
  id         BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  name       TEXT NOT NULL UNIQUE,
  kind       TEXT NOT NULL CHECK (kind IN ('ucits','mandate')),
  archived   BOOLEAN NOT NULL DEFAULT false,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO portfolios (name, kind) VALUES ('Borobudur', 'ucits');

-- Add portfolio_id everywhere time-series, backfilling existing rows to
-- portfolio 1, then drop the default: new writes must name their portfolio.

ALTER TABLE imports            ADD COLUMN portfolio_id BIGINT NOT NULL DEFAULT 1 REFERENCES portfolios(id);
ALTER TABLE nav_history        ADD COLUMN portfolio_id BIGINT NOT NULL DEFAULT 1 REFERENCES portfolios(id);
ALTER TABLE position_snapshots ADD COLUMN portfolio_id BIGINT NOT NULL DEFAULT 1 REFERENCES portfolios(id);
ALTER TABLE dividends          ADD COLUMN portfolio_id BIGINT NOT NULL DEFAULT 1 REFERENCES portfolios(id);
ALTER TABLE operations         ADD COLUMN portfolio_id BIGINT NOT NULL DEFAULT 1 REFERENCES portfolios(id);
ALTER TABLE futures_analytics  ADD COLUMN portfolio_id BIGINT NOT NULL DEFAULT 1 REFERENCES portfolios(id);
ALTER TABLE emir_kpis          ADD COLUMN portfolio_id BIGINT NOT NULL DEFAULT 1 REFERENCES portfolios(id);
ALTER TABLE settings           ADD COLUMN portfolio_id BIGINT NOT NULL DEFAULT 1 REFERENCES portfolios(id);

ALTER TABLE imports            ALTER COLUMN portfolio_id DROP DEFAULT;
ALTER TABLE nav_history        ALTER COLUMN portfolio_id DROP DEFAULT;
ALTER TABLE position_snapshots ALTER COLUMN portfolio_id DROP DEFAULT;
ALTER TABLE dividends          ALTER COLUMN portfolio_id DROP DEFAULT;
ALTER TABLE operations         ALTER COLUMN portfolio_id DROP DEFAULT;
ALTER TABLE futures_analytics  ALTER COLUMN portfolio_id DROP DEFAULT;
ALTER TABLE emir_kpis          ALTER COLUMN portfolio_id DROP DEFAULT;
ALTER TABLE settings           ALTER COLUMN portfolio_id DROP DEFAULT;

-- Re-key the tables whose identity was previously "the fund's".
ALTER TABLE nav_history       DROP CONSTRAINT nav_history_pkey;
ALTER TABLE nav_history       ADD PRIMARY KEY (portfolio_id, date);
ALTER TABLE imports           DROP CONSTRAINT imports_sha256_key;
ALTER TABLE imports           ADD CONSTRAINT imports_portfolio_sha256_key UNIQUE (portfolio_id, sha256);
ALTER TABLE futures_analytics DROP CONSTRAINT futures_analytics_pkey;
ALTER TABLE futures_analytics ADD PRIMARY KEY (portfolio_id, nav_date, ticker);
ALTER TABLE emir_kpis         DROP CONSTRAINT emir_kpis_pkey;
ALTER TABLE emir_kpis         ADD PRIMARY KEY (portfolio_id, month);
ALTER TABLE settings          DROP CONSTRAINT settings_pkey;
ALTER TABLE settings          ADD PRIMARY KEY (portfolio_id, key);

CREATE INDEX position_snapshots_portfolio_date_idx ON position_snapshots (portfolio_id, nav_date);
```

Note: verify the two auto-named constraints before relying on them —
`SELECT conname FROM pg_constraint WHERE conrelid = 'imports'::regclass;`
in a scratch test or via psql on a fresh embedded DB. Postgres names them
`imports_sha256_key` and `settings_pkey` etc. by convention; if a name
differs, use the actual name in the DROP.

- [ ] **Step 2: Append the portfolio section to `crates/db/src/repo.rs`**

```rust
// ---- portfolios ----

#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct Portfolio {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub archived: bool,
    /// Latest imported NAV date, the freshness signal for selector/overview.
    pub latest_nav_date: Option<chrono::NaiveDate>,
}

const SELECT_PORTFOLIO: &str = "SELECT p.id, p.name, p.kind, p.archived,
    (SELECT max(nav_date) FROM imports i WHERE i.portfolio_id = p.id) AS latest_nav_date
 FROM portfolios p";

pub async fn portfolios_list(pool: &PgPool) -> anyhow::Result<Vec<Portfolio>> {
    Ok(sqlx::query_as(&format!("{SELECT_PORTFOLIO} ORDER BY p.id")).fetch_all(pool).await?)
}

pub async fn portfolio_get(pool: &PgPool, id: i64) -> anyhow::Result<Option<Portfolio>> {
    Ok(sqlx::query_as(&format!("{SELECT_PORTFOLIO} WHERE p.id = $1"))
        .bind(id).fetch_optional(pool).await?)
}

pub async fn portfolio_create(pool: &PgPool, name: &str, kind: &str) -> anyhow::Result<Portfolio> {
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO portfolios (name, kind) VALUES ($1, $2) RETURNING id")
        .bind(name).bind(kind).fetch_one(pool).await?;
    Ok(portfolio_get(pool, id).await?.expect("just inserted"))
}

pub async fn portfolio_update(pool: &PgPool, id: i64, name: &str, archived: bool) -> anyhow::Result<Option<Portfolio>> {
    let n = sqlx::query("UPDATE portfolios SET name = $2, archived = $3 WHERE id = $1")
        .bind(id).bind(name).bind(archived).execute(pool).await?.rows_affected();
    if n == 0 { return Ok(None); }
    portfolio_get(pool, id).await
}
```

- [ ] **Step 3: Add error variants**

In `crates/server/src/error.rs`, add to the enum:

```rust
    NotFound(String),
    Conflict(String),
```

and to `into_response`, following the existing arm pattern:

```rust
            AppError::NotFound(msg) => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"title": "Not Found", "status": 404, "detail": msg})),
            )
                .into_response(),
            AppError::Conflict(msg) => (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"title": "Conflict", "status": 409, "detail": msg})),
            )
                .into_response(),
```

- [ ] **Step 4: Write the failing test**

`crates/server/tests/api_portfolios.rs` — inline helpers per house rule
(copy the `get_json` shape from `api_bloomberg.rs`; no sample upload needed
here, an empty DB is enough):

```rust
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

async fn app() -> (axum::Router, sqlx::PgPool, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });
    (app, pool, edb)
}

async fn req_json(app: &axum::Router, method: &str, uri: &str, body: Option<serde_json::Value>)
    -> (StatusCode, serde_json::Value)
{
    let b = match body {
        Some(v) => Request::builder().method(method).uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(v.to_string())).unwrap(),
        None => Request::builder().method(method).uri(uri).body(Body::empty()).unwrap(),
    };
    let res = app.clone().oneshot(b).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let v = if bytes.is_empty() { serde_json::Value::Null }
            else { serde_json::from_slice(&bytes).unwrap() };
    (status, v)
}

#[tokio::test]
async fn portfolio_crud_and_validation() {
    let (app, pool, edb) = app().await;

    // Migration seeds Borobudur as portfolio 1.
    let (st, list) = req_json(&app, "GET", "/api/portfolios", None).await;
    assert_eq!(st, StatusCode::OK);
    let list = list.as_array().unwrap().clone();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["id"], 1);
    assert_eq!(list[0]["name"], "Borobudur");
    assert_eq!(list[0]["kind"], "ucits");
    assert_eq!(list[0]["archived"], false);
    assert_eq!(list[0]["latest_nav_date"], serde_json::Value::Null);

    // Create a mandate.
    let (st, p) = req_json(&app, "POST", "/api/portfolios",
        Some(serde_json::json!({"name": "Mandat Alpha", "kind": "mandate"}))).await;
    assert_eq!(st, StatusCode::OK, "{p}");
    assert_eq!(p["id"], 2);
    assert_eq!(p["kind"], "mandate");

    // Validation: bad kind, empty name, duplicate name -> 422.
    for bad in [
        serde_json::json!({"name": "X", "kind": "hedge"}),
        serde_json::json!({"name": "   ", "kind": "ucits"}),
        serde_json::json!({"name": "Mandat Alpha", "kind": "mandate"}),
    ] {
        let (st, _) = req_json(&app, "POST", "/api/portfolios", Some(bad)).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    }

    // Rename + archive round-trip.
    let (st, p) = req_json(&app, "PUT", "/api/portfolios/2",
        Some(serde_json::json!({"name": "Mandat Beta", "archived": true}))).await;
    assert_eq!(st, StatusCode::OK, "{p}");
    assert_eq!(p["name"], "Mandat Beta");
    assert_eq!(p["archived"], true);

    // Unknown id -> 404; duplicate rename -> 422.
    let (st, _) = req_json(&app, "PUT", "/api/portfolios/99",
        Some(serde_json::json!({"name": "Z", "archived": false}))).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = req_json(&app, "PUT", "/api/portfolios/2",
        Some(serde_json::json!({"name": "Borobudur", "archived": false}))).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);

    pool.close().await;
    edb.stop().await;
}
```

- [ ] **Step 5: Run it — expect FAIL** (`cargo test -p server --test api_portfolios`; routes don't exist yet, GET /api/portfolios falls through to the static handler)

- [ ] **Step 6: Implement the handler**

`crates/server/src/handlers/portfolios.rs`:

```rust
use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::Json;

#[derive(serde::Deserialize)]
pub struct CreateBody { pub name: String, pub kind: String }

#[derive(serde::Deserialize)]
pub struct UpdateBody { pub name: String, pub archived: bool }

fn valid_name(name: &str) -> Result<String, AppError> {
    let n = name.trim();
    if n.is_empty() {
        return Err(AppError::Unprocessable("name must not be empty".into()));
    }
    Ok(n.to_string())
}

fn valid_kind(kind: &str) -> Result<(), AppError> {
    if !matches!(kind, "ucits" | "mandate") {
        return Err(AppError::Unprocessable("kind must be 'ucits' or 'mandate'".into()));
    }
    Ok(())
}

/// Unique-violation on portfolios.name -> 422 with a helpful message; any
/// other DB error stays a 500.
fn map_name_conflict(e: anyhow::Error) -> AppError {
    let is_unique = e.downcast_ref::<sqlx::Error>()
        .and_then(|se| se.as_database_error())
        .map(|de| de.is_unique_violation())
        .unwrap_or(false);
    if is_unique {
        AppError::Unprocessable("a portfolio with that name already exists".into())
    } else {
        AppError::Internal(e)
    }
}

pub async fn list(State(st): State<AppState>) -> Result<Json<Vec<db::repo::Portfolio>>, AppError> {
    Ok(Json(db::repo::portfolios_list(&st.pool).await?))
}

pub async fn create(State(st): State<AppState>, Json(b): Json<CreateBody>)
    -> Result<Json<db::repo::Portfolio>, AppError>
{
    let name = valid_name(&b.name)?;
    valid_kind(&b.kind)?;
    let p = db::repo::portfolio_create(&st.pool, &name, &b.kind).await
        .map_err(map_name_conflict)?;
    Ok(Json(p))
}

pub async fn update(State(st): State<AppState>, Path(id): Path<i64>, Json(b): Json<UpdateBody>)
    -> Result<Json<db::repo::Portfolio>, AppError>
{
    let name = valid_name(&b.name)?;
    let p = db::repo::portfolio_update(&st.pool, id, &name, b.archived).await
        .map_err(map_name_conflict)?
        .ok_or_else(|| AppError::NotFound(format!("no portfolio {id}")))?;
    Ok(Json(p))
}
```

Add `pub mod portfolios;` to `handlers/mod.rs`. In `routes.rs` add:

```rust
        .route("/api/portfolios", get(handlers::portfolios::list).post(handlers::portfolios::create))
        .route("/api/portfolios/{id}", axum::routing::put(handlers::portfolios::update))
```

- [ ] **Step 7: Run tests — expect PASS**, then run the full suite (`cargo test`) to prove the migration breaks nothing.
- [ ] **Step 8: Commit** — `feat(db+server): portfolios table, migration onto Borobudur, /api/portfolios CRUD`

---

### Task 2: Thread portfolio_id through repo, settings, handlers, routes

The wide mechanical task. Everything compiles or nothing does — use the
compiler as the checklist. One commit.

**Files:**
- Modify: `crates/db/src/repo.rs`, `crates/db/src/settings.rs`
- Modify: `crates/server/src/handlers/{imports,data,metrics,limits,pnl,emir,futures,settings}.rs`
- Modify: `crates/server/src/routes.rs`
- Modify: every existing `crates/server/tests/api_*.rs` (URL prefix only)

**Interfaces:**
- Consumes: `Portfolio`, `portfolio_get` from Task 1.
- Produces: `crate::handlers::portfolios::ensure(pool, id, mutating) -> Result<db::repo::Portfolio, AppError>` — 404 if unknown; 409 if `mutating && archived`. Every scoped handler calls it first. Scoped repo signatures below.

- [ ] **Step 1: Re-scope the repo functions.** Each gains `portfolio_id: i64` immediately after `pool`, and its SQL gains a `portfolio_id = $n` predicate (SELECT/DELETE) or column (INSERT). Exact list:

| Function | Change |
| --- | --- |
| `import_workbook(pool, portfolio_id, filename, sha256, wb)` | dedupe lookup `WHERE sha256 = $1` → `WHERE portfolio_id = $1 AND sha256 = $2`; the `max(nav_date)` guard and every INSERT in the transaction (`imports`, `position_snapshots`, `nav_history`, `dividends`, `operations`) writes/filters `portfolio_id`. `nav_history` upsert conflict target `(date)` → `(portfolio_id, date)`. The futures-contract seeding INSERT (`futures_contracts`) is **shared — do not scope it**. |
| `nav_rows(pool, portfolio_id)` | `WHERE portfolio_id = $1` |
| `position_dates(pool, portfolio_id)` | ditto |
| `positions_for(pool, portfolio_id, date)` | ditto |
| `imports_list(pool, portfolio_id)` | ditto |
| `ctd_replace(pool, portfolio_id, …)` | DELETE + INSERT both scoped |
| `ctd_for(pool, portfolio_id, date)` | scoped |
| `aum_for(pool, portfolio_id, date)` | scoped |
| `operations_all(pool, portfolio_id)` | scoped |
| `dividends_all(pool, portfolio_id)` | scoped |
| `emir_kpis_all(pool, portfolio_id)` | scoped |
| `emir_kpi_upsert(pool, portfolio_id, k)` | INSERT column + conflict target `(portfolio_id, month)` |

Untouched (shared): `refs_all`, `refs_upsert`, `contracts_all`, `contracts_upsert`, `fx_all`, `fx_upsert_many`, `classify_upsert_many`, and the new portfolio functions.

In `crates/db/src/settings.rs`: `get_settings(pool, portfolio_id)` and `put_settings(pool, portfolio_id, s)`; every read filters `portfolio_id = $1`, the upsert writes the column with conflict target `(portfolio_id, key)`.

- [ ] **Step 2: Add the guard** to `handlers/portfolios.rs`:

```rust
/// Every scoped handler's first call. `mutating` requests (imports, CTD
/// upload, KPI puts, settings puts) are refused on an archived portfolio;
/// reads stay available so history remains inspectable.
pub async fn ensure(pool: &sqlx::PgPool, id: i64, mutating: bool)
    -> Result<db::repo::Portfolio, AppError>
{
    let p = db::repo::portfolio_get(pool, id).await?
        .ok_or_else(|| AppError::NotFound(format!("no portfolio {id}")))?;
    if mutating && p.archived {
        return Err(AppError::Conflict(format!("portfolio '{}' is archived", p.name)));
    }
    Ok(p)
}
```

- [ ] **Step 3: Re-scope handlers and routes.** In `routes.rs`, the scoped block becomes (kept flat — same style as today, axum 0.8 brace syntax):

```rust
        .route("/api/portfolios", get(handlers::portfolios::list).post(handlers::portfolios::create))
        .route("/api/portfolios/{id}", axum::routing::put(handlers::portfolios::update))
        .route("/api/portfolios/{id}/settings", get(handlers::settings::get).put(handlers::settings::put))
        .route("/api/portfolios/{id}/imports", get(handlers::imports::list).post(handlers::imports::upload))
        .route("/api/portfolios/{id}/nav", get(handlers::data::nav))
        .route("/api/portfolios/{id}/positions", get(handlers::data::positions))
        .route("/api/portfolios/{id}/metrics/summary", get(handlers::metrics::summary))
        .route("/api/portfolios/{id}/metrics/rolling", get(handlers::metrics::rolling))
        .route("/api/portfolios/{id}/metrics/drawdowns", get(handlers::metrics::drawdowns))
        .route("/api/portfolios/{id}/metrics/calendar", get(handlers::metrics::calendar))
        .route("/api/portfolios/{id}/metrics/var", get(handlers::metrics::var))
        .route("/api/portfolios/{id}/metrics/concentration", get(handlers::limits::concentration_h))
        .route("/api/portfolios/{id}/metrics/liquidity", get(handlers::limits::liquidity_h))
        .route("/api/portfolios/{id}/metrics/rates", get(handlers::limits::rates_h))
        .route("/api/portfolios/{id}/metrics/derivatives", get(handlers::limits::derivatives_h))
        .route("/api/portfolios/{id}/metrics/backtest", get(handlers::metrics::backtest))
        .route("/api/portfolios/{id}/pnl", get(handlers::pnl::get))
        .route("/api/portfolios/{id}/emir", get(handlers::emir::get))
        .route("/api/portfolios/{id}/emir/kpis/{month}", axum::routing::put(handlers::emir::put_kpi))
        .route("/api/portfolios/{id}/emir/export", get(handlers::emir::export))
        .route("/api/portfolios/{id}/futures-analytics",
            get(handlers::futures::list_ctd).post(handlers::futures::upload_ctd))
```

Global routes (`health`, `refs`, `refs/{code}`, `futures-contracts`, `futures-contracts/{root}`, `bloomberg/request`, `bloomberg/upload`) stay exactly as they are.

Handler pattern — every scoped handler follows this transformation (shown once; apply to all):

```rust
// before
pub async fn nav(State(st): State<AppState>) -> Result<Json<…>, AppError> {
    let rows = db::repo::nav_rows(&st.pool).await?;
// after
pub async fn nav(State(st): State<AppState>, Path(pid): Path<i64>) -> Result<Json<…>, AppError> {
    super::portfolios::ensure(&st.pool, pid, false).await?;
    let rows = db::repo::nav_rows(&st.pool, pid).await?;
```

`mutating: true` for exactly: `imports::upload`, `futures::upload_ctd`, `emir::put_kpi`, `settings::put`. Everything else `false`.

Handlers whose route already has a path param merge it into one extractor: `emir::put_kpi` takes `Path((pid, month)): Path<(i64, String)>`. Two-segment note: `emir/export` and `emir/kpis/{month}` keep working because axum matches the longer literal first; no wildcard conflicts exist here.

EMIR export filename (in `emir::export`): use the `ensure` return value —
`format!("EMIR - seuils - {} - {}.xlsx", portfolio.name, anchor)`.

- [ ] **Step 4: Mechanically update every existing server test** — URL substitution only, no logic changes: `/api/settings` → `/api/portfolios/1/settings`, `/api/imports` → `/api/portfolios/1/imports`, `/api/nav`, `/api/positions`, `/api/metrics/*`, `/api/pnl`, `/api/emir*`, `/api/futures-analytics` likewise. Global-route tests (`api_refs.rs` refs URLs, `api_bloomberg.rs` bloomberg URLs, futures-contracts URLs) keep their paths — only their *seeding* upload calls move to `/api/portfolios/1/imports`.

- [ ] **Step 5: Run the full suite** (`cargo test`) — everything green. This is the task's whole test story: ~25 existing integration tests re-exercised through the scoped routes on migrated schema. (New behavior tests — isolation, guards — are Task 3.)
- [ ] **Step 6: Commit** — `refactor(db+server): scope repo and routes by portfolio`

---

### Task 3: Isolation and guard tests

Proves the scoping actually isolates. Test-only task; if a test exposes a
Task 2 bug, fix it here.

**Files:**
- Create: `crates/server/tests/api_portfolio_isolation.rs`

**Interfaces:** consumes the sample-upload helper shape from `api_bloomberg.rs` (inline copies, house rule) and Task 1's `req_json` shape.

- [ ] **Step 1: Write the tests** — one file, helpers inlined, sample fixture at `../ingest/tests/fixtures/sample.xlsx`:

```rust
#[tokio::test]
async fn same_file_imports_independently_per_portfolio() {
    // create portfolio 2 (mandate) via POST /api/portfolios
    // upload sample to /api/portfolios/1/imports -> 200, nav_rows > 0
    // upload sample to /api/portfolios/1/imports again -> duplicate: true
    // upload sample to /api/portfolios/2/imports -> 200, duplicate NOT set
    //   (per-portfolio dedupe: same sha256, different portfolio)
    // GET /api/portfolios/1/positions == GET /api/portfolios/2/positions (same file)
    // GET /api/portfolios/{1,2}/metrics/summary both return data
}

#[tokio::test]
async fn settings_and_kpis_do_not_leak_across_portfolios() {
    // create portfolio 2; upload sample to both (EMIR needs positions)
    // PUT /api/portfolios/1/settings with redemption_shock 0.3
    // GET /api/portfolios/2/settings still has the default redemption_shock
    // PUT /api/portfolios/1/emir/kpis/2026-07-01 {unconfirmed_over_5d: 3, reconciliation: "done", disputes. 0, note: ""}
    // GET /api/portfolios/2/emir -> kpis empty; GET /api/portfolios/1/emir -> 1 kpi
}

#[tokio::test]
async fn unknown_and_archived_portfolios_are_refused() {
    // GET /api/portfolios/99/nav -> 404
    // create portfolio 2, archive it via PUT
    // GET /api/portfolios/2/nav -> 200 (reads stay available)
    // POST /api/portfolios/2/imports (sample) -> 409
    // PUT /api/portfolios/2/settings -> 409
}
```

Write them as real assertions, not comments — the sketches above fix the
scenarios; exact JSON field names come from the existing tests
(`duplicate` from `ImportOutcome`, `redemption_shock` from `AppSettings`,
the EMIR KPI body shape from `api_emir.rs`; note the KPI body uses
`disputes: 0` — the `disputes. 0` above is a sketch typo, not content).

- [ ] **Step 2: Run — expect PASS** (Task 2 done right) or fix what they catch.
- [ ] **Step 3: Commit** — `test(server): portfolio isolation, per-portfolio dedupe, 404/409 guards`

---

### Task 4: Bloomberg request unions all portfolios

**Files:**
- Modify: `crates/server/src/handlers/bloomberg.rs` (the `request` handler only)
- Test: extend `crates/server/tests/api_bloomberg.rs`

**Interfaces:** consumes `portfolios_list` (Task 1), scoped `position_dates`/`positions_for`/`nav_rows` (Task 2).

- [ ] **Step 1: Write the failing test** (in `api_bloomberg.rs`; the existing seeding already goes through `/api/portfolios/1/imports` after Task 2):

```rust
#[tokio::test]
async fn request_unions_unclassified_across_portfolios() {
    // create portfolio 2; upload sample into it only (portfolio 1 stays empty)
    // -> GET /api/bloomberg/request still lists sample instruments (union
    //    walks every active portfolio, not just portfolio 1)
    // then: create portfolio 3, archive it, upload refused (409) — archived
    //    portfolios are excluded from the walk by construction
    // classify one ISIN via a REFS response upload (or direct
    //    instrument_refs UPDATE, as in bond_with_country_but_no_sector…)
    // -> that ISIN disappears from a fresh request: shared refs serve everyone
}
```

- [ ] **Step 2: Run — expect FAIL** (portfolio 1 has no positions, handler still reads only its latest date → empty REFS).

- [ ] **Step 3: Rewrite the collection loop in `request`:**

```rust
    let refs = db::repo::refs_all(&st.pool).await?;
    let ref_state: std::collections::BTreeMap<&str, (bool, bool)> = refs.iter()
        .map(|r| (r.code.as_str(), (r.country_of_risk.is_some(), r.gics_sector.is_some())))
        .collect();

    // One request workbook serves the whole fleet: walk every non-archived
    // portfolio at its own latest snapshot and union the still-unclassified
    // instruments and non-EUR currencies.
    let mut items: Vec<RequestItem> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut currencies: BTreeSet<String> = BTreeSet::new();
    let mut latest_any: Option<chrono::NaiveDate> = None;
    let mut earliest_nav: Option<chrono::NaiveDate> = None;
    for pf in db::repo::portfolios_list(&st.pool).await?.iter().filter(|p| !p.archived) {
        let dates = db::repo::position_dates(&st.pool, pf.id).await?;
        let Some(latest) = dates.first().copied() else { continue };
        latest_any = Some(latest_any.map_or(latest, |d| d.max(latest)));
        if let Some(first_nav) = db::repo::nav_rows(&st.pool, pf.id).await?.first().map(|n| n.date) {
            earliest_nav = Some(earliest_nav.map_or(first_nav, |d| d.min(first_nav)));
        }
        for p in db::repo::positions_for(&st.pool, pf.id, latest).await? {
            if let Some(c) = &p.currency {
                if c != "EUR" { currencies.insert(c.clone()); }
            }
            if !matches!(p.asset_type.as_str(), "Action" | "Fonds" | "Obligation") { continue; }
            let (has_country, has_sector) =
                ref_state.get(p.isin.as_str()).copied().unwrap_or((false, false));
            if has_country && (has_sector || p.asset_type == "Obligation") { continue; }
            if !seen.insert(p.isin.clone()) { continue; }
            items.push(RequestItem {
                isin: p.isin.clone(),
                market_sector: market_sector_for(asset_class_of(&p.asset_type)).to_string(),
            });
        }
    }
    let from = earliest_nav.unwrap_or_else(|| chrono::Utc::now().date_naive());
    let to = latest_any.unwrap_or_else(|| chrono::Utc::now().date_naive());
```

(`nav_rows` ordering: check the existing SQL — if it returns DESC, take the min/max accordingly; the goal is earliest NAV-history date across portfolios → `from`, latest snapshot date across portfolios → `to`.)

- [ ] **Step 4: Run all Bloomberg tests + full suite — PASS.**
- [ ] **Step 5: Commit** — `feat(server): Bloomberg request walks every active portfolio`

---

### Task 5: Frontend portfolio scoping — api.ts, context, routes, selector

**Files:**
- Modify: `frontend/src/api.ts`
- Create: `frontend/src/PortfolioContext.tsx`
- Modify: `frontend/src/App.tsx`
- Modify: every `frontend/src/pages/*.tsx` and any component calling `api.ts` functions (compiler-driven: `npm run build` lists every call site)

**Interfaces:**
- Produces: `Portfolio` TS type `{ id: number; name: string; kind: "ucits" | "mandate"; archived: boolean; latest_nav_date: string | null }`; `getPortfolios(): Promise<Portfolio[]>`; `createPortfolio(name, kind)`; `updatePortfolio(id, name, archived)`; `usePortfolio(): Portfolio` hook (current portfolio from route); every scoped `api.ts` function gains `pid: number` as FIRST parameter and builds URLs as `/api/portfolios/${pid}/…`.

- [ ] **Step 1: api.ts.** Add the `Portfolio` type + three admin functions. Prefix every scoped fetcher (settings, imports, nav, positions, all metrics, pnl, emir trio, futures-analytics/CTD) with `pid: number`; `emirExportUrl` becomes `emirExportUrl(pid: number): string`. Global fetchers (refs, futures-contracts, bloomberg) unchanged.

- [ ] **Step 2: PortfolioContext.tsx:**

```tsx
import { createContext, useContext } from "react";
import type { Portfolio } from "./api";

/** The portfolio the current /p/{id}/… route is scoped to. Provided by the
 * route layout in App.tsx; every page reads it instead of a prop. */
export const PortfolioContext = createContext<Portfolio | null>(null);

export function usePortfolio(): Portfolio {
  const p = useContext(PortfolioContext);
  if (!p) throw new Error("usePortfolio outside /p/{id} routes");
  return p;
}

const LAST_KEY = "borobudur.lastPortfolio";
export function rememberPortfolio(id: number) {
  try { localStorage.setItem(LAST_KEY, String(id)); } catch { /* private mode */ }
}
export function lastPortfolio(): number | null {
  try {
    const v = localStorage.getItem(LAST_KEY);
    return v ? Number(v) : null;
  } catch { return null; }
}
```

- [ ] **Step 3: App.tsx.** Load portfolios once at app level (`useFetch(getPortfolios)`-style, matching the existing hook's conventions). Routing:
  - `/p/:pid/*` renders the nav + a `<Routes>` block with the eight existing pages at their current relative paths (`""` → Overview, `performance`, `pnl`, `risk`, `var`, `limits`, `derivatives`, `data`). The layout resolves `:pid` against the loaded list — found: provide `PortfolioContext`, call `rememberPortfolio`; not found: redirect to `/`.
  - `/` redirects to `/p/{lastPortfolio() ?? firstActive.id}/` (fall back to first active when the remembered id is missing or archived; archived portfolios are still reachable by explicit URL, just never the default).
  - The nav gains a `<select>` of active portfolios (label: name, suffix ` (mandat)` when kind is mandate) that navigates to the same relative page under the new prefix (derive the relative path from `useLocation()` by stripping `/p/{pid}`), and keeps the existing NavLinks relative to the prefix.
- [ ] **Step 4: Pages.** Every page/component calling a scoped fetcher adds `const portfolio = usePortfolio();` and passes `portfolio.id` as the first argument (this includes `useFetch` dependency arrays — the pid is a primitive dep, so switching portfolios refetches; that is the whole point). The Derivatives export link becomes `emirExportUrl(portfolio.id) + (date ? `?date=${date}` : "")`.
- [ ] **Step 5: `npm run build`** — chase every type error until green; the build IS the test.
- [ ] **Step 6: Commit** — `feat(ui): portfolio selector, /p/{id} routes, scoped API calls`

---

### Task 6: Data page — portfolios admin card + shared labels

**Files:**
- Create: `frontend/src/components/PortfoliosAdmin.tsx`
- Modify: `frontend/src/pages/DataPage.tsx`

**Interfaces:** consumes `getPortfolios`/`createPortfolio`/`updatePortfolio` and `usePortfolio` from Task 5.

- [ ] **Step 1: PortfoliosAdmin.tsx** — a `.card` titled "Portfolios", listing all portfolios in a `.tbl` (name, kind, latest NAV date or "—", archived badge) with: a rename input per row (pencil→save, same draft-overlay pattern as `FuturesContracts.tsx`), an archive/unarchive button per row (label "Archive"/"Restore"), and a create row (name input + kind `<select>` of `ucits`/`mandate` + "Create" button). Errors from the API (422 duplicate name etc.) render in the card via the existing error-display convention on the Data page. After any mutation, refetch the list and notify the parent (`onChange?: () => void` prop) so the nav selector refreshes.
- [ ] **Step 2: DataPage.tsx** — mount `<PortfoliosAdmin/>` as the FIRST card. Upload panels (NAV Recap, CTD): panel titles gain the current portfolio's name — e.g. `NAV Recap — {portfolio.name}` — so it is unmistakable where a file lands. Shared panels (Bloomberg classification, futures contracts, reference-data editor) each gain a `.kpi-sub` note "Shared across all portfolios".
- [ ] **Step 3: `npm run build`** — green.
- [ ] **Step 4: Commit** — `feat(ui): portfolio admin card; label shared vs per-portfolio panels`

---

### Task 7: README

**Files:**
- Modify: `README.md`

- [ ] **Step 1:** Update: intro sentence ("dashboard for the Borobudur UCITS fund" → monitors several portfolios — UCITS funds and mandates — each analyzed independently; existing data lives on the built-in Borobudur portfolio). Weekly workflow: step 0 "pick the portfolio in the nav; uploads land in the portfolio you are viewing". Features: a "Portfolios" bullet (create/rename/archive on the Data page, per-portfolio settings, shared instrument reference data, Bloomberg request covers every active portfolio in one workbook). Mention the EMIR export filename now carries the portfolio name.
- [ ] **Step 2: Commit** — `docs: multi-portfolio README`

---

## Self-review notes

- Spec coverage: §Scope 1→Task 1, 2→Tasks 1–2, 3→Task 2, 4→Task 5, 5→Task 6, 6→Tasks 2+5, 7→Task 4; API section→Tasks 1–2; Frontend→Tasks 5–6; Testing→Tasks 1–4 (server) + build gate (frontend). Migration-in-place is exercised by every embedded-PG test running the full chain.
- Constraint-name caveat in Task 1 Step 1 is deliberate: auto-generated Postgres constraint names must be verified, not assumed.
- Type consistency: `Portfolio.latest_nav_date` is `Option<NaiveDate>` ↔ TS `string | null`; `pid` is `i64` ↔ `number`; kind strings `'ucits'|'mandate'` everywhere.
