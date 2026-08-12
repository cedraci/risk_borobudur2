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
        ref_facts: vec![],
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
        ref_facts: vec![],
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
        ref_facts: vec![],
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
        ref_facts: vec![],
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
        dividends: None, operations: None, ref_hints: vec![], ref_facts: vec![], warnings: vec![],
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
        dividends: None, operations: None, ref_hints: vec![], ref_facts: vec![], warnings: vec![],
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
        ref_hints: vec![], ref_facts: vec![], warnings: vec![],
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

const HISINV_FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/caceis_hisinv.csv");
const HISINV_FNAME: &str = "HISINVLUX_165878_20260807_20260810130151.csv";

// Task 6: HISINVLUX carries the depositary's own market place and bond
// schedule for every instrument it lists. These are authoritative facts, not
// user-overridable hints:
//   - a later refs_upsert (user-owned columns only) must not erase them;
//   - a LATER import carrying a DIFFERENT fact for the same code must
//     overwrite it — the only test shape that can tell a correct
//     COALESCE(EXCLUDED, existing) apart from an accidentally reversed
//     COALESCE(existing, EXCLUDED), since within a single import the hint
//     loop leaves the fact columns NULL and COALESCE degenerates either way
//     when the existing value is NULL;
//   - and that same second import must not clobber liquidity_days /
//     adv_eligible, the columns an import never owns.
#[tokio::test]
async fn hisinv_facts_overwrite_on_reimport_but_never_touch_user_owned_columns() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    // Import 1: the real HISINVLUX fixture sets the depositary facts.
    let bytes = std::fs::read(HISINV_FIXTURE).unwrap();
    let batch1 = ingest::caceis::parse_hisinv(HISINV_FNAME, &bytes).expect("fixture parses");
    repo::import_batch(&pool, 1, "hisinv.csv", "sha-hisinv-1", &batch1).await.unwrap();

    let refs = repo::refs_all(&pool).await.unwrap();
    let bond = refs.iter().find(|r| r.code == "US105756CL22").expect("bond ref stored");
    assert_eq!(bond.market_place.as_deref(), Some("186"));
    assert_eq!(bond.bond_next_coupon, Some(d("2026-09-15")));

    // A user setting liquidity_days/adv_eligible (refs_upsert only ever
    // writes user-owned columns) must not blank the depositary facts.
    let mut edited = bond.clone();
    edited.liquidity_days = Some(3.0);
    edited.adv_eligible = Some(true);
    repo::refs_upsert(&pool, &edited).await.unwrap();

    let refs_mid = repo::refs_all(&pool).await.unwrap();
    let bond_mid = refs_mid.iter().find(|r| r.code == "US105756CL22").expect("bond ref still present");
    assert_eq!(bond_mid.liquidity_days, Some(3.0));
    assert_eq!(bond_mid.adv_eligible, Some(true));
    assert_eq!(bond_mid.market_place.as_deref(), Some("186"), "refs_upsert must not touch market_place");
    assert_eq!(bond_mid.bond_next_coupon, Some(d("2026-09-15")), "refs_upsert must not touch bond_next_coupon");

    // Import 2: a later depositary file restates the SAME instrument with a
    // DIFFERENT market place and next coupon date. Hand-built rather than a
    // second fixture — no new fixture file is needed or wanted. A distinct
    // sha256 keeps this from being treated as a duplicate of import 1.
    let batch2 = UniversalBatch {
        primary_date: d("2026-08-08"),
        nav_points: vec![],
        snapshots: vec![],
        dividends: None,
        operations: None,
        ref_hints: vec![],
        ref_facts: vec![ingest::adapter::RefFact {
            isin: "US105756CL22".into(),
            market_place: Some("999".into()),
            market_place_name: Some("A DIFFERENT VENUE".into()),
            bond_maturity: None,
            bond_next_coupon: Some(d("2026-12-15")),
            bond_coupon_pct: None,
            bond_nominal: None,
            bond_coupon_freq: None,
        }],
        warnings: vec![],
    };
    repo::import_batch(&pool, 1, "hisinv2.csv", "sha-hisinv-2", &batch2).await.unwrap();

    let refs_after = repo::refs_all(&pool).await.unwrap();
    let bond_after = refs_after.iter().find(|r| r.code == "US105756CL22").expect("bond ref still present");

    // The new fact values win — proves overwrite, not fill-only. A reversed
    // COALESCE would have left these at "186" / 2026-09-15.
    assert_eq!(bond_after.market_place.as_deref(), Some("999"), "a later import's fact must overwrite the earlier one");
    assert_eq!(bond_after.bond_next_coupon, Some(d("2026-12-15")), "a later import's fact must overwrite the earlier one");

    // Fields import 2 said nothing about (bond_maturity, bond_coupon_pct)
    // are left alone, not nulled out, because the new RefFact carries None.
    assert_eq!(bond_after.bond_maturity, Some(d("2035-03-15")), "absent fields must not be nulled out");
    assert_eq!(bond_after.bond_coupon_pct, Some(6.625), "absent fields must not be nulled out");

    // The columns this import never owns must survive untouched.
    assert_eq!(bond_after.liquidity_days, Some(3.0), "an import must never touch liquidity_days");
    assert_eq!(bond_after.adv_eligible, Some(true), "an import must never touch adv_eligible");

    pool.close().await;
    edb.stop().await;
}
