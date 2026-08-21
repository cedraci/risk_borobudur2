//! The register is the fund's compliance record, not a transcript of what one
//! user could see. A run triggered by a principal without reference access
//! must still be computed on the real issuer groups — otherwise finding P3
//! (a denial rendering as data) comes back, persisted and harder to notice.

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
async fn an_import_records_one_run_per_snapshot_date() {
    let (desktop, pool, dbh, edb) = app().await;
    let bytes = std::fs::read(SAMPLE).unwrap();
    let res = desktop.clone().oneshot(upload_req("/api/portfolios/1/imports", &bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let view = scoped.authorize::<Settings, View>(1).unwrap();
    let runs = scoped.runs_for(&view, 50).await.unwrap();
    // `sample.xlsx` (crates/ingest/tests/fixtures/sample.xlsx) parses to a
    // single positions snapshot date (`wb.nav_date == 2026-07-24`, asserted
    // in crates/ingest/tests/parse_sample.rs) — one snapshot date, so exactly
    // one run, not merely "at least one".
    assert_eq!(runs.len(), 1,
        "sample.xlsx has exactly one snapshot date, so one import must record exactly one run: {runs:?}");
    let (run, results) = &runs[0];
    assert_eq!(run.triggered_by, "import");
    assert!(run.import_id.is_some(), "the run must point at the import that caused it");
    assert!(results.iter().any(|r| r.check_key == "issuer_10"),
        "the concentration checks must be recorded: {:?}",
        results.iter().map(|r| &r.check_key).collect::<Vec<_>>());

    // Re-uploading the same file is a duplicate import (same content hash);
    // it must not double-record the run, or the register's evidence trail
    // would no longer correspond 1:1 with what was actually imported.
    let res2 = desktop.clone().oneshot(upload_req("/api/portfolios/1/imports", &bytes)).await.unwrap();
    assert_eq!(res2.status(), StatusCode::OK);
    let runs_after = scoped.runs_for(&view, 50).await.unwrap();
    assert_eq!(runs_after.len(), 1,
        "re-uploading the same file must not record an additional run: {runs_after:?}");

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn a_run_uses_the_real_reference_data_even_when_the_importer_cannot_see_it() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    let pool = dbh.test_pool().clone();
    std::mem::forget(dir);
    let desktop = server::routes::router(server::state::AppState::desktop(dbh.clone()));
    let server = server::routes::router(server::state::AppState::server(dbh.clone()));

    // An issuer-group override that regroups two holdings under one issuer.
    // With reference data denied, the checks would fall back to the default
    // per-name grouping and under-aggregate.
    let admin_ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&admin_ctx);
    let rc = scoped.authorize_global::<db::auth::marker::Reference, db::auth::marker::Configure>().unwrap();
    for code in ["AT000000STR1", "AT0000606306"] {
        scoped.refs_upsert(&rc, &db::repo::InstrumentRef {
            code: code.into(),
            issuer_group: Some("SHARED GROUP".into()),
            liquidity_days: None,
            adv_eligible: None,
            bond_coupon_pct: None, bond_maturity: None, bond_coupon_freq: None,
            bond_next_coupon: None, bond_nominal: None,
            market_place: None, market_place_name: None,
            adv_30d: None, adv_asof: None,
            country_of_risk: None, region: None, gics_sector: None, gics_industry: None,
            ticker: None,
        }).await.unwrap();
    }

    // An importer with import rights and NO reference grant at all.
    let hash = server::auth::local::hash_password("pw").unwrap();
    let admin = db::admin::Admin::new(&pool);
    let uid = admin.create_user("ops@f.lu", "Ops", &hash, false).await.unwrap();
    for d in [Domain::Positions, Domain::Nav, Domain::Transactions] {
        admin.grant_add(uid, Grant { domain: d, action: Action::Import, portfolio: Some(1) }, None).await.unwrap();
    }
    admin.session_create(&server::auth::local::token_hash("ops-t"), uid, 1).await.unwrap();

    let bytes = std::fs::read(SAMPLE).unwrap();
    let mut req = upload_req("/api/portfolios/1/imports", &bytes);
    req.headers_mut().insert("cookie", "borobudur_session=ops-t".parse().unwrap());
    let res = server.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let view = scoped.authorize::<Settings, View>(1).unwrap();
    let runs = scoped.runs_for(&view, 50).await.unwrap();
    let (run, results) = &runs[0];
    // The Ops principal cannot read the reference table. That must not show
    // up as an input problem: the ONLY thing missing from this run is the
    // shareholder register, which no import loads — never a grant the caller
    // happened not to have.
    let notes = run.input_notes.as_object().expect("input_notes is always a JSON object");
    // Pin the map non-empty first: `all` is vacuously true over no keys, so
    // without this the two assertions below would pass while proving nothing
    // if `compute` ever stopped emitting notes at all. No import loads a
    // shareholder register, so this key is always present here.
    assert!(notes.contains_key("liq_top5"),
        "the known-absent input must be noted, or the checks below prove nothing: {notes:?}");
    let noted: Vec<&String> = notes.keys().collect();
    assert!(noted.iter().all(|k| k.starts_with("liq_")),
        "a denial must never surface as an absent input: {noted:?}");
    // The prefix check above is a proxy; this is the real one — whatever the
    // key, no note may read like a permission problem.
    for (k, v) in notes.iter() {
        let text = v.as_str().unwrap_or_default().to_lowercase();
        assert!(!text.contains("permit") && !text.contains("denied")
                && !text.contains("grant") && !text.contains("access"),
            "input note {k} reads like a permission problem: {text}");
    }
    let issuer = results.iter().find(|r| r.check_key == "issuer_10").unwrap();
    assert_ne!(issuer.status, "unavailable",
        "a recorded run must never carry a denial marker: {issuer:?}");
    let groups: Vec<String> = issuer.detail["rows"].as_array().unwrap_or(&vec![])
        .iter().filter_map(|r| r["group"].as_str().map(str::to_string)).collect();
    assert!(groups.iter().any(|g| g == "SHARED GROUP"),
        "the override must have been applied under the system context, got {groups:?}");

    let _ = desktop;
    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn a_run_covers_every_check_that_has_a_limit() {
    let (desktop, pool, dbh, edb) = app().await;
    let bytes = std::fs::read(SAMPLE).unwrap();
    let res = desktop.clone().oneshot(upload_req("/api/portfolios/1/imports", &bytes)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let view = scoped.authorize::<Settings, View>(1).unwrap();
    let (run, results) = &scoped.runs_for(&view, 50).await.unwrap()[0];
    let keys: std::collections::BTreeSet<&str> =
        results.iter().map(|r| r.check_key.as_str()).collect();

    // Sample.xlsx loads no shareholder register (that's a separate PUT
    // endpoint), so liq_top5/liq_hybrid_top5 are the two register-dependent
    // scenarios and are deliberately excluded from this list — see below.
    for expected in ["issuer_10", "forty", "group_20", "fund_20", "deposit_20",
                     "liq_fixed", "liq_hybrid_fixed",
                     "var_limit",
                     "emir_credit", "emir_equity", "emir_interest_rate", "emir_fx",
                     "emir_commodity_other"] {
        assert!(keys.contains(expected), "missing {expected} from a run: {keys:?}");
    }

    // No shareholder register was loaded, so the top-5 redemption scenarios
    // could not run. They must be ABSENT and explained — never recorded as a
    // pass on a stress test that never happened.
    assert!(!keys.contains("liq_top5") && !keys.contains("liq_hybrid_top5"),
        "a scenario that could not be evaluated must not be recorded: {keys:?}");
    let notes = run.input_notes.as_object().expect("input_notes is always a JSON object");
    assert!(notes.contains_key("liq_top5"),
        "the skipped scenario must say why it was skipped: {notes:?}");
    assert!(!run.inputs_complete,
        "a run missing the shareholder register is not a complete run");

    // The liquidity scenarios that DID run have no honest scalar pair.
    let liq = results.iter().find(|r| r.check_key == "liq_fixed").unwrap();
    assert_eq!(liq.limit_value, None);
    assert_eq!(liq.observed_value, None);
    assert!(!liq.scope_label.is_empty());

    // VaR does: the configured limit against the measured utilisation.
    let var = results.iter().find(|r| r.check_key == "var_limit").unwrap();
    assert!(var.limit_value.is_some(), "var_limit stores the configured limit");

    pool.close().await;
    edb.stop().await;
}

// ---- C2: a back-dated import must not rewrite episode state ---------------

const HISINV: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/caceis_hisinv.csv");

fn upload_named(uri: &str, filename: &str, bytes: &[u8]) -> Request<Body> {
    let mut body = Vec::new();
    body.extend_from_slice(format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    ).as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    Request::post(uri)
        .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(Body::from(body)).unwrap()
}

/// C2 (whole-branch review): a corrected or late depositary file dated before
/// the register's current state must not falsify the register.
///
/// `analytics::breach::transitions` emits `Close` for every live episode
/// absent from a run's findings — it cannot tell a back-dated run from a fund
/// that has cleared. `rerun` refuses a non-latest date outright and explains
/// why; the import hook had no equivalent guard, so re-issuing a CACEIS file
/// for an earlier day (an entirely ordinary act) stamped `closed_nav_date`
/// *before* `opened_nav_date` on every open episode, appended a falsified
/// `cleared` event, and dropped the episode out of the register's own
/// "open on the data" view — all behind a 200 and a `tracing::error!`.
///
/// The run itself is still recorded: a back-dated file is legitimate history
/// and the register is meant to be complete. Only the transition phase is
/// skipped, and the skip is written into `input_notes`.
#[tokio::test]
async fn a_back_dated_import_records_its_run_and_leaves_the_episodes_alone() {
    let (desktop, pool, dbh, edb) = app().await;

    // Map the CACEIS fund code onto portfolio 1 so the CSV lands there.
    let res = desktop.clone().oneshot(
        Request::put("/api/portfolios/1/codes")
            .header("content-type", "application/json")
            .body(Body::from(r#"[{"source":"caceis","code":"165878"}]"#)).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let latest = std::fs::read(HISINV).unwrap();
    let res = desktop.clone().oneshot(upload_named(
        "/api/portfolios/1/imports",
        "HISINVLUX_165878_20260807_20260810130151.csv", &latest)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let view = scoped.authorize::<Settings, View>(1).unwrap();
    let runs = scoped.runs_for(&view, 50).await.unwrap();
    assert_eq!(runs.len(), 1, "the first import records one run: {runs:?}");
    let (run, _) = &runs[0];
    assert_eq!(run.nav_date, chrono::NaiveDate::from_ymd_opt(2026, 8, 7).unwrap());

    // A live episode, open since the day this file is dated. This fixture's
    // holdings breach nothing on their own, so the episode is inserted
    // directly — what is under test is what a LATER, EARLIER-DATED import
    // does to it, not how it came to exist.
    let opened = run.nav_date;
    let bid: i64 = sqlx::query_scalar(
        "INSERT INTO limit_breaches
             (portfolio_id, check_key, subject, opened_run_id, opened_nav_date,
              opened_value, peak_value, peak_nav_date)
         VALUES (1, 'issuer_10', 'ACME', $1, $2, 0.15, 0.15, $2) RETURNING id")
        .bind(run.id).bind(opened).fetch_one(&pool).await.unwrap();
    assert_eq!(scoped.live_episodes(&view).await.unwrap().len(), 1,
        "the episode must be live before the back-dated import, or this proves nothing");

    // The same depositary file, re-issued for an earlier day: a different
    // content hash, so not a duplicate import, and the recorder runs for
    // 2026-07-15 — three weeks before the episode opened.
    let back = String::from_utf8(latest.clone()).unwrap().replace("20260807;", "20260715;");
    assert_ne!(back.as_bytes(), &latest[..], "the back-dated file must actually differ");
    let res = desktop.clone().oneshot(upload_named(
        "/api/portfolios/1/imports",
        "HISINVLUX_165878_20260715_20260716130151.csv", back.as_bytes())).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(body[0]["error"].is_null(), "the back-dated file must still import: {body}");

    // The run IS recorded — the register is meant to be complete, and a
    // back-dated file is real history for the date it carries.
    let runs = scoped.runs_for(&view, 50).await.unwrap();
    assert_eq!(runs.len(), 2, "the back-dated run must still be recorded: {runs:?}");
    // ...and the episode lifecycle is untouched.
    let b = scoped.breach_get(&view, bid).await.unwrap().unwrap();
    assert_eq!(b.closed_nav_date, None,
        "a run for a date before the episode opened must not close it");
    assert_eq!(b.opened_nav_date, opened);
    let kinds: Vec<String> = scoped.breach_events(&view, bid).await.unwrap()
        .into_iter().map(|e| e.event).collect();
    assert!(!kinds.iter().any(|k| k == "cleared"),
        "no falsified `cleared` event may be appended: {kinds:?}");
    assert_eq!(scoped.live_episodes(&view).await.unwrap().len(), 1,
        "the episode must still be live on the data");

    // The skip is stated, never left for a reader to infer.
    let back_run = runs.iter().map(|(r, _)| r)
        .find(|r| r.nav_date == chrono::NaiveDate::from_ymd_opt(2026, 7, 15).unwrap())
        .expect("a run for the back-dated day");
    let note = back_run.input_notes[db::repo::TRANSITIONS_SKIPPED_NOTE].as_str()
        .unwrap_or_else(|| panic!("the skip must be recorded, not left to be inferred: {}", back_run.input_notes));
    assert!(note.contains("2026-07-15") && note.contains("2026-08-07"),
        "the note must name both dates so a reader can see why: {note}");

    pool.close().await;
    edb.stop().await;
}
