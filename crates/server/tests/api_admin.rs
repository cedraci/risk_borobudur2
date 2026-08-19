//! Task 13: administration endpoints. Every `/api/admin/*` route requires
//! `ctx.is_administrator`, regardless of any domain/action grant the caller
//! might otherwise hold. Helpers copied from `api_authz_slice.rs` / `api_audit.rs`
//! — this crate has no shared `tests/common` module.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use db::auth::Grant;
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

static NEXT_USER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

async fn user_with(pool: &sqlx::PgPool, is_administrator: bool, grants: &[Grant]) -> (String, i64) {
    let n = NEXT_USER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let hash = server::auth::local::hash_password("pw").unwrap();
    let admin = db::admin::Admin::new(pool);
    let id = admin.create_user(&format!("u{n}@f.lu"), "U", &hash, is_administrator).await.unwrap();
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

async fn send_json(
    app: &axum::Router, method: &str, uri: &str, cookie: Option<&str>, body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let mut b = Request::builder().method(method).uri(uri).header("content-type", "application/json");
    if let Some(c) = cookie { b = b.header("cookie", c); }
    let req = b.body(Body::from(serde_json::to_vec(&body).unwrap())).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let v = if bytes.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
    (status, v)
}

#[tokio::test]
async fn a_non_administrator_cannot_reach_any_admin_route() {
    let (app, pool, edb) = app().await;
    let (cookie, my_id) = user_with(&pool, false, &[]).await;
    let (_, other_id) = user_with(&pool, false, &[]).await;

    assert_eq!(get(&app, "/api/admin/users", Some(&cookie)).await, StatusCode::FORBIDDEN);
    let (s, _) = send_json(&app, "POST", "/api/admin/users", Some(&cookie),
        serde_json::json!({"email": "x@f.lu", "display_name": "X", "password": "whatever-pw"})).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = send_json(&app, "PUT", &format!("/api/admin/users/{other_id}/password"), Some(&cookie),
        serde_json::json!({"password": "whatever-pw"})).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = send_json(&app, "PUT", &format!("/api/admin/users/{other_id}/disabled"), Some(&cookie),
        serde_json::json!({"disabled": true})).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(get(&app, &format!("/api/admin/users/{other_id}/grants"), Some(&cookie)).await, StatusCode::FORBIDDEN);
    let (s, _) = send_json(&app, "POST", &format!("/api/admin/users/{other_id}/grants"), Some(&cookie),
        serde_json::json!({"domain": "nav", "action": "view", "portfolio": null})).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = send_json(&app, "DELETE", &format!("/api/admin/users/{other_id}/grants"), Some(&cookie),
        serde_json::json!({"domain": "nav", "action": "view", "portfolio": null})).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = send_json(&app, "POST", &format!("/api/admin/users/{other_id}/roles"), Some(&cookie),
        serde_json::json!({"role": "auditor"})).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(get(&app, "/api/admin/audit", Some(&cookie)).await, StatusCode::FORBIDDEN);

    let _ = my_id;
    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn an_administrator_creates_a_user_and_grants_them_one_portfolio() {
    let (app, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    let other_pid = portfolio(&pool, "G").await;
    let (admin_cookie, _) = user_with(&pool, true, &[]).await;

    let (status, created) = send_json(&app, "POST", "/api/admin/users", Some(&admin_cookie), serde_json::json!({
        "email": "newbie@f.lu", "display_name": "Newbie", "password": "s3cret-password",
    })).await;
    assert_eq!(status, StatusCode::OK, "{created:?}");
    let new_id = created["id"].as_i64().expect("created user must carry an id");

    let (status, _) = send_json(&app, "POST", &format!("/api/admin/users/{new_id}/grants"), Some(&admin_cookie),
        serde_json::json!({"domain": "nav", "action": "view", "portfolio": pid})).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let login = Request::post("/api/login")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&serde_json::json!({
            "email": "newbie@f.lu", "password": "s3cret-password",
        })).unwrap()))
        .unwrap();
    let res = app.clone().oneshot(login).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let set_cookie = res.headers().get(axum::http::header::SET_COOKIE).unwrap().to_str().unwrap().to_string();
    let new_cookie = set_cookie.split(';').next().unwrap().to_string();

    assert_eq!(get(&app, &format!("/api/portfolios/{pid}/nav"), Some(&new_cookie)).await, StatusCode::OK);
    assert_eq!(get(&app, &format!("/api/portfolios/{other_pid}/nav"), Some(&new_cookie)).await, StatusCode::NOT_FOUND);

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn assigning_a_role_writes_its_expanded_grants() {
    let (app, pool, edb) = app().await;
    let (admin_cookie, _) = user_with(&pool, true, &[]).await;
    let (_, target_id) = user_with(&pool, false, &[]).await;

    let (status, _) = send_json(&app, "POST", &format!("/api/admin/users/{target_id}/roles"), Some(&admin_cookie),
        serde_json::json!({"role": "auditor"})).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, grants) = get_json(&app, &format!("/api/admin/users/{target_id}/grants"), Some(&admin_cookie)).await;
    assert_eq!(status, StatusCode::OK);
    let grants = grants.as_array().expect("grants must be a JSON array");
    let views: std::collections::BTreeSet<String> = grants.iter()
        .filter(|g| g["action"] == "view")
        .map(|g| g["domain"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(views.len(), 6, "auditor must carry view on all six domains: {grants:?}");

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn disabling_a_user_revokes_their_live_session_immediately() {
    let (app, pool, edb) = app().await;
    let (admin_cookie, _) = user_with(&pool, true, &[]).await;
    let (user_cookie, user_id) = user_with(&pool, false, &[]).await;

    assert_eq!(get(&app, "/api/me", Some(&user_cookie)).await, StatusCode::OK);

    let (status, _) = send_json(&app, "PUT", &format!("/api/admin/users/{user_id}/disabled"), Some(&admin_cookie),
        serde_json::json!({"disabled": true})).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    assert_eq!(get(&app, "/api/me", Some(&user_cookie)).await, StatusCode::UNAUTHORIZED,
        "a disabled user's live session must stop working on the very next request");

    // `session_user`'s own `NOT u.disabled` filter would make the request
    // above pass even if `sessions_delete_for` were never called — assert
    // the row is actually gone so this falsifies the deletion itself, not
    // just that filter.
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE user_id = $1")
        .bind(user_id).fetch_one(&pool).await.unwrap();
    assert_eq!(remaining, 0, "disabling a user must delete their session rows, not just deny them");

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn the_audit_route_returns_newest_first() {
    let (app, pool, edb) = app().await;
    let (admin_cookie, _) = user_with(&pool, true, &[]).await;
    let (_, target_id) = user_with(&pool, false, &[]).await;

    let (s, _) = send_json(&app, "PUT", &format!("/api/admin/users/{target_id}/password"), Some(&admin_cookie),
        serde_json::json!({"password": "first-password"})).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = send_json(&app, "PUT", &format!("/api/admin/users/{target_id}/password"), Some(&admin_cookie),
        serde_json::json!({"password": "second-password"})).await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    let (status, rows) = get_json(&app, "/api/admin/audit", Some(&admin_cookie)).await;
    assert_eq!(status, StatusCode::OK);
    let rows = rows.as_array().expect("audit rows must be a JSON array");
    assert!(rows.len() >= 2, "{rows:?}");
    let ids: Vec<i64> = rows.iter().map(|r| r["id"].as_i64().unwrap()).collect();
    let mut sorted = ids.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(ids, sorted, "expected newest-first ordering: {rows:?}");

    pool.close().await;
    edb.stop().await;
}
