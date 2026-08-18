use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::util::ServiceExt;
use db::auth::{Action, Domain, Grant};

async fn app() -> (axum::Router, sqlx::PgPool, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    std::mem::forget(dir);
    let app = server::routes::router(server::state::AppState::server(pool.clone()));
    (app, pool, edb)
}

async fn user_with(pool: &sqlx::PgPool, grants: &[Grant]) -> String {
    let hash = server::auth::local::hash_password("pw").unwrap();
    let admin = db::admin::Admin::new(pool);
    let id = admin.create_user("u@f.lu", "U", &hash, false).await.unwrap();
    for g in grants {
        admin.grant_add(id, *g, None).await.unwrap();
    }
    let token = "t0";
    admin.session_create(&server::auth::local::token_hash(token), id, 1).await.unwrap();
    format!("borobudur_session={token}")
}

async fn portfolio(pool: &sqlx::PgPool, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO portfolios (name, kind) VALUES ($1,'ucits') RETURNING id")
        .bind(name).fetch_one(pool).await.unwrap()
}

async fn get(app: &axum::Router, uri: &str, cookie: Option<&str>) -> StatusCode {
    let mut b = Request::get(uri);
    if let Some(c) = cookie { b = b.header("cookie", c); }
    app.clone().oneshot(b.body(Body::empty()).unwrap()).await.unwrap().status()
}

#[tokio::test]
async fn unauthenticated_requests_are_401() {
    let (app, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    assert_eq!(get(&app, &format!("/api/portfolios/{pid}/nav"), None).await, StatusCode::UNAUTHORIZED);
    edb.stop().await;
}

#[tokio::test]
async fn a_granted_principal_gets_200() {
    let (app, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    let c = user_with(&pool, &[Grant { domain: Domain::Nav, action: Action::View, portfolio: Some(pid) }]).await;
    assert_eq!(get(&app, &format!("/api/portfolios/{pid}/nav"), Some(&c)).await, StatusCode::OK);
    edb.stop().await;
}

#[tokio::test]
async fn a_portfolio_outside_scope_is_404_not_403() {
    let (app, pool, edb) = app().await;
    let mine = portfolio(&pool, "Mine").await;
    let theirs = portfolio(&pool, "Theirs").await;
    let c = user_with(&pool, &[Grant { domain: Domain::Nav, action: Action::View, portfolio: Some(mine) }]).await;
    assert_eq!(get(&app, &format!("/api/portfolios/{theirs}/nav"), Some(&c)).await,
               StatusCode::NOT_FOUND,
               "403 would confirm the fund exists");
    edb.stop().await;
}

#[tokio::test]
async fn a_visible_portfolio_with_a_denied_domain_is_403() {
    let (app, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    let c = user_with(&pool, &[Grant { domain: Domain::Nav, action: Action::View, portfolio: Some(pid) }]).await;
    assert_eq!(get(&app, &format!("/api/portfolios/{pid}/positions"), Some(&c)).await,
               StatusCode::FORBIDDEN);
    edb.stop().await;
}

#[tokio::test]
async fn a_portfolio_scoped_principal_is_403_on_a_global_route() {
    let (app, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    // Only a portfolio-scoped grant — no wildcard, so it must not reach an
    // instance-wide (`.protected_global`) route at all, regardless of domain
    // match. `/api/refs/{code}` is instance-wide reference data, not scoped
    // to any single portfolio.
    let c = user_with(&pool, &[Grant { domain: Domain::Reference, action: Action::Configure, portfolio: Some(pid) }]).await;
    let req = Request::put("/api/refs/SOMECODE")
        .header("cookie", &c)
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let status = app.clone().oneshot(req).await.unwrap().status();
    assert_eq!(status, StatusCode::FORBIDDEN);
    edb.stop().await;
}

#[tokio::test]
async fn desktop_mode_reaches_everything_without_a_cookie() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    std::mem::forget(dir);
    let app = server::routes::router(server::state::AppState::desktop(pool.clone()));
    let pid = portfolio(&pool, "F").await;
    assert_eq!(get(&app, &format!("/api/portfolios/{pid}/nav"), None).await, StatusCode::OK);
    assert_eq!(get(&app, &format!("/api/portfolios/{pid}/positions"), None).await, StatusCode::OK);
    edb.stop().await;
}
