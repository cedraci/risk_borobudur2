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

/// One authenticated GET against portfolio 1's register, returning the status
/// and the decoded body. The `state` filter needs three of these in a row and
/// each is six lines otherwise.
async fn register_get(app: &axum::Router, query: &str, cookie: &str) -> (StatusCode, serde_json::Value) {
    let res = app.clone().oneshot(
        Request::get(format!("/api/portfolios/1/breaches{query}"))
            .header("cookie", cookie).body(Body::empty()).unwrap()
    ).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let body = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, body)
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
    let (desktop, pool, dbh, edb) = app().await;
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
    // A hand-inserted episode has no events; asserting the count states that
    // fact, where `is_some()` would hold for any serialized `Vec` at all.
    assert_eq!(body["events"].as_array().unwrap().len(), 0);

    // 5. An episode belonging to another fund is not readable through this
    // fund's grant. Cases 1-4 are all settled by the middleware and by
    // `authorize(pid)`; this is the only one that reaches `breach_get`'s own
    // `portfolio_id = $2` predicate, so without it that predicate could be
    // deleted with every assertion above still passing.
    let other_run: i64 = sqlx::query_scalar(
        "INSERT INTO limit_check_runs (portfolio_id, nav_date, triggered_by)
         VALUES ($1, $2, 'import') RETURNING id")
        .bind(other_pid).bind(run.nav_date)
        .fetch_one(&pool).await.unwrap();
    let other_bid: i64 = sqlx::query_scalar(
        "INSERT INTO limit_breaches
             (portfolio_id, check_key, subject, opened_run_id, opened_nav_date)
         VALUES ($1, 'issuer_10', 'OTHER FUND ISSUER', $2, $3)
         RETURNING id")
        .bind(other_pid).bind(other_run).bind(run.nav_date)
        .fetch_one(&pool).await.unwrap();
    let res = server_app.clone().oneshot(
        Request::get(format!("/api/portfolios/1/breaches/{other_bid}"))
            .header("cookie", "borobudur_session=ok-t").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND,
        "another fund's episode must not be readable through portfolio 1's grant");

    // 6. The register's `state` filter. Without these three, the whitelist
    // could be deleted and `breaches_for` could be called with `None`, and
    // the suite would stay green: the list test only asserts a 200.
    let (status, _) = register_get(&server_app, "?state=bogus", "borobudur_session=ok-t").await;
    assert_eq!(status, StatusCode::BAD_REQUEST,
        "an unknown state is a bad request, not a silently unfiltered list");
    let (status, body) = register_get(&server_app, "?state=open", "borobudur_session=ok-t").await;
    assert_eq!(status, StatusCode::OK);
    let open: Vec<i64> = body["breaches"].as_array().unwrap()
        .iter().map(|b| b["id"].as_i64().unwrap()).collect();
    assert_eq!(open, vec![bid],
        "the open filter must return this fund's open episode and nothing else");
    let (status, body) = register_get(&server_app, "?state=resolved", "borobudur_session=ok-t").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["breaches"].as_array().unwrap().is_empty(),
        "no episode is resolved, so this must come back empty — which is what proves it filters");

    pool.close().await;
    edb.stop().await;
}

