use chrono::NaiveDate;
use db::auth::marker::{Configure, Reference, View};
use db::auth::AuthCtx;

#[tokio::test]
async fn kpi_upsert_round_trip_and_constraints() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    let pool = dbh.test_pool().clone();
    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let configure = scoped.authorize::<Reference, Configure>(1).unwrap();
    let view = scoped.authorize::<Reference, View>(1).unwrap();

    let d = |s: &str| s.parse::<NaiveDate>().unwrap();
    let k = db::repo::EmirKpi {
        month: d("2026-07-01"),
        unconfirmed_over_5d: 2,
        reconciliation: "done".into(),
        disputes: 0,
        note: Some("one late FX forward confirmation".into()),
    };
    scoped.emir_kpi_upsert(&configure, &k).await.unwrap();

    // Upsert on the same month replaces, not duplicates.
    let k2 = db::repo::EmirKpi { disputes: 1, note: None, ..k.clone() };
    scoped.emir_kpi_upsert(&configure, &k2).await.unwrap();

    let earlier = db::repo::EmirKpi { month: d("2026-06-01"), ..k2.clone() };
    scoped.emir_kpi_upsert(&configure, &earlier).await.unwrap();

    let all = scoped.emir_kpis_all(&view).await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].month, d("2026-07-01")); // DESC order
    assert_eq!(all[0].disputes, 1);
    assert_eq!(all[0].note, None);

    // Mid-month date violates the first-of-month CHECK.
    let bad = db::repo::EmirKpi { month: d("2026-07-15"), ..k2.clone() };
    assert!(scoped.emir_kpi_upsert(&configure, &bad).await.is_err());
    // Unknown reconciliation value violates its CHECK.
    let bad = db::repo::EmirKpi { reconciliation: "maybe".into(), ..k2.clone() };
    assert!(scoped.emir_kpi_upsert(&configure, &bad).await.is_err());

    pool.close().await;
    edb.stop().await;
}
