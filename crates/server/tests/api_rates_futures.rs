use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

const BOUNDARY: &str = "XBOUNDARYX";
const SAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx");
const CTD: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/ctd_sample.csv");

fn upload_req(uri: &str, name: &str, bytes: &[u8]) -> Request<Body> {
    let mut body = Vec::new();
    body.extend_from_slice(format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    ).as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    Request::post(uri)
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
        "curve": null, "price_convention": conv, "confirmed": true,
    })
}

#[tokio::test]
async fn rates_includes_bond_futures_when_ctd_present() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    let wb = std::fs::read(SAMPLE).unwrap();
    assert_eq!(app.clone().oneshot(upload_req("/api/imports", "s.xlsx", &wb)).await.unwrap().status(), StatusCode::OK);

    // Baseline: the cash bond only. Capture it so the restatement can be checked.
    let (_, r0) = get_json(&app, "/api/metrics/rates").await;
    let bond_dv01 = r0["bonds"][0]["dv01_eur"].as_f64().unwrap();
    let total0 = r0["total_dv01_eur"].as_f64().unwrap();
    assert!((total0 - bond_dv01).abs() < 1e-9, "no futures yet");
    assert!(r0["futures"].as_array().unwrap().len() == 4, "four bond futures listed");
    assert!(r0["futures"].as_array().unwrap().iter().all(|f| f["missing"] == true),
            "no CTD analytics uploaded yet");
    assert_eq!(r0["futures_missing_any"], true);

    // The restatement is self-consistent: 100bp sensitivity is 100 x DV01 / AUM.
    let aum = 28_332_753.49f64;
    assert!((r0["nav_sensitivity_100bp"].as_f64().unwrap() - 100.0 * total0 / aum).abs() < 1e-12);

    // Confirm the four bond-future specs, then upload CTD analytics.
    for (root, ccy, conv) in [
        ("RX", "EUR", "decimal"), ("OAT", "EUR", "decimal"),
        ("KOA", "EUR", "decimal"), ("TY", "USD", "th32"),
    ] {
        assert_eq!(put_json(&app, &format!("/api/futures-contracts/{root}"),
                            spec("interest_rate", 1000.0, ccy, conv)).await, StatusCode::OK);
    }
    let ctd = std::fs::read(CTD).unwrap();
    assert_eq!(app.clone().oneshot(upload_req("/api/futures-analytics", "ctd.csv", &ctd)).await.unwrap().status(),
               StatusCode::OK);

    let (_, r) = get_json(&app, "/api/metrics/rates").await;
    let futs = r["futures"].as_array().unwrap();
    assert_eq!(futs.len(), 4);
    assert!(futs.iter().all(|f| f["missing"] == false));
    assert_eq!(r["futures_missing_any"], false);

    // The bond's own DV01 is untouched by adding the futures block: same
    // figure before and after the CTD upload.
    assert!((r["bonds"][0]["dv01_eur"].as_f64().unwrap() - bond_dv01).abs() < 1e-9,
            "bond figures must not move");

    // RX: 8.41 * (98.72 + 0.63) * 1000 * 1e-4 / 0.782145 = 106.8259 per contract,
    // held -8, fx 1.0.
    let rx = futs.iter().find(|f| f["ticker"] == "RXU6 Comdty").unwrap();
    assert!((rx["dv01_eur"].as_f64().unwrap() - -854.607).abs() < 1e-2, "{rx}");
    assert!(rx["dv01_eur"].as_f64().unwrap() < 0.0, "a short is negative DV01");

    // Totals move by exactly the futures' contribution.
    let total = r["total_dv01_eur"].as_f64().unwrap();
    let fut_sum: f64 = futs.iter().map(|f| f["dv01_eur"].as_f64().unwrap()).sum();
    assert!((total - (bond_dv01 + fut_sum)).abs() < 1e-6);
    assert!((r["nav_sensitivity_100bp"].as_f64().unwrap() - 100.0 * total / aum).abs() < 1e-12);
    assert!(total < 0.0, "the book is net short rates once futures are counted");

    pool.close().await;
    edb.stop().await;
}

// A root the user has deliberately confirmed as `other` (not left at the
// import-time default) must drop out of the rates section entirely - and,
// critically, must not keep `futures_missing_any` pinned true forever just
// because no CTD analytics will ever exist for it. This distinguishes
// "unconfirmed default `other`" (still shown, still counted as missing) from
// "user-confirmed `other`" (excluded, and no longer counted). Confirming as
// `other` specifically - rather than some other non-rate category like
// `commodity` - is the case that actually exercises the fix: any category
// other than `interest_rate`/`other` was already excluded by category alone,
// even under the old, buggy filter. Only `other` needs the `confirmed` bit
// to disambiguate "not yet told" from "user says this isn't a bond future."
#[tokio::test]
async fn confirmed_non_rate_future_drops_out_of_rates_section() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    let wb = std::fs::read(SAMPLE).unwrap();
    assert_eq!(app.clone().oneshot(upload_req("/api/imports", "s.xlsx", &wb)).await.unwrap().status(), StatusCode::OK);

    // Baseline: all four Comdty-suffixed roots are unconfirmed and show up
    // as missing bond futures.
    let (_, r0) = get_json(&app, "/api/metrics/rates").await;
    assert_eq!(r0["futures"].as_array().unwrap().len(), 4);
    assert_eq!(r0["futures_missing_any"], true);

    // The user confirms KOA as `other`, on purpose - e.g. having checked it
    // really is a commodity future the regulatory taxonomy has no more
    // specific bucket for. It must never resolve a CTD match, so under a
    // filter that admits any `other`-categorised root regardless of
    // `confirmed`, it would sit in `futures` forever with missing: true.
    assert_eq!(put_json(&app, "/api/futures-contracts/KOA",
                         spec("other", 1000.0, "EUR", "decimal")).await, StatusCode::OK);

    // Confirm the remaining three as interest_rate and supply CTD analytics
    // for all four roots (KOA's row is present but must go unused).
    for (root, ccy, conv) in [
        ("RX", "EUR", "decimal"), ("OAT", "EUR", "decimal"), ("TY", "USD", "th32"),
    ] {
        assert_eq!(put_json(&app, &format!("/api/futures-contracts/{root}"),
                            spec("interest_rate", 1000.0, ccy, conv)).await, StatusCode::OK);
    }
    let ctd = std::fs::read(CTD).unwrap();
    assert_eq!(app.clone().oneshot(upload_req("/api/futures-analytics", "ctd.csv", &ctd)).await.unwrap().status(),
               StatusCode::OK);

    let (_, r) = get_json(&app, "/api/metrics/rates").await;
    let futs = r["futures"].as_array().unwrap();
    assert_eq!(futs.len(), 3, "the confirmed-`other` root must drop out entirely: {futs:?}");
    assert!(futs.iter().all(|f| !f["ticker"].as_str().unwrap().starts_with("KOA")),
            "KOA must not appear in the rates section once confirmed `other`: {futs:?}");
    assert!(futs.iter().all(|f| f["missing"] == false));
    assert_eq!(r["futures_missing_any"], false,
               "a confirmed non-rate future must not permanently pin futures_missing_any");

    pool.close().await;
    edb.stop().await;
}
