use ingest::caceis;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/caceis_hisinv.csv");
const FNAME: &str = "HISINVLUX_165878_20260807_20260810130151.csv";

fn batch() -> ingest::adapter::UniversalBatch {
    let bytes = std::fs::read(FIXTURE).unwrap();
    caceis::parse_hisinv(FNAME, &bytes).expect("fixture parses")
}

#[test]
fn transposes_an_equity_row_exactly() {
    let b = batch();
    assert_eq!(b.primary_date, chrono::NaiveDate::from_ymd_opt(2026, 8, 7).unwrap());
    assert_eq!(b.snapshots.len(), 1);
    let s = &b.snapshots[0];
    let p = s.positions.iter().find(|p| p.isin == "AT000000STR1").expect("STRABAG present");
    assert_eq!(p.asset_type, "Action");
    assert_eq!(p.name.as_deref(), Some("STRABAG SE-BR"));
    assert_eq!(p.currency.as_deref(), Some("EUR"));
    assert_eq!(p.quantity, Some(3400.0));
    assert_eq!(p.price, Some(85.5));
    assert_eq!(p.avg_cost, Some(91.0));
    assert_eq!(p.valuation_eur, Some(290700.0));
    assert_eq!(p.valuation_ccy, Some(290700.0));
    assert_eq!(p.fx_rate, Some(1.0));
    assert!((p.weight.unwrap() - 0.0101).abs() < 1e-9, "weight is a fraction: {:?}", p.weight);
    assert_eq!(p.ticker.as_deref(), Some("STR AV"));
}

#[test]
fn transposes_fx_futures_cash_and_receivables() {
    let b = batch();
    let s = &b.snapshots[0];

    // GBP equity: fx_rate = EUR per GBP = valuation_eur / valuation_ccy.
    let gkp = s.positions.iter().find(|p| p.isin == "BMG4209G2077").unwrap();
    assert_eq!(gkp.asset_type, "Action");
    assert!((gkp.fx_rate.unwrap() - 306425.21 / 262468.51).abs() < 1e-9);

    // JPY currency future: mark-to-market in the valuation column, ticker kept.
    let fut = s.positions.iter().find(|p| p.isin == "RYCU2609").unwrap();
    assert_eq!(fut.asset_type, "Future");
    assert_eq!(fut.quantity, Some(-7.0));
    assert_eq!(fut.valuation_eur, Some(10453.76));
    assert_eq!(fut.ticker.as_deref(), Some("RYU6 Curncy"));

    // Cash account: price is the conversion rate in the file -> None here.
    let cash = s.positions.iter().find(|p| p.isin == "BK001CHF").unwrap();
    assert_eq!(cash.asset_type, "Cash Acc");
    assert_eq!(cash.price, None);
    assert_eq!(cash.quantity, Some(125894.78));
    assert_eq!(cash.valuation_eur, Some(134805.42));

    // Margin account and fee provision map to their NAV Recap labels.
    assert_eq!(s.positions.iter().find(|p| p.isin == "DG1C7JPY").unwrap().asset_type, "Margin Acc");
    assert_eq!(s.positions.iter().find(|p| p.isin == "FP201EUR").unwrap().asset_type, "Frais provisionnés");

    // CPON receivable -> Dividendes, GBP local value preserved.
    let cpon = s.positions.iter().find(|p| p.isin == "GB0009895292" && p.asset_type == "Dividendes").unwrap();
    assert_eq!(cpon.currency.as_deref(), Some("GBP"));
    assert_eq!(cpon.valuation_ccy, Some(636.8));
    assert_eq!(cpon.valuation_eur, Some(743.45));

    // The fund and the 13900 ETC.
    assert_eq!(s.positions.iter().find(|p| p.isin == "FR0010599399").unwrap().asset_type, "Fonds");
    assert_eq!(s.positions.iter().find(|p| p.isin == "DE000A1EK0G3").unwrap().asset_type, "Obligation");
}

#[test]
fn emits_ref_hints_for_securities_only() {
    let b = batch();
    let strabag = b.ref_hints.iter().find(|h| h.isin == "AT000000STR1").expect("hint for STRABAG");
    assert_eq!(strabag.country_of_risk.as_deref(), Some("Germany")); // risk country col 41 = DEU
    assert_eq!(strabag.region.as_deref(), Some("Europe"));
    assert_eq!(strabag.ticker.as_deref(), Some("STR AV"));
    // No hints for cash/margin/CPON rows.
    assert!(!b.ref_hints.iter().any(|h| h.isin.starts_with("BK001") || h.isin.starts_with("DG1C7")));
    // The batch carries no journals.
    assert!(b.dividends.is_none() && b.operations.is_none());
    assert!(b.nav_points.is_empty());
}

