use calamine::Reader;
use chrono::NaiveDate;
use ingest::bloomberg::{build_request, RequestItem};

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
