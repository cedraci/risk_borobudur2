use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

async fn server_app() -> (axum::Router, sqlx::PgPool, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    let pool = dbh.test_pool().clone();
    std::mem::forget(dir);
    let app = server::routes::router(server::state::AppState::server(dbh.clone()));
    (app, pool, edb)
}

async fn seed_user(pool: &sqlx::PgPool, email: &str, password: &str) -> i64 {
    let hash = server::auth::local::hash_password(password).unwrap();
    db::admin::Admin::new(pool).create_user(email, "Risk", &hash, false).await.unwrap()
}

fn login_req(email: &str, password: &str) -> Request<Body> {
    Request::post("/api/login")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::json!({"email": email, "password": password}).to_string()))
        .unwrap()
}

async fn status_of(app: &axum::Router, req: Request<Body>) -> StatusCode {
    app.clone().oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn login_sets_a_session_cookie_and_me_reports_the_principal() {
    let (app, pool, edb) = server_app().await;
    seed_user(&pool, "r@f.lu", "correct horse battery").await;

    let res = app.clone().oneshot(login_req("r@f.lu", "correct horse battery")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let cookie = res.headers().get("set-cookie").unwrap().to_str().unwrap().to_string();
    assert!(cookie.contains("HttpOnly"), "session cookie must be HttpOnly");
    assert!(cookie.contains("SameSite=Strict"));

    let me = app.clone().oneshot(
        Request::get("/api/me").header("cookie", cookie.split(';').next().unwrap())
            .body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(me.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&me.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["display_name"], "Risk");
    assert_eq!(body["is_administrator"], false);
    edb.stop().await;
}

#[tokio::test]
async fn me_without_a_session_is_401() {
    let (app, _pool, edb) = server_app().await;
    assert_eq!(status_of(&app, Request::get("/api/me").body(Body::empty()).unwrap()).await,
               StatusCode::UNAUTHORIZED);
    edb.stop().await;
}

#[tokio::test]
async fn a_wrong_password_is_401_and_does_not_reveal_whether_the_account_exists() {
    let (app, pool, edb) = server_app().await;
    seed_user(&pool, "r@f.lu", "correct horse battery").await;
    let known = app.clone().oneshot(login_req("r@f.lu", "wrong")).await.unwrap();
    let unknown = app.clone().oneshot(login_req("nobody@f.lu", "wrong")).await.unwrap();
    assert_eq!(known.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(unknown.status(), StatusCode::UNAUTHORIZED);
    let kb = known.into_body().collect().await.unwrap().to_bytes();
    let ub = unknown.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(kb, ub, "the two responses must be indistinguishable");
    edb.stop().await;
}

#[tokio::test]
async fn five_failures_lock_the_account_even_with_the_right_password() {
    let (app, pool, edb) = server_app().await;
    seed_user(&pool, "r@f.lu", "correct horse battery").await;
    for _ in 0..5 {
        let _ = app.clone().oneshot(login_req("r@f.lu", "wrong")).await.unwrap();
    }
    let res = app.clone().oneshot(login_req("r@f.lu", "correct horse battery")).await.unwrap();
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(res.headers().contains_key("retry-after"));
    edb.stop().await;
}

#[tokio::test]
async fn a_successful_login_clears_earlier_failures() {
    let (app, pool, edb) = server_app().await;
    seed_user(&pool, "r@f.lu", "correct horse battery").await;
    for _ in 0..3 {
        let _ = app.clone().oneshot(login_req("r@f.lu", "wrong")).await.unwrap();
    }
    assert_eq!(status_of(&app, login_req("r@f.lu", "correct horse battery")).await, StatusCode::OK);
    let st = db::admin::Admin::new(&pool).login_state("r@f.lu").await.unwrap();
    assert_eq!(st.failures, 0);
    edb.stop().await;
}

#[tokio::test]
async fn logout_revokes_the_session_immediately() {
    let (app, pool, edb) = server_app().await;
    seed_user(&pool, "r@f.lu", "correct horse battery").await;
    let res = app.clone().oneshot(login_req("r@f.lu", "correct horse battery")).await.unwrap();
    let cookie = res.headers().get("set-cookie").unwrap().to_str().unwrap()
        .split(';').next().unwrap().to_string();

    let out = app.clone().oneshot(
        Request::post("/api/logout").header("cookie", &cookie).body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(out.status(), StatusCode::NO_CONTENT);

    let me = app.clone().oneshot(
        Request::get("/api/me").header("cookie", &cookie).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(me.status(), StatusCode::UNAUTHORIZED);
    edb.stop().await;
}

#[tokio::test]
async fn desktop_mode_needs_no_login_at_all() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    std::mem::forget(dir);
    let app = server::routes::router(server::state::AppState::desktop(dbh));
    let me = app.clone().oneshot(Request::get("/api/me").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(me.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&me.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["is_administrator"], true);
    edb.stop().await;
}

#[tokio::test]
async fn a_session_token_is_never_stored_in_the_clear() {
    let (app, pool, edb) = server_app().await;
    seed_user(&pool, "r@f.lu", "correct horse battery").await;
    let res = app.clone().oneshot(login_req("r@f.lu", "correct horse battery")).await.unwrap();
    let cookie = res.headers().get("set-cookie").unwrap().to_str().unwrap().to_string();
    let token = cookie.split(';').next().unwrap().split_once('=').unwrap().1.to_string();
    let stored: Vec<String> = sqlx::query_scalar("SELECT token_hash FROM sessions")
        .fetch_all(&pool).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_ne!(stored[0], token, "the raw token must not be in the database");
    edb.stop().await;
}

fn login_req_from(email: &str, password: &str, source: &str) -> Request<Body> {
    Request::post("/api/login")
        .header("content-type", "application/json")
        .header("x-forwarded-for", source)
        .body(Body::from(serde_json::json!({"email": email, "password": password}).to_string()))
        .unwrap()
}

/// Finding P8: account lockout counts failures per email, so it does nothing
/// at all against the shape credential-stuffing actually takes — one source
/// trying one password against many accounts, never reaching five failures on
/// any of them. Throttling the source is what stops that, and it also caps
/// how fast one attacker can drive other people's accounts into lockout.
#[tokio::test]
async fn one_source_failing_across_many_accounts_is_throttled() {
    let (app, pool, edb) = server_app().await;
    seed_user(&pool, "victim@f.lu", "correct horse battery").await;

    // Ten distinct emails, so no single account ever approaches its own
    // five-failure lockout — only the source is repeating.
    let mut sawn_401 = 0;
    for i in 0..10 {
        let email = format!("stuff{i}@f.lu");
        let st = status_of(&app, login_req_from(&email, "guess", "198.51.100.23")).await;
        if st == StatusCode::UNAUTHORIZED { sawn_401 += 1; }
    }
    assert!(sawn_401 >= 5, "the first attempts should be ordinary 401s, saw {sawn_401}");

    let st = status_of(&app, login_req_from("stuff99@f.lu", "guess", "198.51.100.23")).await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "the source should be throttled by now");

    // A different source is unaffected — the counter is per origin, not global.
    assert_eq!(
        status_of(&app, login_req_from("stuff99@f.lu", "guess", "198.51.100.99")).await,
        StatusCode::UNAUTHORIZED,
    );

    // And a real sign-in from a clean source still works.
    assert_eq!(
        status_of(&app, login_req_from("victim@f.lu", "correct horse battery", "198.51.100.99")).await,
        StatusCode::OK,
    );

    pool.close().await;
    edb.stop().await;
}
