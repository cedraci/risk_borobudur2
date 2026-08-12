use chrono::NaiveDate;
use db::repo;
use ingest::adapter::{Snapshot, UniversalBatch};
use ingest::{DividendRow, NavHistoryRow, OperationRow, PositionRow};

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

// Finding 2: with daily CSVs and weekly recaps, the recap's own primary_date
// is almost always older than the newest CSV date. The replace-if-latest
// gate must compare a journal-bearing batch only against OTHER
// journal-bearing imports, never against a CSV import's (later) nav_date —
// otherwise the recap's dividends/operations are silently skipped forever.
#[tokio::test]
async fn csv_import_does_not_poison_replace_gate_for_older_journal_batch() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    // A CACEIS-style CSV import lands first, dated LATER than the recap that
    // follows it — no dividends/operations, just NAV + positions.
    let csv_batch = UniversalBatch {
        primary_date: d("2026-08-10"),
        nav_points: vec![NavHistoryRow { date: d("2026-08-10"), aum: 1000.0, shares: 10.0, nav: 100.0 }],
        snapshots: vec![Snapshot { nav_date: d("2026-08-10"), positions: vec![pos("Action", "FR0000000001", 1000.0)] }],
        dividends: None,
        operations: None,
        ref_hints: vec![],
        warnings: vec![],
    };
    repo::import_batch(&pool, 1, "csv.csv", "sha-csv-1", &csv_batch).await.unwrap();

    // The recap, dated EARLIER than the CSV, is the first journal-bearing
    // batch this portfolio has ever seen — it must still replace.
    let recap_batch = UniversalBatch {
        primary_date: d("2026-08-05"),
        nav_points: vec![NavHistoryRow { date: d("2026-08-05"), aum: 900.0, shares: 9.0, nav: 100.0 }],
        snapshots: vec![Snapshot { nav_date: d("2026-08-05"), positions: vec![pos("Action", "FR0000000001", 900.0)] }],
        dividends: Some(vec![]),
        operations: Some(vec![OperationRow {
            trade_date: d("2026-08-05"), side: "BUY".into(), ticker: None, isin: Some("FR0000000001".into()),
            name: None, currency: Some("EUR".into()), quantity: Some(10.0), price: Some(90.0),
            gross_amount: Some(900.0), fees: None, net_price: None, net_amount: Some(900.0),
        }]),
        ref_hints: vec![],
        warnings: vec![],
    };
    let out = repo::import_batch(&pool, 1, "recap.xlsx", "sha-recap-1", &recap_batch).await.unwrap();

    assert!(out.div_ops_replaced,
        "an older-dated journal-bearing batch must still replace when no journal-bearing import has run yet");
    assert_eq!(out.operations, 1);

    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM operations WHERE portfolio_id = 1")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(n, 1, "operations must have landed despite an intervening later-dated CSV import");

    pool.close().await;
    edb.stop().await;
}

// Findings 1 + 4: a NAV Recap import must not wipe derived dividends without
// rebuilding them, and the import -> derive wiring needs a test that fails
// if it is ever deleted.
#[tokio::test]
async fn nav_recap_replace_preserves_and_re_derives_dividends() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    // Day 1 (baseline): a CACEIS-style CSV import, no journal, a CPON
    // receivable at 580.
    let b1 = UniversalBatch {
        primary_date: d("2026-08-05"),
        nav_points: vec![NavHistoryRow { date: d("2026-08-05"), aum: 1580.0, shares: 10.0, nav: 158.0 }],
        snapshots: vec![Snapshot { nav_date: d("2026-08-05"), positions: vec![
            pos("Dividendes", "GB0000000001", 580.0),
            pos("Action", "FR0000000001", 1000.0),
        ] }],
        dividends: None, operations: None, ref_hints: vec![], warnings: vec![],
    };
    repo::import_batch(&pool, 1, "day1.csv", "sha-d1", &b1).await.unwrap();

    // Day 2: the receivable grows to 920 -> a +340 derive-time growth event.
    let b2 = UniversalBatch {
        primary_date: d("2026-08-06"),
        nav_points: vec![NavHistoryRow { date: d("2026-08-06"), aum: 1920.0, shares: 10.0, nav: 192.0 }],
        snapshots: vec![Snapshot { nav_date: d("2026-08-06"), positions: vec![
            pos("Dividendes", "GB0000000001", 920.0),
            pos("Action", "FR0000000001", 1000.0),
        ] }],
        dividends: None, operations: None, ref_hints: vec![], warnings: vec![],
    };
    let out2 = repo::import_batch(&pool, 1, "day2.csv", "sha-d2", &b2).await.unwrap();
    assert!(out2.warnings.iter().any(|w| w.contains("derived")), "{:?}", out2.warnings);

    let derived_d2: (f64, bool) = sqlx::query_as(
        "SELECT amount::float8, derived FROM dividends WHERE portfolio_id = 1 AND provision_date = '2026-08-06'")
        .fetch_one(&pool).await.unwrap();
    assert!((derived_d2.0 - 340.0).abs() < 1e-9, "{derived_d2:?}");
    assert!(derived_d2.1);

    // Day 3: a NAV Recap arrives — journal-bearing, carrying an EXPLICIT
    // dividend dated 2026-08-06 (the SAME date as the derived event above)
    // plus a further-grown receivable (920 -> 1280) on its own date.
    let b3 = UniversalBatch {
        primary_date: d("2026-08-07"),
        nav_points: vec![NavHistoryRow { date: d("2026-08-07"), aum: 2280.0, shares: 10.0, nav: 228.0 }],
        snapshots: vec![Snapshot { nav_date: d("2026-08-07"), positions: vec![
            pos("Dividendes", "GB0000000001", 1280.0),
            pos("Action", "FR0000000001", 1000.0),
        ] }],
        dividends: Some(vec![DividendRow {
            provision_date: d("2026-08-06"), payment_date: None, issuer: "EXPLICIT".into(),
            amount: 99.0, currency: "EUR".into(),
        }]),
        operations: Some(vec![]),
        ref_hints: vec![], warnings: vec![],
    };
    let out3 = repo::import_batch(&pool, 1, "day3.xlsx", "sha-d3", &b3).await.unwrap();
    assert!(out3.div_ops_replaced);
    assert!(out3.warnings.iter().any(|w| w.contains("derived")), "{:?}", out3.warnings);

    // The explicit row wins on its date — not a re-derived one.
    let explicit: (String, bool) = sqlx::query_as(
        "SELECT issuer, derived FROM dividends WHERE portfolio_id = 1 AND provision_date = '2026-08-06'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(explicit.0, "EXPLICIT");
    assert!(!explicit.1, "the explicit row must win on its date, not a re-derived one");

    // The other date's growth event (2026-08-06 -> 2026-08-07, +360) must
    // have been (re-)derived post-commit — the pin for the import -> derive
    // wiring: delete this without replacement and this assertion fails.
    let derived_d3: (f64, bool) = sqlx::query_as(
        "SELECT amount::float8, derived FROM dividends WHERE portfolio_id = 1 AND provision_date = '2026-08-07'")
        .fetch_one(&pool).await.unwrap();
    assert!((derived_d3.0 - 360.0).abs() < 1e-9, "{derived_d3:?}");
    assert!(derived_d3.1);

    // Exactly two rows total: the explicit one on 08-06 and the re-derived
    // one on 08-07 — the old derived 08-06 row must be gone, superseded by
    // the explicit row, not lingering alongside it.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM dividends WHERE portfolio_id = 1")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(n, 2);

    pool.close().await;
    edb.stop().await;
}
