use chrono::NaiveDate;
use db::auth::marker::{Import, Nav, Positions, Reference, Transactions, View};
use db::auth::AuthCtx;

fn d(y: i32, m: u32, day: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, day).unwrap() }

fn sample() -> ingest::ParsedWorkbook {
    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx")).unwrap();
    ingest::parse_workbook(&bytes).unwrap()
}

#[tokio::test]
async fn import_upsert_and_duplicate_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    let pool = dbh.test_pool().clone();
    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let p = scoped.authorize::<Positions, Import>(1).unwrap();
    let n = scoped.authorize::<Nav, Import>(1).unwrap();
    let t = scoped.authorize::<Transactions, Import>(1).unwrap();
    let nav_view = scoped.authorize::<Nav, View>(1).unwrap();
    let pos_view = scoped.authorize::<Positions, View>(1).unwrap();
    let ref_view = scoped.authorize::<Reference, View>(1).unwrap();
    let wb = sample();

    let o1 = scoped.import_workbook(&p, &n, &t, "sample.xlsx", "sha-1", &wb).await.unwrap();
    assert!(!o1.duplicate);
    assert_eq!(o1.positions, 111);
    assert_eq!(o1.dividends, 53);
    assert_eq!(o1.operations, 2050);
    assert!(o1.div_ops_replaced);
    // 343 HISTO rows + the file's own nav_date row
    assert_eq!(o1.nav_rows, 344);

    let nav = scoped.nav_rows(&nav_view).await.unwrap();
    assert_eq!(nav.len(), 344);
    assert_eq!(nav.last().unwrap().date, d(2026, 7, 24));
    assert!((nav.last().unwrap().nav - 104.42).abs() < 1e-9);
    assert_eq!(nav[0].date, d(2025, 2, 28));

    // same sha -> duplicate no-op
    let o2 = scoped.import_workbook(&p, &n, &t, "sample.xlsx", "sha-1", &wb).await.unwrap();
    assert!(o2.duplicate);
    assert_eq!(o2.import_id, o1.import_id);
    assert_eq!(scoped.nav_rows(&nav_view).await.unwrap().len(), 344);

    // same file, new sha -> re-import replaces the snapshot (still 111 rows, one date)
    let o3 = scoped.import_workbook(&p, &n, &t, "sample2.xlsx", "sha-2", &wb).await.unwrap();
    assert!(!o3.duplicate);
    assert!(o3.div_ops_replaced); // equal nav_date counts as >=
    let dates = scoped.position_dates(&pos_view).await.unwrap();
    assert_eq!(dates, vec![d(2026, 7, 24)]);
    let pos = scoped.positions_for(&pos_view, d(2026, 7, 24)).await.unwrap();
    assert_eq!(pos.len(), 111);
    assert!(pos.iter().any(|p| p.isin == "GRS145003000"));

    let imports = scoped.imports_list(&ref_view).await.unwrap();
    assert_eq!(imports.len(), 2);

    pool.close().await;
    edb.stop().await;
}
