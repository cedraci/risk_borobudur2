use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

const BOUNDARY: &str = "XBOUNDARYX";
const SAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx");

/// Fresh embedded database wired into a router. There is no shared
/// tests/common harness in this crate (house rule: every api_*.rs file
/// inlines its own setup), so this file builds its own instance too, mirroring
/// `app` in `api_portfolios.rs`.
async fn app() -> (axum::Router, sqlx::PgPool, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });
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

fn upload_req(uri: &str, filename: &str, bytes: &[u8]) -> Request<Body> {
    let mut body = Vec::new();
    body.extend_from_slice(format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    ).as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    Request::post(uri)
        .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(Body::from(body)).unwrap()
}

async fn upload(app: &axum::Router, uri: &str, filename: &str, bytes: &[u8]) -> (StatusCode, serde_json::Value) {
    let res = app.clone().oneshot(upload_req(uri, filename, bytes)).await.unwrap();
    let status = res.status();
    let raw = res.into_body().collect().await.unwrap().to_bytes();
    let v = if raw.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&raw).unwrap() };
    (status, v)
}

/// A full, valid `AppSettings` body (see `db::settings::AppSettings` and
/// `handlers::settings::validate`), with `redemption_shock` overridable so
/// callers can push a non-default value.
fn settings_body(redemption_shock: f64) -> serde_json::Value {
    serde_json::json!({
        "risk_free_rate": 0.02,
        "var_confidence": 0.99,
        "var_horizon_days": 20,
        "var_window_days": 252,
        "var_limit": 0.20,
        "short_dd_max_days": 50,
        "liquidity_defaults": {
            "Action": "d1", "Fonds": "d2_7", "Future": "d1", "Obligation": "d8_30",
            "Cash Acc": "d1", "Margin Acc": "d1", "Dividendes": "d1",
            "Frais provisionnés": "d1", "Provisions ordres": "d1"
        },
        "redemption_shock": redemption_shock,
    })
}

async fn create_mandate(app: &axum::Router, name: &str) -> i64 {
    let (st, p) = req_json(app, "POST", "/api/portfolios",
        Some(serde_json::json!({"name": name, "kind": "mandate"}))).await;
    assert_eq!(st, StatusCode::OK, "{p}");
    p["id"].as_i64().unwrap()
}

