use db::repo::InstrumentRef;

fn fixture_bytes() -> Vec<u8> {
    std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx")).unwrap()
}

#[tokio::test]
async fn refs_upsert_seed_and_no_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    // 1. plain upsert + read-back
    let r = InstrumentRef {
        code: "TEST1".into(),
        issuer_group: Some("GROUP A".into()),
        liquidity_days: Some(30.0),
        adv_eligible: None,
        bond_coupon_pct: None, bond_maturity: None, bond_coupon_freq: None,
        bond_next_coupon: None, bond_nominal: None,
        market_place: None, market_place_name: None,
        adv_30d: None, adv_asof: None,
        country_of_risk: None, region: None, gics_sector: None, gics_industry: None, ticker: None,
    };
    db::repo::refs_upsert(&pool, &r).await.unwrap();
    let all = db::repo::refs_all(&pool).await.unwrap();
    let got = all.iter().find(|x| x.code == "TEST1").unwrap();
    assert_eq!(got.issuer_group.as_deref(), Some("GROUP A"));
    assert_eq!(got.liquidity_days, Some(30.0));

    // 2. full-row replace: None reverts to NULL
    let r2 = InstrumentRef { code: "TEST1".into(), issuer_group: None, liquidity_days: None,
        adv_eligible: None,
        bond_coupon_pct: None, bond_maturity: None, bond_coupon_freq: None,
        bond_next_coupon: None, bond_nominal: None,
        market_place: None, market_place_name: None,
        adv_30d: None, adv_asof: None,
        country_of_risk: None, region: None, gics_sector: None, gics_industry: None, ticker: None };
    db::repo::refs_upsert(&pool, &r2).await.unwrap();
    let all = db::repo::refs_all(&pool).await.unwrap();
    let got = all.iter().find(|x| x.code == "TEST1").unwrap();
    assert!(got.issuer_group.is_none() && got.liquidity_days.is_none());

    // 3. pre-seed a user override for the fixture bond, then import: the
    // user's coupon must survive; the empty maturity/freq get filled.
    let user = InstrumentRef { code: "US105756CL22".into(), issuer_group: None, liquidity_days: None,
        adv_eligible: None,
        bond_coupon_pct: Some(7.0), bond_maturity: None, bond_coupon_freq: None,
        bond_next_coupon: None, bond_nominal: None,
        market_place: None, market_place_name: None,
        adv_30d: None, adv_asof: None,
        country_of_risk: None, region: None, gics_sector: None, gics_industry: None, ticker: None };
    db::repo::refs_upsert(&pool, &user).await.unwrap();

    let wb = ingest::parse_workbook(&fixture_bytes()).unwrap();
    db::repo::import_workbook(&pool, 1, "sample.xlsx", "sha-refs-test", &wb).await.unwrap();

    let all = db::repo::refs_all(&pool).await.unwrap();
    let bond = all.iter().find(|x| x.code == "US105756CL22").unwrap();
    assert_eq!(bond.bond_coupon_pct, Some(7.0)); // user value kept
    assert_eq!(bond.bond_maturity, Some(chrono::NaiveDate::from_ymd_opt(2035, 3, 15).unwrap()));
    assert_eq!(bond.bond_coupon_freq, Some(2));

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn liquidity_days_replaces_bucket_and_new_columns_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    let r = InstrumentRef {
        code: "FR0000121014".into(),
        issuer_group: Some("LVMH".into()),
        liquidity_days: Some(3.5),
        adv_eligible: Some(false),
        bond_coupon_pct: None,
        bond_maturity: None,
        bond_coupon_freq: None,
        bond_next_coupon: None,
        bond_nominal: None,
        adv_30d: None,
        adv_asof: None,
        market_place: None,
        market_place_name: None,
        country_of_risk: None,
        region: None,
        gics_sector: None,
        gics_industry: None,
        ticker: None,
    };
    db::repo::refs_upsert(&pool, &r).await.unwrap();

    let back = db::repo::refs_all(&pool).await.unwrap();
    let got = back.iter().find(|x| x.code == "FR0000121014").unwrap();
    assert_eq!(got.liquidity_days, Some(3.5));
    assert_eq!(got.adv_eligible, Some(false));
    // refs_upsert never writes depositary- or Bloomberg-owned columns.
    assert_eq!(got.adv_30d, None);
    assert_eq!(got.market_place, None);

    pool.close().await;
    edb.stop().await;
}