#[test]
fn filename_and_row_disagreement_is_a_file_error() {
    let bytes = std::fs::read(FIXTURE).unwrap();
    let err = caceis::parse_hisinv("HISINVLUX_165878_20991231_20260810130151.csv", &bytes);
    assert!(matches!(err, Err(ingest::ParseFailure::Workbook(_))), "date mismatch must reject the file");
    let err2 = caceis::parse_hisinv("HISINVLUX_999999_20260807_20260810130151.csv", &bytes);
    assert!(matches!(err2, Err(ingest::ParseFailure::Workbook(_))), "fund-code mismatch must reject the file");
}

#[test]
fn unmappable_asset_code_drops_the_row_with_a_warning() {
    let bytes = std::fs::read(FIXTURE).unwrap();
    let text: String = bytes.iter().map(|&b| b as char).collect();
    // Corrupt one row's CATVAL to an unknown category.
    let bad = text.replacen(";VMOB;", ";XXXX;", 1);
    let bad_bytes: Vec<u8> = bad.chars().map(|c| c as u8).collect();
    let b = caceis::parse_hisinv(FNAME, &bad_bytes).unwrap();
    assert!(b.warnings.iter().any(|w| w.contains("XXXX")), "warning names the code: {:?}", b.warnings);
    let total: usize = b.snapshots[0].positions.len();
    let full = batch().snapshots[0].positions.len();
    assert_eq!(total, full - 1, "exactly the corrupted row dropped");
}

#[test]
fn hisinv_emits_depositary_statics_as_authoritative_facts() {
    let b = batch();
    let bond = b.ref_facts.iter().find(|f| f.isin == "US105756CL22").expect("bond fact");
    assert_eq!(bond.bond_maturity, chrono::NaiveDate::from_ymd_opt(2035, 3, 15));
    assert_eq!(bond.bond_next_coupon, chrono::NaiveDate::from_ymd_opt(2026, 9, 15));
    assert_eq!(bond.bond_coupon_pct, Some(6.625));
    assert_eq!(bond.bond_nominal, Some(100.0));
    // Frequency is not in HISINVLUX; it comes from INVJCPLUX or the inference.
    assert_eq!(bond.bond_coupon_freq, None);
    assert_eq!(bond.market_place.as_deref(), Some("186"));
}

#[test]
fn market_place_distinguishes_listed_from_unlisted() {
    let b = batch();
    let by = |isin: &str| b.ref_facts.iter().find(|f| f.isin == isin).cloned();
    let eq = by("AT000000STR1").expect("equity fact");
    assert_eq!(eq.market_place.as_deref(), Some("050"));
    assert_eq!(eq.market_place_name.as_deref(), Some("WIENER BOERSE"));
    // Cash, provisions and futures quote at a forced price and are not listed.
    let forced = b.ref_facts.iter().filter(|f| f.market_place.as_deref() == Some("FOR")).count();
    assert!(forced > 0, "the sample holds cash and futures rows");
}

#[test]
fn an_absent_static_is_none_not_zero() {
    let b = batch();
    let eq = b.ref_facts.iter().find(|f| f.isin == "AT000000STR1").unwrap();
    assert_eq!(eq.bond_maturity, None);
    assert_eq!(eq.bond_next_coupon, None);
}

#[test]
fn a_futures_contract_expiry_is_not_a_bond_maturity() {
    let b = batch();
    for isin in ["CFIN2608", "RYCU2609"] {
        let fut = b.ref_facts.iter().find(|f| f.isin == isin)
            .unwrap_or_else(|| panic!("expected a RefFact for futures contract {isin}"));
        // Column 49 carries a real date here (a contract expiry, e.g.
        // 20260831 / 20260930) — proof the fact was actually pushed and this
        // assertion isn't vacuously true because the row is absent.
        assert_eq!(fut.market_place.as_deref(), Some("FOR"));
        // The coupon-type gate (blank on these rows) must keep that expiry
        // out of the bond fields — a futures mark-to-market must never be
        // mistaken for a bond's principal redemption.
        assert_eq!(fut.bond_maturity, None, "{isin}: contract expiry leaked into bond_maturity");
        assert_eq!(fut.bond_next_coupon, None, "{isin}: contract expiry leaked into bond_next_coupon");
    }
}

