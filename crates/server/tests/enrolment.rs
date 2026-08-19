//! Task 13: first-administrator enrolment. On an empty `users` table,
//! `ensure_first_administrator` creates one administrator with an unusable
//! password and issues a single-use, time-boxed token; `POST /api/enrol`
//! consumes that token to set the real password. Desktop mode has no
//! accounts at all, so the route must not even be reachable there.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::util::ServiceExt;

async fn db_only() -> (db::Db, sqlx::PgPool, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    let pool = dbh.test_pool().clone();
    std::mem::forget(dir);
    (dbh, pool, edb)
}

fn login_req(email: &str, password: &str) -> Request<Body> {
    Request::post("/api/login")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&serde_json::json!({
            "email": email, "password": password,
        })).unwrap()))
        .unwrap()
}

fn enrol_req(token: &str, password: &str) -> Request<Body> {
    Request::post("/api/enrol")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&serde_json::json!({
            "token": token, "password": password,
        })).unwrap()))
        .unwrap()
}

#[tokio::test]
async fn an_empty_server_issues_a_single_use_enrolment_token() {
    let (dbh, pool, edb) = db_only().await;

    let token = server::startup::ensure_first_administrator(&dbh, "risk@firm.lu").await.unwrap();
    assert!(token.is_some(), "an empty server must issue a token");

    let admin = db::admin::Admin::new(&pool);
    let user = admin.user_by_email("risk@firm.lu").await.unwrap().expect("the administrator account must exist");
    assert!(user.is_administrator);
    assert!(!user.disabled);

    // Cannot log in until enrolled: the account starts with an unusable
    // password hash, so no password verifies against it.
    let app = server::routes::router(server::state::AppState::server(dbh.clone()));
    let res = app.oneshot(login_req("risk@firm.lu", "anything")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn enrolment_sets_the_password_and_consumes_the_token() {
    let (dbh, pool, edb) = db_only().await;
    let token = server::startup::ensure_first_administrator(&dbh, "risk2@firm.lu").await.unwrap().unwrap();
    let app = server::routes::router(server::state::AppState::server(dbh.clone()));

    let res = app.clone().oneshot(enrol_req(&token, "correct-horse-battery")).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let res = app.clone().oneshot(login_req("risk2@firm.lu", "correct-horse-battery")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // The token is single use: replaying it must fail even with a fresh password.
    let res = app.clone().oneshot(enrol_req(&token, "another-password")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn a_server_that_already_has_users_issues_nothing() {
    let (dbh, pool, edb) = db_only().await;
    let hash = server::auth::local::hash_password("pw").unwrap();
    db::admin::Admin::new(&pool).create_user("existing@f.lu", "Existing", &hash, false).await.unwrap();

    let token = server::startup::ensure_first_administrator(&dbh, "risk3@firm.lu").await.unwrap();
    assert!(token.is_none(), "a non-empty server must not enrol a new administrator");

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn desktop_mode_never_enrols() {
    let (dbh, pool, edb) = db_only().await;
    let app = server::routes::router(server::state::AppState::desktop(dbh.clone()));
    let res = app.oneshot(enrol_req("whatever", "whatever")).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    pool.close().await;
    edb.stop().await;
}
