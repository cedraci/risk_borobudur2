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
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

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
