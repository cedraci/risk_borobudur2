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
async fn limits_and_backtest_on_sample() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx")).unwrap();
    let res = app.clone().oneshot(upload_req(&bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // concentration
    let (st, c) = get_json(&app, "/api/portfolios/1/metrics/concentration").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(c["date"], "2026-07-24");
    let checks = c["checks"].as_array().unwrap();
    assert_eq!(checks.len(), 5);
    assert_eq!(checks[0]["check"], "issuer_10");
    // no equity is near 10% in the sample; the bond is 6.6% -> forty > 5% picks it up
    assert_eq!(checks[0]["status"], "ok");
    let forty = &checks[1];
    assert!(forty["rows"][0]["weight"].as_f64().unwrap() > 0.05);
    let fund = &checks[3];
    assert!(fund["rows"].as_array().unwrap().iter().any(|r| r["weight"].as_f64().unwrap() > 0.07)); // Eleva ~7.4%
    let dep = &checks[4];
    assert!(dep["rows"].as_array().unwrap().iter().any(|r| r["group"] == "CBLU"));

    // liquidity
    let (_, l) = get_json(&app, "/api/portfolios/1/metrics/liquidity").await;
    let buckets = l["buckets"].as_array().unwrap();
    assert_eq!(buckets.len(), 4);
    assert_eq!(buckets[0]["bucket"], "d1");
    assert!(buckets[0]["weight"].as_f64().unwrap() > 0.5); // equities dominate
    assert!(l["stress_status"] == "ok" || l["stress_status"] == "breach");
    let cum = l["cumulative"].as_array().unwrap();
    assert!(cum[3]["weight"].as_f64().unwrap() >= cum[0]["weight"].as_f64().unwrap());

    // rates: one bond with parsed statics
    let (_, r) = get_json(&app, "/api/portfolios/1/metrics/rates").await;
    let bonds = r["bonds"].as_array().unwrap();
    assert_eq!(bonds.len(), 1);
    assert_eq!(bonds[0]["missing"], false);
    assert!(bonds[0]["ytm"].as_f64().unwrap() > 0.0);
    let md = bonds[0]["mod_duration"].as_f64().unwrap();
    assert!(md > 4.0 && md < 9.0); // 2035 bullet
    assert!(r["total_dv01_eur"].as_f64().unwrap() > 0.0);
    // Futures now carry real DV01 rows rather than a text note; without CTD
    // analytics uploaded they are listed as missing.
    assert_eq!(r["futures"].as_array().unwrap().len(), 4);
    assert_eq!(r["futures_missing_any"], true);

    // backtest: 343 returns, window 252 -> 91 points
    let (_, b) = get_json(&app, "/api/portfolios/1/metrics/backtest").await;
    assert_eq!(b["confidence"].as_f64().unwrap(), 0.99);
    assert_eq!(b["horizon_days"], 1);
    assert_eq!(b["insufficient"], false);
    assert_eq!(b["n_points"], 91);
    let hist = &b["methods"]["historical"];
    assert!(hist["n"].as_u64().unwrap() == 91);
    let zone = hist["zone"].as_str().unwrap();
    assert!(["green", "yellow", "red"].contains(&zone));
    assert_eq!(b["series"].as_array().unwrap().len(), 91);

    // bad date -> 400
    let (st, _) = get_json(&app, "/api/portfolios/1/metrics/concentration?date=notadate").await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn rates_incomplete_bond_statics() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx")).unwrap();
    let res = app.clone().oneshot(upload_req(&bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Initially bond has complete statics and is not marked missing
    let (_, r) = get_json(&app, "/api/portfolios/1/metrics/rates").await;
    let bonds = r["bonds"].as_array().unwrap();
    assert_eq!(bonds.len(), 1);
    assert_eq!(bonds[0]["missing"], false);
    assert!(r["total_dv01_eur"].as_f64().unwrap() > 0.0);
    assert_eq!(r["missing_any"], false);

    // Null out bond statics via PUT /api/refs/{code}
    let (st, _) = put_json(&app, "/api/refs/US105756CL22", serde_json::json!({
        "issuer_group": null,
        "liquidity_days": null,
        "bond_coupon_pct": null,
        "bond_maturity": null,
        "bond_coupon_freq": null,
    })).await;
    assert_eq!(st, StatusCode::OK);

    // After nulling statics, bond should be marked missing
    let (_, r) = get_json(&app, "/api/portfolios/1/metrics/rates").await;
    let bonds = r["bonds"].as_array().unwrap();
    assert_eq!(bonds.len(), 1);
    assert_eq!(bonds[0]["missing"], true);
    assert_eq!(r["total_dv01_eur"].as_f64().unwrap(), 0.0);
    assert_eq!(r["missing_any"], true);

    // Restore bond statics
    let (st, _) = put_json(&app, "/api/refs/US105756CL22", serde_json::json!({
        "issuer_group": null,
        "liquidity_days": null,
        "bond_coupon_pct": 1.125,
        "bond_maturity": "2035-11-15",
        "bond_coupon_freq": 2,
    })).await;
    assert_eq!(st, StatusCode::OK);

    // After restoring, bond should no longer be marked missing
    let (_, r) = get_json(&app, "/api/portfolios/1/metrics/rates").await;
    let bonds = r["bonds"].as_array().unwrap();
    assert_eq!(bonds.len(), 1);
    assert_eq!(bonds[0]["missing"], false);
    assert!(r["total_dv01_eur"].as_f64().unwrap() > 0.0);
    assert_eq!(r["missing_any"], false);

    pool.close().await;
    edb.stop().await;
}
