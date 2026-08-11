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
    Request::post("/api/portfolios/1/imports")
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
             (portfolio_id, nav_date, import_id, asset_type, isin, name, currency, quantity,
              avg_cost, price, valuation_ccy, accrued_interest, fx_rate, valuation_eur, weight, ticker)
         SELECT portfolio_id, $1, import_id, asset_type, isin, name, currency, quantity,
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
    let (status, body) = get_json(&app, "/api/portfolios/1/pnl?from=2020-01-01&to=2030-01-01").await;
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
    let (_, body) = get_json(&app, "/api/portfolios/1/pnl?from=2020-01-01&to=2030-01-01").await;
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
        get_json(&app, "/api/portfolios/1/pnl?from=2020-01-01&to=2030-01-01&dimension=asset_class").await;
    let keys: Vec<String> = body["groups"].as_array().unwrap().iter()
        .map(|g| g["key"].as_str().unwrap().to_string()).collect();
    assert!(keys.iter().any(|k| k == "Equities"), "got {keys:?}");

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn an_unknown_dimension_is_a_bad_request() {
    let (app, pool, edb) = app_with_sample().await;
    let (status, _) = get_json(&app, "/api/portfolios/1/pnl?from=2020-01-01&to=2030-01-01&dimension=zzz").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn group_totals_equal_the_sum_of_their_instruments() {
    let (app, pool, edb) = app_with_sample().await;
    let (_, body) =
        get_json(&app, "/api/portfolios/1/pnl?from=2020-01-01&to=2030-01-01&dimension=currency").await;
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
    let (status, body) = get_json(&app, "/api/portfolios/1/pnl?from=2020-01-01&to=2030-01-01").await;
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
    let (status, body) = get_json(&app, "/api/portfolios/1/pnl?from=2026-07-24&to=2026-07-24").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["empty"], true);
    let warnings: Vec<&str> = body["warnings"].as_array().unwrap().iter()
        .map(|w| w.as_str().unwrap()).collect();
    assert!(warnings.iter().any(|w| w.contains("resolves to a single snapshot")), "got {warnings:?}");

    pool.close().await;
    edb.stop().await;
}

