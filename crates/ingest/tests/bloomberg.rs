use calamine::Reader;
use chrono::NaiveDate;
use ingest::bloomberg::{build_request, parse_response, region_for, RequestItem};
use ingest::ParseFailure;

fn d(y: i32, m: u32, dd: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, dd).unwrap() }

#[test]
fn request_workbook_has_the_three_expected_sheets() {
    let items = vec![RequestItem { isin: "FR0000121014".into() }];
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
}

#[test]
fn refs_and_fx_formulas_reference_the_correct_rows_and_ranges() {
    let items = vec![
        RequestItem { isin: "FR0000121014".into() },
        RequestItem { isin: "US0378331005".into() },
    ];
    let bytes = build_request(&items, &["USD".into()], d(2025, 3, 18), d(2026, 7, 24)).unwrap();

    let mut wb: calamine::Xlsx<_> =
        calamine::Xlsx::new(std::io::Cursor::new(bytes)).expect("valid xlsx");

    let refs = calamine::Reader::worksheet_formula(&mut wb, "REFS").unwrap();
    let f = |r: u32, c: u32| refs.get_value((r, c)).cloned().unwrap_or_default();

    // The NAV Recap carries no Bloomberg ticker, so column B resolves the
    // full security key from the ISIN on the Terminal itself; the
    // classification formulas then query that ticker, not the raw ISIN.
    assert_eq!(f(1, 1), "BDP(\"/isin/FR0000121014\",\"PARSEKYABLE_DES\")");
    assert_eq!(f(2, 1), "BDP(\"/isin/US0378331005\",\"PARSEKYABLE_DES\")");

    // item 1 (row 2 in Excel, row index 1): country/sector/industry BDP formulas
    // must reference ticker cell B2.
    assert_eq!(f(1, 2), "BDP(B2,\"CNTRY_OF_RISK\")");
    assert_eq!(f(1, 3), "BDP(B2,\"GICS_SECTOR_NAME\")");
    assert_eq!(f(1, 4), "BDP(B2,\"GICS_INDUSTRY_GROUP_NAME\")");

    // item 2 (row 3 in Excel, row index 2): same three formulas, referencing B3.
    assert_eq!(f(2, 2), "BDP(B3,\"CNTRY_OF_RISK\")");
    assert_eq!(f(2, 3), "BDP(B3,\"GICS_SECTOR_NAME\")");
    assert_eq!(f(2, 4), "BDP(B3,\"GICS_INDUSTRY_GROUP_NAME\")");

    // FX: one currency owns a two-column block (anchor + "rate" header);
    // the formula spills dates into the anchor column and values into the
    // next, so the anchor is where parse_response expects the date series.
    let fx_values = calamine::Reader::worksheet_range(&mut wb, "FX").unwrap();
    assert_eq!(fx_values.get_value((0, 0)).map(|v| v.to_string()), Some("USD".to_string()));
    assert_eq!(fx_values.get_value((0, 1)).map(|v| v.to_string()), Some("rate".to_string()));

    let fx = calamine::Reader::worksheet_formula(&mut wb, "FX").unwrap();
    let fx_formula = fx.get_value((1, 0)).cloned().unwrap_or_default();
    assert_eq!(
        fx_formula,
        "BDH(\"EURUSD Curncy\",\"PX_LAST\",\"20250318\",\"20260724\",\"Dts=S\",\"Sort=A\")"
    );
}

#[test]
fn fx_currency_blocks_are_two_columns_apart_so_spills_cannot_overlap() {
    let bytes = build_request(&[], &["USD".into(), "GBP".into()], d(2025, 3, 18), d(2026, 7, 24)).unwrap();
    let mut wb: calamine::Xlsx<_> =
        calamine::Xlsx::new(std::io::Cursor::new(bytes)).expect("valid xlsx");
    let fx = calamine::Reader::worksheet_range(&mut wb, "FX").unwrap();
    let h = |r: u32, c: u32| fx.get_value((r, c)).map(|v| v.to_string()).unwrap_or_default();
    assert_eq!(h(0, 0), "USD");
    assert_eq!(h(0, 1), "rate");
    assert_eq!(h(0, 2), "GBP");
    assert_eq!(h(0, 3), "rate");

    let formulas = calamine::Reader::worksheet_formula(&mut wb, "FX").unwrap();
    let f = |r: u32, c: u32| formulas.get_value((r, c)).cloned().unwrap_or_default();
    assert!(f(1, 0).contains("EURUSD"), "USD formula should anchor column 0, got {}", f(1, 0));
    assert!(f(1, 2).contains("EURGBP"), "GBP formula should anchor column 2, got {}", f(1, 2));
}

