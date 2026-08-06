const SAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ingest/tests/fixtures/sample.xlsx");

/// `OPERATIONS` is meant to be a complete lifetime trade history, so a real
/// mismatch between the trade walk and the administrator's PAM column is a
/// genuine data problem worth surfacing, not noise to suppress. But the walk
/// can only speak to positions whose full history is actually present: when
/// the walked quantity disagrees with the snapshot quantity, that disagreement
/// is itself the evidence that OPERATIONS is missing trades for that
/// instrument, and comparing cost basis in that state would blame "PAM drift"
/// on what is really an incomplete record.
///
/// Pinned against the real fund history in sample.xlsx:
/// - `NL0015001FS8`, `ES0105046017`, `FR0014008B43` each have an earlier
///   buy/sell round trip that never closes out in OPERATIONS, leaving the
///   walked quantity higher than the workbook's holding by exactly the
///   unresolved remainder. These fail the quantity gate: incomplete history,
///   cost basis not compared.
/// - `GB00B2B0DG97` has a walked quantity that matches the workbook exactly
///   (its whole visible history round-trips to zero before a single final
///   lot), so it passes the gate - and its cost basis genuinely disagrees
///   with the administrator's PAM. That is a real drift, not a history gap.
/// - `FR0000120859` reconciles cleanly: quantity and cost basis both agree,
///   so it produces neither warning. Without this assertion the other two
///   would trivially pass if the gate suppressed every warning.
#[tokio::test]
async fn pam_check_distinguishes_incomplete_history_from_genuine_drift() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();

    let bytes = std::fs::read(SAMPLE).unwrap();
    let wb = ingest::parse_workbook(&bytes).unwrap();
    let out = db::repo::import_workbook(&pool, "sample.xlsx", "sha-pam-1", &wb).await.unwrap();

    let has = |isin: &str, needle: &str| {
        out.warnings.iter().any(|w| w.starts_with(isin) && w.contains(needle))
    };

    for isin in ["NL0015001FS8", "ES0105046017", "FR0014008B43"] {
        assert!(has(isin, "incomplete trade history"),
            "{isin} should be gated as incomplete history: {:?}", out.warnings);
        assert!(!has(isin, "PAM drift"),
            "{isin} should not also report PAM drift: {:?}", out.warnings);
    }

    assert!(has("GB00B2B0DG97", "PAM drift"),
        "GB00B2B0DG97's quantity matches the workbook, so its genuine cost-basis \
         mismatch should surface as PAM drift: {:?}", out.warnings);
    assert!(!has("GB00B2B0DG97", "incomplete trade history"),
        "GB00B2B0DG97 should not be gated: {:?}", out.warnings);

    assert!(!has("FR0000120859", "PAM drift") && !has("FR0000120859", "incomplete trade history"),
        "FR0000120859 reconciles cleanly and must not be flagged either way: {:?}", out.warnings);

    pool.close().await;
    edb.stop().await;
}
