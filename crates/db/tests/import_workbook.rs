use chrono::NaiveDate;

fn d(y: i32, m: u32, day: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, day).unwrap() }

fn sample() -> ingest::ParsedWorkbook {
    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx")).unwrap();
    ingest::parse_workbook(&bytes).unwrap()
}

#[tokio::test]
async fn import_upsert_and_duplicate_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let wb = sample();

    let o1 = db::repo::import_workbook(&pool, "sample.xlsx", "sha-1", &wb).await.unwrap();
    assert!(!o1.duplicate);
    assert_eq!(o1.positions, 111);
    assert_eq!(o1.dividends, 53);
    assert_eq!(o1.operations, 2050);
    assert!(o1.div_ops_replaced);
    // 343 HISTO rows + the file's own nav_date row
    assert_eq!(o1.nav_rows, 344);

    let nav = db::repo::nav_rows(&pool).await.unwrap();
    assert_eq!(nav.len(), 344);
    assert_eq!(nav.last().unwrap().date, d(2026, 7, 24));
    assert!((nav.last().unwrap().nav - 104.42).abs() < 1e-9);
    assert_eq!(nav[0].date, d(2025, 2, 28));

    // same sha -> duplicate no-op
    let o2 = db::repo::import_workbook(&pool, "sample.xlsx", "sha-1", &wb).await.unwrap();
    assert!(o2.duplicate);
    assert_eq!(o2.import_id, o1.import_id);
    assert_eq!(db::repo::nav_rows(&pool).await.unwrap().len(), 344);

    // same file, new sha -> re-import replaces the snapshot (still 111 rows, one date)
    let o3 = db::repo::import_workbook(&pool, "sample2.xlsx", "sha-2", &wb).await.unwrap();
    assert!(!o3.duplicate);
    assert!(o3.div_ops_replaced); // equal nav_date counts as >=
    let dates = db::repo::position_dates(&pool).await.unwrap();
    assert_eq!(dates, vec![d(2026, 7, 24)]);
    let pos = db::repo::positions_for(&pool, d(2026, 7, 24)).await.unwrap();
    assert_eq!(pos.len(), 111);
    assert!(pos.iter().any(|p| p.isin == "GRS145003000"));

    let imports = db::repo::imports_list(&pool).await.unwrap();
    assert_eq!(imports.len(), 2);

    pool.close().await;
    edb.stop().await;
}