/// The spec's reconciliation promise, pinned on books that actually balance:
/// two snapshots of a synthetic mini-fund constructed by double-entry, so the
/// residual must be ~0 - not merely "within tolerance of a large gross".
///
/// The fixture (all EUR values tie exactly):
///
///   t0 = 2026-06-01                          t1 = 2026-06-30
///   EQ1EUR  (Action, EUR) 100 @ 10 = 1,000   150 @ 11    = 1,650
///   EQ2USD  (Action, USD) 1,000 USD @ 0.90   1,100 USD @ 0.95
///                         =   900 EUR        = 1,045 EUR
///   CASHEUR (Cash Acc)    5,000              4,900
///   DIVBETA (Dividendes)  -                  300 USD @ 0.95 = 285
///   AUM                   6,900              7,880
///
///   2026-06-10  subscription +500 (shares 69 -> 74 at NAV 100)
///               cash 5,000 -> 5,500
///   2026-06-15  buy 50 EQ1EUR @ 12, net_amount -600; cash 5,500 -> 4,900
///   2026-06-20  dividend provisioned on Beta Corp: 300 USD, fx_history
///               rate 0.92 -> 276 EUR of income; receivable carried at the
///               t1 rate 0.95 -> 285 EUR on the balance sheet
///
/// Hand-derived reconciliation for (t0, t1]:
///   investment_pnl  = EQ1: (1,650 - 1,000) - 600      =  50
///                   + EQ2: 1,100*0.95 - 1,000*0.90    = 145   -> 195
///   cash line       = dCash - trade flows - net flows - dividend receipts
///                   = -100 - (-600) - 500 - (276 - 285) = +9
///                     (the +9 is the receivable's FX revaluation
///                      300*(0.95 - 0.92), the only genuine unexplained-cash
///                      -bucket movement in the fixture)
///   accrued_fees    = 0        provisions = 0
///   dividend_income = 300 * 0.92                      = 276
///   total           = 195 + 9 + 0 + 0 + 276           = 480
///   dAUM - flows    = (7,880 - 6,900) - 500           = 480   -> residual 0
///
/// A third snapshot (2026-07-15) pins the payment leg of the same dividend:
/// receivable -285, cash +285, no income - each line 0, residual 0.
///
/// This scenario is exactly the reviewer's three traces (settlement leg of a
/// buy, subscription, dividend accrual + payment): under the pre-fix wiring
/// the period-1 residual is -(flows + trade CFs + dividends)
/// = -(500 - 600 + 300) = -200, so this test fails loudly there.
#[tokio::test]
async fn two_consistent_snapshots_reconcile_to_a_near_zero_residual() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    let import_id: i64 = sqlx::query_scalar(
        "INSERT INTO imports (portfolio_id, filename, sha256, nav_date, row_counts)
         VALUES (1, 'mini.xlsx', 'mini-sha', '2026-06-30', '{}') RETURNING id",
    ).fetch_one(&pool).await.unwrap();

    sqlx::query(
        "INSERT INTO position_snapshots
           (portfolio_id, nav_date, import_id, asset_type, isin, name, currency,
            quantity, price, valuation_ccy, fx_rate, valuation_eur)
         VALUES
           (1, '2026-06-01', $1, 'Action',     'EQ1EUR',  'Alpha SE',      'EUR', 100,  10,   1000, NULL, 1000),
           (1, '2026-06-01', $1, 'Action',     'EQ2USD',  'Beta Corp',     'USD', 200,  5,    1000, 0.90, 900),
           (1, '2026-06-01', $1, 'Cash Acc',   'CASHEUR', 'Cash EUR',      'EUR', NULL, NULL, 5000, NULL, 5000),
           (1, '2026-06-30', $1, 'Action',     'EQ1EUR',  'Alpha SE',      'EUR', 150,  11,   1650, NULL, 1650),
           (1, '2026-06-30', $1, 'Action',     'EQ2USD',  'Beta Corp',     'USD', 200,  5.5,  1100, 0.95, 1045),
           (1, '2026-06-30', $1, 'Cash Acc',   'CASHEUR', 'Cash EUR',      'EUR', NULL, NULL, 4900, NULL, 4900),
           (1, '2026-06-30', $1, 'Dividendes', 'DIVBETA', 'Beta Corp div', 'USD', NULL, NULL, 300,  0.95, 285),
           (1, '2026-07-15', $1, 'Action',     'EQ1EUR',  'Alpha SE',      'EUR', 150,  11,   1650, NULL, 1650),
           (1, '2026-07-15', $1, 'Action',     'EQ2USD',  'Beta Corp',     'USD', 200,  5.5,  1100, 0.95, 1045),
           (1, '2026-07-15', $1, 'Cash Acc',   'CASHEUR', 'Cash EUR',      'EUR', NULL, NULL, 5185, NULL, 5185)",
    ).bind(import_id).execute(&pool).await.unwrap();

    sqlx::query(
        "INSERT INTO nav_history (portfolio_id, date, aum, shares, nav) VALUES
           (1, '2026-06-01', 6900, 69, 100),
           (1, '2026-06-10', 7400, 74, 100),
           (1, '2026-06-30', 7880, 74, 106.486486486486486),
           (1, '2026-07-15', 7880, 74, 106.486486486486486)",
    ).execute(&pool).await.unwrap();

    sqlx::query(
        "INSERT INTO operations (portfolio_id, trade_date, side, isin, name, currency, quantity, net_price, net_amount)
         VALUES (1, '2026-06-15', 'Achat', 'EQ1EUR', 'Alpha SE', 'EUR', 50, 12, -600)",
    ).execute(&pool).await.unwrap();

    sqlx::query(
        "INSERT INTO dividends (portfolio_id, provision_date, payment_date, issuer, amount, currency)
         VALUES (1, '2026-06-20', '2026-07-10', 'Beta Corp', 300, 'USD')",
    ).execute(&pool).await.unwrap();

    sqlx::query(
        "INSERT INTO fx_history (date, currency, rate_to_eur) VALUES ('2026-06-20', 'USD', 0.92)",
    ).execute(&pool).await.unwrap();

    // Period 1: subscription + settled buy + dividend accrual.
    let (status, body) = get_json(&app, "/api/portfolios/1/pnl?from=2026-06-01&to=2026-06-30").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["empty"], false, "{body}");
    assert_eq!(body["warnings"].as_array().unwrap().len(), 0, "{body}");
    let r = &body["reconciliation"];
    assert!((r["investment_pnl"].as_f64().unwrap() - 195.0).abs() < 1e-6, "{r}");
    assert!((r["cash_and_margin"].as_f64().unwrap() - 9.0).abs() < 1e-6, "{r}");
    assert!((r["accrued_fees"].as_f64().unwrap() - 0.0).abs() < 1e-9, "{r}");
    assert!((r["provisions"].as_f64().unwrap() - 0.0).abs() < 1e-9, "{r}");
    assert!((r["dividend_income"].as_f64().unwrap() - 276.0).abs() < 1e-6, "{r}");
    assert!((r["aum_change"].as_f64().unwrap() - 980.0).abs() < 1e-6, "{r}");
    assert!((r["net_flows"].as_f64().unwrap() - 500.0).abs() < 1e-6, "{r}");
    assert!(r["residual"].as_f64().unwrap().abs() < 0.01,
        "consistent books must reconcile to ~0, got {r}");
    assert_eq!(r["within_tolerance"], true, "{r}");

    // Period 2: the dividend's payment leg (receivable -> cash) must net to
    // zero, not resurface as P&L.
    let (status, body) = get_json(&app, "/api/portfolios/1/pnl?from=2026-06-30&to=2026-07-15").await;
    assert_eq!(status, StatusCode::OK);
    let r = &body["reconciliation"];
    assert!((r["investment_pnl"].as_f64().unwrap() - 0.0).abs() < 1e-6, "{r}");
    assert!((r["cash_and_margin"].as_f64().unwrap() - 0.0).abs() < 1e-6, "{r}");
    assert!((r["dividend_income"].as_f64().unwrap() - 0.0).abs() < 1e-9, "{r}");
    assert!(r["residual"].as_f64().unwrap().abs() < 0.01,
        "the payment leg of an already-recognised dividend is a transfer, got {r}");
    assert_eq!(r["within_tolerance"], true, "{r}");

    pool.close().await;
    edb.stop().await;
}

