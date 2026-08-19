//! Task 11: composite endpoints whose SECONDARY domain is denied must
//! degrade to an explicit `{"status": "unavailable", "reason": "..."}"`
//! marker, never to a value that reads as a pass. Helpers copied from
//! `api_authz_slice.rs` / `api_authz_matrix.rs` — this crate has no shared
//! `tests/common` module and every `api_*.rs` file inlines its own setup.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use db::auth::{Action, Domain, Grant};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

const SAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx");
const BOUNDARY: &str = "XBOUNDARYX";

/// A fresh embedded database with two routers over the same pool: `desktop`
/// (full access, used only to seed data) and `server` (grant-gated, used for
/// the actual assertions). Mirrors the two-router pattern already used by
/// `api_authz_slice.rs`'s `desktop_mode_reaches_everything_without_a_cookie`.
async fn app() -> (axum::Router, axum::Router, sqlx::PgPool, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    let pool = dbh.test_pool().clone();
    std::mem::forget(dir);
    let desktop = server::routes::router(server::state::AppState::desktop(dbh.clone()));
    let server = server::routes::router(server::state::AppState::server(dbh.clone()));
    (desktop, server, pool, edb)
}

static NEXT_USER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

async fn user_with(pool: &sqlx::PgPool, grants: &[Grant]) -> String {
    let n = NEXT_USER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let hash = server::auth::local::hash_password("pw").unwrap();
    let admin = db::admin::Admin::new(pool);
    let id = admin.create_user(&format!("u{n}@f.lu"), "U", &hash, false).await.unwrap();
    for g in grants {
        admin.grant_add(id, *g, None).await.unwrap();
    }
    let token = format!("t{n}");
    admin.session_create(&server::auth::local::token_hash(&token), id, 1).await.unwrap();
    format!("borobudur_session={token}")
}

async fn portfolio(pool: &sqlx::PgPool, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO portfolios (name, kind) VALUES ($1,'ucits') RETURNING id")
        .bind(name).fetch_one(pool).await.unwrap()
}

