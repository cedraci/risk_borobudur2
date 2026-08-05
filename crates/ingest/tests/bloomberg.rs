use calamine::Reader;
use chrono::NaiveDate;
use ingest::bloomberg::{build_request, parse_response, region_for, RequestItem};

fn d(y: i32, m: u32, dd: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, dd).unwrap() }

#[test]
fn request_workbook_has_the_three_expected_sheets() {
    let items = vec![RequestItem { isin: "FR0000121014".into(), ticker: "MC FP Equity".into() }];
    let bytes = build_request(&items, &["USD".into(), "GBP".into()], d(2025, 3, 18), d(2026, 7, 24)).unwrap();

    let mut wb: calamine::Xlsx<_> =
        calamine::Xlsx::new(std::io::Cursor::new(bytes)).expect("valid xlsx");
    let names = calamine::Reader::sheet_names(&wb);
    assert!(names.iter().any(|n| n == "REFS"));
    assert!(names.iter().any(|n| n == "FX"));
    assert!(names.iter().any(|n| n == "README"));

    let refs = calamine::Reader::worksheet_range(&mut wb, "REFS").unwrap();
    let header: Vec<String> = refs.rows().next().unwrap().iter().map(|c| c.to_string()).collect();
    assert_eq!(header[0], "isin");
    assert_eq!(header[1], "ticker");
    assert_eq!(header[2], "country_of_risk");
    assert_eq!(header[3], "gics_sector");
    assert_eq!(header[4], "gics_industry");

    let row1: Vec<String> = refs.rows().nth(1).unwrap().iter().map(|c| c.to_string()).collect();
    assert_eq!(row1[0], "FR0000121014");
    assert_eq!(row1[1], "MC FP Equity");
}

#[test]
fn refs_and_fx_formulas_reference_the_correct_rows_and_ranges() {
    let items = vec![
        RequestItem { isin: "FR0000121014".into(), ticker: "MC FP Equity".into() },
        RequestItem { isin: "US0378331005".into(), ticker: "AAPL US Equity".into() },
    ];
    let bytes = build_request(&items, &["USD".into()], d(2025, 3, 18), d(2026, 7, 24)).unwrap();

    let mut wb: calamine::Xlsx<_> =
        calamine::Xlsx::new(std::io::Cursor::new(bytes)).expect("valid xlsx");

    let refs = calamine::Reader::worksheet_formula(&mut wb, "REFS").unwrap();
    let f = |r: u32, c: u32| refs.get_value((r, c)).cloned().unwrap_or_default();

    // item 1 (row 2 in Excel, row index 1): country/sector/industry BDP formulas
    // must reference ticker cell B2.
    assert_eq!(f(1, 2), "BDP(B2,\"CNTRY_OF_RISK\")");
    assert_eq!(f(1, 3), "BDP(B2,\"GICS_SECTOR_NAME\")");
    assert_eq!(f(1, 4), "BDP(B2,\"GICS_INDUSTRY_GROUP_NAME\")");

    // item 2 (row 3 in Excel, row index 2): same three formulas, referencing B3.
    assert_eq!(f(2, 2), "BDP(B3,\"CNTRY_OF_RISK\")");
    assert_eq!(f(2, 3), "BDP(B3,\"GICS_SECTOR_NAME\")");
    assert_eq!(f(2, 4), "BDP(B3,\"GICS_INDUSTRY_GROUP_NAME\")");

    let fx = calamine::Reader::worksheet_formula(&mut wb, "FX").unwrap();
    let fx_formula = fx.get_value((1, 1)).cloned().unwrap_or_default();
    assert_eq!(fx_formula, "BDH(\"EURUSD Curncy\",\"PX_LAST\",$A$2,$A$4)");
}

#[test]
fn an_empty_request_still_produces_a_readable_workbook() {
    let bytes = build_request(&[], &[], d(2025, 3, 18), d(2026, 7, 24)).unwrap();
    let wb: calamine::Xlsx<_> = calamine::Xlsx::new(std::io::Cursor::new(bytes)).expect("valid xlsx");
    assert!(calamine::Reader::sheet_names(&wb).iter().any(|n| n == "REFS"));
}

/// Build a response workbook the way Excel would leave it: values, not formulas.
fn response_xlsx(refs: &[(&str, &str, &str, &str)], fx: &[(&str, &str, f64)]) -> Vec<u8> {
    let mut wb = rust_xlsxwriter::Workbook::new();
    let s = wb.add_worksheet().set_name("REFS").unwrap();
    for (c, h) in ["isin", "ticker", "country_of_risk", "gics_sector", "gics_industry"].iter().enumerate() {
        s.write_string(0, c as u16, *h).unwrap();
    }
    for (i, (isin, country, sector, industry)) in refs.iter().enumerate() {
        let r = (i + 1) as u32;
        s.write_string(r, 0, *isin).unwrap();
        s.write_string(r, 1, "T").unwrap();
        s.write_string(r, 2, *country).unwrap();
        s.write_string(r, 3, *sector).unwrap();
        s.write_string(r, 4, *industry).unwrap();
    }
    let f = wb.add_worksheet().set_name("FX").unwrap();
    f.write_string(0, 0, "date").unwrap();
    f.write_string(0, 1, "USD").unwrap();
    for (i, (date, ccy, rate)) in fx.iter().enumerate() {
        let r = (i + 1) as u32;
        f.write_string(r, 0, *date).unwrap();
        let _ = ccy;
        f.write_number(r, 1, *rate).unwrap();
    }
    wb.save_to_buffer().unwrap()
}

#[test]
fn parses_classifications_and_derives_region() {
    let bytes = response_xlsx(
        &[("FR0000121014", "France", "Consumer Discretionary", "Textiles Apparel & Luxury Goods")],
        &[],
    );
    let out = parse_response(&bytes).unwrap();
    assert_eq!(out.classifications.len(), 1);
    let c = &out.classifications[0];
    assert_eq!(c.isin, "FR0000121014");
    assert_eq!(c.country.as_deref(), Some("France"));
    assert_eq!(region_for("France"), Some("Europe"));
}

#[test]
fn unresolved_cells_are_skipped_and_reported_never_stored() {
    let bytes = response_xlsx(&[("IE00BYTBXV33", "#N/A", "#N/A N/A", "Industrials")], &[]);
    let out = parse_response(&bytes).unwrap();
    // The row survives for its usable field, but the unresolved ones are None.
    let c = out.classifications.iter().find(|c| c.isin == "IE00BYTBXV33").unwrap();
    assert!(c.country.is_none());
    assert!(c.sector.is_none());
    assert_eq!(c.industry.as_deref(), Some("Industrials"));
    assert!(!out.skipped.is_empty(), "unresolved cells must be reported");
}

#[test]
fn fx_rates_are_inverted_to_euros_per_unit() {
    // Bloomberg EURUSD = dollars per euro. 1.1379 USD per EUR -> 0.8788 EUR per USD.
    let bytes = response_xlsx(&[], &[("2026-07-24", "USD", 1.1379)]);
    let out = parse_response(&bytes).unwrap();
    let obs = out.fx.iter().find(|o| o.currency == "USD").unwrap();
    assert!((obs.rate_to_eur - 1.0 / 1.1379).abs() < 1e-9);
}

#[test]
fn a_non_positive_fx_rate_is_rejected_not_inverted() {
    let bytes = response_xlsx(&[], &[("2026-07-24", "USD", 0.0)]);
    let out = parse_response(&bytes).unwrap();
    assert!(out.fx.is_empty());
    assert!(!out.skipped.is_empty());
}
