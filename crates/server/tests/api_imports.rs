use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

const BOUNDARY: &str = "XBOUNDARYX";

fn multipart_body(bytes: &[u8], filename: &str) -> Body {
    let mut body = Vec::new();
    body.extend_from_slice(format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    ).as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    Body::from(body)
}

fn upload_req(bytes: &[u8], filename: &str) -> Request<Body> {
    Request::post("/api/portfolios/1/imports")
        .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(multipart_body(bytes, filename))
        .unwrap()
}

async fn test_app() -> (axum::Router, db::embedded::EmbeddedDb, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    (server::routes::router(server::state::AppState::desktop(pool)), edb, dir)
}

fn sample_bytes() -> Vec<u8> {
    std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx")).unwrap()
}

#[tokio::test]
async fn upload_then_read_back() {
    let (app, _edb, _dir) = test_app().await;

    let res = app.clone().oneshot(upload_req(&sample_bytes(), "sample.xlsx")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body[0]["outcome"]["positions"], 111, "{body}");
    assert_eq!(body[0]["outcome"]["duplicate"], false, "{body}");

    // duplicate upload
    let res = app.clone().oneshot(upload_req(&sample_bytes(), "sample.xlsx")).await.unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body[0]["outcome"]["duplicate"], true, "{body}");

    // garbage upload -> 200, per-file error reported instead of a request failure
    let res = app.clone().oneshot(upload_req(b"not an xlsx", "junk.xlsx")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(body[0]["error"].as_str().is_some(), "{body}");
    assert!(body[0]["outcome"].is_null(), "{body}");

    let res = app.clone().oneshot(Request::get("/api/portfolios/1/nav").body(Body::empty()).unwrap()).await.unwrap();
    let nav: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(nav.as_array().unwrap().len(), 344);

    let res = app.clone().oneshot(Request::get("/api/portfolios/1/positions").body(Body::empty()).unwrap()).await.unwrap();
    let pos: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(pos["date"], "2026-07-24");
    assert_eq!(pos["rows"].as_array().unwrap().len(), 111);

    let res = app.oneshot(Request::get("/api/portfolios/1/imports").body(Body::empty()).unwrap()).await.unwrap();
    let imports: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(imports.as_array().unwrap().len(), 1);
}
