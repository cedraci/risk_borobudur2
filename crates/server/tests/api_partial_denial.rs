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
    let admin = db::admin::Admin::new(&pool);
    // Look the user id back up via a fresh grant on `other`.
    let cookie2 = {
        let hash = server::auth::local::hash_password("pw").unwrap();
        let id = admin.create_user("second@f.lu", "U", &hash, false).await.unwrap();
        admin.grant_add(id, Grant { domain: Domain::Nav, action: Action::View, portfolio: Some(pid) }, None).await.unwrap();
        admin.grant_add(id, Grant { domain: Domain::Positions, action: Action::View, portfolio: Some(other) }, None).await.unwrap();
        let token = "second-token";
        admin.session_create(&server::auth::local::token_hash(token), id, 1).await.unwrap();
        format!("borobudur_session={token}")
    };
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
    assert!(body["checks"].as_array().is_some(), "{body}");
    assert_eq!(body["issuer_overrides"]["status"], "unavailable", "{body}");
    assert_eq!(body["issuer_overrides"]["reason"], "not permitted: reference data", "{body}");

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

    // Export refuses outright rather than emitting an evidence document
    // whose verdicts were computed on a denied read.
    let cookie_export = user_with(&pool, &[
        Grant { domain: Domain::Positions, action: Action::View, portfolio: Some(pid) },
        Grant { domain: Domain::Positions, action: Action::Export, portfolio: Some(pid) },
    ]).await;
    let status = get(&server, &format!("/api/portfolios/{pid}/emir/export"), Some(&cookie_export)).await;
    assert_ne!(status, StatusCode::OK, "export must refuse when its verdicts are built on a denied Reference read");

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

    pool.close().await;
    edb.stop().await;
}
