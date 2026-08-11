use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

async fn test_app() -> (axum::Router, db::embedded::EmbeddedDb, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    (server::routes::router(server::state::AppState { pool }), edb, dir)
}

#[tokio::test]
async fn settings_get_put_and_validation() {
    let (app, _edb, _dir) = test_app().await;

    let res = app.clone().oneshot(Request::get("/api/portfolios/1/settings").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["var_horizon_days"], 20);

    let mut s = body.clone();
    s["risk_free_rate"] = serde_json::json!(0.025);
    let res = app.clone().oneshot(
        Request::put("/api/portfolios/1/settings").header("content-type", "application/json")
            .body(Body::from(s.to_string())).unwrap(),
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let mut bad = body.clone();
    bad["var_confidence"] = serde_json::json!(1.5);
    let res = app.clone().oneshot(
        Request::put("/api/portfolios/1/settings").header("content-type", "application/json")
            .body(Body::from(bad.to_string())).unwrap(),
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let mut boundary = body.clone();
    boundary["var_confidence"] = serde_json::json!(0.5);
    let res = app.clone().oneshot(
        Request::put("/api/portfolios/1/settings").header("content-type", "application/json")
            .body(Body::from(boundary.to_string())).unwrap(),
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let res = app.oneshot(Request::get("/api/health").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn unknown_api_route_returns_404_not_spa_html() {
    let (app, _edb, _dir) = test_app().await;

    // Unknown routes under /api/ must never fall back to the SPA's index.html,
    // regardless of whether the embedded frontend assets are populated in
    // this test build.
    let res = app
        .oneshot(Request::get("/api/definitely-not-a-route").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["status"], 404);
    assert_eq!(body["title"], "Not Found");
}
