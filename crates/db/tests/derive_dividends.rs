use chrono::NaiveDate;
use db::repo;

fn d(s: &str) -> NaiveDate { s.parse().unwrap() }

async fn seed_snapshot(pool: &sqlx::PgPool, date: &str, rows: &[(&str, &str, &str, f64, f64)]) {
    // rows: (asset_type, isin, currency, valuation_ccy, valuation_eur)
    let (import_id,): (i64,) = sqlx::query_as(
        "INSERT INTO imports (portfolio_id, filename, sha256, nav_date, row_counts) VALUES (1, $1, $2, $3, '{}') RETURNING id")
        .bind(format!("seed-{date}.csv")).bind(format!("sha-{date}")).bind(d(date))
        .fetch_one(pool).await.unwrap();
    for (at, isin, ccy, vl, ve) in rows {
        sqlx::query(
            "INSERT INTO position_snapshots (portfolio_id, nav_date, import_id, asset_type, isin, name, currency, valuation_ccy, valuation_eur)
             VALUES (1, $1, $2, $3, $4, $4, $5, $6, $7)")
            .bind(d(date)).bind(import_id).bind(at).bind(isin).bind(ccy).bind(vl).bind(ve)
            .execute(pool).await.unwrap();
    }
}

async fn derived_rows(pool: &sqlx::PgPool) -> Vec<(NaiveDate, String, f64, String)> {
    sqlx::query_as(
        "SELECT provision_date, issuer, amount::float8, currency FROM dividends
         WHERE portfolio_id = 1 AND derived ORDER BY provision_date, issuer")
        .fetch_all(pool).await.unwrap()
}

#[tokio::test]
async fn cpon_deltas_become_derived_dividends() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    // Day 1 (baseline): a GBP receivable of 500 local and an equity (noise).
    seed_snapshot(&pool, "2026-08-05", &[
        ("Dividendes", "GB0000000001", "GBP", 500.0, 580.0),
        ("Action", "FR0000000001", "EUR", 1000.0, 1000.0),
    ]).await;
    // Day 2: GBP receivable grows to 800 local (event: +300 GBP); a new EUR
    // receivable appears at 200 (event: +200 EUR).
    seed_snapshot(&pool, "2026-08-06", &[
        ("Dividendes", "GB0000000001", "GBP", 800.0, 920.0),
        ("Dividendes", "FR0000000002", "EUR", 200.0, 200.0),
        ("Action", "FR0000000001", "EUR", 1000.0, 1000.0),
    ]).await;
    // Day 3: GBP local value unchanged but EUR value moved (FX only — no
    // event); the EUR receivable disappears (paid — no event).
    seed_snapshot(&pool, "2026-08-07", &[
        ("Dividendes", "GB0000000001", "GBP", 800.0, 935.0),
        ("Action", "FR0000000001", "EUR", 1000.0, 1000.0),
    ]).await;

    let n = repo::derive_dividends(&pool, 1).await.unwrap();
    assert_eq!(n, 2, "one growth event + one appearance event");
    let rows = derived_rows(&pool).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], (d("2026-08-06"), "FR0000000002".into(), 200.0, "EUR".into()));
    assert_eq!(rows[1], (d("2026-08-06"), "GB0000000001".into(), 300.0, "GBP".into()));

    // Convergence: re-running (as every import does) yields the same set.
    let n2 = repo::derive_dividends(&pool, 1).await.unwrap();
    assert_eq!(n2, 2);
    assert_eq!(derived_rows(&pool).await.len(), 2);

    // Explicit-beats-derived: an explicit dividend on 2026-08-06 suppresses
    // the derived events on that date.
    sqlx::query("INSERT INTO dividends (portfolio_id, provision_date, issuer, amount, currency, derived) VALUES (1, '2026-08-06', 'EXPLICIT', 99, 'EUR', false)")
        .execute(&pool).await.unwrap();
    let n3 = repo::derive_dividends(&pool, 1).await.unwrap();
    assert_eq!(n3, 0, "explicit journal covers the date");
    assert!(derived_rows(&pool).await.is_empty());

    pool.close().await;
    edb.stop().await;
}
