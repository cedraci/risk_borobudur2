//! Calamine round-trips, matching `emir_evidence.rs`'s rigor: every assertion
//! reads back a specific cell against a distinct sentinel value, so a column
//! swap or a dropped field fails a specific assertion rather than nothing at
//! all. See `.superpowers/sdd/2026-08-20-limit-breach-register/task-9-report.md`
//! for the mutation-testing evidence that these assertions can actually fail.

use calamine::{Data, Reader, Xlsx};
use ingest::breach_evidence::build;
use std::io::Cursor;

/// Blank cells normalize to `""` regardless of whether calamine reports them
/// as `Data::Empty` (inside the sheet's used range) or `None` (outside it) —
/// the run-history union test needs to assert "no status for this check on
/// this run" without caring which of the two calamine happens to return.
fn cell(r: &calamine::Range<Data>, row: u32, col: u32) -> String {
    match r.get_value((row, col)) {
        Some(Data::String(s)) => s.clone(),
        Some(Data::Float(f)) => f.to_string(),
        Some(Data::Bool(b)) => b.to_string(),
        Some(Data::Empty) | None => String::new(),
        other => format!("{other:?}"),
    }
}

fn open(bytes: Vec<u8>) -> Xlsx<Cursor<Vec<u8>>> {
    Xlsx::new(Cursor::new(bytes)).expect("valid xlsx")
}

#[test]
fn the_register_sheet_writes_each_breach_field_to_its_own_column() {
    // Every field a distinct value from every other, so a column swap (e.g.
    // "Acknowledged by" <-> "Resolved by", or "Acknowledged at" <-> "Opened")
    // is caught by a mismatched assertion rather than a coincidental match.
    // `closed_nav_date` is deliberately absent: a still-open episode still
    // records who acted on it, and that must not depend on it being cleared.
    let breach = serde_json::json!({
        "check_key": "issuer_10", "subject": "ACME CORP",
        "opened_nav_date": "2026-08-01", "peak_value": 0.106,
        "state": "acknowledged", "classification": "passive",
        "acknowledged_at": "2026-08-02T10:00:00Z",
        "acknowledged_by_label": "J. Dupont",
        "acknowledgement_note": "market move, no purchase",
        "resolved_at": "2026-08-05T09:00:00Z",
        "resolved_by_label": "M. Martin",
        "resolution_note": "position trimmed on 21 Aug",
    });
    let bytes = build("Borobudur", &[], &[breach]).unwrap();
    let mut wb = open(bytes);

    let mut names = wb.sheet_names().to_vec();
    names.sort();
    assert_eq!(names, ["Register", "Run history"], "unexpected sheet set: {names:?}");

    let r = wb.worksheet_range("Register").unwrap();
    // Row 3 (0-based) is the header row; row 4 is the one data row.
    assert_eq!(cell(&r, 3, 0), "Check");
    assert_eq!(cell(&r, 3, 12), "Resolution note", "header row must have all 13 columns");
    assert_eq!(cell(&r, 4, 0), "issuer_10", "check_key");
    assert_eq!(cell(&r, 4, 1), "ACME CORP", "subject");
    assert_eq!(cell(&r, 4, 2), "2026-08-01", "opened_nav_date");
    assert_eq!(cell(&r, 4, 3), "0.106", "peak_value");
    assert_eq!(cell(&r, 4, 4), "open", "no closed_nav_date -> the still-open fallback");
    assert_eq!(cell(&r, 4, 5), "acknowledged", "state");
    assert_eq!(cell(&r, 4, 6), "passive", "classification");
    assert_eq!(cell(&r, 4, 7), "2026-08-02T10:00:00Z", "acknowledged_at");
    assert_eq!(cell(&r, 4, 8), "J. Dupont", "acknowledged_by_label");
    assert_eq!(cell(&r, 4, 9), "market move, no purchase", "acknowledgement_note");
    assert_eq!(cell(&r, 4, 10), "2026-08-05T09:00:00Z", "resolved_at");
    assert_eq!(cell(&r, 4, 11), "M. Martin", "resolved_by_label");
    assert_eq!(cell(&r, 4, 12), "position trimmed on 21 Aug", "resolution_note");
}

#[test]
fn a_closed_episode_writes_its_closed_nav_date_instead_of_open() {
    let breach = serde_json::json!({
        "check_key": "issuer_10", "subject": "ACME CORP",
        "opened_nav_date": "2026-08-01", "closed_nav_date": "2026-08-09",
    });
    let bytes = build("Borobudur", &[], &[breach]).unwrap();
    let mut wb = open(bytes);
    let r = wb.worksheet_range("Register").unwrap();
    assert_eq!(cell(&r, 4, 4), "2026-08-09", "a closed episode must show its clearing date, not the open fallback");
}

#[test]
fn the_run_history_sheet_unions_check_keys_in_a_stable_order_and_leaves_gaps_blank() {
    // Run A only ran "aaa_check"; run B (a later date) only ran "zzz_check".
    // Names chosen so alphabetical (the union's sort order) and
    // chronological (the runs' own order) agree, which would let a bug that
    // used either "whichever check the newest run has" or "first run only"
    // slip through undetected if the names were reversed.
    let run_a = serde_json::json!({
        "nav_date": "2026-08-01", "run_at": "2026-08-01T09:00:00Z", "triggered_by": "import",
        "results": [{"check_key": "aaa_check", "scope_label": "A", "status": "ok"}],
    });
    let run_b = serde_json::json!({
        "nav_date": "2026-08-02", "run_at": "2026-08-02T09:00:00Z", "triggered_by": "manual",
        "results": [{"check_key": "zzz_check", "scope_label": "Z", "status": "breach"}],
    });
    let bytes = build("Borobudur", &[run_a, run_b], &[]).unwrap();
    let mut wb = open(bytes);
    let r = wb.worksheet_range("Run history").unwrap();

    // Column order: the three fixed columns, then the union of check keys —
    // "aaa_check" before "zzz_check" regardless of which run introduced
    // which key, or which order the runs were supplied in.
    assert_eq!(cell(&r, 3, 0), "NAV date");
    assert_eq!(cell(&r, 3, 3), "aaa_check", "the union's first column");
    assert_eq!(cell(&r, 3, 4), "zzz_check", "the union's second column");

    // Run A's row: its own check has a status; the OTHER run's check-only
    // column is blank. A bug that unioned only the first run's keys would
    // make column 4 simply not exist (a shifted/absent column, not merely a
    // blank cell) — `cell` at 3,4 above already guards that; this guards the
    // per-row fill.
    assert_eq!(cell(&r, 4, 0), "2026-08-01");
    assert_eq!(cell(&r, 4, 3), "ok", "run A's own check");
    assert_eq!(cell(&r, 4, 4), "", "run A never ran zzz_check");

    // Run B's row: the reverse.
    assert_eq!(cell(&r, 5, 0), "2026-08-02");
    assert_eq!(cell(&r, 5, 3), "", "run B never ran aaa_check");
    assert_eq!(cell(&r, 5, 4), "breach", "run B's own check");
}

#[test]
fn an_empty_register_and_run_history_say_so_in_the_workbook() {
    let bytes = build("Borobudur", &[], &[]).unwrap();
    let mut wb = open(bytes);

    let r = wb.worksheet_range("Register").unwrap();
    assert_eq!(cell(&r, 4, 0), "No breaches recorded.");

    let rh = wb.worksheet_range("Run history").unwrap();
    assert_eq!(cell(&rh, 4, 0), "No runs recorded.");
}