const HV_FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/caceis_histovl.csv");
const HV_MULTI: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/caceis_histovl_multiclass.csv");
const HV_FNAME: &str = "HISTOVLLUX_165878_20260729_20260730170850.csv";

#[test]
fn histovl_yields_one_nav_point() {
    let bytes = std::fs::read(HV_FIXTURE).unwrap();
    let b = caceis::parse_histovl(HV_FNAME, &bytes).unwrap();
    assert_eq!(b.primary_date, chrono::NaiveDate::from_ymd_opt(2026, 7, 29).unwrap());
    assert_eq!(b.nav_points.len(), 1);
    let n = &b.nav_points[0];
    assert_eq!(n.nav, 104.04);
    assert_eq!(n.aum, 28224487.14);
    assert_eq!(n.shares, 271295.542);
    assert!(b.snapshots.is_empty() && b.dividends.is_none() && b.operations.is_none());
}

#[test]
fn histovl_rejects_multiple_share_classes() {
    let bytes = std::fs::read(HV_MULTI).unwrap();
    let err = caceis::parse_histovl(HV_FNAME, &bytes);
    match err {
        Err(ingest::ParseFailure::Workbook(m)) => assert!(m.contains("share class"), "{m}"),
        other => panic!("expected multi-share-class rejection, got {other:?}"),
    }
}

#[test]
fn detect_routes_recognizes_and_rejects() {
    use ingest::adapter::{detect, DetectError, FileKind};
    let hisinv = std::fs::read(FIXTURE).unwrap();
    let id = detect(FNAME, &hisinv).unwrap();
    assert_eq!(id.kind, FileKind::CaceisHisinv);
    assert_eq!(id.fund_code, Some(("caceis".to_string(), "165878".to_string())));

    let histovl = std::fs::read(HV_FIXTURE).unwrap();
    let id2 = detect(HV_FNAME, &histovl).unwrap();
    assert_eq!(id2.kind, FileKind::CaceisHistovl);

    // xlsx magic bytes -> NAV Recap, no fund code.
    let id3 = detect("07-08-2026 - Borobudur - NAV Recap.xlsx", b"PK\x03\x04rest").unwrap();
    assert_eq!(id3.kind, FileKind::NavRecap);
    assert_eq!(id3.fund_code, None);

    // Recognized-but-rejected families say why.
    match detect("INVXDVLUX_165878_20260804_20260805132350.csv", b"x") {
        Err(DetectError::Rejected(m)) => assert!(m.contains("HISINVLUX"), "{m}"),
        other => panic!("{other:?}"),
    }
    match detect("JOUROPLUX_165878_20260804_20260805132350.csv", b"x") {
        Err(DetectError::Rejected(m)) => assert!(m.to_lowercase().contains("sample"), "{m}"),
        other => panic!("{other:?}"),
    }
    // Garbage -> Unrecognized.
    assert!(matches!(detect("notes.txt", b"hello"), Err(DetectError::Unrecognized(_))));
    // A renamed random CSV must not slip through the content sniff.
    assert!(matches!(
        detect("HISINVLUX_1_20260101_1.csv", b"just,a,comma,file\n"),
        Err(DetectError::Unrecognized(_))
    ));
}

const JOURSR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/caceis_joursr.csv");
const JOURSR_FNAME: &str = "JOURSRLUX_165878_20260807_20260810130151.csv";

#[test]
fn joursr_reads_both_share_classes() {
    let bytes = std::fs::read(JOURSR).unwrap();
    let b = caceis::parse_joursr(JOURSR_FNAME, &bytes).expect("fixture parses");
    assert_eq!(b.primary_date, chrono::NaiveDate::from_ymd_opt(2026, 8, 7).unwrap());
    assert!(b.snapshots.is_empty(), "a flow file carries no positions");
    assert!(b.nav_points.is_empty(), "NAV history stays HISTOVLLUX's job");
    let flows = b.flows.as_ref().expect("flow journal present");
    assert_eq!(flows.len(), 2);
    let c1 = flows.iter().find(|f| f.share_class == "C1").unwrap();
    assert_eq!(c1.outstanding_shares, Some(271_295.542));
    assert_eq!(c1.nav_per_share, Some(104.04));
    assert_eq!(c1.subscription_amount, 0.0);
    assert_eq!(c1.redemption_amount, 200_000.0);
}