/// The state machine's own refusals: resolving an episode nobody classified,
/// classifying with a nonsense value, and acknowledging with a blank note.
/// Each must be rejected AND must leave the episode's state untouched — a
/// test that only checked the status code would still pass if the handler
/// mutated the row before returning the error.
#[tokio::test]
async fn an_episode_must_be_classified_before_it_can_be_resolved() {
    let (desktop, pool, dbh, edb) = app().await;
    let bytes = std::fs::read(SAMPLE).unwrap();
    let res = desktop.clone().oneshot(upload_req("/api/portfolios/1/imports", &bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Sample.xlsx stays under every limit (see api_breach_recorder.rs), so no
    // import opens an episode here either. Insert one directly through the
    // repository, pinned to a real run row, exactly as
    // `episode_route_authorization` does — the state machine is what is
    // under test, not the fixture's holdings.
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

    // Resolving straight from `open` is refused.
    let res = desktop.clone().oneshot(
        Request::post(format!("/api/portfolios/1/breaches/{bid}/resolve"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"note":"looks fine"}"#)).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY,
        "resolving something nobody classified is the gap this feature closes");

    // Acknowledging without a classification is refused.
    let res = desktop.clone().oneshot(
        Request::post(format!("/api/portfolios/1/breaches/{bid}/acknowledge"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"classification":"unclassified","note":"x"}"#)).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Acknowledging with an empty note is refused.
    let res = desktop.clone().oneshot(
        Request::post(format!("/api/portfolios/1/breaches/{bid}/acknowledge"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"classification":"passive","note":"  "}"#)).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // None of the three refusals above may have moved the episode: still
    // `open`, still unclassified.
    let still_open = scoped.breach_get(&view, bid).await.unwrap().unwrap();
    assert_eq!(still_open.state, "open", "a refused acknowledge/resolve must not change state");
    assert_eq!(still_open.classification, "unclassified");

    // Acknowledge properly, then resolve.
    let res = desktop.clone().oneshot(
        Request::post(format!("/api/portfolios/1/breaches/{bid}/acknowledge"))
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"classification":"passive","note":"market move, no purchase","deadline_date":"2026-09-30"}"#
            )).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let acknowledged = scoped.breach_get(&view, bid).await.unwrap().unwrap();
    assert_eq!(acknowledged.state, "acknowledged");
    assert_eq!(acknowledged.classification, "passive");

    let res = desktop.clone().oneshot(
        Request::post(format!("/api/portfolios/1/breaches/{bid}/resolve"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"note":"position trimmed on 21 Aug"}"#)).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // The timeline carries both acts, in order.
    let res = desktop.clone().oneshot(
        Request::get(format!("/api/portfolios/1/breaches/{bid}")).body(Body::empty()).unwrap()).await.unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["breach"]["state"], "resolved");
    assert_eq!(body["breach"]["classification"], "passive");
    let kinds: Vec<&str> = body["events"].as_array().unwrap().iter()
        .map(|e| e["event"].as_str().unwrap()).collect();
    assert!(kinds.contains(&"acknowledged") && kinds.contains(&"resolved"), "{kinds:?}");

    // Re-acknowledging or re-resolving a resolved episode is refused, not a
    // silent overwrite of the closed record.
    let res = desktop.clone().oneshot(
        Request::post(format!("/api/portfolios/1/breaches/{bid}/resolve"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"note":"again"}"#)).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY,
        "a resolved episode must not be resolvable a second time");

    // Another fund's episode is not writable through this fund's grant. Every
    // assertion above is settled inside portfolio 1, so without this the
    // `AND portfolio_id = $2` in `breach_acknowledge`/`breach_resolve` could
    // be deleted with the suite still green — and a sign-off written onto a
    // fund the signer was never authorized for is worse than a read of one.
    let other_pid: i64 = sqlx::query_scalar(
        "INSERT INTO portfolios (name, kind) VALUES ($1,'ucits') RETURNING id")
        .bind("Other").fetch_one(&pool).await.unwrap();
    let other_run: i64 = sqlx::query_scalar(
        "INSERT INTO limit_check_runs (portfolio_id, nav_date, triggered_by)
         VALUES ($1, $2, 'import') RETURNING id")
        .bind(other_pid).bind(run.nav_date)
        .fetch_one(&pool).await.unwrap();
    // Two episodes, because each route's own state predicate would otherwise
    // refuse the request for the wrong reason and prove nothing: `resolve`
    // rejects anything not already `acknowledged`, so only an acknowledged
    // episode can show that the portfolio predicate is what stopped it.
    let mut other = Vec::new();
    for (subject, state, classification) in [
        ("OTHER FUND OPEN", "open", "unclassified"),
        ("OTHER FUND ACKNOWLEDGED", "acknowledged", "passive"),
    ] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO limit_breaches
                 (portfolio_id, check_key, subject, opened_run_id, opened_nav_date, state, classification)
             VALUES ($1, 'issuer_10', $2, $3, $4, $5, $6)
             RETURNING id")
            .bind(other_pid).bind(subject).bind(other_run).bind(run.nav_date)
            .bind(state).bind(classification)
            .fetch_one(&pool).await.unwrap();
        other.push((id, state, classification));
    }

    for ((id, _, _), (route, body)) in other.iter().zip([
        ("acknowledge", r#"{"classification":"active","note":"not mine to sign"}"#),
        ("resolve", r#"{"note":"not mine to close"}"#),
    ]) {
        let res = desktop.clone().oneshot(
            Request::post(format!("/api/portfolios/1/breaches/{id}/{route}"))
                .header("content-type", "application/json")
                .body(Body::from(body)).unwrap()).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY,
            "{route} must not reach an episode of another fund");
    }
    for (id, state, classification) in &other {
        let untouched: (String, String) = sqlx::query_as(
            "SELECT state, classification FROM limit_breaches WHERE id = $1")
            .bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(untouched, (state.to_string(), classification.to_string()),
            "the other fund's episode {id} must be exactly as it was");
    }

    pool.close().await;
    edb.stop().await;
}

/// A manual re-run records a second run row for the same date and, when the
/// episode it re-checks is still breaching, must not open a duplicate — the
/// live-episode uniqueness constraint is only as good as the transition
/// logic that respects it.
#[tokio::test]
async fn a_manual_rerun_records_a_second_run_for_the_same_date() {
    let (desktop, pool, dbh, edb) = app().await;

    // Force a breach that will still be open at the manual re-run: a VaR
    // limit low enough that the sample workbook's real historical VaR
    // exceeds it, regardless of the workbook's own holdings (which otherwise
    // stay under every limit — see api_breach_recorder.rs).
    let res = desktop.clone().oneshot(
        Request::get("/api/portfolios/1/settings").body(Body::empty()).unwrap()).await.unwrap();
    let mut settings: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    settings["var_limit"] = serde_json::json!(0.0000001);
    let res = desktop.clone().oneshot(
        Request::put("/api/portfolios/1/settings")
            .header("content-type", "application/json")
            .body(Body::from(settings.to_string())).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let bytes = std::fs::read(SAMPLE).unwrap();
    let res = desktop.clone().oneshot(upload_req("/api/portfolios/1/imports", &bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let view = scoped.authorize::<Settings, View>(1).unwrap();
    let runs = scoped.runs_for(&view, 50).await.unwrap();
    assert_eq!(runs.len(), 1, "the import records exactly one run");

    let open = scoped.breaches_for(&view, Some("open")).await.unwrap();
    assert_eq!(open.len(), 1, "the lowered var limit must open exactly one episode: {open:?}");
    assert_eq!(open[0].check_key, "var_limit");

    let res = desktop.clone().oneshot(
        Request::post("/api/portfolios/1/limit-runs")
            .header("content-type", "application/json")
            .body(Body::from("{}")).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let runs_after = scoped.runs_for(&view, 50).await.unwrap();
    assert_eq!(runs_after.len(), 2, "the manual re-run must record a second run: {runs_after:?}");
    assert_eq!(runs_after[0].0.triggered_by, "manual", "the newer run is the manual one");
    assert_eq!(runs_after[1].0.triggered_by, "import");
    assert_eq!(runs_after[0].0.nav_date, runs_after[1].0.nav_date,
        "the manual re-run defaults to the same, latest, snapshot date");

    let open_after = scoped.breaches_for(&view, Some("open")).await.unwrap();
    assert_eq!(open_after.len(), 1,
        "a re-run of a still-breaching check must not open a duplicate episode: {open_after:?}");
    assert_eq!(open_after[0].id, open[0].id, "the same episode, not a new one");

    // A back-dated re-run is refused. Honouring one would compute findings
    // from an earlier day's holdings and then close every live episode absent
    // from them — `transitions` cannot tell a back-dated run from a fund that
    // has cleared — writing `closed_nav_date` and a `cleared` event onto an
    // episode that never cleared.
    let res = desktop.clone().oneshot(
        Request::post("/api/portfolios/1/limit-runs")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"date":"2000-01-01"}"#)).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY,
        "the register is not backfillable through the re-run endpoint");

    assert_eq!(scoped.runs_for(&view, 50).await.unwrap().len(), 2,
        "a refused re-run must not record a run");
    let after_refusal = scoped.breaches_for(&view, Some("open")).await.unwrap();
    assert_eq!(after_refusal.len(), 1, "the live episode must survive a refused re-run");
    assert_eq!(after_refusal[0].id, open[0].id);
    assert_eq!(after_refusal[0].closed_nav_date, None,
        "nothing may have been marked cleared by a request that was refused");

    // The same date, stated explicitly, is accepted — the field lets a caller
    // say which date it believes it is re-checking, and that is all.
    let today = runs_after[0].0.nav_date;
    let res = desktop.clone().oneshot(
        Request::post("/api/portfolios/1/limit-runs")
            .header("content-type", "application/json")
            .body(Body::from(format!(r#"{{"date":"{today}"}}"#))).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    pool.close().await;
    edb.stop().await;
}
