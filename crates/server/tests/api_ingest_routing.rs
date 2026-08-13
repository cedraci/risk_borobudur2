use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

const BOUNDARY: &str = "XBOUNDARYX";
const HISINV: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/caceis_hisinv.csv");
const HISTOVL: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/caceis_histovl.csv");
const JOURSR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/caceis_joursr.csv");

fn multi_upload_req(uri: &str, files: &[(&str, &[u8])]) -> Request<Body> {
    let mut body = Vec::new();
    for (filename, bytes) in files {
        body.extend_from_slice(format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        ).as_bytes());
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    Request::post(uri)
        .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(Body::from(body)).unwrap()
}

async fn json_of(res: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

#[tokio::test]
async fn caceis_files_route_by_code_regardless_of_url_portfolio() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    let hisinv = std::fs::read(HISINV).unwrap();
    let histovl = std::fs::read(HISTOVL).unwrap();

    // Create a mandate and map the CACEIS fund code to it.
    let res = app.clone().oneshot(
        Request::post("/api/portfolios")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"Mandat CSV","kind":"mandate"}"#)).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let pid2 = json_of(res).await["id"].as_i64().unwrap();

    // Before mapping: upload reports an unknown code, writes nothing.
    let res = app.clone().oneshot(multi_upload_req(
        "/api/portfolios/1/imports",
        &[("HISINVLUX_165878_20260807_20260810130151.csv", &hisinv)],
    )).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_of(res).await;
    assert!(body[0]["error"].as_str().unwrap().contains("165878"), "{body}");
    assert!(body[0]["outcome"].is_null());

    // Map the code, re-upload BOTH files through portfolio 1's URL: they must
    // land in the mandate.
    let res = app.clone().oneshot(
        Request::put(format!("/api/portfolios/{pid2}/codes"))
            .header("content-type", "application/json")
            .body(Body::from(r#"[{"source":"caceis","code":"165878"}]"#)).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let res = app.clone().oneshot(multi_upload_req(
        "/api/portfolios/1/imports",
        &[
            ("HISINVLUX_165878_20260807_20260810130151.csv", &hisinv),
            ("HISTOVLLUX_165878_20260729_20260730170850.csv", &histovl),
        ],
    )).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_of(res).await;
    assert_eq!(body.as_array().unwrap().len(), 2);
    for item in body.as_array().unwrap() {
        assert_eq!(item["portfolio_id"].as_i64().unwrap(), pid2, "{item}");
        assert!(item["error"].is_null(), "{item}");
        assert!(item["outcome"]["import_id"].is_i64(), "{item}");
    }

    // The mandate has the snapshot and the NAV point; portfolio 1 has neither.
    let n2: i64 = sqlx::query_scalar("SELECT count(*) FROM position_snapshots WHERE portfolio_id = $1")
        .bind(pid2).fetch_one(&pool).await.unwrap();
    assert!(n2 > 0);
    let n1: i64 = sqlx::query_scalar("SELECT count(*) FROM position_snapshots WHERE portfolio_id = 1")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(n1, 0);
    let nav2: i64 = sqlx::query_scalar("SELECT count(*) FROM nav_history WHERE portfolio_id = $1")
        .bind(pid2).fetch_one(&pool).await.unwrap();
    assert_eq!(nav2, 1);

    // Dedupe is per portfolio: same file again -> duplicate outcome.
    let res = app.clone().oneshot(multi_upload_req(
        "/api/portfolios/1/imports",
        &[("HISINVLUX_165878_20260807_20260810130151.csv", &hisinv)],
    )).await.unwrap();
    let body = json_of(res).await;
    assert_eq!(body[0]["outcome"]["duplicate"], true, "{body}");

    // Archive the mandate: a routed file now fails per-file, request stays 200.
    let res = app.clone().oneshot(
        Request::put(format!("/api/portfolios/{pid2}"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"Mandat CSV","archived":true}"#)).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let res = app.clone().oneshot(multi_upload_req(
        "/api/portfolios/1/imports",
        &[("HISTOVLLUX_165878_20260729_20260730170850.csv", &histovl)],
    )).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_of(res).await;
    assert!(body[0]["error"].as_str().is_some(), "{body}");

    // Rejected families explain themselves.
    let res = app.clone().oneshot(multi_upload_req(
        "/api/portfolios/1/imports",
        &[("JOUROPLUX_165878_20260807_20260810130151.csv", b"x".as_slice())],
    )).await.unwrap();
    let body = json_of(res).await;
    assert!(body[0]["error"].as_str().unwrap().to_lowercase().contains("sample"), "{body}");

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn reglmtlux_is_recognized_and_declined_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    let two_lines = b"165878;20260807;line1\n165878;20260807;line2\n".as_slice();
    let res = app.clone().oneshot(multi_upload_req(
        "/api/portfolios/1/imports",
        &[("REGLMTLUX_165878_20260807_1.csv", two_lines)],
    )).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_of(res).await;
    let err = body[0]["error"].as_str().unwrap();
    assert!(err.contains("not consumed yet"), "{body}");
    assert!(err.contains("double-count"), "{body}");
    assert!(body[0]["outcome"].is_null());

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn joursrlux_routes_by_fund_code_like_hisinvlux() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    let joursr = std::fs::read(JOURSR).unwrap();

    // Create a mandate and map the CACEIS fund code to it, exactly as the
    // HISINVLUX routing test does.
    let res = app.clone().oneshot(
        Request::post("/api/portfolios")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"Mandat JOURSR","kind":"mandate"}"#)).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let pid2 = json_of(res).await["id"].as_i64().unwrap();

    let res = app.clone().oneshot(
        Request::put(format!("/api/portfolios/{pid2}/codes"))
            .header("content-type", "application/json")
            .body(Body::from(r#"[{"source":"caceis","code":"165878"}]"#)).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Uploaded through portfolio 1's URL, but the fund code routes it to the
    // mapped mandate.
    let res = app.clone().oneshot(multi_upload_req(
        "/api/portfolios/1/imports",
        &[("JOURSRLUX_165878_20260807_20260810130151.csv", &joursr)],
    )).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_of(res).await;
    assert!(body[0]["error"].is_null(), "{body}");
    assert_eq!(body[0]["portfolio_id"].as_i64().unwrap(), pid2, "{body}");
    assert!(body[0]["outcome"]["import_id"].is_i64(), "{body}");

    pool.close().await;
    edb.stop().await;
}