#[test]
fn joursr_stores_both_amounts_as_magnitudes() {
    // The depositary's sign convention for the redemption column is not
    // observable without a real file, so direction comes from which column
    // the amount sat in and never from its sign. The same file with the
    // redemption written negative must parse to the same magnitude.
    let text = String::from_utf8(std::fs::read(JOURSR).unwrap()).unwrap();
    let flipped = text.replace(";1922.15;200000.;", ";1922.15;-200000.;");
    assert_ne!(flipped, text, "the fixture's redemption amount must be present to flip");
    let b = caceis::parse_joursr(JOURSR_FNAME, flipped.as_bytes()).unwrap();
    let c1 = b.flows.unwrap().into_iter().find(|f| f.share_class == "C1").unwrap();
    assert_eq!(c1.redemption_amount, 200_000.0);
}

#[test]
fn joursr_rejects_a_mis_shaped_or_mislabelled_file() {
    let short = b"165878;20260807;C1\n";
    assert!(caceis::parse_joursr(JOURSR_FNAME, short).is_err());

    let bytes = std::fs::read(JOURSR).unwrap();
    // Filename fund code disagreeing with the rows is a routing accident, not
    // a row-level anomaly: reject the file rather than import it elsewhere.
    assert!(caceis::parse_joursr("JOURSRLUX_999999_20260807_1.csv", &bytes).is_err());
    assert!(caceis::parse_joursr("JOURSRLUX_165878_20260806_1.csv", &bytes).is_err());
}

#[test]
fn joursr_rejects_a_non_numeric_subscription_amount() {
    // A shifted amount column changes neither the row's field count, its
    // fund code, nor its date, so the only way to catch it is refusing to
    // treat an unparsable-but-present cell as a silent zero.
    let text = String::from_utf8(std::fs::read(JOURSR).unwrap()).unwrap();
    let corrupted = text.replace("3363.325;350000.;0.;0.", "3363.325;EUR;0.;0.");
    assert_ne!(corrupted, text, "the fixture's C2 subscription amount must be present to corrupt");
    assert!(caceis::parse_joursr(JOURSR_FNAME, corrupted.as_bytes()).is_err());
}

#[test]
fn joursr_rejects_a_non_numeric_redemption_amount() {
    let text = String::from_utf8(std::fs::read(JOURSR).unwrap()).unwrap();
    let corrupted = text.replace(";1922.15;200000.;269373.392;", ";1922.15;EUR;269373.392;");
    assert_ne!(corrupted, text, "the fixture's C1 redemption amount must be present to corrupt");
    assert!(caceis::parse_joursr(JOURSR_FNAME, corrupted.as_bytes()).is_err());
}

const INVJCP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/caceis_invjcp.csv");
const INVJCP_FNAME: &str = "INVJCPLUX_165878_20260807_20260810130151.csv";

#[test]
fn invjcp_supplies_the_coupon_frequency() {
    let bytes = std::fs::read(INVJCP).unwrap();
    let b = caceis::parse_invjcp(INVJCP_FNAME, &bytes).expect("fixture parses");
    assert!(b.snapshots.is_empty() && b.flows.is_none());
    let f = b.ref_facts.iter().find(|f| f.isin == "US105756CL22").unwrap();
    assert_eq!(f.bond_coupon_freq, Some(2));
    // bond_maturity and bond_coupon_pct are deliberately left unset by
    // parse_invjcp, even though the fixture carries real-looking values in
    // those columns (2035-03-15, 6.625): those two column indices are
    // inferred from the depositary's glossary with no real sample to
    // confirm them, and RefFact upserts overwrite, so a wrong guess would
    // silently flip-flop against HISINVLUX's sample-verified values on every
    // import. The frequency is the one field INVJCPLUX uniquely contributes.
    assert_eq!(f.bond_maturity, None);
    assert_eq!(f.bond_coupon_pct, None);
}

