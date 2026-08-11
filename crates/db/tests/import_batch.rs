use chrono::NaiveDate;
use db::repo;
use ingest::adapter::{Snapshot, UniversalBatch};
use ingest::{NavHistoryRow, PositionRow};

fn d(s: &str) -> NaiveDate { s.parse().unwrap() }

fn pos(asset_type: &str, isin: &str, valuation_eur: f64) -> PositionRow {
    PositionRow {
        asset_type: asset_type.into(), isin: isin.into(), name: Some(isin.into()),
        currency: Some("EUR".into()), quantity: Some(1.0), avg_cost: None, price: None,
        valuation_ccy: Some(valuation_eur), accrued_interest: None, fx_rate: Some(1.0),
        valuation_eur: Some(valuation_eur), weight: None, ticker: None,
    }
}

#[tokio::test]
async fn batch_without_div_ops_leaves_journals_untouched_and_checks_tna() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    // Seed an explicit dividend so we can prove a journal-less batch leaves it alone.
    sqlx::query("INSERT INTO dividends (portfolio_id, provision_date, issuer, amount, currency) VALUES (1, '2026-08-01', 'SEED', 10, 'EUR')")
        .execute(&pool).await.unwrap();

    // Positions sum 1000, NAV point says 1500 -> TNA warning expected.
    let b = UniversalBatch {
        primary_date: d("2026-08-07"),
        nav_points: vec![NavHistoryRow { date: d("2026-08-07"), aum: 1500.0, shares: 10.0, nav: 150.0 }],
        snapshots: vec![Snapshot { nav_date: d("2026-08-07"), positions: vec![pos("Action", "FR0000000001", 1000.0)] }],
        dividends: None,
        operations: None,
        ref_hints: vec![ingest::adapter::RefHint {
            isin: "FR0000000001".into(),
            country_of_risk: Some("France".into()), region: Some("Europe".into()), ticker: Some("AAA FP".into()),
        }],
        warnings: vec!["row 5: dropped".into()],
    };
    let out = repo::import_batch(&pool, 1, "f.csv", "sha-batch-1", &b).await.unwrap();

    assert!(!out.duplicate);
    assert_eq!(out.nav_rows, 1);
    assert_eq!(out.positions, 1);
    assert_eq!(out.dividends, 0);
    assert!(!out.div_ops_replaced);
    assert!(out.warnings.iter().any(|w| w.contains("TNA cross-check")), "{:?}", out.warnings);
    assert!(out.warnings.iter().any(|w| w.contains("dropped")), "{:?}", out.warnings);

    // Explicit dividend survived a journal-less import.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM dividends WHERE portfolio_id = 1")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(n, 1);

    // Ref hint filled NULL columns.
    let (country, ticker): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT country_of_risk, ticker FROM instrument_refs WHERE code = 'FR0000000001'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(country.as_deref(), Some("France"));
    assert_eq!(ticker.as_deref(), Some("AAA FP"));

    // A second batch must NOT overwrite: hint with a different country is ignored.
    let b2 = UniversalBatch {
        primary_date: d("2026-08-08"),
        nav_points: vec![NavHistoryRow { date: d("2026-08-08"), aum: 1000.0, shares: 10.0, nav: 100.0 }],
        snapshots: vec![Snapshot { nav_date: d("2026-08-08"), positions: vec![pos("Action", "FR0000000001", 1000.0)] }],
        dividends: None, operations: None,
        ref_hints: vec![ingest::adapter::RefHint {
            isin: "FR0000000001".into(), country_of_risk: Some("Germany".into()), region: None, ticker: None,
        }],
        warnings: vec![],
    };
    repo::import_batch(&pool, 1, "f2.csv", "sha-batch-2", &b2).await.unwrap();
    let country2: Option<String> = sqlx::query_scalar("SELECT country_of_risk FROM instrument_refs WHERE code = 'FR0000000001'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(country2.as_deref(), Some("France"), "hint must never overwrite");

    pool.close().await;
    edb.stop().await;
}