/// A snapshot legitimately lists the same ISIN twice: an equity and its
/// `Dividendes` receivable share the code. Indexing a snapshot one-row-per-
/// ISIN lets the receivable evict the instrument row, and the whole position
/// silently vanishes from the P&L. This is a real incident, not a
/// hypothetical: in the 2026-08-05 book a 481 EUR Kering receivable evicted
/// the 249,811 EUR equity row at t0 - the position had been fully sold in
/// the period, so its sale went missing from investment P&L and its 295,668
/// EUR of proceeds were never netted off the cash line, producing a
/// -231,446 EUR residual flagged above tolerance.
///
/// The fixture reproduces both shapes on consistent books (residual must
/// be ~0):
///   EQDUP  (Action, EUR)  t0 1,000; fully sold 08-15 for 1,100; absent t1.
///          Its receivable row (constant 50) is inserted AFTER it at t0, so
///          a one-row-per-ISIN index drops the equity exactly like Kering.
///   EQSHAD (Action, EUR)  t0 200 -> t1 260, held; receivable (constant 10)
///          inserted after it at BOTH dates - the ABN AMRO shape.
///   CASHEUR                5,000 -> 6,100 (the sale proceeds).
///   AUM    6,260 -> 6,420; no flows, no dividend accruals (receivables
///          constant, DIV sheet empty).
///
/// Correct wiring: investment_pnl = (0-1,000)+1,100 + (260-200) = 160,
/// cash line = 1,100 - 1,100 = 0, every receivable delta 0, residual 0.
#[tokio::test]
async fn duplicate_isin_receivable_rows_do_not_evict_the_instrument() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    let import_id: i64 = sqlx::query_scalar(
        "INSERT INTO imports (portfolio_id, filename, sha256, nav_date, row_counts)
         VALUES (1, 'dup.xlsx', 'dup-sha', '2026-08-20', '{}') RETURNING id",
    ).fetch_one(&pool).await.unwrap();

    // Insertion order is load-bearing: positions_for orders by id, so each
    // Dividendes row lands after its equity and wins any one-row-per-ISIN map.
    sqlx::query(
        "INSERT INTO position_snapshots
           (portfolio_id, nav_date, import_id, asset_type, isin, name, currency,
            quantity, price, valuation_ccy, fx_rate, valuation_eur)
         VALUES
           (1, '2026-08-10', $1, 'Action',     'EQDUP',   'Kering-like SA', 'EUR', 100,  10,   1000, NULL, 1000),
           (1, '2026-08-10', $1, 'Dividendes', 'EQDUP',   'Kering-like div','EUR', NULL, NULL, 50,   NULL, 50),
           (1, '2026-08-10', $1, 'Action',     'EQSHAD',  'ABN-like NV',    'EUR', 20,   10,   200,  NULL, 200),
           (1, '2026-08-10', $1, 'Dividendes', 'EQSHAD',  'ABN-like div',   'EUR', NULL, NULL, 10,   NULL, 10),
           (1, '2026-08-10', $1, 'Cash Acc',   'CASHEUR', 'Cash EUR',       'EUR', NULL, NULL, 5000, NULL, 5000),
           (1, '2026-08-20', $1, 'Action',     'EQSHAD',  'ABN-like NV',    'EUR', 20,   13,   260,  NULL, 260),
           (1, '2026-08-20', $1, 'Dividendes', 'EQSHAD',  'ABN-like div',   'EUR', NULL, NULL, 10,   NULL, 10),
           (1, '2026-08-20', $1, 'Dividendes', 'EQDUP',   'Kering-like div','EUR', NULL, NULL, 50,   NULL, 50),
           (1, '2026-08-20', $1, 'Cash Acc',   'CASHEUR', 'Cash EUR',       'EUR', NULL, NULL, 6100, NULL, 6100)",
    ).bind(import_id).execute(&pool).await.unwrap();

    sqlx::query(
        "INSERT INTO nav_history (portfolio_id, date, aum, shares, nav) VALUES
           (1, '2026-08-10', 6260, 62.6, 100),
           (1, '2026-08-20', 6420, 62.6, 102.55591054313099)",
    ).execute(&pool).await.unwrap();

    // The lifetime buy (before t0) seeds the cost basis; only the sale falls
    // inside the period.
    sqlx::query(
        "INSERT INTO operations (portfolio_id, trade_date, side, isin, name, currency, quantity, net_price, net_amount)
         VALUES (1, '2026-08-01', 'Achat', 'EQDUP', 'Kering-like SA', 'EUR', 100, 10, -1000),
                (1, '2026-08-15', 'Vente', 'EQDUP', 'Kering-like SA', 'EUR', -100, 11, 1100)",
    ).execute(&pool).await.unwrap();

    let (status, body) = get_json(&app, "/api/portfolios/1/pnl?from=2026-08-10&to=2026-08-20").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["empty"], false, "{body}");
    assert_eq!(body["warnings"].as_array().unwrap().len(), 0, "{body}");
    let r = &body["reconciliation"];
    assert!((r["investment_pnl"].as_f64().unwrap() - 160.0).abs() < 1e-6,
        "the sold equity's realized 100 and the shadowed equity's 60 must both be counted: {r}");
    assert!((r["cash_and_margin"].as_f64().unwrap() - 0.0).abs() < 1e-6,
        "the sale proceeds must be netted off the cash line: {r}");
    assert!((r["provisions"].as_f64().unwrap() - 0.0).abs() < 1e-9, "{r}");
    assert!(r["residual"].as_f64().unwrap().abs() < 0.01,
        "an evicted duplicate-ISIN row must not leak into the residual, got {r}");
    assert_eq!(r["within_tolerance"], true, "{r}");

    // Both equities must exist as instrument rows with the right components.
    let instruments: Vec<&serde_json::Value> = body["groups"].as_array().unwrap().iter()
        .flat_map(|g| g["instruments"].as_array().unwrap())
        .collect();
    let eqdup = instruments.iter().find(|i| i["isin"] == "EQDUP")
        .expect("the fully-sold equity must appear in instrument P&L");
    assert!((eqdup["realized_price"].as_f64().unwrap() - 100.0).abs() < 1e-6, "{eqdup}");
    let eqshad = instruments.iter().find(|i| i["isin"] == "EQSHAD")
        .expect("the shadowed equity must appear in instrument P&L");
    assert!((eqshad["unrealized_price"].as_f64().unwrap() - 60.0).abs() < 1e-6, "{eqshad}");

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

    // Every equity in the fixture has real trades across the whole
    // 2025-02-28..2026-07-24 window. Remove all of them so `walk_instrument`
    // sees an empty history everywhere: `decompose` then reduces to its
    // simplest form - no realized leg, no split ambiguity - price P&L is
    // exactly `(v1 - v0) * f0`, FX P&L is exactly `v1 * (f1 - f0)`, and the
    // trade-flow sum the cash line is netted of is exactly zero, keeping the
    // reconciliation figures below hand-derivable.
    sqlx::query("DELETE FROM operations").execute(&pool).await.unwrap();

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
    // this one, regardless of row insertion order (each still picks up
    // translation-FX noise `v1 * (f1 - f0)` from the perturbed f0, which is
    // why this test doesn't assert a portfolio-wide total).
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

    // Cash row BK001USD (USD): cash/margin rows bypass `decompose`
    // entirely - they're straight `valuation_eur` deltas. Real t1
    // valuation_eur = 902,830.24; set t0 to 850,000.00. Every other
    // cash/margin/fees/provisions/income row is an untouched clone
    // (e1 - e0 = 0 exactly), so this is the only nonzero contributor to
    // the raw cash delta: 902,830.24 - 850,000.00 = 52,830.24. The
    // `cash_and_margin` LINE nets that delta of trade flows (zero here -
    // operations were deleted above), external subscriptions (this
    // fixture's derived net_flows, asserted below) and dividend receipts
    // (income minus receivable movement) - see the assertion for the
    // arithmetic.
    sqlx::query(
        "UPDATE position_snapshots SET valuation_eur = 850000.0
         WHERE nav_date = $1 AND isin = 'BK001USD'",
    )
    .bind(t0).execute(&pool).await.unwrap();

    let (status, body) = get_json(&app, "/api/portfolios/1/pnl?from=2020-01-01&to=2030-01-01").await;
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
    // Fees/Provisions/Income rows were never touched, so every one of them
    // is a straight clone: their e1 - e0 is exactly zero.
    assert!((r["accrued_fees"].as_f64().unwrap() - 0.0).abs() < 1e-9, "{r}");
    assert!((r["provisions"].as_f64().unwrap() - 0.0).abs() < 1e-9, "{r}");
    // Dividend window is (t0, t1] = (2025-02-28, 2026-07-24], which is every
    // one of the fixture's 53 dividend rows - untouched by the
    // perturbations above, so this independently pins the window filter.
    // Amounts are converted at each currency's snapshot rate (fx_history is
    // empty in this fixture, so the provision-date lookup falls back to the
    // untouched t1 snapshot; DKK and SEK carry no fx_rate on any position
    // row, so their rate is the first row's valuation_eur/valuation_ccy):
    //   CHF:   5,504.51 x 1.07503763       =   5,917.555385
    //   DKK:   1,250.52 x 0.13376495204594 =     167.275748
    //   EUR: 153,114.34 x 1                = 153,114.340000
    //   GBP:  28,668.45 x 1.17123448       =  33,577.477128
    //   SEK:  25,617.00 x 0.09052641011727 =   2,319.015048
    //   USD:   6,969.53 x 0.87881185       =   6,124.905553
    //   total                              = 201,220.568862
    assert!((r["dividend_income"].as_f64().unwrap() - 201_220.568862).abs() < 1e-2, "{r}");
    // Net flows derived from the fixture's own nav_history over
    // (2025-02-28, 2026-07-24]: sum of (shares[i] - shares[i-1]) * nav[i]
    // across all 344 rows (shares 3,010 -> 271,342.492). Machine-derived
    // once from the fixture, stable, and pinned here because the cash line
    // below nets it out.
    assert!((r["net_flows"].as_f64().unwrap() - 27_446_800.194710).abs() < 1e-2, "{r}");
    // cash_and_margin = raw cash delta - trade flows - net flows
    //                 - (dividend income - dividend receivable delta)
    //   raw cash delta            =         52,830.24   (BK001USD, above)
    //   trade flows               =              0      (operations deleted)
    //   net flows                 =     27,446,800.194710
    //   dividend income           =        201,220.568862
    //   dividend receivable delta =              0      (untouched clones)
    //   =>  52,830.24 - 27,446,800.194710 - 201,220.568862
    //     = -27,595,190.523572
    // (Huge and negative because the cloned books really are inconsistent:
    // the subscriptions that AUM history records never landed in the cloned
    // cash rows. The residual, recomputed below, carries the same story.)
    assert!((r["cash_and_margin"].as_f64().unwrap() - (-27_595_190.523572)).abs() < 1e-2, "{r}");
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
