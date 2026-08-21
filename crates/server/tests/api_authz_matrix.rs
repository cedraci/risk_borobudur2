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
    send(app, &r("", Domain::Nav, Action::View), uri, cookie).await
}

/// Issues one case's request. The body only has to get past extraction — the
/// gate under test is middleware, so a route that goes on to reject the body
/// as malformed has still answered the question this file asks.
async fn send(app: &axum::Router, case: &Case, uri: &str, cookie: Option<&str>) -> StatusCode {
    let mut b = Request::builder().method(case.method).uri(uri);
    if let Some(c) = cookie { b = b.header("cookie", c); }
    let body = match case.body {
        None => Body::empty(),
        Some(Payload::Json(j)) => {
            b = b.header("content-type", "application/json");
            Body::from(j)
        }
        Some(Payload::Multipart) => {
            b = b.header("content-type", "multipart/form-data; boundary=XB");
            // A well-formed but empty multipart body: enough to get past
            // extraction, after which the handler's own "missing field"
            // rejection is a perfectly good "reached the handler".
            Body::from("--XB--\r\n")
        }
    };
    app.clone().oneshot(b.body(body).unwrap()).await.unwrap().status()
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

#[derive(Clone, Copy)]
enum Payload { Json(&'static str), Multipart }

struct Case {
    uri: &'static str,
    method: &'static str,
    body: Option<Payload>,
    domain: Domain,
    action: Action,
}

/// Shorthand for the read cases, which are the bulk of the table.
const fn r(uri: &'static str, domain: Domain, action: Action) -> Case {
    Case { uri, method: "GET", body: None, domain, action }
}

const CASES: &[Case] = &[
    r("/api/portfolios/{pid}/nav", Domain::Nav, Action::View),
    r("/api/portfolios/{pid}/positions", Domain::Positions, Action::View),
    r("/api/portfolios/{pid}/metrics/summary", Domain::Nav, Action::View),
    r("/api/portfolios/{pid}/metrics/rolling", Domain::Nav, Action::View),
    r("/api/portfolios/{pid}/metrics/drawdowns", Domain::Nav, Action::View),
    r("/api/portfolios/{pid}/metrics/calendar", Domain::Nav, Action::View),
    r("/api/portfolios/{pid}/metrics/var", Domain::Nav, Action::View),
    r("/api/portfolios/{pid}/metrics/backtest", Domain::Nav, Action::View),
    r("/api/portfolios/{pid}/metrics/concentration", Domain::Positions, Action::View),
    r("/api/portfolios/{pid}/metrics/liquidity", Domain::Positions, Action::View),
    r("/api/portfolios/{pid}/metrics/rates", Domain::Positions, Action::View),
    r("/api/portfolios/{pid}/metrics/derivatives", Domain::Positions, Action::View),
    r("/api/portfolios/{pid}/pnl", Domain::Positions, Action::View),
    r("/api/portfolios/{pid}/emir", Domain::Positions, Action::View),
    r("/api/portfolios/{pid}/emir/export", Domain::Positions, Action::Export),
    r("/api/portfolios/{pid}/shareholders", Domain::Shareholders, Action::View),
    r("/api/portfolios/{pid}/flows", Domain::Shareholders, Action::View),
    r("/api/portfolios/{pid}/codes", Domain::Settings, Action::View),
    r("/api/portfolios/{pid}/settings", Domain::Settings, Action::View),
    r("/api/portfolios/{pid}/imports", Domain::Settings, Action::View),
    r("/api/portfolios/{pid}/futures-analytics", Domain::MarketData, Action::View),
    r("/api/portfolios/{pid}/limit-runs", Domain::Settings, Action::View),
    r("/api/portfolios/{pid}/breaches", Domain::Settings, Action::View),
    // `GET /api/portfolios/{pid}/breaches/{bid}` is NOT in this table. The
    // exact-grant case above (`the_exact_grant_never_401s_403s_or_404s`)
    // asserts that a 404 never comes back, but this file's `app()` seeds no
    // breach episodes at all — any concrete `bid` here would 404 for the
    // right reason (no such episode) and fail that assertion for the wrong
    // one. Its authorization contract (no cookie / wrong domain / wrong
    // portfolio / exact grant) is pinned instead in
    // `api_breach_register.rs::episode_route_authorization`, against a
    // real episode.
    // Writes. Every mutating portfolio-scoped route in `routes.rs` — the
    // half the matrix used to skip. A mis-declared write is both likelier
    // (they arrive one at a time) and more expensive than a mis-declared
    // read (finding P9).
    Case { uri: "/api/portfolios/{pid}", method: "PUT", body: Some(Payload::Json(r#"{"name":"X","archived":false}"#)),
           domain: Domain::Reference, action: Action::Configure },
    Case { uri: "/api/portfolios/{pid}/codes", method: "PUT", body: Some(Payload::Json("[]")),
           domain: Domain::Settings, action: Action::Configure },
    Case { uri: "/api/portfolios/{pid}/settings", method: "PUT", body: Some(Payload::Json("{}")),
           domain: Domain::Settings, action: Action::Configure },
    Case { uri: "/api/portfolios/{pid}/shareholders", method: "PUT", body: Some(Payload::Json("[]")),
           domain: Domain::Shareholders, action: Action::Import },
    Case { uri: "/api/portfolios/{pid}/emir/kpis/2026-08-01", method: "PUT", body: Some(Payload::Json("{}")),
           domain: Domain::Settings, action: Action::Configure },
    Case { uri: "/api/portfolios/{pid}/imports", method: "POST", body: Some(Payload::Multipart),
           domain: Domain::Positions, action: Action::Import },
    Case { uri: "/api/portfolios/{pid}/futures-analytics", method: "POST", body: Some(Payload::Multipart),
           domain: Domain::MarketData, action: Action::Import },
];

/// Instance-wide routes (`.protected_global`). Their contract differs from
/// the table above in the way that matters most: a grant scoped to a single
/// portfolio never reaches them, however right its domain and action.
const GLOBAL_CASES: &[Case] = &[
    r("/api/refs", Domain::Reference, Action::View),
    r("/api/futures-contracts", Domain::Reference, Action::View),
    r("/api/bloomberg/request", Domain::Positions, Action::Export),
    r("/api/bloomberg/adv-request", Domain::Positions, Action::Export),
    r("/api/bloomberg/adv-due", Domain::MarketData, Action::View),
    Case { uri: "/api/refs/XS0000000001", method: "PUT", body: Some(Payload::Json("{}")),
           domain: Domain::Reference, action: Action::Configure },
    Case { uri: "/api/futures-contracts/FGBL", method: "PUT", body: Some(Payload::Json("{}")),
           domain: Domain::Reference, action: Action::Configure },
    Case { uri: "/api/bloomberg/upload", method: "POST", body: Some(Payload::Multipart),
           domain: Domain::MarketData, action: Action::Import },
    Case { uri: "/api/portfolios", method: "POST", body: Some(Payload::Json(r#"{"name":"New","kind":"ucits"}"#)),
           domain: Domain::Reference, action: Action::Configure },
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
            send(&app, case, &uri, None).await, StatusCode::UNAUTHORIZED,
            "expected 401 with no cookie for {} {uri}", case.method
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
        let status = send(&app, case, &uri, Some(&cookie)).await;
        assert!(
            !matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
            "expected the exact grant ({:?}, {:?}) to reach the handler for {} {uri}, got {status}",
            case.domain, case.action, case.method
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
            send(&app, case, &uri, Some(&cookie)).await, StatusCode::NOT_FOUND,
            "expected 404 (out of scope) for {} {uri} with a grant only on portfolio {mine}", case.method
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
        assert_ne!(other, case.domain, "the contrast domain must differ from the case's own");
        let cookie = user_with(&pool, &[Grant { domain: other, action: Action::View, portfolio: Some(pid) }]).await;
        let uri = uri_for(case, pid);
        assert_eq!(
            send(&app, case, &uri, Some(&cookie)).await, StatusCode::FORBIDDEN,
            "expected 403 for {} {uri} with only a {other:?} grant", case.method
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

#[tokio::test]
async fn no_cookie_is_401_for_every_instance_wide_case() {
    let (app, _pool, edb) = app().await;
    for case in GLOBAL_CASES {
        assert_eq!(
            send(&app, case, case.uri, None).await, StatusCode::UNAUTHORIZED,
            "expected 401 with no cookie for {} {}", case.method, case.uri
        );
    }
    edb.stop().await;
}

#[tokio::test]
async fn the_exact_wildcard_grant_reaches_every_instance_wide_route() {
    let (app, pool, edb) = app().await;
    for case in GLOBAL_CASES {
        let cookie = user_with(&pool, &[Grant { domain: case.domain, action: case.action, portfolio: None }]).await;
        let status = send(&app, case, case.uri, Some(&cookie)).await;
        assert!(
            !matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
            "expected the exact wildcard grant ({:?}, {:?}) to reach the handler for {} {}, got {status}",
            case.domain, case.action, case.method, case.uri
        );
    }
    edb.stop().await;
}

/// The property that separates `.protected_global` from `.protected`: an
/// instance-wide resource is answered only by an instance-wide grant. A
/// portfolio-scoped grant of exactly the right domain and action must not
/// reach it — otherwise "reference data on fund A" would quietly become
/// "the shared reference tables for the whole fleet".
#[tokio::test]
async fn a_portfolio_scoped_grant_never_reaches_an_instance_wide_route() {
    let (app, pool, edb) = app().await;
    let pid = portfolio(&pool, "Scoped").await;
    for case in GLOBAL_CASES {
        let cookie = user_with(&pool, &[Grant { domain: case.domain, action: case.action, portfolio: Some(pid) }]).await;
        assert_eq!(
            send(&app, case, case.uri, Some(&cookie)).await, StatusCode::FORBIDDEN,
            "expected 403 for {} {} with a grant scoped to one portfolio", case.method, case.uri
        );
    }
    edb.stop().await;
}
