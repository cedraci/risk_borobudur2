use db::auth::AuthCtx;

#[tokio::test]
async fn settings_v2_fields_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let dbh = db::Db::from_pool(pool.clone());
    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);

    let mut s = scoped.get_settings(1).await.unwrap();
    assert!((s.redemption_shock - 0.30).abs() < 1e-12);
    assert_eq!(s.liquidity_default_days["Fonds"], 7);
    assert_eq!(s.liquidity_default_days["Obligation"], 30);

    s.redemption_shock = 0.25;
    s.liquidity_default_days["Fonds"] = serde_json::json!(30);
    scoped.put_settings(1, &s).await.unwrap();

    let s2 = scoped.get_settings(1).await.unwrap();
    assert!((s2.redemption_shock - 0.25).abs() < 1e-12);
    assert_eq!(s2.liquidity_default_days["Fonds"], 30);

    pool.close().await;
    edb.stop().await;
}
