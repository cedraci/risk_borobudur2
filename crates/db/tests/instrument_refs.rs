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
        liquidity_bucket: Some("d8_30".into()),
        bond_coupon_pct: None, bond_maturity: None, bond_coupon_freq: None,
    };
    db::repo::refs_upsert(&pool, &r).await.unwrap();
    let all = db::repo::refs_all(&pool).await.unwrap();
    let got = all.iter().find(|x| x.code == "TEST1").unwrap();
    assert_eq!(got.issuer_group.as_deref(), Some("GROUP A"));
    assert_eq!(got.liquidity_bucket.as_deref(), Some("d8_30"));

    // 2. full-row replace: None reverts to NULL
    let r2 = InstrumentRef { code: "TEST1".into(), issuer_group: None, liquidity_bucket: None,
        bond_coupon_pct: None, bond_maturity: None, bond_coupon_freq: None };
    db::repo::refs_upsert(&pool, &r2).await.unwrap();
    let all = db::repo::refs_all(&pool).await.unwrap();
    let got = all.iter().find(|x| x.code == "TEST1").unwrap();
    assert!(got.issuer_group.is_none() && got.liquidity_bucket.is_none());

    // 3. pre-seed a user override for the fixture bond, then import: the
    // user's coupon must survive; the empty maturity/freq get filled.
    let user = InstrumentRef { code: "US105756CL22".into(), issuer_group: None, liquidity_bucket: None,
        bond_coupon_pct: Some(7.0), bond_maturity: None, bond_coupon_freq: None };
    db::repo::refs_upsert(&pool, &user).await.unwrap();

    let wb = ingest::parse_workbook(&fixture_bytes()).unwrap();
    db::repo::import_workbook(&pool, "sample.xlsx", "sha-refs-test", &wb).await.unwrap();

    let all = db::repo::refs_all(&pool).await.unwrap();
    let bond = all.iter().find(|x| x.code == "US105756CL22").unwrap();
    assert_eq!(bond.bond_coupon_pct, Some(7.0)); // user value kept
    assert_eq!(bond.bond_maturity, Some(chrono::NaiveDate::from_ymd_opt(2035, 3, 15).unwrap()));
    assert_eq!(bond.bond_coupon_freq, Some(2));

    pool.close().await;
    edb.stop().await;
}
