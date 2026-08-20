use chrono::NaiveDate;
use db::auth::marker::{Configure, Settings, View};
use db::auth::AuthCtx;
use db::repo::{CheckResultRow, NewRun};

async fn fresh() -> (db::Db, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    std::mem::forget(dir);
    (dbh, edb)
}

fn result(key: &str, status: &str, observed: Option<f64>) -> CheckResultRow {
    CheckResultRow {
        check_key: key.into(),
        scope_label: "Issuer <= 10% NAV (equities + bonds)".into(),
        limit_value: Some(0.10),
        observed_value: observed,
        status: status.into(),
        detail: serde_json::json!({"rows": []}),
    }
}

#[tokio::test]
async fn a_run_and_its_results_round_trip() {
    let (dbh, edb) = fresh().await;
    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let configure = scoped.authorize::<Settings, Configure>(1).unwrap();
    let view = scoped.authorize::<Settings, View>(1).unwrap();

    let run = NewRun {
        nav_date: NaiveDate::from_ymd_opt(2026, 8, 7).unwrap(),
        triggered_by: "import".into(),
        import_id: None,
        actor_user_id: None,
        inputs_complete: true,
        input_notes: serde_json::json!({}),
        results: vec![
            result("issuer_10", "breach", Some(0.106)),
            result("group_20", "ok", Some(0.04)),
        ],
    };
    let run_id = scoped.record_run(&configure, &run).await.unwrap();
    assert!(run_id > 0);

    let rows = scoped.runs_for(&view, 50).await.unwrap();
    assert_eq!(rows.len(), 1, "one run recorded");
    let (recorded, results) = &rows[0];
    assert_eq!(recorded.nav_date, NaiveDate::from_ymd_opt(2026, 8, 7).unwrap());
    assert_eq!(recorded.triggered_by, "import");
    assert!(recorded.inputs_complete);
    assert_eq!(results.len(), 2);
    let issuer = results.iter().find(|r| r.check_key == "issuer_10").unwrap();
    assert_eq!(issuer.status, "breach");
    assert_eq!(issuer.observed_value, Some(0.106));
    assert_eq!(issuer.detail["rows"], serde_json::json!([]));

    edb.stop().await;
}

#[tokio::test]
async fn a_result_with_no_natural_scalar_pair_stores_nulls() {
    let (dbh, edb) = fresh().await;
    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let configure = scoped.authorize::<Settings, Configure>(1).unwrap();
    let view = scoped.authorize::<Settings, View>(1).unwrap();

    let run = NewRun {
        nav_date: NaiveDate::from_ymd_opt(2026, 8, 7).unwrap(),
        triggered_by: "manual".into(),
        import_id: None,
        actor_user_id: None,
        inputs_complete: false,
        input_notes: serde_json::json!({"shareholders": "no register loaded"}),
        results: vec![CheckResultRow {
            check_key: "liq_top5".into(),
            scope_label: "Top 5 holders".into(),
            limit_value: None,
            observed_value: None,
            status: "breach".into(),
            detail: serde_json::json!({"waterfall": {"days": null}}),
        }],
    };
    scoped.record_run(&configure, &run).await.unwrap();

    let rows = scoped.runs_for(&view, 50).await.unwrap();
    let (recorded, results) = &rows[0];
    assert!(!recorded.inputs_complete);
    assert_eq!(recorded.input_notes["shareholders"], "no register loaded");
    assert_eq!(results[0].limit_value, None);
    assert_eq!(results[0].observed_value, None);

    edb.stop().await;
}
