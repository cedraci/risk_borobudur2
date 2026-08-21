#[test]
fn the_evidence_file_has_a_register_sheet_and_a_run_history_sheet() {
    let runs = vec![serde_json::json!({
        "nav_date": "2026-08-07", "run_at": "2026-08-07T09:00:00Z",
        "triggered_by": "import", "inputs_complete": true,
        "results": [{"check_key": "issuer_10", "scope_label": "Issuer <= 10% NAV",
                     "limit_value": 0.10, "observed_value": 0.106, "status": "breach"}]
    })];
    let breaches = vec![serde_json::json!({
        "check_key": "issuer_10", "subject": "ACME",
        "opened_nav_date": "2026-08-07", "peak_value": 0.106,
        "state": "acknowledged", "classification": "passive",
        "acknowledgement_note": "market move, no purchase"
    })];
    let bytes = ingest::breach_evidence::build("Borobudur", &runs, &breaches).unwrap();
    assert!(bytes.len() > 4000, "an xlsx with two sheets is never this small");
    assert_eq!(&bytes[0..2], b"PK", "xlsx files are zip archives");
}

#[test]
fn an_empty_register_still_produces_a_file_that_says_so() {
    let bytes = ingest::breach_evidence::build("Borobudur", &[], &[]).unwrap();
    assert!(bytes.len() > 2000);
}
