use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

const BOUNDARY: &str = "XBOUNDARYX";

fn upload_req(bytes: &[u8]) -> Request<Body> {
    let mut body = Vec::new();
    body.extend_from_slice(format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"s.xlsx\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    ).as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    Request::post("/api/portfolios/1/imports")
        .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(Body::from(body))
        .unwrap()
}

async fn get_json(app: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let res = app.clone().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    let body = serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    (status, body)
}

async fn put_json(app: &axum::Router, uri: &str, payload: serde_json::Value) -> (StatusCode, serde_json::Value) {
    use axum::http::Method;
    let body_bytes = serde_json::to_vec(&payload).unwrap();
    let req = Request::builder()
        .method(Method::PUT)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body_bytes))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let body = serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    (status, body)
}

#[tokio::test]
async fn shareholder_register_crud_and_validation() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState::desktop(pool.clone()));

    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx")).unwrap();
    let res = app.clone().oneshot(upload_req(&bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let (s, _) = put_json(&app, "/api/portfolios/1/shareholders", serde_json::json!([
        {"label": "Founder family", "pct_of_nav": 18.0, "as_of": "2026-08-07"},
        {"label": "Pension fund A", "pct_of_nav": 12.5, "as_of": "2026-08-07"}
    ])).await;
    assert_eq!(s, StatusCode::OK);

    let (s, body) = get_json(&app, "/api/portfolios/1/shareholders").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 2);
    // Largest first: the top-five scenario reads straight off this order.
    assert_eq!(body[0]["label"], "Founder family");

    // A PUT replaces the register rather than appending to it.
    let (_, _) = put_json(&app, "/api/portfolios/1/shareholders", serde_json::json!([
        {"label": "Founder family", "pct_of_nav": 20.0, "as_of": "2026-08-10"}
    ])).await;
    let (_, body) = get_json(&app, "/api/portfolios/1/shareholders").await;
    assert_eq!(body.as_array().unwrap().len(), 1);

    for bad in [
        serde_json::json!([{"label": "X", "pct_of_nav": 0.0, "as_of": "2026-08-07"}]),
        serde_json::json!([{"label": "X", "pct_of_nav": 101.0, "as_of": "2026-08-07"}]),
        serde_json::json!([{"label": "  ", "pct_of_nav": 10.0, "as_of": "2026-08-07"}]),
        // A register summing past the whole fund is a typo, not a portfolio.
        serde_json::json!([
            {"label": "A", "pct_of_nav": 60.0, "as_of": "2026-08-07"},
            {"label": "B", "pct_of_nav": 60.0, "as_of": "2026-08-07"}
        ]),
    ] {
        let (s, _) = put_json(&app, "/api/portfolios/1/shareholders", bad).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    }

    // The rejected payloads left the stored register untouched.
    let (_, body) = get_json(&app, "/api/portfolios/1/shareholders").await;
    assert_eq!(body.as_array().unwrap().len(), 1);

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn liquidity_response_shape_and_scenarios() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState::desktop(pool.clone()));

    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx")).unwrap();
    let res = app.clone().oneshot(upload_req(&bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let (s, b) = get_json(&app, "/api/portfolios/1/metrics/liquidity").await;
    assert_eq!(s, StatusCode::OK);

    // Every displayed number is explained by an echoed parameter.
    for k in ["participation_rate", "adv_stress_factor", "liquidity_horizon_days",
              "settlement_deadline_days", "redemption_shock", "day_unit"] {
        assert!(!b["params"][k].is_null(), "params.{k} missing");
    }
    assert_eq!(b["params"]["day_unit"], "business days (Mon-Fri, no holiday calendar)");

    // Two asset profiles over the same four bands.
    for view in ["normal", "stressed"] {
        let buckets = b["asset"][view]["buckets"].as_array().unwrap();
        assert_eq!(buckets.len(), 4);
        assert_eq!(buckets[0]["bucket"], "d1");
        assert_eq!(b["asset"][view]["cumulative"].as_array().unwrap().len(), 4);
    }

    // Four scenarios, always present, always keyed.
    let keys: Vec<&str> = b["scenarios"].as_array().unwrap().iter()
        .map(|s| s["key"].as_str().unwrap()).collect();
    assert_eq!(keys, vec!["top5", "fixed", "hybrid_top5", "hybrid_fixed"]);

    // With no register loaded, the top-five scenarios are explicitly
    // unavailable — never a zero and never a pass.
    let top5 = &b["scenarios"][0];
    assert_eq!(top5["status"], "unavailable");
    assert_eq!(top5["reason"], "no shareholder register");
    assert!(top5["waterfall"].is_null());

    // The fixed scenario computes against the configured shock.
    let fixed = &b["scenarios"][1];
    assert!(fixed["required_eur"].as_f64().unwrap() > 0.0);
    assert!((fixed["required_pct"].as_f64().unwrap() - 0.30).abs() < 1e-9);
    assert!(matches!(fixed["status"].as_str().unwrap(), "ok" | "breach"));

    assert!(!b["coverage"]["adv_pct_of_nav"].is_null());
    assert!(b["coverage"]["fallbacks"].is_array());

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn a_loaded_register_drives_the_top_five_scenarios() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState::desktop(pool.clone()));

    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx")).unwrap();
    let res = app.clone().oneshot(upload_req(&bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    put_json(&app, "/api/portfolios/1/shareholders", serde_json::json!([
        {"label": "A", "pct_of_nav": 10.0, "as_of": "2026-08-07"},
        {"label": "B", "pct_of_nav": 8.0,  "as_of": "2026-08-07"},
        {"label": "C", "pct_of_nav": 6.0,  "as_of": "2026-08-07"},
        {"label": "D", "pct_of_nav": 4.0,  "as_of": "2026-08-07"},
        {"label": "E", "pct_of_nav": 2.0,  "as_of": "2026-08-07"},
        {"label": "F", "pct_of_nav": 1.0,  "as_of": "2026-08-07"}
    ])).await;

    let (_, b) = get_json(&app, "/api/portfolios/1/metrics/liquidity").await;
    let top5 = &b["scenarios"][0];
    assert_ne!(top5["status"], "unavailable");
    // The five largest only: 10 + 8 + 6 + 4 + 2 = 30%, not 31%.
    assert!((top5["required_pct"].as_f64().unwrap() - 0.30).abs() < 1e-9);
    assert_eq!(top5["register_count"], 5);

    // The hybrid runs the same requirement against stressed volumes, so it is
    // never faster than its unstressed twin.
    let hy = &b["scenarios"][2];
    let (a, h) = (top5["waterfall"]["days"].as_u64(), hy["waterfall"]["days"].as_u64());
    if let (Some(a), Some(h)) = (a, h) { assert!(h >= a); }

    // Slice is always the slower ordering.
    assert!(top5["slice_days"].as_f64().unwrap() >= top5["waterfall"]["days"].as_f64().unwrap_or(0.0));

    pool.close().await;
    edb.stop().await;
}

// `shareholders_put` is a mutating write and must be refused on an archived
// portfolio, same as settings/imports/CTD/codes puts. `shareholders_list`
// is a read and must stay available even when archived, so history remains
// inspectable — a "fix" that also locked down the read would pass a test
// that only checked the refusal, so both halves are asserted here.
#[tokio::test]
async fn shareholders_put_refused_but_read_allowed_on_archived_portfolio() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState::desktop(pool.clone()));

    // Archive portfolio 1 (seeded by migration as "Borobudur").
    let (st, _) = put_json(&app, "/api/portfolios/1",
        serde_json::json!({"name": "Borobudur", "archived": true})).await;
    assert_eq!(st, StatusCode::OK);

    // Mutating write refused: ensure() maps this to AppError::Conflict, 409.
    let (st, _) = put_json(&app, "/api/portfolios/1/shareholders", serde_json::json!([
        {"label": "Founder family", "pct_of_nav": 18.0, "as_of": "2026-08-07"}
    ])).await;
    assert_eq!(st, StatusCode::CONFLICT);

    // Read stays available on an archived portfolio.
    let (st, body) = get_json(&app, "/api/portfolios/1/shareholders").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 0);

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn flows_are_unavailable_until_enough_history_is_loaded() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState::desktop(pool.clone()));

    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx")).unwrap();
    let res = app.clone().oneshot(upload_req(&bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let (s, b) = get_json(&app, "/api/portfolios/1/flows").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["status"], "unavailable");
    assert_eq!(b["n_observations"], 0);
    // Never a percentage computed from too little history.
    assert!(b["worst"].is_null());
    assert!(b["reason"].as_str().unwrap().contains("observation"));

    pool.close().await;
    edb.stop().await;
}

