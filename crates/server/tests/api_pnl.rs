use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

const BOUNDARY: &str = "XBOUNDARYX";
const SAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx");

fn upload_req(bytes: &[u8]) -> Request<Body> {
    let mut body = Vec::new();
    body.extend_from_slice(format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"s.xlsx\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    ).as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    Request::post("/api/imports")
        .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(Body::from(body)).unwrap()
}

async fn get_json(app: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let res = app.clone().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    let body = serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    (status, body)
}

/// Fresh embedded database, seeded with the sample workbook, wired into a
/// router. Mirrors `app_with_sample` from the brief; there is no shared
/// `tests/common` harness in this crate (every other `api_*.rs` test file
/// inlines this same setup), so each test builds its own instance rather
/// than adding a new parallel harness module.
///
/// The sample workbook carries exactly one position snapshot date
/// (2026-07-24) even though its NAV/AUM history runs back to 2025-02-28
/// (`import_upsert_and_duplicate_semantics` in `crates/db/tests/import_workbook.rs`
/// pins this: `position_dates` is `vec![2026-07-24]` after import, no matter
/// how many times the same file is re-imported). A period P&L needs two
/// distinct position snapshots to compute a delta, so this clones the single
/// snapshot onto the earliest NAV history date, giving the handler a real
/// second endpoint to snap to (the resulting period P&L is economically
/// inert - same positions at both ends - but exercises the endpoint's period
/// resolution, grouping and reconciliation wiring, which is what these tests
/// check).
async fn app_with_sample() -> (axum::Router, sqlx::PgPool, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    let bytes = std::fs::read(SAMPLE).unwrap();
    assert_eq!(app.clone().oneshot(upload_req(&bytes)).await.unwrap().status(), StatusCode::OK);

    let earliest: chrono::NaiveDate = sqlx::query_scalar("SELECT MIN(date) FROM nav_history")
        .fetch_one(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO position_snapshots
             (nav_date, import_id, asset_type, isin, name, currency, quantity,
              avg_cost, price, valuation_ccy, accrued_interest, fx_rate, valuation_eur, weight, ticker)
         SELECT $1, import_id, asset_type, isin, name, currency, quantity,
                avg_cost, price, valuation_ccy, accrued_interest, fx_rate, valuation_eur, weight, ticker
         FROM position_snapshots WHERE nav_date = (SELECT MAX(nav_date) FROM position_snapshots)",
    )
    .bind(earliest)
    .execute(&pool)
    .await
    .unwrap();

    (app, pool, edb)
}

#[tokio::test]
async fn pnl_snaps_to_snapshot_dates_and_reports_which_it_used() {
    let (app, pool, edb) = app_with_sample().await;
    let (status, body) = get_json(&app, "/api/pnl?from=2020-01-01&to=2030-01-01").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["empty"], false);
    let p = &body["period"];
    assert!(p["actual_from"].is_string());
    assert!(p["actual_to"].is_string());
    assert!(p["snapshots"].as_i64().unwrap() >= 1);

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn reconciliation_residual_is_always_present() {
    let (app, pool, edb) = app_with_sample().await;
    let (_, body) = get_json(&app, "/api/pnl?from=2020-01-01&to=2030-01-01").await;
    let r = &body["reconciliation"];
    assert!(r["residual"].is_number(), "residual must always be returned");
    assert!(r["within_tolerance"].is_boolean());
    assert!(r["gross"].is_number());

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn groups_by_the_requested_dimension() {
    let (app, pool, edb) = app_with_sample().await;
    let (_, body) =
        get_json(&app, "/api/pnl?from=2020-01-01&to=2030-01-01&dimension=asset_class").await;
    let keys: Vec<String> = body["groups"].as_array().unwrap().iter()
        .map(|g| g["key"].as_str().unwrap().to_string()).collect();
    assert!(keys.iter().any(|k| k == "Equities"), "got {keys:?}");

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn an_unknown_dimension_is_a_bad_request() {
    let (app, pool, edb) = app_with_sample().await;
    let (status, _) = get_json(&app, "/api/pnl?from=2020-01-01&to=2030-01-01&dimension=zzz").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn group_totals_equal_the_sum_of_their_instruments() {
    let (app, pool, edb) = app_with_sample().await;
    let (_, body) =
        get_json(&app, "/api/pnl?from=2020-01-01&to=2030-01-01&dimension=currency").await;
    for g in body["groups"].as_array().unwrap() {
        let sum: f64 = g["instruments"].as_array().unwrap().iter()
            .map(|i| i["realized_price"].as_f64().unwrap() + i["unrealized_price"].as_f64().unwrap()
                   + i["realized_fx"].as_f64().unwrap() + i["unrealized_fx"].as_f64().unwrap())
            .sum();
        assert!((g["total"].as_f64().unwrap() - sum).abs() < 1e-6);
    }

    pool.close().await;
    edb.stop().await;
}
