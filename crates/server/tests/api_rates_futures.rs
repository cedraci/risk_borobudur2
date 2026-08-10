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
        "curve": null, "price_convention": conv, "confirmed": true, "otc": false,
    })
}

#[tokio::test]
async fn rates_includes_bond_futures_when_ctd_present() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    let wb = std::fs::read(SAMPLE).unwrap();
    assert_eq!(app.clone().oneshot(upload_req("/api/portfolios/1/imports", "s.xlsx", &wb)).await.unwrap().status(), StatusCode::OK);

    // Baseline: the cash bond only. Capture it so the restatement can be checked.
    let (_, r0) = get_json(&app, "/api/portfolios/1/metrics/rates").await;
    let bond_dv01 = r0["bonds"][0]["dv01_eur"].as_f64().unwrap();
    let total0 = r0["total_dv01_eur"].as_f64().unwrap();
    assert!((total0 - bond_dv01).abs() < 1e-9, "no futures yet");
    assert!(r0["futures"].as_array().unwrap().len() == 4, "four bond futures listed");
    assert!(r0["futures"].as_array().unwrap().iter().all(|f| f["missing"] == true),
            "no CTD analytics uploaded yet");
    assert_eq!(r0["futures_missing_any"], true);

    // The restatement is self-consistent: 100bp sensitivity is -100 x DV01 / AUM.
    // The minus sign is the point. `dP = -D_mod x P x dy`, so a book that is
    // long rates loses on a +100bp move and must print a NEGATIVE sensitivity.
    let aum = 28_332_753.49f64;
    let sens0 = r0["nav_sensitivity_100bp"].as_f64().unwrap();
    assert!((sens0 - -100.0 * total0 / aum).abs() < 1e-12);
    assert!(total0 > 0.0, "the cash bond alone is long rates");
    assert!(sens0 < 0.0, "long rates -> +100bp costs NAV, so the figure is negative: {sens0}");

    // Confirm the four bond-future specs, then upload CTD analytics.
    for (root, ccy, conv) in [
        ("RX", "EUR", "decimal"), ("OAT", "EUR", "decimal"),
        ("KOA", "EUR", "decimal"), ("TY", "USD", "th32"),
    ] {
        assert_eq!(put_json(&app, &format!("/api/futures-contracts/{root}"),
                            spec("interest_rate", 1000.0, ccy, conv)).await, StatusCode::OK);
    }
    let ctd = std::fs::read(CTD).unwrap();
    assert_eq!(app.clone().oneshot(upload_req("/api/portfolios/1/futures-analytics", "ctd.csv", &ctd)).await.unwrap().status(),
               StatusCode::OK);

    let (_, r) = get_json(&app, "/api/portfolios/1/metrics/rates").await;
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
    let sens = r["nav_sensitivity_100bp"].as_f64().unwrap();
    assert!((sens - -100.0 * total / aum).abs() < 1e-12);
    assert!(total < 0.0, "the book is net short rates once futures are counted");
    // The regression this pins: a net-short-rates book GAINS on a rate rise, so
    // its "NAV sensitivity per +100bp" must be positive. The unsigned figure
    // printed a negative number here, which reads as "rates up 100bp, NAV down"
    // - exactly backwards - and the sign only became load-bearing once futures
    // could push the total DV01 negative.
    assert!(sens > 0.0, "net short rates -> +100bp gains NAV, so the figure is positive: {sens}");
    assert!(sens0 < 0.0 && sens > 0.0, "and the sign genuinely flips with the book");

    // An unknown AUM is a gap, not a zero: with no NAV row for the snapshot
    // date the sensitivity is null while the DV01 beside it stays populated.
    sqlx::query("DELETE FROM nav_history WHERE date = '2026-07-24'").execute(&pool).await.unwrap();
    let (_, r2) = get_json(&app, "/api/portfolios/1/metrics/rates").await;
    assert!(r2["nav_sensitivity_100bp"].is_null(), "{}", r2["nav_sensitivity_100bp"]);
    assert!((r2["total_dv01_eur"].as_f64().unwrap() - total).abs() < 1e-9,
            "the DV01 itself does not depend on AUM");

    pool.close().await;
    edb.stop().await;
}

