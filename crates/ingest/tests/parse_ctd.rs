use chrono::NaiveDate;

const CSV: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ctd_sample.csv");
const XLSX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ctd_sample.xlsx");
const XLSX_TEXT_DATE: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ctd_text_date.xlsx");

#[test]
fn parses_the_sample_csv() {
    let bytes = std::fs::read(CSV).unwrap();
    let rows = ingest::parse_ctd_file(&bytes, "ctd_sample.csv").unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].nav_date, NaiveDate::from_ymd_opt(2026, 7, 24).unwrap());
    assert_eq!(rows[0].ticker, "RXU6 Comdty");
    assert_eq!(rows[0].ctd_isin, "DE0001102580");
    assert!((rows[0].ctd_mod_duration - 8.41).abs() < 1e-12);
    assert!((rows[0].conversion_factor - 0.782145).abs() < 1e-12);
}

#[test]
fn parses_the_sample_xlsx() {
    // Same four rows as ctd_sample.csv, on a sheet named "CTD", with nav_date
    // written as a real Excel date value and the numerics as real numbers —
    // the shape a Bloomberg export or hand-built workbook actually produces,
    // and a code path (read_xlsx) the CSV tests above cannot exercise.
    let bytes = std::fs::read(XLSX).unwrap();
    let rows = ingest::parse_ctd_file(&bytes, "ctd_sample.xlsx").unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].nav_date, NaiveDate::from_ymd_opt(2026, 7, 24).unwrap());
    assert_eq!(rows[0].ticker, "RXU6 Comdty");
    assert_eq!(rows[0].ctd_isin, "DE0001102580");
    assert!((rows[0].ctd_mod_duration - 8.41).abs() < 1e-12);
    assert!((rows[0].ctd_clean_price - 98.72).abs() < 1e-12);
    assert!((rows[0].ctd_accrued - 0.63).abs() < 1e-12);
    assert!((rows[0].conversion_factor - 0.782145).abs() < 1e-12);
}

#[test]
fn xlsx_text_formatted_date_cell_still_parses() {
    // Users retype nav_date cells constantly, which Excel is happy to store
    // as plain text rather than a typed date. calamine hands that back as
    // Data::String, and read_xlsx's fallback `other.to_string()` branch
    // passes the literal text through unchanged, so the same
    // "%Y-%m-%d" parse the CSV path relies on still applies. A text
    // "2026-07-24" cell is expected to parse successfully, not fail the row.
    let bytes = std::fs::read(XLSX_TEXT_DATE).unwrap();
    let rows = ingest::parse_ctd_file(&bytes, "ctd_text_date.xlsx").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].nav_date, NaiveDate::from_ymd_opt(2026, 7, 24).unwrap());
    assert_eq!(rows[0].ticker, "RXU6 Comdty");
    assert_eq!(rows[0].ctd_isin, "DE0001102580");
    assert!((rows[0].ctd_mod_duration - 8.41).abs() < 1e-12);
    assert!((rows[0].ctd_clean_price - 98.72).abs() < 1e-12);
}

#[test]
fn header_order_is_free_and_case_insensitive() {
    let src = "Ticker, NAV_DATE ,conversion_factor,ctd_accrued,ctd_clean_price,ctd_mod_duration,ctd_isin\n\
               RXU6 Comdty,2026-07-24,0.78,0.6,98.7,8.4,DE0001102580\n";
    let rows = ingest::parse_ctd_file(src.as_bytes(), "x.csv").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].ticker, "RXU6 Comdty");
    assert!((rows[0].ctd_mod_duration - 8.4).abs() < 1e-12);
}

#[test]
fn rejects_missing_header_column() {
    let src = "nav_date,ticker,ctd_isin,ctd_mod_duration,ctd_clean_price,ctd_accrued\n\
               2026-07-24,RXU6 Comdty,DE0001102580,8.4,98.7,0.6\n";
    match ingest::parse_ctd_file(src.as_bytes(), "x.csv") {
        Err(ingest::ParseFailure::Workbook(m)) => assert!(m.contains("conversion_factor"), "{m}"),
        other => panic!("expected a workbook-level failure, got {other:?}"),
    }
}

#[test]
fn rejects_empty_file() {
    let src = "nav_date,ticker,ctd_isin,ctd_mod_duration,ctd_clean_price,ctd_accrued,conversion_factor\n";
    match ingest::parse_ctd_file(src.as_bytes(), "x.csv") {
        Err(ingest::ParseFailure::Workbook(m)) => assert!(m.contains("no data rows"), "{m}"),
        other => panic!("expected a workbook-level failure, got {other:?}"),
    }
}

#[test]
fn rejects_disagreeing_nav_dates() {
    let src = "nav_date,ticker,ctd_isin,ctd_mod_duration,ctd_clean_price,ctd_accrued,conversion_factor\n\
               2026-07-24,RXU6 Comdty,DE0001102580,8.4,98.7,0.6,0.78\n\
               2026-07-17,OATU6 Comdty,FR0014007L00,7.9,95.3,1.1,0.74\n";
    match ingest::parse_ctd_file(src.as_bytes(), "x.csv") {
        Err(ingest::ParseFailure::Workbook(m)) => assert!(m.contains("nav_date"), "{m}"),
        other => panic!("expected a workbook-level failure, got {other:?}"),
    }
}

#[test]
fn collects_all_row_errors_before_failing() {
    let src = "nav_date,ticker,ctd_isin,ctd_mod_duration,ctd_clean_price,ctd_accrued,conversion_factor\n\
               2026-07-24,RXU6 Comdty,DE0001102580,0,98.7,0.6,0.78\n\
               2026-07-24,,FR0014007L00,7.9,95.3,1.1,0.74\n\
               2026-07-24,KOAU6 Comdty,ES0000012L44,7.6,97.0,-1,0.76\n\
               2026-07-24,TYU6 Comdty,US91282CJK17,6.4,99.1,0.4,abc\n\
               2026-07-24,RXU6 Comdty,DE0001102580,8.4,98.7,0.6,0.78\n";
    match ingest::parse_ctd_file(src.as_bytes(), "x.csv") {
        Err(ingest::ParseFailure::Rows(rows)) => {
            assert_eq!(rows.len(), 5, "one per bad row, all collected");
            assert_eq!(rows[0].row, 2, "1-based, header is row 1");
            assert!(rows[0].message.contains("ctd_mod_duration"));
            assert!(rows[1].message.contains("ticker"));
            assert!(rows[2].message.contains("ctd_accrued"));
            assert!(rows[3].message.contains("conversion_factor"));
            assert!(rows[4].message.contains("duplicate"), "{}", rows[4].message);
        }
        other => panic!("expected row failures, got {other:?}"),
    }
}
