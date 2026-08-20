//! The register is the fund's compliance record, not a transcript of what one
//! user could see. A run triggered by a principal without reference access
//! must still be computed on the real issuer groups — otherwise finding P3
//! (a denial rendering as data) comes back, persisted and harder to notice.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use db::auth::marker::{Settings, View};
use db::auth::{Action, AuthCtx, Domain, Grant};
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
    assert!(!runs.is_empty(), "an import must record a run");
    let (run, results) = &runs[0];
    assert_eq!(run.triggered_by, "import");
    assert!(run.import_id.is_some(), "the run must point at the import that caused it");
    assert!(results.iter().any(|r| r.check_key == "issuer_10"),
        "the concentration checks must be recorded: {:?}",
        results.iter().map(|r| &r.check_key).collect::<Vec<_>>());

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
    assert!(run.inputs_complete,
        "inputs_complete is about missing data, never about the caller's grants");
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