fn upload_req(uri: &str, bytes: &[u8]) -> Request<Body> {
    let mut body = Vec::new();
    body.extend_from_slice(format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"s.xlsx\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    ).as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    Request::post(uri)
        .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(Body::from(body))
        .unwrap()
}

async fn get(app: &axum::Router, uri: &str, cookie: Option<&str>) -> StatusCode {
    let mut b = Request::get(uri);
    if let Some(c) = cookie { b = b.header("cookie", c); }
    app.clone().oneshot(b.body(Body::empty()).unwrap()).await.unwrap().status()
}

async fn get_json(app: &axum::Router, uri: &str, cookie: Option<&str>) -> (StatusCode, serde_json::Value) {
    let mut b = Request::get(uri);
    if let Some(c) = cookie { b = b.header("cookie", c); }
    let res = app.clone().oneshot(b.body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let v = if bytes.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
    (status, v)
}

/// Seed `pid` with the sample workbook through the desktop (unrestricted)
/// router, so the restricted-grant router used for assertions never needs
/// import-level grants at all.
async fn seed(desktop: &axum::Router, pid: i64) {
    let bytes = std::fs::read(SAMPLE).unwrap();
    let res = desktop.clone().oneshot(upload_req(&format!("/api/portfolios/{pid}/imports"), &bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn liquidity_computes_the_asset_side_and_marks_the_liability_side_unavailable() {
    let (desktop, server, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    seed(&desktop, pid).await;

    // Positions + market_data + nav, but NOT shareholders. Nav is granted so
    // the asset side actually computes rather than falling into the generic
    // "no data yet" shape liquidity_h uses when NAV itself is unavailable
    // (a separate secondary-domain degrade, out of this test's scope).
    let cookie = user_with(&pool, &[
        Grant { domain: Domain::Positions, action: Action::View, portfolio: Some(pid) },
        Grant { domain: Domain::MarketData, action: Action::View, portfolio: Some(pid) },
        Grant { domain: Domain::Nav, action: Action::View, portfolio: Some(pid) },
    ]).await;

    let (status, body) = get_json(&server, &format!("/api/portfolios/{pid}/metrics/liquidity"), Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The asset side (positions-only) is still computed.
    assert!(!body["asset"]["normal"].is_null(), "asset side missing: {body}");
    let buckets = body["asset"]["normal"]["buckets"].as_array().unwrap();
    assert!(!buckets.is_empty());

    // The liability side (top-5 redemption scenario, which reads
    // Shareholders) is explicitly unavailable — never a computed pass.
    let scenarios = body["scenarios"].as_array().unwrap();
    let top5 = scenarios.iter().find(|s| s["key"] == "top5").unwrap();
    assert_eq!(top5["status"], "unavailable", "{top5}");
    assert_eq!(top5["reason"], "not permitted: shareholder register", "{top5}");
    assert!(top5["waterfall"].is_null());

    let hybrid_top5 = scenarios.iter().find(|s| s["key"] == "hybrid_top5").unwrap();
    assert_eq!(hybrid_top5["status"], "unavailable", "{hybrid_top5}");
    assert_eq!(hybrid_top5["reason"], "not permitted: shareholder register", "{hybrid_top5}");

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn concentration_is_unavailable_rather_than_a_pass_when_positions_are_denied() {
    let (desktop, server, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    let other = portfolio(&pool, "Other").await;
    seed(&desktop, pid).await;

    // Nav only: visible (some domain grant exists) but Positions denied.
    let cookie = user_with(&pool, &[
        Grant { domain: Domain::Nav, action: Action::View, portfolio: Some(pid) },
    ]).await;
    assert_eq!(
        get(&server, &format!("/api/portfolios/{pid}/metrics/concentration"), Some(&cookie)).await,
        StatusCode::FORBIDDEN
    );

    // Grant positions/view on a DIFFERENT portfolio and re-check the same
    // portfolio: still 403. No computed result is ever produced from a
    // cross-portfolio grant.
    let cookie2 = user_with(&pool, &[
        Grant { domain: Domain::Nav, action: Action::View, portfolio: Some(pid) },
        Grant { domain: Domain::Positions, action: Action::View, portfolio: Some(other) },
    ]).await;
    assert_eq!(
        get(&server, &format!("/api/portfolios/{pid}/metrics/concentration"), Some(&cookie2)).await,
        StatusCode::FORBIDDEN
    );

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn a_denied_component_reason_is_distinguishable_from_missing_data() {
    let (desktop, server, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    seed(&desktop, pid).await;

    // (a) Shareholders granted, but the register was never loaded.
    let cookie_a = user_with(&pool, &[
        Grant { domain: Domain::Positions, action: Action::View, portfolio: Some(pid) },
        Grant { domain: Domain::Nav, action: Action::View, portfolio: Some(pid) },
        Grant { domain: Domain::Shareholders, action: Action::View, portfolio: Some(pid) },
    ]).await;
    let (status, body) = get_json(&server, &format!("/api/portfolios/{pid}/metrics/liquidity"), Some(&cookie_a)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let top5_a = body["scenarios"].as_array().unwrap().iter().find(|s| s["key"] == "top5").unwrap();
    assert_eq!(top5_a["status"], "unavailable");
    let reason_missing = top5_a["reason"].as_str().unwrap().to_string();
    assert_eq!(reason_missing, "no shareholder register");

    // (b) Shareholders denied outright.
    let cookie_b = user_with(&pool, &[
        Grant { domain: Domain::Positions, action: Action::View, portfolio: Some(pid) },
        Grant { domain: Domain::Nav, action: Action::View, portfolio: Some(pid) },
    ]).await;
    let (status, body) = get_json(&server, &format!("/api/portfolios/{pid}/metrics/liquidity"), Some(&cookie_b)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let top5_b = body["scenarios"].as_array().unwrap().iter().find(|s| s["key"] == "top5").unwrap();
    assert_eq!(top5_b["status"], "unavailable");
    let reason_denied = top5_b["reason"].as_str().unwrap().to_string();
    assert_eq!(reason_denied, "not permitted: shareholder register");

    assert_ne!(reason_missing, reason_denied);

    pool.close().await;
    edb.stop().await;
}

/// Ruling 2 (Task 9 review, bound to Task 11): a denied Reference read must
/// not silently drop issuer-group overrides and render every 5/10/40 check a
/// pass with no marker. `checks` still computes (Positions is granted, the
/// route's primary domain), but the response must say the overrides could
/// not be checked.
#[tokio::test]
async fn concentration_marks_reference_overrides_unavailable_when_reference_is_denied() {
    let (desktop, server, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    seed(&desktop, pid).await;

    let cookie = user_with(&pool, &[
        Grant { domain: Domain::Positions, action: Action::View, portfolio: Some(pid) },
    ]).await;
    let (status, body) = get_json(&server, &format!("/api/portfolios/{pid}/metrics/concentration"), Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["issuer_overrides"]["status"], "unavailable", "{body}");
    assert_eq!(body["issuer_overrides"]["reason"], "not permitted: reference data", "{body}");

    // Round 1 review (Important 2): a green check beside the marker is
    // exactly the under-aggregation risk the marker exists to flag — no
    // check's own status may still read "ok"/"watch"/"breach" (a pass-
    // adjacent value) when the overrides behind its grouping were denied.
    let checks = body["checks"].as_array().unwrap();
    assert!(!checks.is_empty(), "{body}");
    for c in checks {
        assert_eq!(c["status"], "unavailable", "check {c} still carries a computed status");
        for r in c["rows"].as_array().unwrap() {
            assert_eq!(r["status"], "unavailable", "row {r} still carries a computed status");
        }
    }

    pool.close().await;
    edb.stop().await;
}

/// Ruling 1 (Task 9 review, bound to Task 11): a denied Reference read
/// (`contracts_all`) must not degrade to empty contract specs and render
/// every clearing-obligation verdict "ok". The report must mark the
/// component unavailable, and the evidence export must refuse to produce a
/// document built on that denied read.
#[tokio::test]
async fn emir_marks_clearing_obligation_unavailable_and_export_refuses_when_reference_is_denied() {
    let (desktop, server, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    seed(&desktop, pid).await;

    let cookie = user_with(&pool, &[
        Grant { domain: Domain::Positions, action: Action::View, portfolio: Some(pid) },
    ]).await;

    let (status, body) = get_json(&server, &format!("/api/portfolios/{pid}/emir"), Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["clearing_obligation"]["status"], "unavailable", "{body}");
    assert_eq!(body["clearing_obligation"]["reason"], "not permitted: reference data", "{body}");

    // Round 1 review (Important 2): a per-class "ok" verdict beside the
    // marker is a pass value one field away from the denial — every class's
    // own verdict must also read "unavailable", not the computed pass/fail
    // the (empty, denied) contract specs happened to produce.
    let classes = body["classes"].as_array().unwrap();
    assert!(!classes.is_empty(), "{body}");
    for c in classes {
        assert_eq!(c["verdict"], "unavailable", "class {c} still carries a computed verdict");
        assert!(c["avg_otc_eur"].is_null(), "class {c} still carries a computed avg_otc_eur");
    }

    // Export refuses outright rather than emitting an evidence document
    // whose verdicts were computed on a denied read. `Denied::kind` is
    // `NotGranted` here (the portfolio itself is visible via Positions), so
    // this is a 403, not a 404 — DeniedKind exists precisely to make that
    // distinction, so pin it exactly rather than merely "not 200".
    let cookie_export = user_with(&pool, &[
        Grant { domain: Domain::Positions, action: Action::View, portfolio: Some(pid) },
        Grant { domain: Domain::Positions, action: Action::Export, portfolio: Some(pid) },
    ]).await;
    let status = get(&server, &format!("/api/portfolios/{pid}/emir/export"), Some(&cookie_export)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "export must refuse when its verdicts are built on a denied Reference read");

    pool.close().await;
    edb.stop().await;
}

/// The brief's own worklist ("Apply the same treatment to any other
/// composite result reading a second domain — the P&L attribution's
/// transaction detail"): a denied Transactions read must be surfaced, not
/// silently folded into an unexplained reconciliation residual.
#[tokio::test]
async fn pnl_marks_transaction_detail_unavailable_when_transactions_are_denied() {
    let (desktop, server, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    seed(&desktop, pid).await;
    // A second snapshot date is needed to strike a P&L period.
    {
        let earliest: chrono::NaiveDate = sqlx::query_scalar("SELECT MIN(date) FROM nav_history WHERE portfolio_id = $1")
            .bind(pid).fetch_one(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO position_snapshots
                 (portfolio_id, nav_date, import_id, asset_type, isin, name, currency, quantity,
                  avg_cost, price, valuation_ccy, accrued_interest, fx_rate, valuation_eur, weight, ticker)
             SELECT portfolio_id, $1, import_id, asset_type, isin, name, currency, quantity,
                    avg_cost, price, valuation_ccy, accrued_interest, fx_rate, valuation_eur, weight, ticker
             FROM position_snapshots WHERE portfolio_id = $2 AND nav_date = (SELECT MAX(nav_date) FROM position_snapshots WHERE portfolio_id = $2)",
        )
        .bind(earliest).bind(pid)
        .execute(&pool)
        .await
        .unwrap();
    }

    let cookie = user_with(&pool, &[
        Grant { domain: Domain::Positions, action: Action::View, portfolio: Some(pid) },
    ]).await;
    let (status, body) = get_json(&server, &format!("/api/portfolios/{pid}/pnl"), Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["transaction_detail"]["status"], "unavailable", "{body}");
    assert_eq!(body["transaction_detail"]["reason"], "not permitted: transactions", "{body}");

    // Regression pin (round 1 review): the denial also surfaces in
    // `warnings`, not only in the new `transaction_detail` field — a reader
    // who only skims `warnings` (the pre-existing UI surface) must not miss
    // that the reconciliation residual may be distorted by the denial.
    let warnings = body["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|w| w.as_str().unwrap().contains("not permitted: transactions")),
        "{body}"
    );

    pool.close().await;
    edb.stop().await;
}

/// Critical (round 1 review): liquidity's `refs_all` degrade is verdict-
/// falsifying, not merely lossy enrichment. With Reference denied, every
/// position's ADV/liquidity-days override is dropped, `build_positions`
/// falls back to `liquidity_default_days` (1 day for equities —
/// db/src/settings.rs), and a holding that measures tens-of-days liquid
/// reports same-week liquid instead — flipping a scenario from breach to
/// ok with only a `"no adv"` fallback reason, which reads as missing data,
/// not denial. The fix required here is the marker (mirroring
/// concentration's `issuer_overrides`), not (yet) suppressing the
/// scenario's own status — see the round-1 report for the reviewer's exact
/// scope.
#[tokio::test]
async fn liquidity_marks_issuer_overrides_unavailable_when_reference_is_denied() {
    let (desktop, server, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    seed(&desktop, pid).await;

    // Positions + market_data + shareholders + nav granted, but NOT
    // Reference.
    let cookie = user_with(&pool, &[
        Grant { domain: Domain::Positions, action: Action::View, portfolio: Some(pid) },
        Grant { domain: Domain::MarketData, action: Action::View, portfolio: Some(pid) },
        Grant { domain: Domain::Shareholders, action: Action::View, portfolio: Some(pid) },
        Grant { domain: Domain::Nav, action: Action::View, portfolio: Some(pid) },
    ]).await;

    let (status, body) = get_json(&server, &format!("/api/portfolios/{pid}/metrics/liquidity"), Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["issuer_overrides"]["status"], "unavailable", "{body}");
    assert_eq!(body["issuer_overrides"]["reason"], "not permitted: reference data", "{body}");

    pool.close().await;
    edb.stop().await;
}

/// CRITICAL RESIDUAL (round 2 review): `issuer_overrides` is a sibling
/// marker, not a suppression — on `a75d84f` the scenario itself still
/// computed a pass-reading "ok"/"breach" status (plus waterfall/slice_days/
/// residual) from the same Reference-degraded `caps`, one field away from
/// the marker. This pins the actual non-flip, not just the marker's
/// presence: override nearly every held instrument's `liquidity_days` to 90
/// (a Reference fact), which pushes the fixed 30% redemption scenario past
/// the 3-day settlement deadline — a genuine breach — whenever the override
/// is visible. With Reference denied, the same portfolio, same positions,
/// same settings must read `"unavailable"`, never "ok" and never "breach":
/// the fallback (`liquidity_default_days`, 1 day for equities) would
/// otherwise silently report same-week liquidity and flip the verdict.
#[tokio::test]
async fn liquidity_scenario_status_does_not_flip_from_breach_to_ok_when_reference_is_denied() {
    let (desktop, server, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    seed(&desktop, pid).await;

    // Every instrument this portfolio holds becomes "slow" (90 days) via a
    // Reference-domain fact (instrument_refs.liquidity_days), not a
    // Positions fact — direct SQL rather than the HTTP PUT endpoint purely
    // to avoid enumerating every ISIN by hand; the effect on the read path
    // is identical either way. An `INSERT ... ON CONFLICT`, not a bare
    // `UPDATE`: the NAV Recap adapter (`sample.xlsx`'s kind) emits no ref
    // hints/facts at import time (`ingest::adapter::to_batch`), so no
    // `instrument_refs` row exists yet for any of these codes — a plain
    // `UPDATE` would match zero rows and silently do nothing.
    sqlx::query(
        "INSERT INTO instrument_refs (code, liquidity_days)
         SELECT DISTINCT isin, 90 FROM position_snapshots WHERE portfolio_id = $1
         ON CONFLICT (code) DO UPDATE SET liquidity_days = EXCLUDED.liquidity_days",
    )
    .bind(pid)
    .execute(&pool)
    .await
    .unwrap();

    // With Reference granted, the override is visible: cash/margin is the
    // only instantly-liquid slice of this fund (a small fraction of NAV —
    // see the concentration suite's deposit_20 check), so raising 30% of
    // NAV within the 3-day settlement deadline is not possible. The test
    // setup itself must produce a breach, or this test proves nothing about
    // the flip.
    let cookie_granted = user_with(&pool, &[
        Grant { domain: Domain::Positions, action: Action::View, portfolio: Some(pid) },
        Grant { domain: Domain::Nav, action: Action::View, portfolio: Some(pid) },
        Grant { domain: Domain::Reference, action: Action::View, portfolio: None },
    ]).await;
    let (status, body) = get_json(&server, &format!("/api/portfolios/{pid}/metrics/liquidity"), Some(&cookie_granted)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let fixed_granted = body["scenarios"].as_array().unwrap().iter().find(|s| s["key"] == "fixed").unwrap();
    assert_eq!(fixed_granted["status"], "breach", "test setup did not produce a breach: {fixed_granted}");

    // With Reference denied — same portfolio, same positions, same
    // settings — the scenario must read "unavailable", never "ok" and never
    // "breach": a plain `assert_ne!(status, "ok")` would not catch a
    // regression that instead started emitting a (wrong) "breach" without
    // the reason/marker, so every one of the three is checked.
    let cookie_denied = user_with(&pool, &[
        Grant { domain: Domain::Positions, action: Action::View, portfolio: Some(pid) },
        Grant { domain: Domain::Nav, action: Action::View, portfolio: Some(pid) },
    ]).await;
    let (status, body) = get_json(&server, &format!("/api/portfolios/{pid}/metrics/liquidity"), Some(&cookie_denied)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let fixed_denied = body["scenarios"].as_array().unwrap().iter().find(|s| s["key"] == "fixed").unwrap();
    assert_ne!(fixed_denied["status"], "ok", "{fixed_denied}");
    assert_ne!(fixed_denied["status"], "breach", "{fixed_denied}");
    assert_eq!(fixed_denied["status"], "unavailable", "{fixed_denied}");
    assert_eq!(fixed_denied["reason"], "not permitted: reference data", "{fixed_denied}");
    assert!(fixed_denied["waterfall"].is_null(), "{fixed_denied}");
    assert!(fixed_denied["slice_days"].is_null(), "{fixed_denied}");
    assert!(fixed_denied["residual"].is_null(), "{fixed_denied}");
    assert!(fixed_denied["curve"].is_null(), "{fixed_denied}");

    pool.close().await;
    edb.stop().await;
}

/// Critical item 3 (round 1 review): a denied Nav grant must be
/// distinguishable from a genuinely empty NAV history — both currently
/// collapse into the same "no data yet" empty shape.
#[tokio::test]
async fn liquidity_nav_denial_is_distinguishable_from_empty_nav_history() {
    let (desktop, server, pool, edb) = app().await;

    // (a) Nav granted, but the NAV history row for this date is genuinely
    // absent — not a denial.
    let pid_empty = portfolio(&pool, "Empty").await;
    seed(&desktop, pid_empty).await;
    sqlx::query("DELETE FROM nav_history WHERE portfolio_id = $1")
        .bind(pid_empty).execute(&pool).await.unwrap();
    let cookie_a = user_with(&pool, &[
        Grant { domain: Domain::Positions, action: Action::View, portfolio: Some(pid_empty) },
        Grant { domain: Domain::Nav, action: Action::View, portfolio: Some(pid_empty) },
    ]).await;
    let (status, body) = get_json(&server, &format!("/api/portfolios/{pid_empty}/metrics/liquidity"), Some(&cookie_a)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["nav_status"]["status"], "unavailable", "{body}");
    let reason_missing = body["nav_status"]["reason"].as_str().unwrap().to_string();

    // (b) Nav denied outright, on a different portfolio whose NAV history
    // is intact.
    let pid_denied = portfolio(&pool, "Denied").await;
    seed(&desktop, pid_denied).await;
    let cookie_b = user_with(&pool, &[
        Grant { domain: Domain::Positions, action: Action::View, portfolio: Some(pid_denied) },
    ]).await;
    let (status, body) = get_json(&server, &format!("/api/portfolios/{pid_denied}/metrics/liquidity"), Some(&cookie_b)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["nav_status"]["status"], "unavailable", "{body}");
    let reason_denied = body["nav_status"]["reason"].as_str().unwrap().to_string();
    assert_eq!(reason_denied, "not permitted: NAV history");

    assert_ne!(reason_missing, reason_denied);

    pool.close().await;
    edb.stop().await;
}

/// Round 2 review item 3: with no position snapshot at all (`date: None`),
/// Nav authorization must still be consulted — on the pre-fix code the
/// `None` branch hardcoded "no NAV data" without ever checking the grant,
/// so a denied Nav on an empty portfolio silently read as missing data
/// rather than a denial.
#[tokio::test]
async fn liquidity_nav_denial_is_reported_even_with_no_position_snapshot_at_all() {
    let (_desktop, server, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    // Deliberately not seeded: no position snapshot exists, so `date`
    // resolves to `None` inside `liquidity_h`.

    let cookie = user_with(&pool, &[
        Grant { domain: Domain::Positions, action: Action::View, portfolio: Some(pid) },
    ]).await;
    let (status, body) = get_json(&server, &format!("/api/portfolios/{pid}/metrics/liquidity"), Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["nav_status"]["status"], "unavailable", "{body}");
    assert_eq!(body["nav_status"]["reason"], "not permitted: NAV history", "{body}");

    pool.close().await;
    edb.stop().await;
}

/// Regression pin (round 1 review): `fx_check_skipped` was added but had no
/// coverage. A portfolio the uploader cannot see (no Positions grant) must
/// be named, not silently absent from the fleet-wide fx-drift check.
#[tokio::test]
async fn bloomberg_upload_names_a_positions_denied_portfolio_in_fx_check_skipped() {
    let (desktop, server, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    seed(&desktop, pid).await;

    // Global MarketData/Import (the route's own gate), but no Positions
    // grant on `pid` at all.
    let cookie = user_with(&pool, &[
        Grant { domain: Domain::MarketData, action: Action::Import, portfolio: None },
    ]).await;

    let mut wb = rust_xlsxwriter::Workbook::new();
    let s = wb.add_worksheet().set_name("REFS").unwrap();
    for (c, h) in ["isin", "ticker", "country_of_risk", "gics_sector", "gics_industry"].iter().enumerate() {
        s.write_string(0, c as u16, *h).unwrap();
    }
    let bytes = wb.save_to_buffer().unwrap();

    let mut body = Vec::new();
    body.extend_from_slice(format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"resp.xlsx\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    ).as_bytes());
    body.extend_from_slice(&bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    let req = Request::post("/api/bloomberg/upload")
        .header("cookie", &cookie)
        .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(Body::from(body)).unwrap();
    let res = server.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let resp_body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();

    let skipped = resp_body["fx_check_skipped"].as_array().unwrap();
    assert!(
        skipped.iter().any(|s| s["portfolio_id"] == pid && s["reason"] == "not permitted: positions"),
        "{resp_body}"
    );

    pool.close().await;
    edb.stop().await;
}

/// Worklist item 1: a denied Reference/Configure grant must not silently
/// read as "nothing to classify" (`classified: 0` is indistinguishable from
/// a response workbook that genuinely resolved zero cells). Mirrors
/// `fx_check_skipped`'s approach with a `classification_status` marker.
#[tokio::test]
async fn bloomberg_upload_marks_classification_unavailable_when_reference_configure_is_denied() {
    let (_desktop, server, pool, edb) = app().await;

    // Global MarketData/Import (the route's own gate), but no Reference
    // grant at all.
    let cookie = user_with(&pool, &[
        Grant { domain: Domain::MarketData, action: Action::Import, portfolio: None },
    ]).await;

    // A response workbook that WOULD classify a real instrument if Reference
    // were granted — proves the zero below is suppression, not coincidence.
    let mut wb = rust_xlsxwriter::Workbook::new();
    let s = wb.add_worksheet().set_name("REFS").unwrap();
    for (c, h) in ["isin", "ticker", "country_of_risk", "gics_sector", "gics_industry"].iter().enumerate() {
        s.write_string(0, c as u16, *h).unwrap();
    }
    s.write_string(1, 0, "FR0000121014").unwrap();
    s.write_string(1, 1, "MC FP Equity").unwrap();
    s.write_string(1, 2, "France").unwrap();
    s.write_string(1, 3, "Consumer Discretionary").unwrap();
    s.write_string(1, 4, "Textiles, Apparel & Luxury Goods").unwrap();
    let bytes = wb.save_to_buffer().unwrap();

    let mut body = Vec::new();
    body.extend_from_slice(format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"resp.xlsx\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    ).as_bytes());
    body.extend_from_slice(&bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    let req = Request::post("/api/bloomberg/upload")
        .header("cookie", &cookie)
        .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(Body::from(body)).unwrap();
    let res = server.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let resp_body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();

    assert_eq!(resp_body["classified"], 0, "{resp_body}");
    assert_eq!(resp_body["classification_status"]["status"], "unavailable", "{resp_body}");
    assert_eq!(
        resp_body["classification_status"]["reason"], "not permitted: reference data",
        "{resp_body}"
    );

    pool.close().await;
    edb.stop().await;
}

/// Worklist item 2 (rates_h): a denied Reference read must not silently
/// collapse every bond to `missing: true` with no marker — that reads
/// identically to "these bonds genuinely lack coupon/maturity data" — nor
/// leave `total_dv01_eur`/`nav_sensitivity_100bp` reporting a confident zero
/// (computed on the remainder, which is empty precisely because Reference is
/// denied). Denied must stay distinguishable from genuinely-missing refs.
#[tokio::test]
async fn rates_marks_bonds_and_dv01_unavailable_when_reference_is_denied() {
    let (desktop, server, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    seed(&desktop, pid).await;

    // Positions + Nav granted, but NOT Reference.
    let cookie = user_with(&pool, &[
        Grant { domain: Domain::Positions, action: Action::View, portfolio: Some(pid) },
        Grant { domain: Domain::Nav, action: Action::View, portfolio: Some(pid) },
    ]).await;

    let (status, body) = get_json(&server, &format!("/api/portfolios/{pid}/metrics/rates"), Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["reference_status"]["status"], "unavailable", "{body}");
    assert_eq!(body["reference_status"]["reason"], "not permitted: reference data", "{body}");

    let bonds = body["bonds"].as_array().unwrap();
    assert!(!bonds.is_empty(), "{body}");
    for b in bonds {
        assert_eq!(b["missing"], true, "{b}");
        assert_eq!(b["status"], "unavailable", "bond {b} carries no denial marker");
        assert_eq!(b["reason"], "not permitted: reference data", "bond {b} carries no denial marker");
    }

    // The pass-adjacent computed values must not report a confident zero —
    // they must read `unavailable`/null, not "verified flat".
    assert!(body["total_dv01_eur"].is_null(), "{body}");
    assert!(body["nav_sensitivity_100bp"].is_null(), "{body}");

    pool.close().await;
    edb.stop().await;
}

/// Worklist item 2 (derivatives_h): a denied Reference read must not
/// silently misclassify every future as "Other" with no marker (`categories`
/// would otherwise read as a real "all other" breakdown), and a denied Nav
/// grant must not silently read as `aum: 0` — indistinguishable from a fund
/// that genuinely has no NAV row yet.
#[tokio::test]
async fn derivatives_marks_categories_and_aum_unavailable_when_reference_and_nav_are_denied() {
    let (desktop, server, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    seed(&desktop, pid).await;

    // Positions only — neither Reference nor Nav.
    let cookie = user_with(&pool, &[
        Grant { domain: Domain::Positions, action: Action::View, portfolio: Some(pid) },
    ]).await;

    let (status, body) = get_json(&server, &format!("/api/portfolios/{pid}/metrics/derivatives"), Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    assert_eq!(body["reference_status"]["status"], "unavailable", "{body}");
    assert_eq!(body["reference_status"]["reason"], "not permitted: reference data", "{body}");
    assert!(body["categories"].is_null(), "{body}");

    assert_eq!(body["nav_status"]["status"], "unavailable", "{body}");
    assert_eq!(body["nav_status"]["reason"], "not permitted: NAV history", "{body}");
    assert!(body["aum"].is_null(), "{body}");

    pool.close().await;
    edb.stop().await;
}
