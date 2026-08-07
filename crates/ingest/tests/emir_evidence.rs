use calamine::{Data, Reader, Xlsx};
use ingest::emir_file::{build_evidence, ContractRow, EmirEvidence, KpiRow, MonthRow, SummaryRow};
use std::io::Cursor;

fn cell(r: &calamine::Range<Data>, row: u32, col: u32) -> String {
    match r.get_value((row, col)) {
        Some(Data::String(s)) => s.clone(),
        Some(Data::Float(f)) => f.to_string(),
        Some(Data::Bool(b)) => b.to_string(),
        other => format!("{other:?}"),
    }
}

#[test]
fn evidence_round_trips_through_calamine() {
    let e = EmirEvidence {
        anchor: "2026-07-24".parse().unwrap(),
        months_present: 2,
        months_total: 12,
        summary: vec![SummaryRow {
            label: "Interest-rate derivatives".into(),
            threshold_eur: 3e9,
            avg_otc_eur: 0.0,
            pct_of_threshold: 0.0,
            verdict: "ok".into(),
            avg_total_eur: 1.25e7,
        }],
        months: vec![
            MonthRow { label: "Interest-rate derivatives".into(), month: "2026-06".into(), snapshot_date: Some("2026-06-26".into()), total_eur: Some(1.0e7), otc_eur: Some(0.0) },
            MonthRow { label: "Interest-rate derivatives".into(), month: "2026-05".into(), snapshot_date: None, total_eur: None, otc_eur: None },
        ],
        contracts: vec![ContractRow { root: "RX".into(), label: "Euro-Bund".into(), category: "interest_rate".into(), otc: false, confirmed: true, point_value: Some(1000.0), currency: "EUR".into() }],
        kpis: vec![KpiRow { month: "2026-07".into(), unconfirmed_over_5d: 0, reconciliation: "not_applicable".into(), disputes: 0, note: "".into() }],
        warnings: vec!["2026-05: no snapshot in this month; excluded from the average".into()],
    };
    let bytes = build_evidence(&e).unwrap();

    let mut wb: Xlsx<_> = Xlsx::new(Cursor::new(bytes)).expect("valid xlsx");
    let names = wb.sheet_names().to_vec();
    for n in ["Seuils", "Contrats", "KPI"] {
        assert!(names.iter().any(|x| x == n), "missing sheet {n} in {names:?}");
    }

    let s = wb.worksheet_range("Seuils").unwrap();
    assert!(cell(&s, 0, 0).contains("EMIR"));
    assert!(cell(&s, 1, 0).contains("2026-07-24"));
    assert!(cell(&s, 2, 0).contains("2 of 12"));
    // Summary table: header row then the one class row.
    assert_eq!(cell(&s, 5, 0), "Class");
    assert_eq!(cell(&s, 6, 0), "Interest-rate derivatives");
    assert_eq!(cell(&s, 6, 1), "3000000000");
    assert_eq!(cell(&s, 6, 4), "ok");
    // Detail table: one blank row after the summary block (summary ends at
    // row 6, row 7 blank), header at row 8, rows at 9-10.
    assert_eq!(cell(&s, 8, 0), "Class");
    assert_eq!(cell(&s, 9, 1), "2026-06");
    assert_eq!(cell(&s, 9, 2), "2026-06-26");
    assert_eq!(cell(&s, 10, 2), "missing");
    // Warnings block ends the sheet: blank row 11, "Warnings" at 12, line at 13.
    assert_eq!(cell(&s, 12, 0), "Warnings");
    assert!(cell(&s, 13, 0).contains("2026-05"));

    let c = wb.worksheet_range("Contrats").unwrap();
    assert_eq!(cell(&c, 1, 0), "RX");
    assert_eq!(cell(&c, 1, 3), "false");

    let k = wb.worksheet_range("KPI").unwrap();
    assert_eq!(cell(&k, 1, 0), "2026-07");
    assert_eq!(cell(&k, 1, 2), "not_applicable");
}
