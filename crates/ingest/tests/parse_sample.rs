use chrono::NaiveDate;

fn d(y: i32, m: u32, day: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, day).unwrap() }

#[test]
fn parses_the_real_sample_workbook() {
    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample.xlsx")).unwrap();
    let wb = ingest::parse_workbook(&bytes).expect("sample must parse cleanly");

    assert_eq!(wb.nav_date, d(2026, 7, 24));
    assert!((wb.nav - 104.42).abs() < 1e-9);
    assert!((wb.aum - 28_332_753.49).abs() < 1e-2);
    assert!((wb.shares - 271_342.492).abs() < 1e-3);

    assert_eq!(wb.positions.len(), 111);
    let p0 = &wb.positions[0];
    assert_eq!(p0.asset_type, "Action");
    assert_eq!(p0.isin, "GRS145003000");
    assert_eq!(p0.quantity, Some(7400.0));
    assert_eq!(p0.valuation_eur, Some(316_572.0));
    assert_eq!(p0.ticker.as_deref(), Some("GEKTERNA GA Equity"));
    // cash-section remap: margin row weight must land in `weight`, not accrued_interest
    let margin = wb.positions.iter().find(|p| p.isin == "MA1C7EUR").unwrap();
    assert_eq!(margin.asset_type, "Margin Acc");
    assert_eq!(margin.valuation_eur, Some(-19_688.0));
    assert!(margin.weight.unwrap() < 0.0);
    assert_eq!(margin.price, None);

    assert_eq!(wb.nav_history.len(), 343);
    assert_eq!(wb.nav_history[0].date, d(2025, 2, 28));
    assert!((wb.nav_history[0].nav - 100.0).abs() < 1e-9);
    assert_eq!(wb.nav_history.last().unwrap().date, d(2026, 7, 23));
    assert!((wb.nav_history.last().unwrap().nav - 103.99).abs() < 1e-9);

    assert_eq!(wb.dividends.len(), 53);
    assert_eq!(wb.operations.len(), 2050);
    assert_eq!(wb.operations[0].trade_date, d(2025, 3, 18));
    assert_eq!(wb.operations[0].side, "Achat");
}

#[test]
fn garbage_bytes_fail_cleanly() {
    assert!(matches!(ingest::parse_workbook(b"not an xlsx"), Err(ingest::ParseFailure::Workbook(_))));
}
