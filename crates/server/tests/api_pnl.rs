use axum::body::Body;
use axum::http::{Request, StatusCode};
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
    Request::post("/api/imports")
        .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(Body::from(body)).unwrap()
}

async fn get_json(app: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let res = app.clone().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    let body = serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    (status, body)
}

/// Fresh embedded database, seeded with the sample workbook, wired into a
/// router. Mirrors `common::app_with_sample` from the brief; there is no
/// shared `tests/common` harness in this crate (every other `api_*.rs` test
/// file inlines this same setup), so each test builds its own instance
/// rather than adding a new parallel harness module.
///
/// This is the fixture's natural, un-doctored state: exactly one position
/// snapshot date (2026-07-24). Used directly by
/// `fewer_than_two_snapshots_reports_empty_with_a_reason`, and as the base
/// that `app_with_sample` below builds on.
async fn upload_sample() -> (axum::Router, sqlx::PgPool, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    let bytes = std::fs::read(SAMPLE).unwrap();
    assert_eq!(app.clone().oneshot(upload_req(&bytes)).await.unwrap().status(), StatusCode::OK);

    (app, pool, edb)
}

/// `upload_sample()`, plus a second position snapshot.
///
/// The sample workbook carries exactly one position snapshot date
/// (2026-07-24) even though its NAV/AUM history runs back to 2025-02-28
/// (`import_upsert_and_duplicate_semantics` in `crates/db/tests/import_workbook.rs`
/// pins this: `position_dates` is `vec![2026-07-24]` after import, no matter
/// how many times the same file is re-imported). A period P&L needs two
/// distinct position snapshots to compute a delta, so this clones the single
/// snapshot onto the earliest NAV history date, giving the handler a real
/// second endpoint to snap to (the resulting period P&L is economically
/// inert - same positions at both ends - but exercises the endpoint's period
/// resolution, grouping and reconciliation wiring, which is what most of
/// these tests check; the one test that needs non-degenerate figures
/// perturbs specific rows on top of this clone - see
/// `instrument_and_reconciliation_arithmetic_matches_a_hand_checked_scenario`).
async fn app_with_sample() -> (axum::Router, sqlx::PgPool, db::embedded::EmbeddedDb) {
    let (app, pool, edb) = upload_sample().await;

    let earliest: chrono::NaiveDate = sqlx::query_scalar("SELECT MIN(date) FROM nav_history")
        .fetch_one(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO position_snapshots
             (nav_date, import_id, asset_type, isin, name, currency, quantity,
              avg_cost, price, valuation_ccy, accrued_interest, fx_rate, valuation_eur, weight, ticker)
         SELECT $1, import_id, asset_type, isin, name, currency, quantity,
                avg_cost, price, valuation_ccy, accrued_interest, fx_rate, valuation_eur, weight, ticker
         FROM position_snapshots WHERE nav_date = (SELECT MAX(nav_date) FROM position_snapshots)",
    )
    .bind(earliest)
    .execute(&pool)
    .await
    .unwrap();

    (app, pool, edb)
}

