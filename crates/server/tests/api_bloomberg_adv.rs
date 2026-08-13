use axum::body::Body;
use axum::http::{Request, StatusCode};
use calamine::{DataType, Reader};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

const BOUNDARY: &str = "XBOUNDARYX";
const HISINV: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/caceis_hisinv.csv");

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

async fn get_json(app: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let res = app.clone().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    (status, body)
}

async fn post_multipart_json(app: &axum::Router, uri: &str, filename: &str, bytes: &[u8]) -> serde_json::Value {
    let res = app.clone().oneshot(upload_req(uri, filename, bytes)).await.unwrap();
    serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

/// Read the ADV sheet's column A (isin) from a generated request workbook,
/// mirroring the pattern `crates/ingest/tests/bloomberg.rs` already uses for
/// the REFS sheet.
fn adv_sheet_isins(bytes: &[u8]) -> Vec<String> {
    let mut wb: calamine::Xlsx<_> =
        calamine::Xlsx::new(std::io::Cursor::new(bytes.to_vec())).expect("valid xlsx");
    let range = wb.worksheet_range("ADV").expect("ADV sheet present");
    range.rows().skip(1)
        .filter_map(|r| r.first().and_then(|c| c.get_string()).map(str::to_string))
        .collect()
}

/// Fresh embedded database with the CACEIS HISINVLUX fixture imported into a
/// mapped mandate, so `instrument_refs.market_place` is populated (Task 6)
/// for the venue rule `adv_scope`/`analytics::adv_eligible` relies on.
/// CACEIS files route by fund code regardless of the URL portfolio (Task 4/5
/// multi-file routing), so the mandate must exist and be code-mapped first —
/// mirrors the setup in `api_ingest_routing.rs`.
async fn app_with_hisinv() -> (axum::Router, sqlx::PgPool, db::embedded::EmbeddedDb, i64) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    let res = app.clone().oneshot(
        Request::post("/api/portfolios")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"Mandat ADV","kind":"mandate"}"#)).unwrap(),
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let pid = body["id"].as_i64().unwrap();

    let res = app.clone().oneshot(
        Request::put(format!("/api/portfolios/{pid}/codes"))
            .header("content-type", "application/json")
            .body(Body::from(r#"[{"source":"caceis","code":"165878"}]"#)).unwrap(),
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let hisinv = std::fs::read(HISINV).unwrap();
    let res = app.clone().oneshot(upload_req(
        "/api/portfolios/1/imports",
        "HISINVLUX_165878_20260807_20260810130151.csv",
        &hisinv,
    )).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(body[0]["error"].is_null(), "hisinv import failed: {body}");

    (app, pool, edb, pid)
}

#[tokio::test]
async fn adv_request_is_scoped_to_listed_instruments_that_are_due() {
    let (app, pool, edb, _pid) = app_with_hisinv().await;

    let (status, b) = get_json(&app, "/api/bloomberg/adv-due").await;
    assert_eq!(status, StatusCode::OK);
    let due = b["due"].as_u64().unwrap();
    let held = b["held"].as_u64().unwrap();
    assert!(due > 0 && due <= held, "you see the cost before you pay it: {due} of {held}");

    let res = app.clone().oneshot(
        Request::get("/api/bloomberg/adv-request").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let isins = adv_sheet_isins(&bytes);

    // The listed ETF is in; the unlisted target fund, the futures and the
    // cash accounts are not.
    assert!(isins.iter().any(|i| i == "AT000000STR1"), "a listed equity is requested: {isins:?}");
    assert!(!isins.iter().any(|i| i == "FR0010599399"), "an unlisted target fund is not: {isins:?}");
    // The gold ETC: venue 046 (Mercato Continuo), classified Obligation by
    // the depositary's asset-type rule. The single strongest test of the
    // venue rule — the old asset_type == "Action" rule would have excluded
    // it, and the venue rule exists precisely to admit exchange-traded
    // instruments whose asset type is not Action.
    assert!(isins.iter().any(|i| i == "DE000A1EK0G3"),
        "the gold ETC, listed but not an Action, is still requested: {isins:?}");
    assert!(!isins.iter().any(|i| i.starts_with("FVS")), "futures are never requested: {isins:?}");

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn a_fresh_adv_drops_out_until_it_goes_stale() {
    let (app, pool, edb, _pid) = app_with_hisinv().await;

    let (_, before) = get_json(&app, "/api/bloomberg/adv-due").await;
    let due_before = before["due"].as_u64().unwrap();
    let held_before = before["held"].as_u64().unwrap();
    assert!(due_before > 0, "nothing due to begin with: {before}");

    // Upload an ADV response resolving one instrument's volume — the upload
    // path accepts a workbook carrying only an ADV sheet.
    let mut wb = rust_xlsxwriter::Workbook::new();
    let s = wb.add_worksheet().set_name("ADV").unwrap();
    for (c, h) in ["isin", "adv_30d", "market_sector"].iter().enumerate() {
        s.write_string(0, c as u16, *h).unwrap();
    }
    s.write_string(1, 0, "AT000000STR1").unwrap();
    s.write_number(1, 1, 125_000.0).unwrap();
    s.write_string(1, 2, "Equity").unwrap();
    let bytes = wb.save_to_buffer().unwrap();

    let body = post_multipart_json(&app, "/api/bloomberg/upload", "adv_resp.xlsx", &bytes).await;
    assert_eq!(body["adv_rows"], 1, "{body}");

    let (_, after) = get_json(&app, "/api/bloomberg/adv-due").await;
    let due_after = after["due"].as_u64().unwrap();
    let held_after = after["held"].as_u64().unwrap();
    assert_eq!(due_after, due_before - 1,
        "a freshly-fetched instrument must drop out of the due count: before {before} after {after}");
    assert_eq!(held_after, held_before,
        "the held set (everything eligible) is unaffected by staleness: before {before} after {after}");

    // ?all=true still includes it: it stays held, only staleness changed.
    let res = app.clone().oneshot(
        Request::get("/api/bloomberg/adv-request?all=true").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let resp_bytes = res.into_body().collect().await.unwrap().to_bytes();
    let isins = adv_sheet_isins(&resp_bytes);
    assert!(isins.iter().any(|i| i == "AT000000STR1"),
        "?all=true serves the full held set regardless of staleness: {isins:?}");

    pool.close().await;
    edb.stop().await;
}