#[test]
fn a_month_count_frequency_token_is_not_accepted() {
    // The database CHECK constraint only allows {1, 2, 4, 12}. A French
    // depositary may plausibly encode frequency as a month count (e.g. "6"
    // for semi-annual), which is a real integer but not a member of that
    // set. It must take the warn-and-NULL path — exactly like an
    // unrecognised letter code — rather than being accepted here and later
    // rejected by the CHECK constraint, which would fail the entire import
    // and discard every valid row in the file.
    let text = String::from_utf8(std::fs::read(INVJCP).unwrap()).unwrap();
    let substituted = text.replace(";2;20250915;20350315;", ";6;20250915;20350315;");
    assert_ne!(substituted, text, "the fixture's frequency token 2 must be present to substitute");
    let b = caceis::parse_invjcp(INVJCP_FNAME, substituted.as_bytes()).unwrap();
    let f = b.ref_facts.iter().find(|f| f.isin == "US105756CL22").unwrap();
    assert_eq!(f.bond_coupon_freq, None, "a month-count 6 must never resolve to a frequency");
    assert!(b.warnings.iter().any(|w| w.contains("\"6\"")),
            "the 6 token must be named in the warning: {:?}", b.warnings);
}

#[test]
fn an_unrecognised_frequency_warns_and_stays_null() {
    // Never a guess: the engine falls to the accrued-interest inference, and
    // if that is inconclusive it credits no coupon at all.
    let bytes = std::fs::read(INVJCP).unwrap();
    let b = caceis::parse_invjcp(INVJCP_FNAME, &bytes).unwrap();
    let f = b.ref_facts.iter().find(|f| f.isin == "XS9999999999").unwrap();
    assert_eq!(f.bond_coupon_freq, None);
    assert!(b.warnings.iter().any(|w| w.contains("frequency")),
            "the first real file settles the encoding; the warning is how we find out: {:?}", b.warnings);
}

#[test]
fn a_bare_s_frequency_token_is_not_accepted() {
    // "S" is ambiguous between *semestriel* (2) and *semaine* (weekly),
    // which differ by a factor of 26. The fixture's other unrecognised-token
    // test uses "SEMI", which would still fall through to the warning path
    // even if someone later "helpfully" mapped "S" to 2 — so this test must
    // exercise the bare token directly, or the exclusion has no coverage.
    let text = String::from_utf8(std::fs::read(INVJCP).unwrap()).unwrap();
    let substituted = text.replace(";SEMI;", ";S;");
    assert_ne!(substituted, text, "the fixture's SEMI token must be present to substitute");
    let b = caceis::parse_invjcp(INVJCP_FNAME, substituted.as_bytes()).unwrap();
    let f = b.ref_facts.iter().find(|f| f.isin == "XS9999999999").unwrap();
    assert_eq!(f.bond_coupon_freq, None, "a bare S must never resolve to a frequency");
    assert!(b.warnings.iter().any(|w| w.contains("\"S\"")),
            "the S token must be named in the warning: {:?}", b.warnings);
}

#[test]
fn invjcp_rejects_a_mis_shaped_or_mislabelled_file() {
    let short = b"165878;INVJCP-DETAIL;20260807\n";
    assert!(caceis::parse_invjcp(INVJCP_FNAME, short).is_err(), "too few fields must reject the file");

    let bytes = std::fs::read(INVJCP).unwrap();
    // Filename fund code disagreeing with the rows is a routing accident, not
    // a row-level anomaly: reject the file rather than import it elsewhere.
    assert!(caceis::parse_invjcp("INVJCPLUX_999999_20260807_1.csv", &bytes).is_err(),
            "fund-code mismatch must reject the file");
    assert!(caceis::parse_invjcp("INVJCPLUX_165878_20260806_1.csv", &bytes).is_err(),
            "date mismatch must reject the file");
}

#[test]
fn joursr_treats_an_empty_amount_cell_as_zero() {
    // An empty cell is a legitimate "no flow that day" — only a present but
    // unparsable cell should be fatal, not a blank one.
    let text = String::from_utf8(std::fs::read(JOURSR).unwrap()).unwrap();
    let blanked = text.replace("104.04;0.;0.;1922.15", "104.04;0.;;1922.15");
    assert_ne!(blanked, text, "the fixture's C1 subscription amount must be present to blank");
    let b = caceis::parse_joursr(JOURSR_FNAME, blanked.as_bytes()).expect("empty cell still parses");
    let c1 = b.flows.unwrap().into_iter().find(|f| f.share_class == "C1").unwrap();
    assert_eq!(c1.subscription_amount, 0.0, "the empty cell was accepted, not rejected");
    assert_eq!(c1.redemption_amount, 200_000.0, "the neighbouring amount column is untouched");
}
