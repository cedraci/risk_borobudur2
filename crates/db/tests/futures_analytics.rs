use chrono::NaiveDate;
use db::auth::marker::{Import, MarketData, View};
use db::auth::AuthCtx;

fn d(s: &str) -> NaiveDate { s.parse().unwrap() }

fn row(ticker: &str, dur: f64) -> ingest::CtdRow {
    ingest::CtdRow {
        nav_date: d("2026-07-24"),
        ticker: ticker.into(),
        ctd_isin: "DE0001102580".into(),
        ctd_mod_duration: dur,
        ctd_clean_price: 98.72,
        ctd_accrued: 0.63,
        conversion_factor: 0.782145,
    }
}

#[tokio::test]
async fn ctd_upload_replaces_the_whole_date() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let dbh = db::Db::connect(&edb.url).await.unwrap();
    let pool = dbh.test_pool().clone();
    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let view = scoped.authorize::<MarketData, View>(1).unwrap();
    let import = scoped.authorize::<MarketData, Import>(1).unwrap();

    assert!(scoped.ctd_for(&view, d("2026-07-24")).await.unwrap().is_empty());

    let n = scoped.ctd_replace(&import, d("2026-07-24"), "a.csv",
        &[row("RXU6 Comdty", 8.41), row("OATU6 Comdty", 7.92)]).await.unwrap();
    assert_eq!(n, 2);
    let got = scoped.ctd_for(&view, d("2026-07-24")).await.unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].ticker, "OATU6 Comdty", "sorted by ticker");

    // A corrected re-upload replaces the date wholesale rather than merging.
    let n = scoped.ctd_replace(&import, d("2026-07-24"), "b.csv",
        &[row("RXU6 Comdty", 9.99)]).await.unwrap();
    assert_eq!(n, 1);
    let got = scoped.ctd_for(&view, d("2026-07-24")).await.unwrap();
    assert_eq!(got.len(), 1, "the OAT row from the first upload is gone");
    assert!((got[0].ctd_mod_duration - 9.99).abs() < 1e-12);

    // Other dates are untouched.
    scoped.ctd_replace(&import, d("2026-07-17"), "c.csv", &[row("RXU6 Comdty", 8.0)]).await.unwrap();
    assert_eq!(scoped.ctd_for(&view, d("2026-07-24")).await.unwrap().len(), 1);
    assert_eq!(scoped.ctd_for(&view, d("2026-07-17")).await.unwrap().len(), 1);

    pool.close().await;
    edb.stop().await;
}
