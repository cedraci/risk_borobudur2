use chrono::NaiveDate;
use db::repo;

fn d(s: &str) -> NaiveDate { s.parse().unwrap() }

fn flow(date: NaiveDate, class: &str, outstanding: f64, nav: f64, sub: f64, red: f64) -> ingest::ShareClassFlowRow {
    ingest::ShareClassFlowRow {
        flow_date: date,
        share_class: class.into(),
        outstanding_shares: Some(outstanding),
        nav_per_share: Some(nav),
        subscription_amount: sub,
        redemption_amount: red,
    }
}

// Upserts two days of flows, then re-upserts one of those days with changed
// amounts. `flows_for` must return three rows total (not four) — the same
// day loaded twice corrects the row rather than double-counting it — with
// the updated values winning.
#[tokio::test]
async fn flows_upsert_is_idempotent_per_portfolio_date_and_share_class() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    let day1 = vec![
        flow(d("2026-08-06"), "C1", 271_295.542, 104.04, 0.0, 100_000.0),
        flow(d("2026-08-06"), "C2", 500_000.001, 104.04, 50_000.0, 0.0),
    ];
    let day2 = vec![
        flow(d("2026-08-07"), "C1", 269_373.392, 104.10, 0.0, 200_000.0),
    ];

    {
        let mut conn = pool.acquire().await.unwrap();
        let n1 = repo::flows_upsert(&mut conn, 1, &day1).await.unwrap();
        assert_eq!(n1, 2);
        let n2 = repo::flows_upsert(&mut conn, 1, &day2).await.unwrap();
        assert_eq!(n2, 1);
    } // conn returns to the pool here, so pool.close() below does not hang

    let rows = repo::flows_for(&pool, 1, 10).await.unwrap();
    assert_eq!(rows.len(), 3);
    // Oldest first.
    assert_eq!(rows[0].flow_date, d("2026-08-06"));
    assert_eq!(rows[2].flow_date, d("2026-08-07"));

    // Re-upsert 2026-08-06's C1 row with corrected amounts.
    let corrected = vec![flow(d("2026-08-06"), "C1", 271_295.542, 104.04, 0.0, 150_000.0)];
    {
        let mut conn = pool.acquire().await.unwrap();
        let n3 = repo::flows_upsert(&mut conn, 1, &corrected).await.unwrap();
        assert_eq!(n3, 1);
    } // conn returns to the pool here, so pool.close() below does not hang

    let rows_after = repo::flows_for(&pool, 1, 10).await.unwrap();
    assert_eq!(rows_after.len(), 3, "correcting a day must overwrite, not add a fourth row");
    let c1_0806 = rows_after.iter()
        .find(|r| r.flow_date == d("2026-08-06") && r.share_class == "C1")
        .expect("corrected row still present");
    assert_eq!(c1_0806.redemption_amount, 150_000.0, "the corrected amount must win");

    // The other rows on that date and the other date are untouched.
    let c2_0806 = rows_after.iter()
        .find(|r| r.flow_date == d("2026-08-06") && r.share_class == "C2")
        .expect("C2 row untouched by the C1 correction");
    assert_eq!(c2_0806.subscription_amount, 50_000.0);
    let c1_0807 = rows_after.iter()
        .find(|r| r.flow_date == d("2026-08-07") && r.share_class == "C1")
        .expect("2026-08-07 row untouched");
    assert_eq!(c1_0807.redemption_amount, 200_000.0);

    pool.close().await;
    edb.stop().await;
}

// import_batch stores the flows count in row_counts before the INSERT, and a
// JOURSRLUX batch carries no positions or NAV history — snapshots and
// nav_history stay empty for its date.
#[tokio::test]
async fn import_batch_stores_flows_and_row_count() {
    use ingest::adapter::UniversalBatch;

    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    let b = UniversalBatch {
        primary_date: d("2026-08-07"),
        nav_points: Vec::new(),
        snapshots: Vec::new(),
        dividends: None,
        operations: None,
        flows: Some(vec![
            flow(d("2026-08-07"), "C1", 271_295.542, 104.04, 0.0, 200_000.0),
            flow(d("2026-08-07"), "C2", 500_000.001, 104.04, 350_000.0, 0.0),
        ]),
        ref_hints: Vec::new(),
        ref_facts: Vec::new(),
        warnings: Vec::new(),
    };
    let out = repo::import_batch(&pool, 1, "joursr.csv", "sha-joursr-1", &b).await.unwrap();
    assert!(!out.duplicate);
    assert_eq!(out.nav_rows, 0);
    assert_eq!(out.positions, 0);

    let row_counts: serde_json::Value = sqlx::query_scalar(
        "SELECT row_counts FROM imports WHERE portfolio_id = 1 AND sha256 = 'sha-joursr-1'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(row_counts["flows"], serde_json::json!(2));

    let rows = repo::flows_for(&pool, 1, 10).await.unwrap();
    assert_eq!(rows.len(), 2);

    // No positions or NAV history landed for this date.
    let n_pos: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM position_snapshots WHERE portfolio_id = 1 AND nav_date = '2026-08-07'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(n_pos, 0);
    let n_nav: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM nav_history WHERE portfolio_id = 1 AND date = '2026-08-07'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(n_nav, 0);

    pool.close().await;
    edb.stop().await;
}

// shareholders_replace deletes and re-inserts in one transaction; the read
// back must come out largest-first, with `id` as a deterministic tiebreak
// for equal percentages.
#[tokio::test]
async fn shareholders_replace_is_transactional_and_ordered_largest_first() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    let first = vec![
        ("Founder family".to_string(), 18.0, d("2026-08-07")),
        ("Pension fund A".to_string(), 12.5, d("2026-08-07")),
        // Same pct as another later row, further down: id tiebreak matters.
        ("Tied holder A".to_string(), 5.0, d("2026-08-07")),
        ("Tied holder B".to_string(), 5.0, d("2026-08-07")),
    ];
    repo::shareholders_replace(&pool, 1, &first).await.unwrap();

    let rows = repo::shareholders_for(&pool, 1).await.unwrap();
    assert_eq!(rows.len(), 4);
    // Largest first.
    assert_eq!(rows[0].label, "Founder family");
    assert_eq!(rows[1].label, "Pension fund A");
    // Equal percentages break the tie on ascending id (insertion order here).
    assert_eq!(rows[2].label, "Tied holder A");
    assert_eq!(rows[3].label, "Tied holder B");
    assert!(rows[2].id < rows[3].id);

    // A second replace fully supersedes the first: no leftover rows, no
    // duplicate accumulation.
    let second = vec![("Founder family".to_string(), 20.0, d("2026-08-10"))];
    repo::shareholders_replace(&pool, 1, &second).await.unwrap();

    let rows_after = repo::shareholders_for(&pool, 1).await.unwrap();
    assert_eq!(rows_after.len(), 1, "replace must not append to the previous register");
    assert_eq!(rows_after[0].label, "Founder family");
    assert_eq!(rows_after[0].pct_of_nav, 20.0);
    assert_eq!(rows_after[0].as_of, d("2026-08-10"));

    pool.close().await;
    edb.stop().await;
}
