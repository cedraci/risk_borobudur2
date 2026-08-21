//! The read side of the breach register: `GET .../limit-runs`,
//! `GET .../breaches` and `GET .../breaches/{bid}`. Helpers copied from
//! `api_breach_recorder.rs` and `api_authz_matrix.rs` — this crate has no
//! shared `tests/common` module and every `api_*.rs` file inlines its own
//! setup.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use db::auth::marker::{Settings, View};
use db::auth::{Action, AuthCtx, Domain, Grant};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

const SAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx");
const BOUNDARY: &str = "XBOUNDARYX";

async fn app() -> (axum::Router, sqlx::PgPool, db::Db, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    let pool = dbh.test_pool().clone();
    std::mem::forget(dir);
    let desktop = server::routes::router(server::state::AppState::desktop(dbh.clone()));
    (desktop, pool, dbh, edb)
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
        .body(Body::from(body)).unwrap()
}

#[tokio::test]
async fn the_register_lists_runs_and_open_episodes() {
    let (desktop, pool, _dbh, edb) = app().await;
    let bytes = std::fs::read(SAMPLE).unwrap();
    let res = desktop.clone().oneshot(upload_req("/api/portfolios/1/imports", &bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let res = desktop.clone().oneshot(
        Request::get("/api/portfolios/1/limit-runs").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let runs = body["runs"].as_array().unwrap();
    assert!(!runs.is_empty());
    assert!(runs[0]["results"].as_array().unwrap().iter()
        .any(|r| r["check_key"] == "issuer_10"));

    let res = desktop.clone().oneshot(
        Request::get("/api/portfolios/1/breaches").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    pool.close().await;
    edb.stop().await;
}

/// `GET /api/portfolios/{pid}/breaches/{bid}` is deliberately excluded from
/// `api_authz_matrix.rs`'s `CASES`: that matrix's exact-grant case asserts a
/// 404-never response, and the matrix seeds no breach episodes, so any
/// concrete `bid` there would 404 for the right (no-such-episode) reason and
/// fail the test for the wrong one. This test pins the same four-way
/// authorization contract (no cookie / wrong domain / wrong portfolio /
/// correct grant) directly against a real episode instead.
#[tokio::test]
async fn episode_route_authorization() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    let pool = dbh.test_pool().clone();
    std::mem::forget(dir);
    let desktop = server::routes::router(server::state::AppState::desktop(dbh.clone()));
    let server_app = server::routes::router(server::state::AppState::server(dbh.clone()));

    // Portfolio 1 exists from the seed migration; a second portfolio to prove
    // a grant on the wrong fund is out of scope, not merely wrong-domain.
    let other_pid: i64 = sqlx::query_scalar(
        "INSERT INTO portfolios (name, kind) VALUES ($1,'ucits') RETURNING id")
        .bind("Other").fetch_one(&pool).await.unwrap();

    // Import through the desktop router (no auth) so a real run exists, then
    // insert a breach episode by hand: no fixture in this crate opens one
    // (sample.xlsx stays under every limit — see api_breach_recorder.rs), and
    // the design brief prefers a real episode over a synthetic route stub, so
    // this pins the episode to a real `limit_check_runs` row rather than an
    // orphaned foreign key.
    let bytes = std::fs::read(SAMPLE).unwrap();
    let res = desktop.clone().oneshot(upload_req("/api/portfolios/1/imports", &bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let view = scoped.authorize::<Settings, View>(1).unwrap();
    let (run, _results) = &scoped.runs_for(&view, 1).await.unwrap()[0];

    let bid: i64 = sqlx::query_scalar(
        "INSERT INTO limit_breaches
             (portfolio_id, check_key, subject, opened_run_id, opened_nav_date, opened_value, peak_value, peak_nav_date)
         VALUES (1, 'issuer_10', 'TEST ISSUER', $1, $2, 0.15, 0.15, $2)
         RETURNING id")
        .bind(run.id).bind(run.nav_date)
        .fetch_one(&pool).await.unwrap();

    let uri = format!("/api/portfolios/1/breaches/{bid}");

    // 1. No cookie -> 401, same contract as every other protected route.
    let res = server_app.clone().oneshot(Request::get(&uri).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    let admin = db::admin::Admin::new(&pool);
    let hash = server::auth::local::hash_password("pw").unwrap();

    // 2. A grant on a different domain, same portfolio -> 403.
    let wrong_domain_uid = admin.create_user("wrongdomain@f.lu", "U", &hash, false).await.unwrap();
    admin.grant_add(wrong_domain_uid, Grant { domain: Domain::Nav, action: Action::View, portfolio: Some(1) }, None).await.unwrap();
    admin.session_create(&server::auth::local::token_hash("wd-t"), wrong_domain_uid, 1).await.unwrap();
    let res = server_app.clone().oneshot(
        Request::get(&uri).header("cookie", "borobudur_session=wd-t").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    // 3. The right domain/action, but on a different portfolio -> 404
    // (out-of-scope non-disclosure, not a permission error).
    let wrong_pid_uid = admin.create_user("wrongpid@f.lu", "U", &hash, false).await.unwrap();
    admin.grant_add(wrong_pid_uid, Grant { domain: Domain::Settings, action: Action::View, portfolio: Some(other_pid) }, None).await.unwrap();
    admin.session_create(&server::auth::local::token_hash("wp-t"), wrong_pid_uid, 1).await.unwrap();
    let res = server_app.clone().oneshot(
        Request::get(&uri).header("cookie", "borobudur_session=wp-t").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    // 4. The exact grant on the right portfolio reaches the handler and
    // returns the episode.
    let ok_uid = admin.create_user("ok@f.lu", "U", &hash, false).await.unwrap();
    admin.grant_add(ok_uid, Grant { domain: Domain::Settings, action: Action::View, portfolio: Some(1) }, None).await.unwrap();
    admin.session_create(&server::auth::local::token_hash("ok-t"), ok_uid, 1).await.unwrap();
    let res = server_app.clone().oneshot(
        Request::get(&uri).header("cookie", "borobudur_session=ok-t").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["breach"]["id"], bid);
    assert!(body["events"].as_array().is_some());

    pool.close().await;
    edb.stop().await;
}
