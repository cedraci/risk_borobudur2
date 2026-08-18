//! Endpoint authorization matrix: every portfolio-scoped route in
//! `routes.rs` pinned against the (domain, action) it declares. Helpers
//! copied from `api_authz_slice.rs` — this crate has no shared `tests/common`
//! module and every `api_*.rs` file inlines its own setup.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use db::auth::{Action, Domain, Grant};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

async fn app() -> (axum::Router, sqlx::PgPool, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    let pool = dbh.test_pool().clone();
    std::mem::forget(dir);
    let app = server::routes::router(server::state::AppState::server(dbh.clone()));
    (app, pool, edb)
}

/// Unlike `api_authz_slice.rs`'s single-call original, this file calls
/// `user_with` in a loop over `CASES` within one test — each call needs its
/// own user (distinct email) and its own session (distinct token), or the
/// second call in any loop collides on `users.email` / `sessions.token_hash`.
static NEXT_USER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

async fn user_with(pool: &sqlx::PgPool, grants: &[Grant]) -> String {
    let n = NEXT_USER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let hash = server::auth::local::hash_password("pw").unwrap();
    let admin = db::admin::Admin::new(pool);
    let id = admin.create_user(&format!("u{n}@f.lu"), "U", &hash, false).await.unwrap();
    for g in grants {
        admin.grant_add(id, *g, None).await.unwrap();
    }
    let token = format!("t{n}");
    admin.session_create(&server::auth::local::token_hash(&token), id, 1).await.unwrap();
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

async fn get_json(app: &axum::Router, uri: &str, cookie: Option<&str>) -> (StatusCode, serde_json::Value) {
    let mut b = Request::get(uri);
    if let Some(c) = cookie { b = b.header("cookie", c); }
    let res = app.clone().oneshot(b.body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let v = if bytes.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
    (status, v)
}

struct Case { uri: &'static str, domain: Domain, action: Action }

const CASES: &[Case] = &[
    Case { uri: "/api/portfolios/{pid}/nav",                    domain: Domain::Nav,          action: Action::View },
    Case { uri: "/api/portfolios/{pid}/positions",              domain: Domain::Positions,    action: Action::View },
    Case { uri: "/api/portfolios/{pid}/metrics/summary",        domain: Domain::Nav,          action: Action::View },
    Case { uri: "/api/portfolios/{pid}/metrics/rolling",        domain: Domain::Nav,          action: Action::View },
    Case { uri: "/api/portfolios/{pid}/metrics/drawdowns",      domain: Domain::Nav,          action: Action::View },
    Case { uri: "/api/portfolios/{pid}/metrics/calendar",       domain: Domain::Nav,          action: Action::View },
    Case { uri: "/api/portfolios/{pid}/metrics/var",            domain: Domain::Nav,          action: Action::View },
    Case { uri: "/api/portfolios/{pid}/metrics/backtest",       domain: Domain::Nav,          action: Action::View },
    Case { uri: "/api/portfolios/{pid}/metrics/concentration",  domain: Domain::Positions,    action: Action::View },
    Case { uri: "/api/portfolios/{pid}/metrics/liquidity",      domain: Domain::Positions,    action: Action::View },
    Case { uri: "/api/portfolios/{pid}/metrics/rates",          domain: Domain::Positions,    action: Action::View },
    Case { uri: "/api/portfolios/{pid}/metrics/derivatives",    domain: Domain::Positions,    action: Action::View },
    Case { uri: "/api/portfolios/{pid}/pnl",                    domain: Domain::Positions,    action: Action::View },
    Case { uri: "/api/portfolios/{pid}/emir",                   domain: Domain::Positions,    action: Action::View },
    Case { uri: "/api/portfolios/{pid}/emir/export",            domain: Domain::Positions,    action: Action::Export },
    Case { uri: "/api/portfolios/{pid}/shareholders",           domain: Domain::Shareholders, action: Action::View },
    Case { uri: "/api/portfolios/{pid}/flows",                  domain: Domain::Shareholders, action: Action::View },
    Case { uri: "/api/portfolios/{pid}/settings",               domain: Domain::Reference,    action: Action::View },
    Case { uri: "/api/portfolios/{pid}/imports",                domain: Domain::Reference,    action: Action::View },
    Case { uri: "/api/portfolios/{pid}/futures-analytics",      domain: Domain::MarketData,   action: Action::View },
];

fn uri_for(case: &Case, pid: i64) -> String {
    case.uri.replace("{pid}", &pid.to_string())
}

#[tokio::test]
async fn no_cookie_is_401_for_every_case() {
    let (app, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    for case in CASES {
        let uri = uri_for(case, pid);
        assert_eq!(
            get(&app, &uri, None).await, StatusCode::UNAUTHORIZED,
            "expected 401 with no cookie for {uri}"
        );
    }
    edb.stop().await;
}

#[tokio::test]
async fn the_exact_grant_never_401s_403s_or_404s() {
    let (app, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    for case in CASES {
        let cookie = user_with(&pool, &[Grant { domain: case.domain, action: case.action, portfolio: Some(pid) }]).await;
        let uri = uri_for(case, pid);
        let status = get(&app, &uri, Some(&cookie)).await;
        assert!(
            !matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
            "expected the exact grant ({:?}, {:?}) to reach the handler for {uri}, got {status}",
            case.domain, case.action
        );
    }
    edb.stop().await;
}

#[tokio::test]
async fn a_grant_on_a_different_portfolio_is_404() {
    let (app, pool, edb) = app().await;
    let mine = portfolio(&pool, "Mine").await;
    let theirs = portfolio(&pool, "Theirs").await;
    for case in CASES {
        let cookie = user_with(&pool, &[Grant { domain: case.domain, action: case.action, portfolio: Some(mine) }]).await;
        let uri = uri_for(case, theirs);
        assert_eq!(
            get(&app, &uri, Some(&cookie)).await, StatusCode::NOT_FOUND,
            "expected 404 (out of scope) for {uri} with a grant only on portfolio {mine}"
        );
    }
    edb.stop().await;
}

#[tokio::test]
async fn a_grant_on_a_different_domain_is_403() {
    let (app, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    for case in CASES {
        // A domain the case does not itself use, so the grant is visible on
        // this portfolio (avoiding the out-of-scope 404) but wrong.
        let other = if case.domain == Domain::Reference { Domain::Nav } else { Domain::Reference };
        let cookie = user_with(&pool, &[Grant { domain: other, action: Action::View, portfolio: Some(pid) }]).await;
        let uri = uri_for(case, pid);
        assert_eq!(
            get(&app, &uri, Some(&cookie)).await, StatusCode::FORBIDDEN,
            "expected 403 for {uri} with only a {other:?} grant"
        );
    }
    edb.stop().await;
}

/// `GET /api/portfolios` is not in `CASES` — it is `.authenticated`, not
/// `.protected`/`.protected_global`: `Scoped::portfolios_list` filters to
/// the visible set rather than requiring a single (domain, action) grant, so
/// it does not fit the matrix's authorize-or-deny shape. Ruling 4 (Task 9)
/// still requires coverage of its contract: unauthenticated is 401, a
/// portfolio-scoped grant sees only the portfolios it covers, and a
/// wildcard grant sees everything.
#[tokio::test]
async fn portfolios_list_is_authenticated_and_filters_to_the_visible_scope() {
    let (app, pool, edb) = app().await;
    let a = portfolio(&pool, "A").await;
    let b = portfolio(&pool, "B").await;

    // 1. No cookie -> 401, same as every other route.
    assert_eq!(get(&app, "/api/portfolios", None).await, StatusCode::UNAUTHORIZED);

    // 2. A single portfolio-scoped grant (on A only) sees exactly A, not B —
    // the domain/action of the grant is irrelevant to visibility here, only
    // which portfolio it names.
    let scoped_cookie = user_with(&pool, &[Grant { domain: Domain::Positions, action: Action::View, portfolio: Some(a) }]).await;
    let (status, body) = get_json(&app, "/api/portfolios", Some(&scoped_cookie)).await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<i64> = body.as_array().unwrap().iter().map(|p| p["id"].as_i64().unwrap()).collect();
    assert_eq!(ids, vec![a], "expected exactly the granted portfolio, got {ids:?}");

    // 3. A wildcard grant (portfolio: None) sees the whole fleet — at least
    // A and B (an exact-set check would also have to know about portfolio 1,
    // which 0008_seed.sql seeds into every fresh database regardless of this
    // test, so this deliberately checks superset rather than equality).
    let wildcard_cookie = user_with(&pool, &[Grant { domain: Domain::Reference, action: Action::View, portfolio: None }]).await;
    let (status, body) = get_json(&app, "/api/portfolios", Some(&wildcard_cookie)).await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<i64> = body.as_array().unwrap().iter().map(|p| p["id"].as_i64().unwrap()).collect();
    assert!(ids.contains(&a) && ids.contains(&b),
        "expected the wildcard principal to see every portfolio including A and B, got {ids:?}");

    edb.stop().await;
}
