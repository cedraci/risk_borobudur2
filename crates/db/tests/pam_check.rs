use db::auth::marker::{Import, Nav, Positions, Transactions};
use db::auth::AuthCtx;

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
///
/// `pam_warnings` used to `continue` silently - with no warning at all -
/// whenever the trade walk yielded a *zero* quantity while the workbook
/// still held a non-zero position: either because OPERATIONS has no rows
/// for the ISIN at all (`mine.is_empty()`), or because the recognized trades
/// round-trip exactly back to flat (`basis_end.qty <= 0.0`, which
/// `analytics::pnl` documents as the signature of a truncated history). Both
/// are now folded into the same quantity gate as `walked = 0.0`, which the
/// workbook's confirmed non-zero holding always fails, producing the same
/// "incomplete trade history" warning instead of nothing.
///
/// Neither silent-skip path is exercised by a real position in sample.xlsx as
/// pure cases (checked with a throwaway probe over every cash position): no
/// ISIN has zero recognized trades, and the only two ISINs whose walk ends
/// flat (`ES0113900J37`, `FR0010599399`) are *also* oversold, so the
/// oversold check - which runs first and is treated as subsuming the
/// quantity gate (a broken history is a broken history; double-tagging the
/// same instrument would just be noise) - reports them before the gate is
/// ever reached. The two assertions below pin that: they are exactly the
/// real ISINs available to exercise the boundary the fix touches, and they
/// guard against a regression where the restructuring accidentally routes
/// oversold positions through the gate too (which would either duplicate the
/// warning or, worse, replace "sells exceed recorded buys" with the less
/// specific "incomplete trade history").
#[tokio::test]
async fn pam_check_distinguishes_incomplete_history_from_genuine_drift() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    let dbh = db::Db::from_pool(pool.clone());
    let ctx = AuthCtx::desktop();
    let scoped = dbh.scope(&ctx);
    let p = scoped.authorize::<Positions, Import>(1).unwrap();
    let n = scoped.authorize::<Nav, Import>(1).unwrap();
    let t = scoped.authorize::<Transactions, Import>(1).unwrap();

    let bytes = std::fs::read(SAMPLE).unwrap();
    let wb = ingest::parse_workbook(&bytes).unwrap();
    let out = scoped.import_workbook(&p, &n, &t, "sample.xlsx", "sha-pam-1", &wb).await.unwrap();

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

    // Both walk to a flat (zero) ending quantity against a non-zero workbook
    // holding - exactly the shape the fix now routes through the quantity
    // gate - but both are also oversold, so the oversold check (which runs
    // first and subsumes the gate) must report them, and only them: no
    // "incomplete trade history" double-tag.
    for isin in ["ES0113900J37", "FR0010599399"] {
        assert!(has(isin, "sells exceed recorded buys"),
            "{isin} has a walk that ends flat (zero quantity) via an oversold sell, so it should \
             still warn as oversold: {:?}", out.warnings);
        assert!(!has(isin, "incomplete trade history"),
            "{isin} is oversold, which subsumes the quantity gate; it should not also be tagged \
             as incomplete trade history: {:?}", out.warnings);
        assert!(!has(isin, "PAM drift"),
            "{isin} should not report PAM drift when oversold: {:?}", out.warnings);
    }

    pool.close().await;
    edb.stop().await;
}
