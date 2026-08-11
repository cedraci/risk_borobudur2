use db::repo;

#[tokio::test]
async fn codes_roundtrip_and_uniqueness() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    // Borobudur is portfolio 1 (seeded by 0008); create a second portfolio.
    let p2 = repo::portfolio_create(&pool, "Mandat A", "mandate").await.unwrap();

    repo::portfolio_codes_replace(&pool, 1, &[("caceis".into(), "165878".into())]).await.unwrap();
    assert_eq!(repo::portfolio_by_code(&pool, "caceis", "165878").await.unwrap(), Some(1));
    assert_eq!(repo::portfolio_by_code(&pool, "caceis", "999999").await.unwrap(), None);

    // Replace removes what the new set omits.
    repo::portfolio_codes_replace(&pool, 1, &[("caceis".into(), "111111".into())]).await.unwrap();
    assert_eq!(repo::portfolio_by_code(&pool, "caceis", "165878").await.unwrap(), None);
    let codes = repo::portfolio_codes_for(&pool, 1).await.unwrap();
    assert_eq!(codes.len(), 1);
    assert_eq!(codes[0].code, "111111");

    // A code claimed by portfolio 1 cannot also be claimed by portfolio 2.
    let err = repo::portfolio_codes_replace(&pool, p2.id, &[("caceis".into(), "111111".into())]).await;
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
