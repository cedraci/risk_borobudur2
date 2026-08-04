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
    Request::post("/api/imports")
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

#[tokio::test]
async fn metrics_pipeline_on_sample() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    // empty state first
    let (st, body) = get_json(&app, "/api/metrics/summary").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["empty"], true);

    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx")).unwrap();
    let res = app.clone().oneshot(upload_req(&bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let (_, s) = get_json(&app, "/api/metrics/summary").await;
    assert_eq!(s["empty"], false);
    assert_eq!(s["as_of"], "2026-07-24");
    assert!((s["nav"].as_f64().unwrap() - 104.42).abs() < 1e-9);
    assert!(s["ytd"].as_f64().is_some());
    assert!(s["vol_1y"].as_f64().unwrap() > 0.0);
    assert!(s["max_drawdown"].as_f64().unwrap() <= 0.0);
    let var = &s["var_ucits"];
    assert_eq!(var["confidence"].as_f64().unwrap(), 0.99);
    assert!(var["historical"]["var"].as_f64().unwrap() > 0.0);
    assert!(var["gaussian"]["es"].as_f64().unwrap() >= var["gaussian"]["var"].as_f64().unwrap());

    let (_, r) = get_json(&app, "/api/metrics/rolling?window=60").await;
    assert_eq!(r["window"], 60);
    assert!(!r["vol"].as_array().unwrap().is_empty());
    assert_eq!(r["vol"].as_array().unwrap().len(), r["sharpe"].as_array().unwrap().len());

    let (_, dd) = get_json(&app, "/api/metrics/drawdowns").await;
    assert_eq!(dd["underwater"].as_array().unwrap().len(), 344);
    assert!(!dd["yearly"].as_array().unwrap().is_empty());
    assert!(dd["top_short"].as_array().unwrap().len() <= 5);

    let (_, cal) = get_json(&app, "/api/metrics/calendar").await;
    let monthly = cal["monthly"].as_array().unwrap();
    assert!(monthly.len() >= 17); // Feb 2025 .. Jul 2026
    assert_eq!(monthly[0]["year"], 2025);
    assert_eq!(monthly[0]["period"], 2);

    let (_, v) = get_json(&app, "/api/metrics/var?confidence=0.95&horizon=1&window=252").await;
    assert_eq!(v["confidence"].as_f64().unwrap(), 0.95);
    assert!(v["methods"]["historical"]["var"].as_f64().unwrap() > 0.0);
    assert!(!v["rolling"].as_array().unwrap().is_empty());

    // boundary: confidence == 0.5 is out of the strict (0.5, 1) range -> 400
    let (st, _) = get_json(&app, "/api/metrics/var?confidence=0.5&horizon=1&window=252").await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    pool.close().await;
    edb.stop().await;
}