#[tokio::test]
async fn same_file_imports_independently_per_portfolio() {
    let (app, pool, edb) = app().await;
    let bytes = std::fs::read(SAMPLE).unwrap();

    let pid2 = create_mandate(&app, "Mandat Alpha").await;
    assert_eq!(pid2, 2);

    // Upload to portfolio 1: fresh import, real rows.
    let (st, body) = upload(&app, "/api/portfolios/1/imports", "s.xlsx", &bytes).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["duplicate"], false, "{body}");
    let nav_rows_1 = body["nav_rows"].as_u64().unwrap();
    assert!(nav_rows_1 > 0, "{body}");

    // Re-upload the same file to portfolio 1: dedupe kicks in.
    let (st, body) = upload(&app, "/api/portfolios/1/imports", "s.xlsx", &bytes).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["duplicate"], true, "{body}");

    // Upload the SAME bytes to portfolio 2: dedupe is per-portfolio, so this
    // must succeed as a fresh import there, not bounce off portfolio 1's row.
    let (st, body) = upload(&app, "/api/portfolios/2/imports", "s.xlsx", &bytes).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["duplicate"], false, "{body}");
    assert_eq!(body["nav_rows"].as_u64().unwrap(), nav_rows_1, "{body}");

    // Positions: same file imported into both portfolios must yield the same
    // meaningful content (asset_type/isin/quantity/valuation/... - none of
    // which carry a portfolio_id) — compare full row sets, and guard against
    // a vacuous pass by asserting both are non-empty first.
    let (st, pos1) = req_json(&app, "GET", "/api/portfolios/1/positions", None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, pos2) = req_json(&app, "GET", "/api/portfolios/2/positions", None).await;
    assert_eq!(st, StatusCode::OK);
    let rows1 = pos1["rows"].as_array().unwrap();
    let rows2 = pos2["rows"].as_array().unwrap();
    assert!(!rows1.is_empty(), "{pos1}");
    assert_eq!(rows1.len(), rows2.len(), "row count mismatch: {} vs {}", rows1.len(), rows2.len());
    assert_eq!(pos1["date"], pos2["date"]);
    assert_eq!(rows1, rows2, "positions diverged across portfolios for the same source file");

    // Metrics summary must return real data for both, not the empty stub.
    let (st, m1) = req_json(&app, "GET", "/api/portfolios/1/metrics/summary", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(m1["empty"], false, "{m1}");
    assert!(m1["nav"].is_number(), "{m1}");

    let (st, m2) = req_json(&app, "GET", "/api/portfolios/2/metrics/summary", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(m2["empty"], false, "{m2}");
    assert!(m2["nav"].is_number(), "{m2}");

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn settings_and_kpis_do_not_leak_across_portfolios() {
    let (app, pool, edb) = app().await;
    let bytes = std::fs::read(SAMPLE).unwrap();

    let pid2 = create_mandate(&app, "Mandat Alpha").await;
    assert_eq!(pid2, 2);

    // EMIR needs positions to assemble a real (non-"empty") report.
    let (st, body) = upload(&app, "/api/portfolios/1/imports", "s.xlsx", &bytes).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let (st, body) = upload(&app, "/api/portfolios/2/imports", "s.xlsx", &bytes).await;
    assert_eq!(st, StatusCode::OK, "{body}");

    // Sanity: default redemption_shock is 0.30 before any write (see
    // db::settings::default_redemption_shock).
    let (st, s1_before) = req_json(&app, "GET", "/api/portfolios/1/settings", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!((s1_before["redemption_shock"].as_f64().unwrap() - 0.30).abs() < 1e-9, "{s1_before}");

    // Push a non-default redemption_shock onto portfolio 1 only.
    let (st, put_body) = req_json(&app, "PUT", "/api/portfolios/1/settings", Some(settings_body(0.3))).await;
    assert_eq!(st, StatusCode::OK, "{put_body}");
    assert!((put_body["redemption_shock"].as_f64().unwrap() - 0.3).abs() < 1e-9, "{put_body}");

    // Portfolio 2 must still read the default.
    let (st, s2) = req_json(&app, "GET", "/api/portfolios/2/settings", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!((s2["redemption_shock"].as_f64().unwrap() - 0.30).abs() < 1e-9,
        "portfolio 2 settings leaked portfolio 1's write: {s2}");

    // Re-read portfolio 1 to be sure the write actually persisted (not just
    // echoed back in the PUT response).
    let (st, s1_after) = req_json(&app, "GET", "/api/portfolios/1/settings", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!((s1_after["redemption_shock"].as_f64().unwrap() - 0.3).abs() < 1e-9, "{s1_after}");

    // EMIR KPI on portfolio 1 only.
    let kpi = serde_json::json!({
        "unconfirmed_over_5d": 3, "reconciliation": "done", "disputes": 0, "note": "",
    });
    let (st, kpi_body) = req_json(&app, "PUT", "/api/portfolios/1/emir/kpis/2026-07-01", Some(kpi)).await;
    assert_eq!(st, StatusCode::OK, "{kpi_body}");
    assert_eq!(kpi_body["month"], "2026-07-01", "{kpi_body}");
    assert_eq!(kpi_body["unconfirmed_over_5d"], 3, "{kpi_body}");
    assert_eq!(kpi_body["reconciliation"], "done", "{kpi_body}");
    assert_eq!(kpi_body["disputes"], 0, "{kpi_body}");

    // Portfolio 2's EMIR report has positions (so it is not the "empty"
    // stub) but must show zero KPIs — the write above must not leak.
    let (st, emir2) = req_json(&app, "GET", "/api/portfolios/2/emir", None).await;
    assert_eq!(st, StatusCode::OK, "{emir2}");
    assert_ne!(emir2["empty"], serde_json::Value::Bool(true), "{emir2}");
    assert_eq!(emir2["kpis"].as_array().unwrap().len(), 0, "portfolio 2 saw portfolio 1's KPI: {emir2}");

    let (st, emir1) = req_json(&app, "GET", "/api/portfolios/1/emir", None).await;
    assert_eq!(st, StatusCode::OK, "{emir1}");
    let kpis1 = emir1["kpis"].as_array().unwrap();
    assert_eq!(kpis1.len(), 1, "{emir1}");
    assert_eq!(kpis1[0]["month"], "2026-07-01", "{emir1}");

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn unknown_and_archived_portfolios_are_refused() {
    let (app, pool, edb) = app().await;
    let bytes = std::fs::read(SAMPLE).unwrap();

    // Unknown portfolio: every scoped route 404s via ensure().
    let (st, body) = req_json(&app, "GET", "/api/portfolios/99/nav", None).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{body}");

    let pid2 = create_mandate(&app, "Mandat Alpha").await;
    assert_eq!(pid2, 2);

    // Archive portfolio 2.
    let (st, p) = req_json(&app, "PUT", "/api/portfolios/2",
        Some(serde_json::json!({"name": "Mandat Alpha", "archived": true}))).await;
    assert_eq!(st, StatusCode::OK, "{p}");
    assert_eq!(p["archived"], true, "{p}");

    // Reads on an archived portfolio stay available.
    let (st, nav) = req_json(&app, "GET", "/api/portfolios/2/nav", None).await;
    assert_eq!(st, StatusCode::OK, "{nav}");
    assert_eq!(nav.as_array().unwrap().len(), 0, "no import happened yet: {nav}");

    // Mutating requests on an archived portfolio are refused.
    let (st, body) = upload(&app, "/api/portfolios/2/imports", "s.xlsx", &bytes).await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");

    let (st, body) = req_json(&app, "PUT", "/api/portfolios/2/settings", Some(settings_body(0.3))).await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");

    pool.close().await;
    edb.stop().await;
}
