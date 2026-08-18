use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

async fn app() -> (axum::Router, sqlx::PgPool, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    let pool = dbh.test_pool().clone();
    let app = server::routes::router(server::state::AppState::desktop(dbh.clone()));
    (app, pool, edb)
}

async fn req_json(app: &axum::Router, method: &str, uri: &str, body: Option<serde_json::Value>)
    -> (StatusCode, serde_json::Value)
{
    let b = match body {
        Some(v) => Request::builder().method(method).uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(v.to_string())).unwrap(),
        None => Request::builder().method(method).uri(uri).body(Body::empty()).unwrap(),
    };
    let res = app.clone().oneshot(b).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let v = if bytes.is_empty() { serde_json::Value::Null }
            else { serde_json::from_slice(&bytes).unwrap() };
    (status, v)
}

#[tokio::test]
async fn portfolio_crud_and_validation() {
    let (app, pool, edb) = app().await;

    // Migration seeds Borobudur as portfolio 1.
    let (st, list) = req_json(&app, "GET", "/api/portfolios", None).await;
    assert_eq!(st, StatusCode::OK);
    let list = list.as_array().unwrap().clone();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["id"], 1);
    assert_eq!(list[0]["name"], "Borobudur");
    assert_eq!(list[0]["kind"], "ucits");
    assert_eq!(list[0]["archived"], false);
    assert_eq!(list[0]["latest_nav_date"], serde_json::Value::Null);

    // Create a mandate.
    let (st, p) = req_json(&app, "POST", "/api/portfolios",
        Some(serde_json::json!({"name": "Mandat Alpha", "kind": "mandate"}))).await;
    assert_eq!(st, StatusCode::OK, "{p}");
    assert_eq!(p["id"], 2);
    assert_eq!(p["kind"], "mandate");

    // Validation: bad kind, empty name, duplicate name -> 422.
    for bad in [
        serde_json::json!({"name": "X", "kind": "hedge"}),
        serde_json::json!({"name": "   ", "kind": "ucits"}),
        serde_json::json!({"name": "Mandat Alpha", "kind": "mandate"}),
    ] {
        let (st, _) = req_json(&app, "POST", "/api/portfolios", Some(bad)).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    }

    // Rename + archive round-trip.
    let (st, p) = req_json(&app, "PUT", "/api/portfolios/2",
        Some(serde_json::json!({"name": "Mandat Beta", "archived": true}))).await;
    assert_eq!(st, StatusCode::OK, "{p}");
    assert_eq!(p["name"], "Mandat Beta");
    assert_eq!(p["archived"], true);

    // Unknown id -> 404; duplicate rename -> 422.
    let (st, _) = req_json(&app, "PUT", "/api/portfolios/99",
        Some(serde_json::json!({"name": "Z", "archived": false}))).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = req_json(&app, "PUT", "/api/portfolios/2",
        Some(serde_json::json!({"name": "Borobudur", "archived": false}))).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);

    pool.close().await;
    edb.stop().await;
}
