use db::auth::marker::{Configure, Reference, View};
use db::auth::AuthCtx;

#[tokio::test]
async fn contracts_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let dbh = db::Db::from_pool(pool.clone());
    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let view = scoped.authorize_global::<Reference, View>().unwrap();
    let configure = scoped.authorize_global::<Reference, Configure>().unwrap();

    assert!(scoped.contracts_all(&view).await.unwrap().is_empty());

    let c = db::repo::FuturesContract {
        contract_root: "RX".into(),
        label: "Euro-Bund".into(),
        category: "interest_rate".into(),
        point_value: Some(1000.0),
        currency: "EUR".into(),
        curve: Some("DE-10y".into()),
        price_convention: "decimal".into(),
        confirmed: true,
        otc: false,
    };
    scoped.contracts_upsert(&configure, &c).await.unwrap();

    let all = scoped.contracts_all(&view).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].contract_root, "RX");
    assert_eq!(all[0].point_value, Some(1000.0));
    assert_eq!(all[0].curve.as_deref(), Some("DE-10y"));
    assert!(all[0].confirmed);

    // upsert replaces the whole row
    let c2 = db::repo::FuturesContract { curve: None, confirmed: false, ..c };
    scoped.contracts_upsert(&configure, &c2).await.unwrap();
    let all = scoped.contracts_all(&view).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].curve, None);
    assert!(!all[0].confirmed);

    // CHECK constraints reject invalid enums and non-positive point values
    assert!(scoped.contracts_upsert(&configure, &db::repo::FuturesContract {
        contract_root: "ZZ".into(), category: "nonsense".into(), ..c2.clone()
    }).await.is_err());
    assert!(scoped.contracts_upsert(&configure, &db::repo::FuturesContract {
        contract_root: "YY".into(), point_value: Some(0.0), ..c2.clone()
    }).await.is_err());

    pool.close().await;
    edb.stop().await;
}
