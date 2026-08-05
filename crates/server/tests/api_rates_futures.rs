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
        .body(Body::from(body)).unwrap()
}

async fn get_json(app: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let res = app.clone().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    (status, serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap())
}

async fn put_json(app: &axum::Router, uri: &str, payload: serde_json::Value) -> StatusCode {
    let req = Request::builder().method(Method::PUT).uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap())).unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

fn spec(cat: &str, pv: f64, ccy: &str, conv: &str) -> serde_json::Value {
    serde_json::json!({
        "label": "x", "category": cat, "point_value": pv, "currency": ccy,
        "curve": null, "price_convention": conv, "confirmed": true,
    })
}

#[tokio::test]
async fn rates_includes_bond_futures_when_ctd_present() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    let wb = std::fs::read(SAMPLE).unwrap();
    assert_eq!(app.clone().oneshot(upload_req("/api/imports", "s.xlsx", &wb)).await.unwrap().status(), StatusCode::OK);

    // Baseline: the cash bond only. Capture it so the restatement can be checked.
    let (_, r0) = get_json(&app, "/api/metrics/rates").await;
    let bond_dv01 = r0["bonds"][0]["dv01_eur"].as_f64().unwrap();
    let total0 = r0["total_dv01_eur"].as_f64().unwrap();
    assert!((total0 - bond_dv01).abs() < 1e-9, "no futures yet");
    assert!(r0["futures"].as_array().unwrap().len() == 4, "four bond futures listed");
    assert!(r0["futures"].as_array().unwrap().iter().all(|f| f["missing"] == true),
            "no CTD analytics uploaded yet");
    assert_eq!(r0["futures_missing_any"], true);

    // The restatement is self-consistent: 100bp sensitivity is 100 x DV01 / AUM.
    let aum = 28_332_753.49f64;
    assert!((r0["nav_sensitivity_100bp"].as_f64().unwrap() - 100.0 * total0 / aum).abs() < 1e-12);

    // Confirm the four bond-future specs, then upload CTD analytics.
    for (root, ccy, conv) in [
        ("RX", "EUR", "decimal"), ("OAT", "EUR", "decimal"),
        ("KOA", "EUR", "decimal"), ("TY", "USD", "th32"),
    ] {
        assert_eq!(put_json(&app, &format!("/api/futures-contracts/{root}"),
                            spec("interest_rate", 1000.0, ccy, conv)).await, StatusCode::OK);
    }
    let ctd = std::fs::read(CTD).unwrap();
    assert_eq!(app.clone().oneshot(upload_req("/api/futures-analytics", "ctd.csv", &ctd)).await.unwrap().status(),
               StatusCode::OK);

    let (_, r) = get_json(&app, "/api/metrics/rates").await;
    let futs = r["futures"].as_array().unwrap();
    assert_eq!(futs.len(), 4);
    assert!(futs.iter().all(|f| f["missing"] == false));
    assert_eq!(r["futures_missing_any"], false);

    // The bond's own DV01 is untouched by adding the futures block: same
    // figure before and after the CTD upload.
    assert!((r["bonds"][0]["dv01_eur"].as_f64().unwrap() - bond_dv01).abs() < 1e-9,
            "bond figures must not move");

    // RX: 8.41 * (98.72 + 0.63) * 1000 * 1e-4 / 0.782145 = 106.8259 per contract,
    // held -8, fx 1.0.
    let rx = futs.iter().find(|f| f["ticker"] == "RXU6 Comdty").unwrap();
    assert!((rx["dv01_eur"].as_f64().unwrap() - -854.607).abs() < 1e-2, "{rx}");
    assert!(rx["dv01_eur"].as_f64().unwrap() < 0.0, "a short is negative DV01");

    // Totals move by exactly the futures' contribution.
    let total = r["total_dv01_eur"].as_f64().unwrap();
    let fut_sum: f64 = futs.iter().map(|f| f["dv01_eur"].as_f64().unwrap()).sum();
    assert!((total - (bond_dv01 + fut_sum)).abs() < 1e-6);
    assert!((r["nav_sensitivity_100bp"].as_f64().unwrap() - 100.0 * total / aum).abs() < 1e-12);
    assert!(total < 0.0, "the book is net short rates once futures are counted");

    pool.close().await;
    edb.stop().await;
}