fn flow_row(date: chrono::NaiveDate, class: &str, outstanding: Option<f64>, nav: Option<f64>, sub: f64, red: f64) -> ingest::ShareClassFlowRow {
    ingest::ShareClassFlowRow {
        flow_date: date,
        share_class: class.into(),
        outstanding_shares: outstanding,
        nav_per_share: nav,
        subscription_amount: sub,
        redemption_amount: red,
    }
}

// A date whose NAV has not yet been struck for one of its share classes
// (outstanding_shares/nav_per_share both None — the ingest layer's "not yet
// struck" state, distinct from a genuinely blank amount which defaults to
// 0.0) must be excluded from the fund-level series entirely: counting its
// full redemption against a fabricated (zero) NAV would inflate every
// window's worst-outflow percentage. The excluded date is reported via
// `dates_excluded_no_nav` rather than silently dropped.
#[tokio::test]
async fn flows_exclude_a_date_missing_nav_for_any_class_instead_of_zero_filling() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState::desktop(pool.clone()));

    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx")).unwrap();
    let res = app.clone().oneshot(upload_req(&bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let base = chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
    let mut rows = Vec::new();
    // 21 complete daily observations at a constant 100m NAV, all flat except
    // for a single, known 5m outflow on day 15 (a 0.05 worst 1-day ratio).
    for i in 0..21 {
        let date = base + chrono::Duration::days(i);
        let red = if i == 14 { 5_000_000.0 } else { 0.0 };
        rows.push(flow_row(date, "C1", Some(1_000_000.0), Some(100.0), 0.0, red));
    }
    // Day 10 (2026-01-10) additionally books a second share class whose NAV
    // has not been struck yet, alongside a much larger redemption. If this
    // date were zero-filled rather than excluded, it would dominate every
    // window it appears in.
    rows.push(flow_row(base + chrono::Duration::days(9), "C2", None, None, 0.0, 40_000_000.0));

    {
        let dbh = db::Db::from_pool(pool.clone());
        let ctx = db::auth::AuthCtx::desktop();
        let scoped = dbh.scope(&ctx);
        let a = scoped.authorize::<db::auth::marker::Shareholders, db::auth::marker::Import>(1).unwrap();
        scoped.flows_upsert(&a, &rows).await.unwrap();
    }

    let (s, b) = get_json(&app, "/api/portfolios/1/flows").await;
    assert_eq!(s, StatusCode::OK);
    // 21 days loaded, 1 excluded for missing NAV -> 20 complete observations,
    // exactly meeting the minimum, so flow_stats returns a result.
    assert_eq!(b["dates_excluded_no_nav"], 1);
    assert_eq!(b["n_observations"], 20);

    // The worst 1-day outflow reflects only the known 5m/100m = 0.05 event,
    // never the excluded date's fabricated 40m/0 ratio.
    let w1 = b["worst"].as_array().unwrap().iter()
        .find(|w| w["window"] == 1).unwrap();
    assert!((w1["pct_of_nav"].as_f64().unwrap() - 0.05).abs() < 1e-9,
        "worst outflow inflated by the excluded date: {w1}");

    pool.close().await;
    edb.stop().await;
}