#[tokio::test]
async fn pnl_snaps_to_snapshot_dates_and_reports_which_it_used() {
    let (app, pool, edb) = app_with_sample().await;
    let (status, body) = get_json(&app, "/api/pnl?from=2020-01-01&to=2030-01-01").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["empty"], false);
    let p = &body["period"];
    assert!(p["actual_from"].is_string());
    assert!(p["actual_to"].is_string());
    assert!(p["snapshots"].as_i64().unwrap() >= 1);

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn reconciliation_residual_is_always_present() {
    let (app, pool, edb) = app_with_sample().await;
    let (_, body) = get_json(&app, "/api/pnl?from=2020-01-01&to=2030-01-01").await;
    let r = &body["reconciliation"];
    assert!(r["residual"].is_number(), "residual must always be returned");
    assert!(r["within_tolerance"].is_boolean());
    assert!(r["gross"].is_number());

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn groups_by_the_requested_dimension() {
    let (app, pool, edb) = app_with_sample().await;
    let (_, body) =
        get_json(&app, "/api/pnl?from=2020-01-01&to=2030-01-01&dimension=asset_class").await;
    let keys: Vec<String> = body["groups"].as_array().unwrap().iter()
        .map(|g| g["key"].as_str().unwrap().to_string()).collect();
    assert!(keys.iter().any(|k| k == "Equities"), "got {keys:?}");

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn an_unknown_dimension_is_a_bad_request() {
    let (app, pool, edb) = app_with_sample().await;
    let (status, _) = get_json(&app, "/api/pnl?from=2020-01-01&to=2030-01-01&dimension=zzz").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn group_totals_equal_the_sum_of_their_instruments() {
    let (app, pool, edb) = app_with_sample().await;
    let (_, body) =
        get_json(&app, "/api/pnl?from=2020-01-01&to=2030-01-01&dimension=currency").await;
    for g in body["groups"].as_array().unwrap() {
        let sum: f64 = g["instruments"].as_array().unwrap().iter()
            .map(|i| i["realized_price"].as_f64().unwrap() + i["unrealized_price"].as_f64().unwrap()
                   + i["realized_fx"].as_f64().unwrap() + i["unrealized_fx"].as_f64().unwrap())
            .sum();
        assert!((g["total"].as_f64().unwrap() - sum).abs() < 1e-6);
    }

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn fewer_than_two_snapshots_reports_empty_with_a_reason() {
    // The fixture's natural state - one import, one position-snapshot date -
    // is exactly the "not enough history" case `app_with_sample` works
    // around for the other tests in this file. Assert that degraded state
    // directly here.
    let (app, pool, edb) = upload_sample().await;
    let (status, body) = get_json(&app, "/api/pnl?from=2020-01-01&to=2030-01-01").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["empty"], true);
    let warnings: Vec<&str> = body["warnings"].as_array().unwrap().iter()
        .map(|w| w.as_str().unwrap()).collect();
    assert!(warnings.iter().any(|w| w.contains("at least two")), "got {warnings:?}");

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn a_range_resolving_to_a_single_snapshot_reports_empty_with_a_reason() {
    // Two snapshot dates exist (2025-02-28 and 2026-07-24), but a request
    // that pins from and to to the same one of them still resolves to a
    // single endpoint - no delta to compute.
    let (app, pool, edb) = app_with_sample().await;
    let (status, body) = get_json(&app, "/api/pnl?from=2026-07-24&to=2026-07-24").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["empty"], true);
    let warnings: Vec<&str> = body["warnings"].as_array().unwrap().iter()
        .map(|w| w.as_str().unwrap()).collect();
    assert!(warnings.iter().any(|w| w.contains("resolves to a single snapshot")), "got {warnings:?}");

    pool.close().await;
    edb.stop().await;
}

/// Pins the handler's arithmetic - not the analytics library's, which has
/// its own exhaustive unit tests in `crates/analytics/src/pnl.rs` - against
/// a hand-checked scenario. `app_with_sample`'s cloned earlier snapshot
/// makes v0 == v1 for every position, so nothing here would catch the
/// handler wiring the wrong DB column into the wrong analytics argument.
/// This perturbs three rows on the earlier (t0) snapshot by known amounts:
/// one EUR equity, one non-EUR (GBP) equity, and one Cash row, and checks
/// the API's numbers against the resulting hand arithmetic.
#[tokio::test]
async fn instrument_and_reconciliation_arithmetic_matches_a_hand_checked_scenario() {
    let (app, pool, edb) = app_with_sample().await;
    let t0: chrono::NaiveDate = sqlx::query_scalar("SELECT MIN(nav_date) FROM position_snapshots")
        .fetch_one(&pool).await.unwrap();

    // Both equities have real trades across the whole 2025-02-28..2026-07-24
    // window (every equity in the fixture does). Remove them so
    // `walk_instrument` sees an empty history: `decompose` then reduces to
    // its simplest form - no realized leg, no split ambiguity - price P&L is
    // exactly `(v1 - v0) * f0` and FX P&L is exactly `v1 * (f1 - f0)`.
    sqlx::query("DELETE FROM operations WHERE isin IN ('GRS145003000', 'GB0007188757')")
        .execute(&pool).await.unwrap();

    // EUR equity GRS145003000: real t1 valuation_ccy = 316,572.00. EUR is
    // special-cased to f0 = f1 = 1.0 regardless of stored fx_rate, so with
    // v0 set to 300,000.00:
    //   unrealized_price = 316,572.00 - 300,000.00 = 16,572.00
    //   everything else (realized_price, realized_fx, unrealized_fx) = 0
    sqlx::query(
        "UPDATE position_snapshots SET valuation_ccy = 300000.0, valuation_eur = 300000.0
         WHERE nav_date = $1 AND isin = 'GRS145003000'",
    )
    .bind(t0).execute(&pool).await.unwrap();

    // GBP equity GB0007188757: real t1 valuation_ccy = 64,431.22, real
    // fx_rate = 1.17123448. Set v0 = 60,000.00 at a different rate,
    // f0 = 1.10:
    //   local_pnl        = 64,431.22 - 60,000.00           =  4,431.22
    //   unrealized_price = 4,431.22 * 1.10                 =  4,874.342
    //   unrealized_fx    = 64,431.22 * (1.17123448 - 1.10) =  4,589.7244524656
    //   total                                               =  9,464.0664524656
    // Every other GBP row's fx_rate at t0 is nulled so `snap_rate`'s search
    // for "any GBP row with a positive fx_rate" deterministically lands on
    // this one, regardless of row insertion order (their own price P&L is
    // still real trade-driven noise from the untouched clone, which is why
    // this test doesn't assert a portfolio-wide total).
    sqlx::query(
        "UPDATE position_snapshots SET valuation_ccy = 60000.0, valuation_eur = 66000.0, fx_rate = 1.10
         WHERE nav_date = $1 AND isin = 'GB0007188757'",
    )
    .bind(t0).execute(&pool).await.unwrap();
    sqlx::query(
        "UPDATE position_snapshots SET fx_rate = NULL
         WHERE nav_date = $1 AND currency = 'GBP' AND isin <> 'GB0007188757'",
    )
    .bind(t0).execute(&pool).await.unwrap();

    // Cash row BK001USD (USD): cash/margin lines bypass `decompose`
    // entirely - they're straight `valuation_eur` deltas. Real t1
    // valuation_eur = 902,830.24; set t0 to 850,000.00. Every other
    // cash/margin/fees/provisions/income row is an untouched clone
    // (e1 - e0 = 0 exactly), so this is the only nonzero contributor to
    // `cash_and_margin`: 902,830.24 - 850,000.00 = 52,830.24.
    sqlx::query(
        "UPDATE position_snapshots SET valuation_eur = 850000.0
         WHERE nav_date = $1 AND isin = 'BK001USD'",
    )
    .bind(t0).execute(&pool).await.unwrap();

    let (status, body) = get_json(&app, "/api/pnl?from=2020-01-01&to=2030-01-01").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["empty"], false);

    let equities = body["groups"].as_array().unwrap().iter()
        .find(|g| g["key"] == "Equities").unwrap();
    let instrument = |isin: &str| equities["instruments"].as_array().unwrap().iter()
        .find(|i| i["isin"] == isin).unwrap().clone();

    let eur_eq = instrument("GRS145003000");
    assert!((eur_eq["realized_price"].as_f64().unwrap() - 0.0).abs() < 1e-9, "{eur_eq}");
    assert!((eur_eq["unrealized_price"].as_f64().unwrap() - 16572.0).abs() < 1e-6, "{eur_eq}");
    assert!((eur_eq["realized_fx"].as_f64().unwrap() - 0.0).abs() < 1e-9, "{eur_eq}");
    assert!((eur_eq["unrealized_fx"].as_f64().unwrap() - 0.0).abs() < 1e-9, "{eur_eq}");

    let gbp_eq = instrument("GB0007188757");
    assert!((gbp_eq["realized_price"].as_f64().unwrap() - 0.0).abs() < 1e-9, "{gbp_eq}");
    assert!((gbp_eq["unrealized_price"].as_f64().unwrap() - 4874.342).abs() < 1e-3, "{gbp_eq}");
    assert!((gbp_eq["realized_fx"].as_f64().unwrap() - 0.0).abs() < 1e-9, "{gbp_eq}");
    assert!((gbp_eq["unrealized_fx"].as_f64().unwrap() - 4589.7244524656).abs() < 1e-3, "{gbp_eq}");

    let r = &body["reconciliation"];
    assert!((r["cash_and_margin"].as_f64().unwrap() - 52830.24).abs() < 1e-6, "{r}");
    // Fees/Provisions/Income rows were never touched, so every one of them
    // is a straight clone: their e1 - e0 is exactly zero.
    assert!((r["accrued_fees"].as_f64().unwrap() - 0.0).abs() < 1e-9, "{r}");
    assert!((r["provisions"].as_f64().unwrap() - 0.0).abs() < 1e-9, "{r}");
    // Dividend window is (t0, t1] = (2025-02-28, 2026-07-24], which is every
    // one of the fixture's 53 dividend rows - untouched by the
    // perturbations above, so this independently pins the window filter.
    assert!((r["dividend_income"].as_f64().unwrap() - 221124.35).abs() < 1e-2, "{r}");
    // aum_change from the fixture's own NAV history, independent of every
    // perturbation above: 28,332,753.49 (2026-07-24) - 301,000.00 (2025-02-28).
    assert!((r["aum_change"].as_f64().unwrap() - 28_031_753.49).abs() < 1e-2, "{r}");

    // Residual isn't perturbed directly: recompute it from the
    // reconciliation's own other fields per `reconcile`'s documented formula
    // and check the endpoint reports the number it derives internally,
    // rather than some other figure that happens to also be a number.
    let total_pnl = r["investment_pnl"].as_f64().unwrap() + r["cash_and_margin"].as_f64().unwrap()
        + r["accrued_fees"].as_f64().unwrap() + r["provisions"].as_f64().unwrap()
        + r["dividend_income"].as_f64().unwrap();
    assert!((r["total_pnl"].as_f64().unwrap() - total_pnl).abs() < 1e-6, "{r}");
    let expected_residual =
        (r["aum_change"].as_f64().unwrap() - r["net_flows"].as_f64().unwrap()) - total_pnl;
    assert!((r["residual"].as_f64().unwrap() - expected_residual).abs() < 1e-6, "{r}");

    pool.close().await;
    edb.stop().await;
}
