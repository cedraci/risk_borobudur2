use db::auth::marker::{Configure, Reference, View};
use db::auth::AuthCtx;

#[tokio::test]
async fn codes_roundtrip_and_uniqueness() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    let pool = dbh.test_pool().clone();
    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);

    // Borobudur is portfolio 1 (seeded by 0008); create a second portfolio.
    let global_configure = scoped.authorize_global::<Reference, Configure>().unwrap();
    let p2 = scoped.portfolio_create(&global_configure, "Mandat A", "mandate").await.unwrap();

    let configure1 = scoped.authorize::<Reference, Configure>(1).unwrap();
    let view1 = scoped.authorize::<Reference, View>(1).unwrap();

    scoped.portfolio_codes_replace(&configure1, &[("caceis".into(), "165878".into())]).await.unwrap();
    assert_eq!(scoped.portfolio_by_code("caceis", "165878").await.unwrap(), Some(1));
    assert_eq!(scoped.portfolio_by_code("caceis", "999999").await.unwrap(), None);

    // Replace removes what the new set omits.
    scoped.portfolio_codes_replace(&configure1, &[("caceis".into(), "111111".into())]).await.unwrap();
    assert_eq!(scoped.portfolio_by_code("caceis", "165878").await.unwrap(), None);
    let codes = scoped.portfolio_codes_for(&view1).await.unwrap();
    assert_eq!(codes.len(), 1);
    assert_eq!(codes[0].code, "111111");

    // A code claimed by portfolio 1 cannot also be claimed by portfolio 2.
    let configure2 = scoped.authorize::<Reference, Configure>(p2.id).unwrap();
    let err = scoped.portfolio_codes_replace(&configure2, &[("caceis".into(), "111111".into())]).await;
    assert!(err.is_err(), "duplicate (source, code) across portfolios must fail");

    // dividends.derived exists and defaults false.
    let derived: bool = sqlx::query_scalar(
        "INSERT INTO dividends (portfolio_id, provision_date, issuer, amount, currency)
         VALUES (1, '2026-08-07', 'X', 1, 'EUR') RETURNING derived")
        .fetch_one(&pool).await.unwrap();
    assert!(!derived);

    pool.close().await;
    edb.stop().await;
}
