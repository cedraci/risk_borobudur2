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

// The pre-existing-installation case. The embedded PostgreSQL under
// %LOCALAPPDATA% survives binary upgrades, so on upgrade a user has the
// workbook already imported and `futures_contracts` empty - the seeding
// shipped later than the import did. Re-dropping the same file is that user's
// only repair path, and that re-drop is a *duplicate* import, which is exactly
// why no fresh-tempdir test could see the hole: seeding used to live inside
// the transaction the sha256 short-circuit returned before.
#[tokio::test]
async fn duplicate_import_seeds_specs_a_pre_existing_database_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    let bytes = std::fs::read(SAMPLE).unwrap();
    let wb = ingest::parse_workbook(&bytes).unwrap();
    db::repo::import_workbook(&pool, "s.xlsx", "sha-dup", &wb).await.unwrap();

    // Rewind to what an upgraded installation actually looks like: the import
    // is on record, the specs are not.
    sqlx::query("DELETE FROM futures_contracts").execute(&pool).await.unwrap();
    assert!(db::repo::contracts_all(&pool).await.unwrap().is_empty());

    // Same file, same sha256 -> the duplicate arm.
    let out = db::repo::import_workbook(&pool, "s.xlsx", "sha-dup", &wb).await.unwrap();
    assert!(out.duplicate, "same sha256 must still be recognised as a duplicate");
    assert_eq!(out.positions, 0, "a duplicate re-ingests nothing");

    let cs = db::repo::contracts_all(&pool).await.unwrap();
    let roots: Vec<&str> = cs.iter().map(|c| c.contract_root.as_str()).collect();
    assert_eq!(roots, vec!["CF", "KOA", "NQ", "OAT", "RX", "RY", "TY", "VG"],
               "the duplicate import repairs the missing specs");
    let cf = cs.iter().find(|c| c.contract_root == "CF").unwrap();
    assert!((cf.point_value.unwrap() - 10.0).abs() < 1e-6, "{:?}", cf.point_value);
    let rx = cs.iter().find(|c| c.contract_root == "RX").unwrap();
    assert!((rx.point_value.unwrap() - 1000.0).abs() < 1e-6, "{:?}", rx.point_value);
    assert!(cs.iter().all(|c| !c.confirmed), "repaired specs still need confirming");
    assert!(out.warnings.iter().any(|w| w.starts_with("CF:")),
            "the repair is announced, not silent: {:?}", out.warnings);

    // And the repair is safe to re-run: a duplicate import must never clobber
    // a spec the user has since corrected by hand.
    db::repo::contracts_upsert(&pool, &db::repo::FuturesContract {
        contract_root: "TY".into(), label: "US 10Y Note".into(), category: "interest_rate".into(),
        point_value: Some(1000.0), currency: "USD".into(), curve: Some("US-10y".into()),
        price_convention: "th32".into(), confirmed: true,
    }).await.unwrap();
    let out = db::repo::import_workbook(&pool, "s.xlsx", "sha-dup", &wb).await.unwrap();
    assert!(out.duplicate);
    let after = db::repo::contracts_all(&pool).await.unwrap();
    let ty = after.iter().find(|c| c.contract_root == "TY").unwrap();
    assert_eq!(ty.point_value, Some(1000.0), "user edits survive a duplicate import");
    assert_eq!(ty.price_convention, "th32");
    assert!(ty.confirmed);
    assert_eq!(after.len(), 8, "no duplicate rows");
    assert!(out.warnings.is_empty(), "nothing new to seed, and th32 reconciles: {:?}", out.warnings);

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
