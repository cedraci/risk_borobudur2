const SAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx");

#[tokio::test]
async fn import_seeds_futures_contracts_unconfirmed() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    let bytes = std::fs::read(SAMPLE).unwrap();
    let wb = ingest::parse_workbook(&bytes).unwrap();
    let out = db::repo::import_workbook(&pool, "s.xlsx", "sha-seed", &wb).await.unwrap();
    assert!(!out.duplicate);

    let cs = db::repo::contracts_all(&pool).await.unwrap();
    let roots: Vec<&str> = cs.iter().map(|c| c.contract_root.as_str()).collect();
    assert_eq!(roots, vec!["CF", "KOA", "NQ", "OAT", "RX", "RY", "TY", "VG"], "one row per root, sorted");

    let cf = cs.iter().find(|c| c.contract_root == "CF").unwrap();
    // Same floating-point tolerance as analytics::recovers_exchange_point_values,
    // whose CF case documents this exact division as not landing on 10.0 bit-exact.
    assert!((cf.point_value.unwrap() - 10.0).abs() < 1e-6, "derived from the workbook identity: {:?}", cf.point_value);
    assert_eq!(cf.category, "equity", "Index suffix");
    assert_eq!(cf.currency, "EUR");
    assert!(!cf.confirmed, "seeded rows always need confirmation");

    let ry = cs.iter().find(|c| c.contract_root == "RY").unwrap();
    assert_eq!(ry.category, "fx", "Curncy suffix");
    assert!((ry.point_value.unwrap() - 125000.0).abs() < 1e-6, "got {:?}", ry.point_value);

    let rx = cs.iter().find(|c| c.contract_root == "RX").unwrap();
    assert_eq!(rx.category, "other", "Comdty is ambiguous - never guessed");
    assert!((rx.point_value.unwrap() - 1000.0).abs() < 1e-6, "got {:?}", rx.point_value);

    // TY is quoted in 32nds; read as decimal its implied point value is ~1081.7,
    // so the seeded value is wrong until the convention is corrected. It is
    // seeded anyway, unconfirmed, rather than being silently dropped.
    let ty = cs.iter().find(|c| c.contract_root == "TY").unwrap();
    assert_eq!(ty.price_convention, "decimal");
    assert!((ty.point_value.unwrap() - 1081.73).abs() < 0.1);

    pool.close().await;
    edb.stop().await;
}

#[tokio::test]
async fn reimport_warns_on_point_value_mismatch_and_never_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    let bytes = std::fs::read(SAMPLE).unwrap();
    let wb = ingest::parse_workbook(&bytes).unwrap();
    db::repo::import_workbook(&pool, "s.xlsx", "sha-a", &wb).await.unwrap();

    // Correct TY by hand, exactly as the user would on the Data page.
    let ty = db::repo::FuturesContract {
        contract_root: "TY".into(), label: "US 10Y Note".into(), category: "interest_rate".into(),
        point_value: Some(1000.0), currency: "USD".into(), curve: Some("US-10y".into()),
        price_convention: "th32".into(), confirmed: true,
    };
    db::repo::contracts_upsert(&pool, &ty).await.unwrap();

    // Re-import the same workbook under a new hash.
    let out = db::repo::import_workbook(&pool, "s.xlsx", "sha-b", &wb).await.unwrap();

    let after = db::repo::contracts_all(&pool).await.unwrap();
    let ty2 = after.iter().find(|c| c.contract_root == "TY").unwrap();
    assert_eq!(ty2.point_value, Some(1000.0), "user edits are never overwritten");
    assert_eq!(ty2.price_convention, "th32");
    assert!(ty2.confirmed);
    assert!(out.warnings.is_empty(), "th32 now reconciles exactly, so no warning");

    // Now break it: claim decimal for a contract that is quoted in 32nds.
    db::repo::contracts_upsert(&pool, &db::repo::FuturesContract {
        price_convention: "decimal".into(), ..ty
    }).await.unwrap();
    let out = db::repo::import_workbook(&pool, "s.xlsx", "sha-c", &wb).await.unwrap();
    let w = out.warnings.join(" | ");
    assert!(w.contains("TY"), "warning names the contract: {w}");
    assert!(w.contains("th32"), "warning names the likely convention: {w}");

    pool.close().await;
    edb.stop().await;
}
