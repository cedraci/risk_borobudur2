use axum::body::Body;
use axum::http::{Request, StatusCode};
use calamine::{DataType, Reader};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

const BOUNDARY: &str = "XBOUNDARYX";
const SAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx");

fn upload_req(uri: &str, filename: &str, bytes: &[u8]) -> Request<Body> {
    let mut body = Vec::new();
    body.extend_from_slice(format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    ).as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    Request::post(uri)
        .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(Body::from(body)).unwrap()
}

async fn get_json(app: &axum::Router, uri: &str) -> serde_json::Value {
    let res = app.clone().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
    serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

async fn get_bytes(app: &axum::Router, uri: &str) -> (StatusCode, String, Vec<u8>) {
    let res = app.clone().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    let ctype = res.headers().get(axum::http::header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_string()).unwrap_or_default();
    let bytes = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, ctype, bytes)
}

async fn post_multipart_json(app: &axum::Router, uri: &str, filename: &str, bytes: &[u8]) -> serde_json::Value {
    let res = app.clone().oneshot(upload_req(uri, filename, bytes)).await.unwrap();
    serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

/// Fresh embedded database, seeded with the sample workbook and a second
/// position snapshot cloned onto the earliest NAV history date, wired into a
/// router. Mirrors `app_with_sample` in `api_pnl.rs`: there is no shared
/// `tests/common` harness in this crate (every `api_*.rs` file inlines this
/// same setup), so this file builds its own instance rather than adding a
/// new parallel harness module. The second snapshot date is needed so
/// `/api/pnl` (Task 10) has two dates to strike a period against, which the
/// upload test below exercises.
async fn app_with_sample() -> (axum::Router, sqlx::PgPool, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    let bytes = std::fs::read(SAMPLE).unwrap();
    assert_eq!(app.clone().oneshot(upload_req("/api/portfolios/1/imports", "s.xlsx", &bytes)).await.unwrap().status(), StatusCode::OK);

    let earliest: chrono::NaiveDate = sqlx::query_scalar("SELECT MIN(date) FROM nav_history")
        .fetch_one(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO position_snapshots
             (portfolio_id, nav_date, import_id, asset_type, isin, name, currency, quantity,
              avg_cost, price, valuation_ccy, accrued_interest, fx_rate, valuation_eur, weight, ticker)
         SELECT portfolio_id, $1, import_id, asset_type, isin, name, currency, quantity,
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
async fn request_endpoint_returns_a_readable_workbook() {
    let (app, pool, edb) = app_with_sample().await;

    let (status, ctype, bytes) = get_bytes(&app, "/api/bloomberg/request").await;
    assert_eq!(status, 200);
    assert!(ctype.contains("spreadsheet"), "got {ctype}");
    let wb: calamine::Xlsx<_> = calamine::Xlsx::new(std::io::Cursor::new(bytes)).expect("valid xlsx");
    assert!(calamine::Reader::sheet_names(&wb).iter().any(|n| n == "REFS"));

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn bond_with_country_but_no_sector_is_not_re_requested() {
    let (app, pool, edb) = app_with_sample().await;

    // Bloomberg never returns a GICS sector for Corp/Govt securities, so a
    // bond is as classified as it can get once its country is known. An
    // equity in the same state is still missing real data and must stay in
    // the request list.
    sqlx::query("UPDATE instrument_refs SET country_of_risk = 'BR' WHERE code = 'US105756CL22'")
        .execute(&pool).await.unwrap();
    sqlx::query("UPDATE instrument_refs SET country_of_risk = 'FR' WHERE code = 'FR0000121014'")
        .execute(&pool).await.unwrap();

    let (status, _, bytes) = get_bytes(&app, "/api/bloomberg/request").await;
    assert_eq!(status, 200);
    let mut wb: calamine::Xlsx<_> = calamine::Xlsx::new(std::io::Cursor::new(bytes)).unwrap();
    let range = wb.worksheet_range("REFS").unwrap();
    let isins: Vec<String> = range.rows().skip(1)
        .filter_map(|r| r.first().and_then(|c| c.get_string()).map(str::to_string))
        .collect();
    assert!(!isins.iter().any(|i| i == "US105756CL22"), "bond re-requested: {isins:?}");
    assert!(isins.iter().any(|i| i == "FR0000121014"), "equity dropped: {isins:?}");

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn request_unions_unclassified_across_portfolios() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });
    let bytes = std::fs::read(SAMPLE).unwrap();

    async fn create_mandate(app: &axum::Router, name: &str) -> i64 {
        let res = app.clone().oneshot(
            Request::post("/api/portfolios")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::json!({"name": name, "kind": "mandate"}).to_string()))
                .unwrap(),
        ).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
        body["id"].as_i64().unwrap()
    }

    // Portfolio 1 stays empty for the whole test. Portfolio 2 gets the
    // sample workbook — request must still union its unclassified
    // instruments, not just portfolio 1's (empty) latest snapshot.
    let pid2 = create_mandate(&app, "Mandat Alpha").await;
    assert_eq!(pid2, 2);
    let res = app.clone().oneshot(upload_req(&format!("/api/portfolios/{pid2}/imports"), "s.xlsx", &bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let (status, _, resp_bytes) = get_bytes(&app, "/api/bloomberg/request").await;
    assert_eq!(status, 200);
    let mut wb: calamine::Xlsx<_> = calamine::Xlsx::new(std::io::Cursor::new(resp_bytes)).unwrap();
    let range = wb.worksheet_range("REFS").unwrap();
    let isins: Vec<String> = range.rows().skip(1)
        .filter_map(|r| r.first().and_then(|c| c.get_string()).map(str::to_string))
        .collect();
    assert!(isins.iter().any(|i| i == "FR0000121014"),
        "sample instrument from portfolio 2 missing from the fleet-wide union: {isins:?}");

    // Portfolio 3, archived: excluded from the walk by construction, and a
    // mutating request (import) against it is refused outright.
    let pid3 = create_mandate(&app, "Mandat Beta").await;
    assert_eq!(pid3, 3);
    let res = app.clone().oneshot(
        Request::put(format!("/api/portfolios/{pid3}"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::json!({"name": "Mandat Beta", "archived": true}).to_string()))
            .unwrap(),
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let res = app.clone().oneshot(upload_req(&format!("/api/portfolios/{pid3}/imports"), "s.xlsx", &bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);

    // Classify one ISIN directly (as bond_with_country_but_no_sector... does
    // for its bond) and confirm it drops out of a fresh request: refs are
    // shared/global, so the classification serves every portfolio.
    sqlx::query("UPDATE instrument_refs SET country_of_risk = 'US' WHERE code = 'US105756CL22'")
        .execute(&pool).await.unwrap();

    let (status, _, resp_bytes2) = get_bytes(&app, "/api/bloomberg/request").await;
    assert_eq!(status, 200);
    let mut wb2: calamine::Xlsx<_> = calamine::Xlsx::new(std::io::Cursor::new(resp_bytes2)).unwrap();
    let range2 = wb2.worksheet_range("REFS").unwrap();
    let isins2: Vec<String> = range2.rows().skip(1)
        .filter_map(|r| r.first().and_then(|c| c.get_string()).map(str::to_string))
        .collect();
    assert!(!isins2.iter().any(|i| i == "US105756CL22"), "classified bond re-requested: {isins2:?}");

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn upload_stores_classifications_and_reports_unresolved_cells() {
    let (app, pool, edb) = app_with_sample().await;

    let mut wb = rust_xlsxwriter::Workbook::new();
    let s = wb.add_worksheet().set_name("REFS").unwrap();
    for (c, h) in ["isin", "ticker", "country_of_risk", "gics_sector", "gics_industry"].iter().enumerate() {
        s.write_string(0, c as u16, *h).unwrap();
    }
    s.write_string(1, 0, "FR0000121014").unwrap();
    s.write_string(1, 1, "MC FP Equity").unwrap();
    s.write_string(1, 2, "France").unwrap();
    s.write_string(1, 3, "Consumer Discretionary").unwrap();
    s.write_string(1, 4, "#N/A").unwrap();
    let bytes = wb.save_to_buffer().unwrap();

    let body = post_multipart_json(&app, "/api/bloomberg/upload", "resp.xlsx", &bytes).await;
    assert_eq!(body["classified"], 1, "{body}");
    assert!(body["skipped"].as_array().unwrap().iter()
        .any(|e| e["message"].as_str().unwrap().contains("gics_industry")), "{body}");

    // The stored value must now appear in the P&L grouping.
    let pnl = get_json(&app, "/api/portfolios/1/pnl?from=2020-01-01&to=2030-01-01&dimension=sector").await;
    let keys: Vec<String> = pnl["groups"].as_array().unwrap().iter()
        .map(|g| g["key"].as_str().unwrap().to_string()).collect();
    assert!(keys.iter().any(|k| k == "Consumer Discretionary"), "got {keys:?}");

    pool.close().await;
    edb.stop().await;
}
