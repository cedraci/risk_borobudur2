use axum::body::Body;
use axum::http::{Request, StatusCode};
use calamine::{DataType, Reader};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

const BOUNDARY: &str = "XBOUNDARYX";
const HISINV: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/caceis_hisinv.csv");
const SAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx");

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

async fn get_json(app: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let res = app.clone().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    (status, body)
}

async fn post_multipart_json(app: &axum::Router, uri: &str, filename: &str, bytes: &[u8]) -> serde_json::Value {
    let res = app.clone().oneshot(upload_req(uri, filename, bytes)).await.unwrap();
    serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

/// Read the ADV sheet's column A (isin) from a generated request workbook,
/// mirroring the pattern `crates/ingest/tests/bloomberg.rs` already uses for
/// the REFS sheet.
fn adv_sheet_isins(bytes: &[u8]) -> Vec<String> {
    let mut wb: calamine::Xlsx<_> =
        calamine::Xlsx::new(std::io::Cursor::new(bytes.to_vec())).expect("valid xlsx");
    let range = wb.worksheet_range("ADV").expect("ADV sheet present");
    range.rows().skip(1)
        .filter_map(|r| r.first().and_then(|c| c.get_string()).map(str::to_string))
        .collect()
}

async fn create_mandate(app: &axum::Router, name: &str) -> i64 {
    let res = app.clone().oneshot(
        Request::post("/api/portfolios")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::json!({"name": name, "kind": "mandate"}).to_string()))
            .unwrap(),
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    body["id"].as_i64().unwrap()
}

/// Fetch a portfolio's current settings and PUT them back with
/// `adv_max_age_days` overridden — mirrors the GET-then-PUT pattern in
/// `api_settings.rs`.
async fn set_adv_max_age_days(app: &axum::Router, pid: i64, days: u32) {
    let (status, mut s) = get_json(app, &format!("/api/portfolios/{pid}/settings")).await;
    assert_eq!(status, StatusCode::OK, "{s}");
    s["adv_max_age_days"] = serde_json::json!(days);
    let res = app.clone().oneshot(
        Request::put(format!("/api/portfolios/{pid}/settings"))
            .header("content-type", "application/json")
            .body(Body::from(s.to_string())).unwrap(),
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK, "settings PUT failed for portfolio {pid}");
}

