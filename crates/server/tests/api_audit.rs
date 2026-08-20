//! Task 12: mutating/exporting handlers must write one audit row per action;
//! read paths must write none. Helpers copied from `api_authz_slice.rs` /
//! `api_partial_denial.rs` — this crate has no shared `tests/common` module
//! and every `api_*.rs` file inlines its own setup.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use db::auth::{Action, Domain, Grant};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

const SAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx");
const BOUNDARY: &str = "XBOUNDARYX";

async fn app() -> (axum::Router, axum::Router, sqlx::PgPool, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    let pool = dbh.test_pool().clone();
    std::mem::forget(dir);
    let desktop = server::routes::router(server::state::AppState::desktop(dbh.clone()));
    let server = server::routes::router(server::state::AppState::server(dbh.clone()));
    (desktop, server, pool, edb)
}

static NEXT_USER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

async fn user_with(pool: &sqlx::PgPool, grants: &[Grant]) -> (String, i64) {
    let n = NEXT_USER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let hash = server::auth::local::hash_password("pw").unwrap();
    let admin = db::admin::Admin::new(pool);
    let id = admin.create_user(&format!("u{n}@f.lu"), "U", &hash, false).await.unwrap();
    for g in grants {
        admin.grant_add(id, *g, None).await.unwrap();
    }
    let token = format!("t{n}");
    admin.session_create(&server::auth::local::token_hash(&token), id, 1).await.unwrap();
    (format!("borobudur_session={token}"), id)
}

async fn portfolio(pool: &sqlx::PgPool, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO portfolios (name, kind) VALUES ($1,'ucits') RETURNING id")
        .bind(name).fetch_one(pool).await.unwrap()
}

