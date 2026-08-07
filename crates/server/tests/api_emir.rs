use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use calamine::Reader;
use http_body_util::BodyExt;
use tower::util::ServiceExt;

const BOUNDARY: &str = "XBOUNDARYX";
const SAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx");

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

async fn get_bytes(app: &axum::Router, uri: &str) -> (StatusCode, String, Vec<u8>) {
    let res = app.clone().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    let ctype = res.headers().get(axum::http::header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_string()).unwrap_or_default();
    let bytes = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, ctype, bytes)
}

/// Fresh embedded database seeded with the sample workbook through the HTTP
/// API, wired into a router. Mirrors `app_with_sample` in `api_pnl.rs`; there
/// is no shared tests/common harness in this crate, so this file inlines its
/// own instance. sample.xlsx has exactly one snapshot date (2026-07-24) with
/// 8 futures positions and 9 Margin Acc rows; import seeds 8 unconfirmed
/// contract roots.
async fn app_with_sample() -> (axum::Router, sqlx::PgPool, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });
    let bytes = std::fs::read(SAMPLE).unwrap();
    assert_eq!(
        app.clone().oneshot(upload_req("/api/imports", "s.xlsx", &bytes)).await.unwrap().status(),
        StatusCode::OK
    );
    (app, pool, edb)
}

#[tokio::test]
async fn emir_empty_before_any_import() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });
    let (status, body) = get_json(&app, "/api/emir").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["empty"], true, "{body}");
    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn emir_report_on_sample() {
    let (app, pool, edb) = app_with_sample().await;

    let (status, body) = get_json(&app, "/api/emir").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["date"], "2026-07-24", "{body}");
    assert_eq!(body["months_total"], 12);
    assert_eq!(body["months_present"], 1);
    let classes = body["classes"].as_array().unwrap();
    assert_eq!(classes.len(), 5);
    // All contracts default to non-OTC, so every threshold line reads zero OK.
    for c in classes {
        assert_eq!(c["avg_otc_eur"], 0.0, "{c}");
        assert_eq!(c["verdict"], "ok", "{c}");
        assert_eq!(c["months"].as_array().unwrap().len(), 12);
    }
    // The seeded specs are unconfirmed: total notional is provisional and the
    // warnings say so per contract.
    assert!(body["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("provisional")), "{body}");
    // Eleven of the twelve months predate the sample's history.
    assert_eq!(
        body["warnings"].as_array().unwrap().iter().filter(|w| w.as_str().unwrap().contains("no snapshot")).count(),
        11, "{body}"
    );
    assert_eq!(body["monitors"]["otc_open_contracts"], 0);
    assert_eq!(body["monitors"]["reconciliation"], "not_triggered");
    assert_eq!(body["monitors"]["compression_required"], false);
    assert_eq!(body["margin"].as_array().unwrap().len(), 9, "{body}");
    assert_eq!(body["futures_count"], 8, "{body}");
    assert_eq!(body["kpis"].as_array().unwrap().len(), 0);

    // Flag one contract OTC: its notional must appear on the OTC line of its
    // class. RX is interest_rate; confirm it with its real point value so the
    // notional is definite.
    let (status, _) = put_json(&app, "/api/futures-contracts/RX", serde_json::json!({
        "label": "Euro-Bund", "category": "interest_rate", "point_value": 1000.0,
        "currency": "EUR", "curve": null, "price_convention": "decimal",
        "confirmed": true, "otc": true,
    })).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = get_json(&app, "/api/emir").await;
    assert_eq!(status, StatusCode::OK);
    let ir = body["classes"].as_array().unwrap().iter().find(|c| c["class"] == "interest_rate").unwrap();
    assert!(ir["avg_otc_eur"].as_f64().unwrap() > 0.0, "{ir}");
    assert!(ir["avg_otc_eur"].as_f64().unwrap() <= ir["avg_total_eur"].as_f64().unwrap(), "{ir}");
    assert_eq!(body["monitors"]["otc_open_contracts"], 1, "{body}");
    assert_eq!(body["monitors"]["reconciliation"], "quarterly", "{body}");

    // Bad date is a 400.
    let (status, _) = get_json(&app, "/api/emir?date=garbage").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn evidence_export_round_trips() {
    let (app, pool, edb) = app_with_sample().await;

    let (status, ctype, bytes) = get_bytes(&app, "/api/emir/export").await;
    assert_eq!(status, 200);
    assert!(ctype.contains("spreadsheet"), "got {ctype}");
    let mut wb: calamine::Xlsx<_> = calamine::Xlsx::new(std::io::Cursor::new(bytes)).expect("valid xlsx");
    let names = calamine::Reader::sheet_names(&wb).to_vec();
    for n in ["Seuils", "Contrats", "KPI"] {
        assert!(names.iter().any(|x| x == n), "missing {n} in {names:?}");
    }
    let s = calamine::Reader::worksheet_range(&mut wb, "Seuils").unwrap();
    let text: Vec<String> = s.cells().filter_map(|(_, _, v)| match v {
        calamine::Data::String(s) => Some(s.clone()),
        _ => None,
    }).collect();
    assert!(text.iter().any(|t| t.contains("2026-07-24")), "{text:?}");
    assert!(text.iter().any(|t| t == "Interest-rate derivatives"), "{text:?}");
    assert!(text.iter().any(|t| t.contains("1 of 12")), "{text:?}");
    let c = calamine::Reader::worksheet_range(&mut wb, "Contrats").unwrap();
    // Header + the 8 seeded contracts.
    assert_eq!(c.rows().count(), 9, "{:?}", c.rows().collect::<Vec<_>>());

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn evidence_export_refuses_empty_db() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });
    let res = app.clone().oneshot(Request::get("/api/emir/export").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn kpi_upsert_validation_and_echo_in_report() {
    let (app, pool, edb) = app_with_sample().await;

    let good = serde_json::json!({
        "unconfirmed_over_5d": 1, "reconciliation": "done", "disputes": 0, "note": "  trimmed  ",
    });
    let (status, body) = put_json(&app, "/api/emir/kpis/2026-07-01", good.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["month"], "2026-07-01");
    assert_eq!(body["note"], "trimmed"); // trimmed, not stored raw

    // Blank note collapses to null.
    let blank = serde_json::json!({
        "unconfirmed_over_5d": 0, "reconciliation": "not_applicable", "disputes": 0, "note": "   ",
    });
    let (status, body) = put_json(&app, "/api/emir/kpis/2026-06-01", blank).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["note"], serde_json::Value::Null);

    // Mid-month date, unknown status and negative counts are rejected.
    let (status, _) = put_json(&app, "/api/emir/kpis/2026-07-15", good.clone()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = put_json(&app, "/api/emir/kpis/2026-07-01", serde_json::json!({
        "unconfirmed_over_5d": 0, "reconciliation": "maybe", "disputes": 0, "note": null,
    })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = put_json(&app, "/api/emir/kpis/2026-07-01", serde_json::json!({
        "unconfirmed_over_5d": -1, "reconciliation": "done", "disputes": 0, "note": null,
    })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = put_json(&app, "/api/emir/kpis/garbage", good).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Both records come back in the report, newest first.
    let (status, body) = get_json(&app, "/api/emir").await;
    assert_eq!(status, StatusCode::OK);
    let kpis = body["kpis"].as_array().unwrap();
    assert_eq!(kpis.len(), 2, "{body}");
    assert_eq!(kpis[0]["month"], "2026-07-01");

    pool.close().await;
    edb.stop().await;
}
