use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

const BOUNDARY: &str = "XBOUNDARYX";
const SAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx");

fn upload_req(bytes: &[u8]) -> Request<Body> {
    let mut body = Vec::new();
    body.extend_from_slice(format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"s.xlsx\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    ).as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    Request::post("/api/portfolios/1/imports")
        .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(Body::from(body)).unwrap()
}

async fn get_json(app: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let res = app.clone().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    (status, serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap())
}

async fn put_json(app: &axum::Router, uri: &str, payload: serde_json::Value) -> StatusCode {
    let req = Request::builder().method(Method::PUT).uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap())).unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

fn spec(cat: &str, pv: f64, ccy: &str, conv: &str) -> serde_json::Value {
    serde_json::json!({
        "label": "x", "category": cat, "point_value": pv, "currency": ccy,
        "curve": null, "price_convention": conv, "confirmed": true, "otc": false,
    })
}

/// The eight real specs, as a user would confirm them on the Data page.
const TRUE_SPECS: [(&str, &str, f64, &str, &str); 8] = [
    ("CF", "equity", 10.0, "EUR", "decimal"),
    ("VG", "equity", 10.0, "EUR", "decimal"),
    ("NQ", "equity", 20.0, "USD", "decimal"),
    ("RX", "interest_rate", 1000.0, "EUR", "decimal"),
    ("OAT", "interest_rate", 1000.0, "EUR", "decimal"),
    ("KOA", "interest_rate", 1000.0, "EUR", "decimal"),
    ("TY", "interest_rate", 1000.0, "USD", "th32"),
    ("RY", "fx", 125000.0, "JPY", "decimal"),
];

