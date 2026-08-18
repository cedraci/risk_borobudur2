use db::auth::AuthCtx;

#[tokio::test]
async fn settings_defaults_and_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let dbh = db::Db::from_pool(pool.clone());
    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);

    let s = scoped.get_settings(1).await.unwrap();
    assert!((s.risk_free_rate - 0.02).abs() < 1e-12);
    assert!((s.var_confidence - 0.99).abs() < 1e-12);
    assert_eq!(s.var_horizon_days, 20);
    assert_eq!(s.var_window_days, 252);
    assert!((s.var_limit - 0.20).abs() < 1e-12);
    assert_eq!(s.short_dd_max_days, 50);

    let mut s2 = s.clone();
    s2.risk_free_rate = 0.031;
    s2.var_horizon_days = 10;
    scoped.put_settings(1, &s2).await.unwrap();
    let s3 = scoped.get_settings(1).await.unwrap();
    assert!((s3.risk_free_rate - 0.031).abs() < 1e-12);
    assert_eq!(s3.var_horizon_days, 10);

    pool.close().await;
    edb.stop().await;
}