// I1: a futures row whose contract root has no spec at all cannot pass the
// rates candidate filter (it is neither `interest_rate` nor `unconfirmed`), so
// it is dropped from the futures array - and its DV01 is dropped from
// `total_dv01_eur` with it. `futures_missing_any: false` is a positive
// assertion of completeness and must not be emitted in that state.
#[tokio::test]
async fn a_future_with_no_spec_at_all_blocks_the_completeness_claim() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    let wb = std::fs::read(SAMPLE).unwrap();
    assert_eq!(app.clone().oneshot(upload_req("/api/portfolios/1/imports", "s.xlsx", &wb)).await.unwrap().status(), StatusCode::OK);

    // Confirm every root properly and supply CTD analytics, so the only thing
    // left that could be incomplete is the spec we are about to remove.
    for (root, cat, ccy, conv) in [
        ("RX", "interest_rate", "EUR", "decimal"), ("OAT", "interest_rate", "EUR", "decimal"),
        ("KOA", "interest_rate", "EUR", "decimal"), ("TY", "interest_rate", "USD", "th32"),
        ("CF", "equity", "EUR", "decimal"), ("VG", "equity", "EUR", "decimal"),
        ("NQ", "equity", "USD", "decimal"), ("RY", "fx", "JPY", "decimal"),
    ] {
        assert_eq!(put_json(&app, &format!("/api/futures-contracts/{root}"),
                            spec(cat, 1000.0, ccy, conv)).await, StatusCode::OK);
    }
    let ctd = std::fs::read(CTD).unwrap();
    assert_eq!(app.clone().oneshot(upload_req("/api/portfolios/1/futures-analytics", "ctd.csv", &ctd)).await.unwrap().status(),
               StatusCode::OK);

    let (_, ok) = get_json(&app, "/api/portfolios/1/metrics/rates").await;
    assert_eq!(ok["futures_missing_any"], false, "baseline: everything resolves");
    assert!(ok["futures_no_spec"].as_array().unwrap().is_empty());
    let total_ok = ok["total_dv01_eur"].as_f64().unwrap();

    // Now drop OAT's spec, the way a row too incomplete for the importer to
    // seed would leave it: the position is still held, nothing identifies it.
    sqlx::query("DELETE FROM futures_contracts WHERE contract_root = 'OAT'")
        .execute(&pool).await.unwrap();

    let (_, r) = get_json(&app, "/api/portfolios/1/metrics/rates").await;
    let futs = r["futures"].as_array().unwrap();
    assert!(futs.iter().all(|f| !f["ticker"].as_str().unwrap().starts_with("OAT")),
            "the spec-less root is still dropped from the table: {futs:?}");
    assert!((r["total_dv01_eur"].as_f64().unwrap() - total_ok).abs() > 1.0,
            "and its DV01 really has gone missing from the total");
    assert_eq!(r["futures_missing_any"], true,
               "so completeness must not be claimed");
    assert_eq!(r["futures_no_spec"].as_array().unwrap(), &vec![serde_json::json!("OATU6 Comdty")],
               "and the reason is named: {}", r["futures_no_spec"]);

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
    assert_eq!(app.clone().oneshot(upload_req("/api/portfolios/1/imports", "s.xlsx", &wb)).await.unwrap().status(), StatusCode::OK);

    // Baseline: all four Comdty-suffixed roots are unconfirmed and show up
    // as missing bond futures.
    let (_, r0) = get_json(&app, "/api/portfolios/1/metrics/rates").await;
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
    assert_eq!(app.clone().oneshot(upload_req("/api/portfolios/1/futures-analytics", "ctd.csv", &ctd)).await.unwrap().status(),
               StatusCode::OK);

    let (_, r) = get_json(&app, "/api/portfolios/1/metrics/rates").await;
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