/// Fresh embedded database with the CACEIS HISINVLUX fixture imported into a
/// mapped mandate, so `instrument_refs.market_place` is populated (Task 6)
/// for the venue rule `adv_scope`/`analytics::adv_eligible` relies on.
/// CACEIS files route by fund code regardless of the URL portfolio (Task 4/5
/// multi-file routing), so the mandate must exist and be code-mapped first —
/// mirrors the setup in `api_ingest_routing.rs`.
async fn app_with_hisinv() -> (axum::Router, sqlx::PgPool, db::embedded::EmbeddedDb, i64) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });

    let res = app.clone().oneshot(
        Request::post("/api/portfolios")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"Mandat ADV","kind":"mandate"}"#)).unwrap(),
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let pid = body["id"].as_i64().unwrap();

    let res = app.clone().oneshot(
        Request::put(format!("/api/portfolios/{pid}/codes"))
            .header("content-type", "application/json")
            .body(Body::from(r#"[{"source":"caceis","code":"165878"}]"#)).unwrap(),
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let hisinv = std::fs::read(HISINV).unwrap();
    let res = app.clone().oneshot(upload_req(
        "/api/portfolios/1/imports",
        "HISINVLUX_165878_20260807_20260810130151.csv",
        &hisinv,
    )).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(body[0]["error"].is_null(), "hisinv import failed: {body}");

    (app, pool, edb, pid)
}

#[tokio::test]
async fn adv_request_is_scoped_to_listed_instruments_that_are_due() {
    let (app, pool, edb, _pid) = app_with_hisinv().await;

    let (status, b) = get_json(&app, "/api/bloomberg/adv-due").await;
    assert_eq!(status, StatusCode::OK);
    let due = b["due"].as_u64().unwrap();
    let held = b["held"].as_u64().unwrap();
    assert!(due > 0 && due <= held, "you see the cost before you pay it: {due} of {held}");

    let res = app.clone().oneshot(
        Request::get("/api/bloomberg/adv-request").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let isins = adv_sheet_isins(&bytes);

    // The listed ETF is in; the unlisted target fund, the futures and the
    // cash accounts are not.
    assert!(isins.iter().any(|i| i == "AT000000STR1"), "a listed equity is requested: {isins:?}");
    assert!(!isins.iter().any(|i| i == "FR0010599399"), "an unlisted target fund is not: {isins:?}");
    // The gold ETC: venue 046 (Mercato Continuo), classified Obligation by
    // the depositary's asset-type rule. The single strongest test of the
    // venue rule — the old asset_type == "Action" rule would have excluded
    // it, and the venue rule exists precisely to admit exchange-traded
    // instruments whose asset type is not Action.
    assert!(isins.iter().any(|i| i == "DE000A1EK0G3"),
        "the gold ETC, listed but not an Action, is still requested: {isins:?}");
    // The fixture's two futures rows (CAC40 and EUR/JPY, both FUTU) carry
    // instrument codes, not ISINs: CFIN2608 and RYCU2609 — never "FVS"
    // prefixed, so the previous assertion (`!starts_with("FVS")`) passed
    // unconditionally and guarded nothing. Assert against the codes that
    // actually appear. Note this fixture's futures both carry market_place
    // "FOR" (forced price), which is itself in NON_MARKET_CODES, so their
    // exclusion here is doubly covered by the venue rule; the row that
    // isolates `adv_eligible`'s *unconditional* asset_type == "Future"
    // exclusion (ahead of, not instead of, the venue rule) is the analytics
    // unit test in crates/analytics/src/liquidity.rs (a future with
    // `adv_eligible` force-overridden to `Some(true)` is still excluded).
    // This assertion still matters at the handler/workbook layer: it is the
    // only check confirming a Future position never makes it into the
    // exported ADV sheet at all, regardless of which rule excludes it.
    assert!(!isins.iter().any(|i| i == "CFIN2608"), "the CAC40 future is never requested: {isins:?}");
    assert!(!isins.iter().any(|i| i == "RYCU2609"), "the EUR/JPY future is never requested: {isins:?}");

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn a_fresh_adv_drops_out_until_it_goes_stale() {
    let (app, pool, edb, _pid) = app_with_hisinv().await;

    let (_, before) = get_json(&app, "/api/bloomberg/adv-due").await;
    let due_before = before["due"].as_u64().unwrap();
    let held_before = before["held"].as_u64().unwrap();
    assert!(due_before > 0, "nothing due to begin with: {before}");

    // Upload an ADV response resolving one instrument's volume — the upload
    // path accepts a workbook carrying only an ADV sheet.
    let mut wb = rust_xlsxwriter::Workbook::new();
    let s = wb.add_worksheet().set_name("ADV").unwrap();
    for (c, h) in ["isin", "adv_30d", "market_sector"].iter().enumerate() {
        s.write_string(0, c as u16, *h).unwrap();
    }
    s.write_string(1, 0, "AT000000STR1").unwrap();
    s.write_number(1, 1, 125_000.0).unwrap();
    s.write_string(1, 2, "Equity").unwrap();
    let bytes = wb.save_to_buffer().unwrap();

    let body = post_multipart_json(&app, "/api/bloomberg/upload", "adv_resp.xlsx", &bytes).await;
    assert_eq!(body["adv_rows"], 1, "{body}");

    let (_, after) = get_json(&app, "/api/bloomberg/adv-due").await;
    let due_after = after["due"].as_u64().unwrap();
    let held_after = after["held"].as_u64().unwrap();
    assert_eq!(due_after, due_before - 1,
        "a freshly-fetched instrument must drop out of the due count: before {before} after {after}");
    assert_eq!(held_after, held_before,
        "the held set (everything eligible) is unaffected by staleness: before {before} after {after}");

    // ?all=true still includes it: it stays held, only staleness changed.
    let res = app.clone().oneshot(
        Request::get("/api/bloomberg/adv-request?all=true").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let resp_bytes = res.into_body().collect().await.unwrap().to_bytes();
    let isins = adv_sheet_isins(&resp_bytes);
    assert!(isins.iter().any(|i| i == "AT000000STR1"),
        "?all=true serves the full held set regardless of staleness: {isins:?}");

    pool.close().await;
    edb.stop().await;
}

/// Regression test for the fix to `adv_scope`: staleness must be decided per
/// `(portfolio, ISIN)` pair and then unioned, not decided once by whichever
/// portfolio happens to be walked first (`portfolios_list` orders by id —
/// deterministic but arbitrary) and then locked in by the fleet-wide dedup.
///
/// Portfolio 1 (walked first, id 1) holds the instrument with a lax 30-day
/// threshold; portfolio 2 (walked second) holds the SAME instrument with a
/// strict 7-day one. The instrument was "fetched" 20 days before the latest
/// snapshot date — fresh by 30 days, stale by 7. Before the fix, portfolio 1
/// would judge it fresh, `seen.insert` would mark the ISIN handled, and
/// portfolio 2's stricter verdict would never be reached. After the fix, the
/// instrument must still appear in `due`.
#[tokio::test]
async fn a_stricter_portfolios_threshold_is_not_overridden_by_a_looser_one_walked_first() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let app = server::routes::router(server::state::AppState { pool: pool.clone() });
    let bytes = std::fs::read(SAMPLE).unwrap();

    // Portfolio 1 (pre-existing, id 1) imports the sample workbook.
    let res = app.clone().oneshot(upload_req("/api/portfolios/1/imports", "s.xlsx", &bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(body[0]["error"].is_null(), "portfolio 1 import failed: {body}");

    // Portfolio 2, a fresh mandate, imports the SAME bytes independently.
    // `same_file_imports_independently_per_portfolio` in
    // api_portfolio_isolation.rs confirms this yields the same instrument
    // set (isin/asset_type/...) in both portfolios, since none of that is
    // portfolio-scoped.
    let pid2 = create_mandate(&app, "Mandat Strict").await;
    assert_eq!(pid2, 2, "walked after portfolio 1 in portfolios_list's ORDER BY id");
    let res = app.clone().oneshot(upload_req(&format!("/api/portfolios/{pid2}/imports"), "s.xlsx", &bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(body[0]["error"].is_null(), "portfolio 2 import failed: {body}");

    set_adv_max_age_days(&app, 1, 30).await;
    set_adv_max_age_days(&app, pid2, 7).await;

    // Both portfolios' latest snapshot is the same date (same source file).
    let (status, pos1) = get_json(&app, "/api/portfolios/1/positions").await;
    assert_eq!(status, StatusCode::OK);
    let latest: chrono::NaiveDate = pos1["date"].as_str().unwrap().parse().unwrap();

    // FR0000121014 is the sample's equity (an Action, market_place NULL —
    // the NAV Recap adapter carries no venue column, so `adv_eligible` falls
    // back to the pre-v2 asset_type == "Action" rule; either way it is
    // eligible). No instrument_refs row exists for it yet — the NAV Recap
    // adapter never emits ref_hints/ref_facts — so this seeds one directly
    // with an as-of 20 days before the latest snapshot.
    let stale_asof = latest - chrono::Duration::days(20);
    sqlx::query(
        "INSERT INTO instrument_refs (code, adv_asof) VALUES ($1, $2)
         ON CONFLICT (code) DO UPDATE SET adv_asof = EXCLUDED.adv_asof",
    )
    .bind("FR0000121014")
    .bind(stale_asof)
    .execute(&pool)
    .await
    .unwrap();

    let res = app.clone().oneshot(
        Request::get("/api/bloomberg/adv-request").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let resp_bytes = res.into_body().collect().await.unwrap().to_bytes();
    let isins = adv_sheet_isins(&resp_bytes);
    assert!(isins.iter().any(|i| i == "FR0000121014"),
        "portfolio 2's stricter 7-day threshold must still flag the instrument stale, \
         even though portfolio 1 (walked first, 30-day threshold, fresh by 20 <= 30) \
         considers it fresh: {isins:?}");

    pool.close().await;
    edb.stop().await;
}
