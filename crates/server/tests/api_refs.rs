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

async fn put_json(app: &axum::Router, uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let res = app.clone().oneshot(
        Request::put(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    ).await.unwrap();
    let status = res.status();
    let body = serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    (status, body)
}

fn upload_req_to(uri: &str, bytes: &[u8]) -> Request<Body> {
    let mut body = Vec::new();
    body.extend_from_slice(format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"s.xlsx\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    ).as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    Request::post(uri)
        .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn refs_list_unions_instruments_held_only_by_another_portfolio() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    // Portfolio 1 stays empty for the whole test. The sample is imported
    // into portfolio 2 only — the editor context must still show its
    // instruments, since it walks every non-archived portfolio's latest
    // snapshot, not just portfolio 1's.
    let res = app.clone().oneshot(
        Request::post("/api/portfolios")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::json!({"name": "Mandat Alpha", "kind": "mandate"}).to_string()))
            .unwrap(),
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let created: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let pid2 = created["id"].as_i64().unwrap();
    assert_eq!(pid2, 2);

    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx")).unwrap();
    let res = app.clone().oneshot(upload_req_to(&format!("/api/portfolios/{pid2}/imports"), &bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let (st, rows) = get_json(&app, "/api/refs").await;
    assert_eq!(st, StatusCode::OK);
    let rows = rows.as_array().unwrap();
    assert!(rows.iter().any(|r| r["code"] == "FR0000121014"),
        "instrument held only by portfolio 2 missing from the fleet-wide union: {rows:?}");

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn refs_editor_flow() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    // empty DB -> empty list
    let (st, body) = get_json(&app, "/api/refs").await;
    assert_eq!(st, StatusCode::OK);
    assert!(body.as_array().unwrap().is_empty());

    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx")).unwrap();
    let res = app.clone().oneshot(upload_req(&bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let (_, rows) = get_json(&app, "/api/refs").await;
    let rows = rows.as_array().unwrap().clone();
    assert!(rows.len() >= 100); // 111 positions minus duplicate codes
    let bond = rows.iter().find(|r| r["code"] == "US105756CL22").unwrap();
    assert_eq!(bond["is_bond"], true);
    assert_eq!(bond["bond_coupon_pct"].as_f64().unwrap(), 6.625);
    assert_eq!(bond["bond_maturity"], "2035-03-15");
    assert_eq!(bond["bond_coupon_freq"], 2);
    assert_eq!(bond["effective_days"], 30.0); // Obligation default
    let cash = rows.iter().find(|r| r["asset_type"] == "Cash Acc").unwrap();
    assert_eq!(cash["effective_issuer_group"], "CBLU");

    // set an issuer-group override on a fund code
    let helium = rows.iter().find(|r| r["code"] == "LU1112771255").unwrap();
    assert_eq!(helium["issuer_group_override"], serde_json::Value::Null);
    let (st, _) = put_json(&app, "/api/refs/LU1112771255", serde_json::json!({
        "issuer_group": "HELIUM GROUP", "liquidity_days": 30,
        "bond_coupon_pct": null, "bond_maturity": null, "bond_coupon_freq": null
    })).await;
    assert_eq!(st, StatusCode::OK);
    let (_, rows2) = get_json(&app, "/api/refs").await;
    let helium2 = rows2.as_array().unwrap().iter().find(|r| r["code"] == "LU1112771255").unwrap();
    assert_eq!(helium2["effective_issuer_group"], "HELIUM GROUP");
    assert_eq!(helium2["effective_days"], 30.0);

    // revert with nulls
    let (st, _) = put_json(&app, "/api/refs/LU1112771255", serde_json::json!({
        "issuer_group": null, "liquidity_days": null,
        "bond_coupon_pct": null, "bond_maturity": null, "bond_coupon_freq": null
    })).await;
    assert_eq!(st, StatusCode::OK);
    let (_, rows3) = get_json(&app, "/api/refs").await;
    let helium3 = rows3.as_array().unwrap().iter().find(|r| r["code"] == "LU1112771255").unwrap();
    assert_eq!(helium3["issuer_group_override"], serde_json::Value::Null);
    assert_eq!(helium3["effective_days"], 7.0); // back to Fonds default

    // invalid liquidity_days -> 422
    let (st, err) = put_json(&app, "/api/refs/LU1112771255", serde_json::json!({
        "issuer_group": null, "liquidity_days": -5,
        "bond_coupon_pct": null, "bond_maturity": null, "bond_coupon_freq": null
    })).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(err["status"], 422);

    // settings validation: bad redemption_shock -> 400
    let (_, mut s) = get_json(&app, "/api/portfolios/1/settings").await;
    s["redemption_shock"] = serde_json::json!(1.5);
    let (st, _) = put_json(&app, "/api/portfolios/1/settings", s).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    pool.close().await;
    edb.stop().await;
}
