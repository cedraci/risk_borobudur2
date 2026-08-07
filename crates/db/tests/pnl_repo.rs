use chrono::NaiveDate;

fn d(y: i32, m: u32, dd: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, dd).unwrap() }

#[tokio::test]
async fn fx_upsert_is_idempotent_and_readable() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    let rows = vec![
        db::repo::FxRow { date: d(2026, 7, 24), currency: "USD".into(), rate_to_eur: 0.8788 },
        db::repo::FxRow { date: d(2026, 7, 24), currency: "GBP".into(), rate_to_eur: 1.1726 },
    ];
    assert_eq!(db::repo::fx_upsert_many(&pool, &rows).await.unwrap(), 2);
    // Re-upserting the same dates must overwrite, not duplicate.
    let rows2 = vec![db::repo::FxRow { date: d(2026, 7, 24), currency: "USD".into(), rate_to_eur: 0.88 }];
    db::repo::fx_upsert_many(&pool, &rows2).await.unwrap();

    let all = db::repo::fx_all(&pool).await.unwrap();
    assert_eq!(all.len(), 2);
    let usd = all.iter().find(|r| r.currency == "USD").unwrap();
    assert!((usd.rate_to_eur - 0.88).abs() < 1e-12);

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn classify_upsert_never_overwrites_an_existing_value() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    db::repo::classify_upsert_many(
        &pool,
        &[("FR0000121014".into(), Some("MC FP Equity".into()), Some("France".into()), Some("Europe".into()),
           Some("Consumer Discretionary".into()), Some("Textiles Apparel & Luxury Goods".into()))],
    ).await.unwrap();

    // A second load carrying different values must not clobber the first.
    db::repo::classify_upsert_many(
        &pool,
        &[("FR0000121014".into(), Some("WRONG Equity".into()), Some("Wrong".into()), None, None, None)],
    ).await.unwrap();

    let refs = db::repo::refs_all(&pool).await.unwrap();
    let r = refs.iter().find(|r| r.code == "FR0000121014").unwrap();
    assert_eq!(r.ticker.as_deref(), Some("MC FP Equity"));
    assert_eq!(r.country_of_risk.as_deref(), Some("France"));
    assert_eq!(r.gics_sector.as_deref(), Some("Consumer Discretionary"));

    pool.close().await;
    edb.stop().await;
}
