#[tokio::test]
async fn settings_v2_fields_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    let mut s = db::settings::get_settings(&pool).await.unwrap();
    assert!((s.redemption_shock - 0.30).abs() < 1e-12);
    assert_eq!(s.liquidity_defaults["Fonds"], "d2_7");
    assert_eq!(s.liquidity_defaults["Obligation"], "d8_30");

    s.redemption_shock = 0.25;
    s.liquidity_defaults["Fonds"] = serde_json::json!("d8_30");
    db::settings::put_settings(&pool, &s).await.unwrap();

    let s2 = db::settings::get_settings(&pool).await.unwrap();
    assert!((s2.redemption_shock - 0.25).abs() < 1e-12);
    assert_eq!(s2.liquidity_defaults["Fonds"], "d8_30");

    pool.close().await;
    edb.stop().await;
}
