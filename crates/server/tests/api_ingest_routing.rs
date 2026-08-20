use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

const BOUNDARY: &str = "XBOUNDARYX";
const HISINV: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/caceis_hisinv.csv");
const HISTOVL: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/caceis_histovl.csv");
const JOURSR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/caceis_joursr.csv");

fn multi_upload_req(uri: &str, files: &[(&str, &[u8])]) -> Request<Body> {
    multi_upload_req_with_cookie(uri, files, None)
}

fn multi_upload_req_with_cookie(uri: &str, files: &[(&str, &[u8])], cookie: Option<&str>) -> Request<Body> {
    let mut body = Vec::new();
    for (filename, bytes) in files {
        body.extend_from_slice(format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        ).as_bytes());
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    let mut b = Request::post(uri)
        .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"));
    if let Some(c) = cookie { b = b.header("cookie", c); }
    b.body(Body::from(body)).unwrap()
}

async fn json_of(res: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

#[tokio::test]
async fn caceis_files_route_by_code_regardless_of_url_portfolio() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    let pool = dbh.test_pool().clone();
    let app = server::routes::router(server::state::AppState::desktop(dbh.clone()));

    let hisinv = std::fs::read(HISINV).unwrap();
    let histovl = std::fs::read(HISTOVL).unwrap();

    // Create a mandate and map the CACEIS fund code to it.
    let res = app.clone().oneshot(
        Request::post("/api/portfolios")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"Mandat CSV","kind":"mandate"}"#)).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let pid2 = json_of(res).await["id"].as_i64().unwrap();

    // Before mapping: upload reports an unknown code, writes nothing.
    let res = app.clone().oneshot(multi_upload_req(
        "/api/portfolios/1/imports",
        &[("HISINVLUX_165878_20260807_20260810130151.csv", &hisinv)],
    )).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_of(res).await;
    assert!(body[0]["error"].as_str().unwrap().contains("165878"), "{body}");
    assert!(body[0]["outcome"].is_null());

    // Map the code, re-upload BOTH files through portfolio 1's URL: they must
    // land in the mandate.
    let res = app.clone().oneshot(
        Request::put(format!("/api/portfolios/{pid2}/codes"))
            .header("content-type", "application/json")
            .body(Body::from(r#"[{"source":"caceis","code":"165878"}]"#)).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let res = app.clone().oneshot(multi_upload_req(
        "/api/portfolios/1/imports",
        &[
            ("HISINVLUX_165878_20260807_20260810130151.csv", &hisinv),
            ("HISTOVLLUX_165878_20260729_20260730170850.csv", &histovl),
        ],
    )).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_of(res).await;
    assert_eq!(body.as_array().unwrap().len(), 2);
    for item in body.as_array().unwrap() {
        assert_eq!(item["portfolio_id"].as_i64().unwrap(), pid2, "{item}");
        assert!(item["error"].is_null(), "{item}");
        assert!(item["outcome"]["import_id"].is_i64(), "{item}");
    }

    // The mandate has the snapshot and the NAV point; portfolio 1 has neither.
    let n2: i64 = sqlx::query_scalar("SELECT count(*) FROM position_snapshots WHERE portfolio_id = $1")
        .bind(pid2).fetch_one(&pool).await.unwrap();
    assert!(n2 > 0);
    let n1: i64 = sqlx::query_scalar("SELECT count(*) FROM position_snapshots WHERE portfolio_id = 1")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(n1, 0);
    let nav2: i64 = sqlx::query_scalar("SELECT count(*) FROM nav_history WHERE portfolio_id = $1")
        .bind(pid2).fetch_one(&pool).await.unwrap();
    assert_eq!(nav2, 1);

    // Dedupe is per portfolio: same file again -> duplicate outcome.
    let res = app.clone().oneshot(multi_upload_req(
        "/api/portfolios/1/imports",
        &[("HISINVLUX_165878_20260807_20260810130151.csv", &hisinv)],
    )).await.unwrap();
    let body = json_of(res).await;
    assert_eq!(body[0]["outcome"]["duplicate"], true, "{body}");

    // Archive the mandate: a routed file now fails per-file, request stays 200.
    let res = app.clone().oneshot(
        Request::put(format!("/api/portfolios/{pid2}"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"Mandat CSV","archived":true}"#)).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let res = app.clone().oneshot(multi_upload_req(
        "/api/portfolios/1/imports",
        &[("HISTOVLLUX_165878_20260729_20260730170850.csv", &histovl)],
    )).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_of(res).await;
    assert!(body[0]["error"].as_str().is_some(), "{body}");

    // Rejected families explain themselves.
    let res = app.clone().oneshot(multi_upload_req(
        "/api/portfolios/1/imports",
        &[("JOUROPLUX_165878_20260807_20260810130151.csv", b"x".as_slice())],
    )).await.unwrap();
    let body = json_of(res).await;
    assert!(body[0]["error"].as_str().unwrap().to_lowercase().contains("sample"), "{body}");

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn reglmtlux_is_recognized_and_declined_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    let pool = dbh.test_pool().clone();
    let app = server::routes::router(server::state::AppState::desktop(dbh.clone()));

    let two_lines = b"165878;20260807;line1\n165878;20260807;line2\n".as_slice();
    let res = app.clone().oneshot(multi_upload_req(
        "/api/portfolios/1/imports",
        &[("REGLMTLUX_165878_20260807_1.csv", two_lines)],
    )).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_of(res).await;
    let err = body[0]["error"].as_str().unwrap();
    assert!(err.contains("not consumed yet"), "{body}");
    assert!(err.contains("double-count"), "{body}");
    assert!(body[0]["outcome"].is_null());

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn joursrlux_routes_by_fund_code_like_hisinvlux() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    let pool = dbh.test_pool().clone();
    let app = server::routes::router(server::state::AppState::desktop(dbh.clone()));

    let joursr = std::fs::read(JOURSR).unwrap();

    // Create a mandate and map the CACEIS fund code to it, exactly as the
    // HISINVLUX routing test does.
    let res = app.clone().oneshot(
        Request::post("/api/portfolios")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"Mandat JOURSR","kind":"mandate"}"#)).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let pid2 = json_of(res).await["id"].as_i64().unwrap();

    let res = app.clone().oneshot(
        Request::put(format!("/api/portfolios/{pid2}/codes"))
            .header("content-type", "application/json")
            .body(Body::from(r#"[{"source":"caceis","code":"165878"}]"#)).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Uploaded through portfolio 1's URL, but the fund code routes it to the
    // mapped mandate.
    let res = app.clone().oneshot(multi_upload_req(
        "/api/portfolios/1/imports",
        &[("JOURSRLUX_165878_20260807_20260810130151.csv", &joursr)],
    )).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_of(res).await;
    assert!(body[0]["error"].is_null(), "{body}");
    assert_eq!(body[0]["portfolio_id"].as_i64().unwrap(), pid2, "{body}");
    assert!(body[0]["outcome"]["import_id"].is_i64(), "{body}");

    pool.close().await;
    edb.stop().await;
}

/// Task-9 review round 1, finding 1: a self-identifying (CACEIS) file can
/// resolve to a portfolio the uploading principal has no grant on at all.
/// Before the fix, `ensure()` ran before authorization, so its "portfolio
/// 'X' is archived" / existence errors could leak that portfolio's name and
/// archived state to a principal with zero visibility into it. This pins
/// the fixed order (authorize all three Import tokens against the resolved
/// target FIRST, only then read/report the row) end to end over the real
/// HTTP surface, with a server-mode (non-desktop) scoped principal — and
/// that a sibling file coded to the principal's own granted portfolio still
/// imports successfully in the same request.
#[tokio::test]
async fn caceis_file_coded_to_an_ungranted_portfolio_is_refused_without_leaking_its_identity() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    let pool = dbh.test_pool().clone();
    let app = server::routes::router(server::state::AppState::server(dbh.clone()));

    // Portfolio A (the principal's own, granted) and portfolio B (given a
    // deliberately distinctive name — the assertion below is that this
    // exact string never appears anywhere in the response), NOT granted to
    // the principal in any domain.
    let a_id: i64 = sqlx::query_scalar(
        "INSERT INTO portfolios (name, kind) VALUES ('Mine', 'mandate') RETURNING id")
        .fetch_one(&pool).await.unwrap();
    let b_id: i64 = sqlx::query_scalar(
        "INSERT INTO portfolios (name, kind) VALUES ('Confidential Fund B', 'mandate') RETURNING id")
        .fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO portfolio_codes (portfolio_id, source, code) VALUES ($1, 'caceis', '165878')")
        .bind(b_id).execute(&pool).await.unwrap();

    // A principal scoped to A's three import domains, plus a wildcard
    // Reference/View grant — but no grant of any kind, on any domain, for B.
    // The wildcard is incidental here (code resolution no longer needs it,
    // see `Scoped::portfolio_by_code`); it is kept because it makes the
    // assertion stronger: even an instance-wide reference reader learns
    // neither B's name nor whether it exists.
    let hash = server::auth::local::hash_password("pw").unwrap();
    let admin = db::admin::Admin::new(&pool);
    let uid = admin.create_user("scoped@f.lu", "Scoped", &hash, false).await.unwrap();
    for g in [
        db::auth::Grant { domain: db::auth::Domain::Positions, action: db::auth::Action::Import, portfolio: Some(a_id) },
        db::auth::Grant { domain: db::auth::Domain::Nav, action: db::auth::Action::Import, portfolio: Some(a_id) },
        db::auth::Grant { domain: db::auth::Domain::Transactions, action: db::auth::Action::Import, portfolio: Some(a_id) },
        db::auth::Grant { domain: db::auth::Domain::Reference, action: db::auth::Action::View, portfolio: None },
    ] {
        admin.grant_add(uid, g, None).await.unwrap();
    }
    let token = "scoped-token";
    admin.session_create(&server::auth::local::token_hash(token), uid, 1).await.unwrap();
    let cookie = format!("borobudur_session={token}");

    let hisinv = std::fs::read(HISINV).unwrap();
    let sample = std::fs::read(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx")).unwrap();

    // One file coded to B (out of scope), one non-identifying file that
    // lands at the URL portfolio A (granted) — uploaded through A's URL in
    // the same request.
    let req = multi_upload_req_with_cookie(
        &format!("/api/portfolios/{a_id}/imports"),
        &[
            ("HISINVLUX_165878_20260807_20260810130151.csv", &hisinv),
            ("sample.xlsx", &sample),
        ],
        Some(&cookie),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_of(res).await;

    // B's name must not appear anywhere in the response body — not in the
    // refused file's error message, not in any other field.
    assert!(!body.to_string().contains("Confidential Fund B"), "{body}");

    let items = body.as_array().unwrap();
    let hisinv_result = items.iter()
        .find(|i| i["filename"] == "HISINVLUX_165878_20260807_20260810130151.csv")
        .unwrap();
    assert!(hisinv_result["error"].as_str().is_some(), "expected a per-file refusal: {hisinv_result}");
    assert!(hisinv_result["portfolio_id"].is_null(),
        "an out-of-scope target's id must not be surfaced either: {hisinv_result}");
    assert!(hisinv_result["portfolio_name"].is_null(), "{hisinv_result}");
    assert!(hisinv_result["outcome"].is_null(), "{hisinv_result}");

    // The sibling file (no fund code -> lands at the URL portfolio A, which
    // this principal IS granted on) must still import successfully — a
    // denial on one file's resolved target does not abort the batch.
    let sample_result = items.iter().find(|i| i["filename"] == "sample.xlsx").unwrap();
    assert!(sample_result["error"].is_null(), "{sample_result}");
    assert_eq!(sample_result["portfolio_id"].as_i64().unwrap(), a_id, "{sample_result}");
    assert!(sample_result["outcome"]["import_id"].is_i64(), "{sample_result}");

    pool.close().await;
    edb.stop().await;
}

/// Finding P4: the Operations role exists for the people who load the weekly
/// files, and is normally granted with a portfolio scope. Routing a
/// self-identifying CACEIS file used to demand an *instance-wide*
/// Reference/View grant to resolve its fund code, so a scoped Operations
/// principal was refused on the very feed the role exists for — on a fund it
/// holds every import grant over.
#[tokio::test]
async fn a_scoped_operations_principal_can_import_a_caceis_file_for_its_own_portfolio() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    let pool = dbh.test_pool().clone();
    let app = server::routes::router(server::state::AppState::server(dbh.clone()));

    let a_id: i64 = sqlx::query_scalar(
        "INSERT INTO portfolios (name, kind) VALUES ('Mandat Ops', 'mandate') RETURNING id")
        .fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO portfolio_codes (portfolio_id, source, code) VALUES ($1, 'caceis', '165878')")
        .bind(a_id).execute(&pool).await.unwrap();

    // Exactly what `Role::Operations` expands to at scope A — every grant
    // scoped to the portfolio, nothing instance-wide.
    let hash = server::auth::local::hash_password("pw").unwrap();
    let admin = db::admin::Admin::new(&pool);
    let uid = admin.create_user("ops@f.lu", "Ops", &hash, false).await.unwrap();
    for g in db::auth::Role::Operations.expand(Some(a_id)) {
        admin.grant_add(uid, g, None).await.unwrap();
    }
    let token = "ops-token";
    admin.session_create(&server::auth::local::token_hash(token), uid, 1).await.unwrap();
    let cookie = format!("borobudur_session={token}");

    let histovl = std::fs::read(HISTOVL).unwrap();
    let res = app.clone().oneshot(multi_upload_req_with_cookie(
        &format!("/api/portfolios/{a_id}/imports"),
        &[("HISTOVLLUX_165878_20260729_20260730170850.csv", &histovl)],
        Some(&cookie),
    )).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_of(res).await;
    assert!(body[0]["error"].is_null(), "scoped Operations must be able to load its own fund's feed: {body}");
    assert_eq!(body[0]["portfolio_id"].as_i64().unwrap(), a_id, "{body}");
    assert!(body[0]["outcome"]["nav_rows"].as_i64().unwrap() > 0, "{body}");

    pool.close().await;
    edb.stop().await;
}
