use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

const BOUNDARY: &str = "XBOUNDARYX";
const SAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx");
const CTD: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/ctd_sample.csv");

fn upload_req(uri: &str, name: &str, bytes: &[u8]) -> Request<Body> {
    let mut body = Vec::new();
    body.extend_from_slice(format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    ).as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    Request::post(uri)
        .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(Body::from(body))
        .unwrap()
}

async fn get_json(app: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let res = app.clone().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    (status, serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap())
}

async fn put_json(app: &axum::Router, uri: &str, payload: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let req = Request::builder().method(Method::PUT).uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap())).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    (status, serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap())
}

#[tokio::test]
async fn contracts_and_ctd_upload() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    // Uploading CTD before any NAV snapshot exists is rejected, with guidance.
    let ctd = std::fs::read(CTD).unwrap();
    let res = app.clone().oneshot(upload_req("/api/futures-analytics", "ctd.csv", &ctd)).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(body["detail"].as_str().unwrap().contains("NAV Recap"), "{body}");

    // Import the workbook: contracts are seeded unconfirmed.
    let wb = std::fs::read(SAMPLE).unwrap();
    let res = app.clone().oneshot(upload_req("/api/imports", "s.xlsx", &wb)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let (st, cs) = get_json(&app, "/api/futures-contracts").await;
    assert_eq!(st, StatusCode::OK);
    let cs = cs.as_array().unwrap();
    // sample.xlsx seeds one row per contract root: CF, KOA, NQ, OAT, RX, RY, TY, VG (8),
    // per crates/db/tests/futures_seeding.rs::import_seeds_futures_contracts_unconfirmed.
    assert_eq!(cs.len(), 8);
    assert!(cs.iter().all(|c| c["confirmed"] == false));

    // Confirm RX by hand.
    let (st, _) = put_json(&app, "/api/futures-contracts/RX", serde_json::json!({
        "label": "Euro-Bund", "category": "interest_rate", "point_value": 1000.0,
        "currency": "EUR", "curve": "DE-10y", "price_convention": "decimal", "confirmed": true,
    })).await;
    assert_eq!(st, StatusCode::OK);

    // Invalid category and point value are rejected.
    let (st, _) = put_json(&app, "/api/futures-contracts/RX", serde_json::json!({
        "label": "x", "category": "bogus", "point_value": 1000.0,
        "currency": "EUR", "curve": null, "price_convention": "decimal", "confirmed": true,
    })).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    let (st, _) = put_json(&app, "/api/futures-contracts/RX", serde_json::json!({
        "label": "x", "category": "interest_rate", "point_value": -1.0,
        "currency": "EUR", "curve": null, "price_convention": "decimal", "confirmed": true,
    })).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);

    // Now the CTD file is accepted.
    let res = app.clone().oneshot(upload_req("/api/futures-analytics", "ctd.csv", &ctd)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["rows"], 4);
    assert_eq!(body["nav_date"], "2026-07-24");

    let (_, rows) = get_json(&app, "/api/futures-analytics?date=2026-07-24").await;
    assert_eq!(rows.as_array().unwrap().len(), 4);

    // A ticker absent from that snapshot is a row error.
    let bad = "nav_date,ticker,ctd_isin,ctd_mod_duration,ctd_clean_price,ctd_accrued,conversion_factor\n\
               2026-07-24,ZZZ9 Comdty,DE0001102580,8.4,98.7,0.6,0.78\n";
    let res = app.clone().oneshot(upload_req("/api/futures-analytics", "bad.csv", bad.as_bytes())).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(body["rows"][0]["message"].as_str().unwrap().contains("ZZZ9"), "{body}");

    pool.close().await;
    edb.stop().await;
}