#[test]
fn an_empty_request_still_produces_a_readable_workbook() {
    let bytes = build_request(&[], &[], d(2025, 3, 18), d(2026, 7, 24)).unwrap();
    let wb: calamine::Xlsx<_> = calamine::Xlsx::new(std::io::Cursor::new(bytes)).expect("valid xlsx");
    assert!(calamine::Reader::sheet_names(&wb).iter().any(|n| n == "REFS"));
}

/// Build a response workbook the way Excel would leave it: values, not
/// formulas. FX currencies are laid out in two-column blocks (date, rate),
/// one block per currency in order of first appearance in `fx`, matching
/// the layout `build_request` writes and `parse_response` reads.
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
    let mut currencies: Vec<&str> = Vec::new();
    for (_, ccy, _) in fx {
        if !currencies.contains(ccy) {
            currencies.push(ccy);
        }
    }
    for (i, ccy) in currencies.iter().enumerate() {
        let a = (i * 2) as u16;
        f.write_string(0, a, *ccy).unwrap();
        f.write_string(0, a + 1, "rate").unwrap();
        let mut row = 1u32;
        for (date, c, rate) in fx {
            if c != ccy {
                continue;
            }
            f.write_string(row, a, *date).unwrap();
            f.write_number(row, a + 1, *rate).unwrap();
            row += 1;
        }
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

/// The round trip the brief promised: build the request, confirm the FX
/// column layout `build_request` actually wrote (not an assumed one), then
/// feed a resolved workbook laid out at those exact columns through
/// `parse_response`. This is the test that would have caught the two
/// halves disagreeing about where a currency's dates and rates live.
#[test]
fn round_trip_through_build_request_recovers_fx_rates() {
    let from = d(2026, 7, 20);
    let to = d(2026, 7, 24);
    let request_bytes =
        build_request(&[], &["USD".into(), "GBP".into()], from, to).unwrap();

    // Confirm the exact cells the formulas spilled into: a two-column block
    // per currency, blocks two columns apart.
    let mut req_wb: calamine::Xlsx<_> =
        calamine::Xlsx::new(std::io::Cursor::new(request_bytes)).expect("valid xlsx");
    let hdr = calamine::Reader::worksheet_range(&mut req_wb, "FX").unwrap();
    let h = |r: u32, c: u32| hdr.get_value((r, c)).map(|v| v.to_string()).unwrap_or_default();
    assert_eq!(h(0, 0), "USD");
    assert_eq!(h(0, 1), "rate");
    assert_eq!(h(0, 2), "GBP");
    assert_eq!(h(0, 3), "rate");
    let formulas = calamine::Reader::worksheet_formula(&mut req_wb, "FX").unwrap();
    let f = |r: u32, c: u32| formulas.get_value((r, c)).cloned().unwrap_or_default();
    assert!(f(1, 0).contains("EURUSD"));
    assert!(f(1, 2).contains("EURGBP"));

    // Simulate what Excel leaves behind once the BDH formulas resolve and
    // are saved as values: two dates per currency, at those same columns.
    let bytes = response_xlsx(
        &[],
        &[
            ("2026-07-21", "USD", 1.10),
            ("2026-07-22", "USD", 1.12),
            ("2026-07-21", "GBP", 0.86),
            ("2026-07-22", "GBP", 0.85),
        ],
    );

    let out = parse_response(&bytes).unwrap();
    assert_eq!(out.fx.len(), 4);

    let usd: Vec<&ingest::bloomberg::FxObservation> =
        out.fx.iter().filter(|o| o.currency == "USD").collect();
    assert_eq!(usd.len(), 2);
    assert_eq!(usd[0].date, d(2026, 7, 21));
    assert!((usd[0].rate_to_eur - 1.0 / 1.10).abs() < 1e-9);
    assert_eq!(usd[1].date, d(2026, 7, 22));
    assert!((usd[1].rate_to_eur - 1.0 / 1.12).abs() < 1e-9);

    let gbp: Vec<&ingest::bloomberg::FxObservation> =
        out.fx.iter().filter(|o| o.currency == "GBP").collect();
    assert_eq!(gbp.len(), 2);
    assert_eq!(gbp[0].date, d(2026, 7, 21));
    assert!((gbp[0].rate_to_eur - 1.0 / 0.86).abs() < 1e-9);
    assert_eq!(gbp[1].date, d(2026, 7, 22));
    assert!((gbp[1].rate_to_eur - 1.0 / 0.85).abs() < 1e-9);
}

/// `calamine` surfaces a genuinely unresolved Bloomberg cell as
/// `Data::Error(..)`, not the literal string `"#N/A"` — this is the form a
/// real, unresolved file actually carries. Built via a formula whose cached
/// result is a spreadsheet error code, which `rust_xlsxwriter` writes with
/// the `t="e"` cell type that `calamine` reads back as `Data::Error`.
#[test]
fn data_error_cells_are_treated_as_unresolved_same_as_the_na_string() {
    let mut wb = rust_xlsxwriter::Workbook::new();
    let s = wb.add_worksheet().set_name("REFS").unwrap();
    for (c, h) in ["isin", "ticker", "country_of_risk", "gics_sector", "gics_industry"].iter().enumerate() {
        s.write_string(0, c as u16, *h).unwrap();
    }
    s.write_string(1, 0, "IE00BYTBXV33").unwrap();
    s.write_string(1, 1, "T").unwrap();
    s.write_formula(1, 2, rust_xlsxwriter::Formula::new("NA()").set_result("#N/A")).unwrap();
    s.write_string(1, 3, "Industrials").unwrap();
    s.write_string(1, 4, "Industrials Group").unwrap();
    wb.add_worksheet().set_name("FX").unwrap();
    let bytes = wb.save_to_buffer().unwrap();

    // Verify the fixture itself: this must be a real Data::Error cell, not
    // the literal string, or the test below would silently exercise the
    // already-covered string branch instead of the one this test is for.
    let mut check_wb: calamine::Xlsx<_> =
        calamine::Xlsx::new(std::io::Cursor::new(bytes.clone())).expect("valid xlsx");
    let refs = calamine::Reader::worksheet_range(&mut check_wb, "REFS").unwrap();
    assert!(
        matches!(refs.get_value((1, 2)), Some(calamine::Data::Error(_))),
        "fixture must produce a genuine Data::Error cell, got {:?}",
        refs.get_value((1, 2))
    );

    let out = parse_response(&bytes).unwrap();
    let c = out.classifications.iter().find(|c| c.isin == "IE00BYTBXV33").unwrap();
    assert!(c.country.is_none(), "Data::Error cell must not be stored as data");
    assert_eq!(c.sector.as_deref(), Some("Industrials"));
    assert!(!out.skipped.is_empty(), "the Data::Error cell must be reported");
}

#[test]
fn a_workbook_with_neither_refs_nor_fx_sheet_is_rejected() {
    let mut wb = rust_xlsxwriter::Workbook::new();
    wb.add_worksheet().set_name("SOMETHING_ELSE").unwrap();
    let bytes = wb.save_to_buffer().unwrap();

    match parse_response(&bytes) {
        Err(ParseFailure::Workbook(msg)) => {
            assert!(msg.contains("REFS"), "message should name REFS: {msg}");
            assert!(msg.contains("FX"), "message should name FX: {msg}");
        }
        other => panic!("expected ParseFailure::Workbook, got {other:?}"),
    }
}