// Every other test in this branch starts from an empty database and a fresh
// import, which is precisely the shape that hid this: the sha256 short-circuit
// in `import_workbook` returned before the futures seeding ran, so a database
// populated before futures support shipped had the positions and no specs, and
// re-uploading the workbook - the only repair a user can perform - was itself
// the duplicate that skipped the repair. On that database the feature was inert
// while reporting itself complete.
#[tokio::test]
async fn duplicate_import_restores_exposure_on_a_pre_existing_database() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    let pool = dbh.test_pool().clone();
    let app = server::routes::router(server::state::AppState::desktop(dbh.clone()));

    let bytes = std::fs::read(SAMPLE).unwrap();
    assert_eq!(app.clone().oneshot(upload_req(&bytes)).await.unwrap().status(), StatusCode::OK);

    // Rewind to an upgraded installation: positions on record, specs absent.
    sqlx::query("DELETE FROM futures_contracts").execute(&pool).await.unwrap();

    let (_, broken) = get_json(&app, "/api/portfolios/1/metrics/derivatives").await;
    assert_eq!(broken["rows"].as_array().unwrap().len(), 8);
    assert_eq!(broken["excluded"].as_array().unwrap().len(), 8, "nothing is computable");
    assert!((broken["total"]["gross_pct"].as_f64().unwrap()).abs() < 1e-12);
    // The rates section must not claim completeness in this state (I1).
    let (_, broken_rates) = get_json(&app, "/api/portfolios/1/metrics/rates").await;
    assert_eq!(broken_rates["futures"].as_array().unwrap().len(), 0);
    assert_eq!(broken_rates["futures_missing_any"], true,
               "four bond futures are held and none was evaluated");
    assert_eq!(broken_rates["futures_no_spec"].as_array().unwrap().len(), 8);

    // The repair: re-drop the exact same file. Same bytes, same sha256, so this
    // is a duplicate import that re-ingests nothing - and must still seed.
    let res = app.clone().oneshot(upload_req(&bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let out: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(out[0]["outcome"]["duplicate"], true, "still recognised as a duplicate: {out}");
    assert_eq!(out[0]["outcome"]["positions"], 0, "and nothing is re-ingested");
    assert_eq!(out[0]["outcome"]["warnings"].as_array().unwrap().len(), 8, "eight roots re-seeded: {out}");

    let (_, seeded) = get_json(&app, "/api/portfolios/1/metrics/derivatives").await;
    assert_eq!(seeded["unconfirmed"].as_array().unwrap().len(), 8, "seeded, needing confirmation");
    assert!(seeded["excluded"].as_array().unwrap().is_empty(), "every row is computable again");

    // Confirm the eight true specs and the independently verified total is back.
    for (root, cat, pv, ccy, conv) in TRUE_SPECS {
        assert_eq!(put_json(&app, &format!("/api/futures-contracts/{root}"), spec(cat, pv, ccy, conv)).await,
                   StatusCode::OK, "{root}");
    }
    let (_, d) = get_json(&app, "/api/portfolios/1/metrics/derivatives").await;
    assert!((d["total"]["gross_pct"].as_f64().unwrap() - 0.255045).abs() < 1e-5,
            "25.5045% of net assets, as verified from the workbook: {}", d["total"]);

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn derivatives_exposure_on_sample() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    let pool = dbh.test_pool().clone();
    let app = server::routes::router(server::state::AppState::desktop(dbh.clone()));

    let bytes = std::fs::read(SAMPLE).unwrap();
    assert_eq!(app.clone().oneshot(upload_req(&bytes)).await.unwrap().status(), StatusCode::OK);

    // Seeded but unconfirmed: rows are listed and flagged.
    let (st, d) = get_json(&app, "/api/portfolios/1/metrics/derivatives").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(d["date"], "2026-07-24");
    assert_eq!(d["rows"].as_array().unwrap().len(), 8);
    // All 8 contract roots (CF, VG, TY, RY, RX, OAT, NQ, KOA) are brand new on
    // a fresh database, so the import-time seeding (db::repo::import_workbook)
    // inserts all 8 with confirmed = false. See task-9-report.md for the
    // deviation from the brief, which expected 7.
    assert_eq!(d["unconfirmed"].as_array().unwrap().len(), 8);
    // The unconfirmed state is marked on each row, not only in the page-level
    // banner: an unconfirmed spec is still fully counted in the totals, and the
    // banner lists roots (`TY`) while the table lists tickers (`TYU6 Comdty`),
    // leaving the reader to make the join.
    assert!(d["rows"].as_array().unwrap().iter().all(|r| r["unconfirmed"] == true), "{}", d["rows"]);

    // Confirm every contract with its true spec. TY is the 32nds one.
    for (root, cat, pv, ccy, conv) in TRUE_SPECS {
        assert_eq!(put_json(&app, &format!("/api/futures-contracts/{root}"), spec(cat, pv, ccy, conv)).await,
                   StatusCode::OK, "{root}");
    }

    let (_, d) = get_json(&app, "/api/portfolios/1/metrics/derivatives").await;
    assert!(d["unconfirmed"].as_array().unwrap().is_empty());
    assert!(d["rows"].as_array().unwrap().iter().all(|r| r["unconfirmed"] == false), "{}", d["rows"]);
    assert!(d["excluded"].as_array().unwrap().is_empty());
    assert!((d["aum"].as_f64().unwrap() - 28_332_753.49).abs() < 1e-6);

    let cat = |name: &str| -> serde_json::Value {
        d["categories"].as_array().unwrap().iter()
            .find(|c| c["category"] == name).unwrap().clone()
    };
    let eq = cat("equity");
    assert!((eq["long_pct"].as_f64().unwrap() - 0.0).abs() < 1e-9);
    assert!((eq["short_pct"].as_f64().unwrap() - 0.073086).abs() < 1e-5, "{eq}");

    let ir = cat("interest_rate");
    assert!((ir["long_pct"].as_f64().unwrap() - 0.033832).abs() < 1e-5, "{ir}");
    assert!((ir["short_pct"].as_f64().unwrap() - 0.117307).abs() < 1e-5, "{ir}");

    let fx = cat("fx");
    assert!((fx["short_pct"].as_f64().unwrap() - 0.030817).abs() < 1e-5, "{fx}");

    assert!((d["total"]["gross_pct"].as_f64().unwrap() - 0.255045).abs() < 1e-5, "{}", d["total"]);

    // The TY row proves the 32nds path: notional is qty * 1000 * 108.328125.
    let ty = d["rows"].as_array().unwrap().iter()
        .find(|r| r["ticker"] == "TYU6 Comdty").unwrap();
    assert!((ty["price"].as_f64().unwrap() - 108.328125).abs() < 1e-9, "{ty}");
    assert!((ty["notional_ccy"].as_f64().unwrap() - -649_968.75).abs() < 1e-6, "{ty}");

    // Empty categories are still present, at zero.
    assert!((cat("commodity")["gross_pct"].as_f64().unwrap()).abs() < 1e-12);
    assert_eq!(d["categories"].as_array().unwrap().len(), 6);

    // End-to-end degradation: a contract with no usable point value must
    // still be listed, flagged, and dropped from the totals it contributed
    // to - not silently absorbed as a zero. Null out CF's point value, the
    // way a user would on the Data page, and recompute.
    //
    // CF's own contribution to equity short_pct was qty(-12) * point_value(10)
    // * price(8388) / aum(28,332,753.49) = -1,006,560 / 28,332,753.49 =
    // -0.0355264 (a short, reported positive). Before this step equity
    // short_pct was 0.073086 (CF + VG + NQ); with CF excluded, only VG
    // (0.020015) and NQ (0.017545) remain: 0.037560. The overall gross_pct
    // must drop by CF's same 0.035526, from 0.255045 to 0.219519.
    assert_eq!(
        put_json(&app, "/api/futures-contracts/CF", serde_json::json!({
            "label": "x", "category": "equity", "point_value": null, "currency": "EUR",
            "curve": null, "price_convention": "decimal", "confirmed": true, "otc": false,
        })).await,
        StatusCode::OK,
    );

    let (_, d2) = get_json(&app, "/api/portfolios/1/metrics/derivatives").await;
    assert_eq!(d2["rows"].as_array().unwrap().len(), 8, "the row stays listed");
    let cf_row = d2["rows"].as_array().unwrap().iter()
        .find(|r| r["ticker"] == "CFQ6 Index").unwrap();
    assert_eq!(cf_row["spec_missing"], true, "{cf_row}");
    assert!(cf_row["notional_ccy"].is_null(), "{cf_row}");
    assert!(
        d2["excluded"].as_array().unwrap().iter().any(|t| t == "CFQ6 Index"),
        "{}", d2["excluded"]
    );

    let eq2 = d2["categories"].as_array().unwrap().iter()
        .find(|c| c["category"] == "equity").unwrap();
    assert!((eq2["short_pct"].as_f64().unwrap() - 0.037560).abs() < 1e-5,
        "totals must lose CF's exact contribution, not absorb a zero: {eq2}");

    assert!((d2["total"]["gross_pct"].as_f64().unwrap() - 0.219519).abs() < 1e-5,
        "{}", d2["total"]);

    // Bad date -> 400, consistent with the other limits endpoints.
    let (st, _) = get_json(&app, "/api/portfolios/1/metrics/derivatives?date=notadate").await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    pool.close().await;
    edb.stop().await;
}