fn upload_req(uri: &str, bytes: &[u8]) -> Request<Body> {
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

fn upload_req_with_cookie(uri: &str, bytes: &[u8], cookie: &str) -> Request<Body> {
    let mut req = upload_req(uri, bytes);
    req.headers_mut().insert("cookie", cookie.parse().unwrap());
    req
}

async fn get(app: &axum::Router, uri: &str, cookie: Option<&str>) -> StatusCode {
    let mut b = Request::get(uri);
    if let Some(c) = cookie { b = b.header("cookie", c); }
    app.clone().oneshot(b.body(Body::empty()).unwrap()).await.unwrap().status()
}

async fn get_json(app: &axum::Router, uri: &str, cookie: Option<&str>) -> (StatusCode, serde_json::Value) {
    let mut b = Request::get(uri);
    if let Some(c) = cookie { b = b.header("cookie", c); }
    let res = app.clone().oneshot(b.body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let v = if bytes.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
    (status, v)
}

/// Seed `pid` with the sample workbook through the desktop (unrestricted)
/// router, so the restricted-grant router used for assertions never needs
/// import-level grants at all.
async fn seed(desktop: &axum::Router, pid: i64) {
    let bytes = std::fs::read(SAMPLE).unwrap();
    let res = desktop.clone().oneshot(upload_req(&format!("/api/portfolios/{pid}/imports"), &bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK, "seed import failed");
}

async fn audit_rows(pool: &sqlx::PgPool) -> Vec<db::admin::AuditRow> {
    db::admin::Admin::new(pool).audit_recent(100).await.unwrap()
}

#[tokio::test]
async fn an_export_writes_one_audit_row() {
    let (desktop, server, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    seed(&desktop, pid).await;

    // The evidence export refuses outright rather than emit a degraded
    // document, so it needs every read behind it: the contract specs
    // (Reference, fleet-wide) and the monthly KPI records (Settings, which
    // took the per-portfolio half in the P10 split).
    let (cookie, _) = user_with(&pool, &[
        Grant { domain: Domain::Positions, action: Action::Export, portfolio: Some(pid) },
        Grant { domain: Domain::Reference, action: Action::View, portfolio: None },
        Grant { domain: Domain::Settings, action: Action::View, portfolio: Some(pid) },
    ]).await;

    let status = get(&server, &format!("/api/portfolios/{pid}/emir/export"), Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);

    let rows = audit_rows(&pool).await;
    let exports: Vec<_> = rows.iter().filter(|r| r.action == "export").collect();
    assert_eq!(exports.len(), 1, "{rows:?}");
    assert_eq!(exports[0].portfolio_id, Some(pid));

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn a_view_writes_nothing() {
    let (desktop, server, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    // `seed` imports through the desktop router, which itself writes an
    // "import" audit row (correctly — that IS a mutating action). This test
    // is about the read below, so the baseline is taken after seeding.
    seed(&desktop, pid).await;
    let before = audit_rows(&pool).await.len();

    let (cookie, _) = user_with(&pool, &[
        Grant { domain: Domain::Nav, action: Action::View, portfolio: Some(pid) },
    ]).await;

    let status = get(&server, &format!("/api/portfolios/{pid}/nav"), Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);

    let rows = audit_rows(&pool).await;
    assert_eq!(rows.len(), before, "a read must never write an audit row: {rows:?}");

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn a_settings_change_records_before_and_after() {
    let (_desktop, server, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;

    let (cookie, _) = user_with(&pool, &[
        Grant { domain: Domain::Settings, action: Action::Configure, portfolio: Some(pid) },
    ]).await;

    let (_, before) = get_json(&server, &format!("/api/portfolios/{pid}/settings"), Some(&cookie)).await;
    let mut new_settings = before.clone();
    let new_var_limit = before["var_limit"].as_f64().unwrap() / 2.0;
    new_settings["var_limit"] = serde_json::json!(new_var_limit);

    let req = Request::put(format!("/api/portfolios/{pid}/settings"))
        .header("cookie", &cookie)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&new_settings).unwrap()))
        .unwrap();
    let res = server.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let rows = audit_rows(&pool).await;
    let configures: Vec<_> = rows.iter().filter(|r| r.action == "configure").collect();
    assert_eq!(configures.len(), 1, "{rows:?}");
    let detail = &configures[0].detail;
    assert_ne!(detail["before"]["var_limit"], detail["after"]["var_limit"], "{detail}");
    assert_eq!(detail["after"]["var_limit"].as_f64().unwrap(), new_var_limit);

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn an_import_writes_a_row_tied_to_the_import_ledger() {
    let (_desktop, server, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;

    let (cookie, _) = user_with(&pool, &[
        Grant { domain: Domain::Positions, action: Action::Import, portfolio: Some(pid) },
        Grant { domain: Domain::Nav, action: Action::Import, portfolio: Some(pid) },
        Grant { domain: Domain::Transactions, action: Action::Import, portfolio: Some(pid) },
    ]).await;

    let bytes = std::fs::read(SAMPLE).unwrap();
    let res = server.clone().oneshot(upload_req_with_cookie(
        &format!("/api/portfolios/{pid}/imports"), &bytes, &cookie)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let rows = audit_rows(&pool).await;
    let imports: Vec<_> = rows.iter().filter(|r| r.action == "import").collect();
    assert_eq!(imports.len(), 1, "{rows:?}");
    assert!(imports[0].detail["import_id"].is_number(), "{:?}", imports[0].detail);

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn a_grant_change_records_who_granted_it() {
    let (_desktop, server, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;

    let admin_hash = server::auth::local::hash_password("pw").unwrap();
    let admin_row = db::admin::Admin::new(&pool);
    let admin_id = admin_row.create_user("admin-audit@f.lu", "Admin", &admin_hash, true).await.unwrap();
    let admin_token = "admin-t0";
    admin_row.session_create(&server::auth::local::token_hash(admin_token), admin_id, 1).await.unwrap();
    let admin_cookie = format!("borobudur_session={admin_token}");

    let (_, target_id) = user_with(&pool, &[]).await;

    let req = Request::post(format!("/api/admin/users/{target_id}/grants"))
        .header("cookie", &admin_cookie)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&serde_json::json!({
            "domain": "nav", "action": "view", "portfolio": pid,
        })).unwrap()))
        .unwrap();
    let res = server.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let rows = audit_rows(&pool).await;
    let grant_rows: Vec<_> = rows.iter().filter(|r| r.action == "grant_added").collect();
    assert_eq!(grant_rows.len(), 1, "{rows:?}");
    let detail = &grant_rows[0].detail;
    assert_eq!(detail["domain"], "nav", "{detail}");
    assert_eq!(detail["action"], "view", "{detail}");
    assert_eq!(detail["target_user_id"].as_i64().unwrap(), target_id, "{detail}");

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn login_success_failure_and_lockout_are_all_recorded() {
    let (_desktop, server, pool, edb) = app().await;
    let hash = server::auth::local::hash_password("correct-horse").unwrap();
    let admin = db::admin::Admin::new(&pool);
    admin.create_user("audit-login@f.lu", "Auditee", &hash, false).await.unwrap();

    let login = |password: &'static str| {
        let server = server.clone();
        async move {
            let req = Request::post("/api/login")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&serde_json::json!({
                    "email": "audit-login@f.lu", "password": password,
                })).unwrap()))
                .unwrap();
            server.oneshot(req).await.unwrap().status()
        }
    };

    // One wrong attempt.
    assert_eq!(login("wrong").await, StatusCode::UNAUTHORIZED);
    // One right attempt — resets the failure counter.
    assert_eq!(login("correct-horse").await, StatusCode::OK);
    // Five more wrong attempts: the first four fail normally, the fifth
    // crosses the lockout threshold (LOCK_AFTER = 5).
    for _ in 0..4 {
        assert_eq!(login("wrong").await, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(login("wrong").await, StatusCode::TOO_MANY_REQUESTS);

    let rows = audit_rows(&pool).await;
    let count = |action: &str| rows.iter().filter(|r| r.action == action).count();
    assert_eq!(count("login_failed"), 5, "{rows:?}");
    assert_eq!(count("login"), 1, "{rows:?}");
    assert_eq!(count("login_locked"), 1, "{rows:?}");

    pool.close().await;
    edb.stop().await;
}

/// Finding P7: `audit::record` hardcoded `source_addr: None`, so every row in
/// the production audit log had a null origin — including `login_failed` and
/// `login_locked`, the two rows where "from where?" is the first question
/// asked. Server mode is documented as sitting behind a TLS terminator, so
/// the forwarded header is the address that means anything.
#[tokio::test]
async fn auth_events_record_the_client_address_from_the_proxy() {
    let (_desktop, server, pool, edb) = app().await;
    let hash = server::auth::local::hash_password("pw").unwrap();
    let admin = db::admin::Admin::new(&pool);
    admin.create_user("addr@f.lu", "Addr", &hash, false).await.unwrap();

    let login = |body: &'static str, xff: &'static str| {
        let app = server.clone();
        async move {
            app.oneshot(
                Request::post("/api/login")
                    .header("content-type", "application/json")
                    .header("x-forwarded-for", xff)
                    .body(Body::from(body)).unwrap()
            ).await.unwrap().status()
        }
    };

    assert_eq!(login(r#"{"email":"addr@f.lu","password":"pw"}"#, "203.0.113.9").await, StatusCode::OK);
    assert_eq!(login(r#"{"email":"addr@f.lu","password":"nope"}"#, "203.0.113.7, 10.0.0.1").await,
               StatusCode::UNAUTHORIZED);

    let rows = admin.audit_recent(50).await.unwrap();
    let ok = rows.iter().find(|r| r.action == "login").expect("a login row");
    assert_eq!(ok.source_addr.as_deref(), Some("203.0.113.9"), "{:?}", ok);

    // Only the left-most hop is the client; everything after it is proxy
    // chatter the client itself can forge just as easily.
    let failed = rows.iter().find(|r| r.action == "login_failed").expect("a login_failed row");
    assert_eq!(failed.source_addr.as_deref(), Some("203.0.113.7"), "{:?}", failed);

    pool.close().await;
    edb.stop().await;
}
