//! The universal ingest contract. Every source adapter produces a
//! `UniversalBatch`; the import pipeline consumes nothing else.

use crate::{DividendRow, NavHistoryRow, OperationRow, ParsedWorkbook, PositionRow};
use chrono::NaiveDate;

#[derive(Debug)]
pub struct Snapshot {
    pub nav_date: NaiveDate,
    pub positions: Vec<PositionRow>,
}

/// Optional reference enrichment a file happens to carry. Applied to the
/// shared `instrument_refs` only where the target column is NULL.
#[derive(Debug, Clone)]
pub struct RefHint {
    pub isin: String,
    pub country_of_risk: Option<String>,
    pub region: Option<String>,
    pub ticker: Option<String>,
}

#[derive(Debug)]
pub struct UniversalBatch {
    /// The file's own NAV date — keys the `imports` row.
    pub primary_date: NaiveDate,
    pub nav_points: Vec<NavHistoryRow>,
    pub snapshots: Vec<Snapshot>,
    /// `Some` = this file carries the authoritative dividend journal
    /// (replace-if-latest, the existing NAV Recap rule). `None` = the
    /// journal is untouched by this import.
    pub dividends: Option<Vec<DividendRow>>,
    pub operations: Option<Vec<OperationRow>>,
    pub ref_hints: Vec<RefHint>,
    /// Row-level anomalies that dropped rows without rejecting the file.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind { NavRecap, CaceisHisinv, CaceisHistovl }

#[derive(Debug)]
pub struct Identification {
    pub kind: FileKind,
    /// `(source, code)` for `portfolio_codes` routing, e.g.
    /// `("caceis", "165878")`. `None` = the file cannot identify its
    /// portfolio (NAV Recap) and lands in the selected one.
    pub fund_code: Option<(String, String)>,
}

#[derive(Debug, thiserror::Error)]
pub enum DetectError {
    #[error("unrecognized file format: {0:?}. Supported: NAV Recap (.xlsx), CACEIS HISINVLUX / HISTOVLLUX (.csv)")]
    Unrecognized(String),
    #[error("{0}")]
    Rejected(String),
}

/// NAV Recap → universal batch. The recap's own NAV row joins the history
/// (the upsert dedupes by date, matching the old import path exactly).
pub fn to_batch(wb: ParsedWorkbook) -> UniversalBatch {
    let mut nav_points = wb.nav_history;
    nav_points.push(NavHistoryRow { date: wb.nav_date, aum: wb.aum, shares: wb.shares, nav: wb.nav });
    UniversalBatch {
        primary_date: wb.nav_date,
        nav_points,
        snapshots: vec![Snapshot { nav_date: wb.nav_date, positions: wb.positions }],
        dividends: Some(wb.dividends),
        operations: Some(wb.operations),
        ref_hints: Vec::new(),
        warnings: Vec::new(),
    }
}

/// Route a file to its adapter. Content sniffs guard against renamed files:
/// a CACEIS CSV must actually parse its first line's column count and date.
pub fn detect(filename: &str, bytes: &[u8]) -> Result<Identification, DetectError> {
    let lower = filename.to_ascii_lowercase();
    let caceis_meta = || crate::caceis::filename_meta(filename)
        .map(|(code, _)| (crate::caceis::SOURCE.to_string(), code));

    if lower.starts_with("hisinvlux_") {
        let Some(fund_code) = caceis_meta() else {
            return Err(DetectError::Unrecognized(filename.to_string()));
        };
        if !sniff_semicolons(bytes, 66) { return Err(DetectError::Unrecognized(filename.to_string())); }
        return Ok(Identification { kind: FileKind::CaceisHisinv, fund_code: Some(fund_code) });
    }
    if lower.starts_with("histovllux_") {
        let Some(fund_code) = caceis_meta() else {
            return Err(DetectError::Unrecognized(filename.to_string()));
        };
        if !sniff_semicolons(bytes, 20) { return Err(DetectError::Unrecognized(filename.to_string())); }
        return Ok(Identification { kind: FileKind::CaceisHistovl, fund_code: Some(fund_code) });
    }
    if lower.starts_with("invxdvlux_") {
        return Err(DetectError::Rejected(
            "INVXDVLUX is not needed: HISINVLUX already carries the positions. Upload HISINVLUX and HISTOVLLUX.".into()));
    }
    if lower.starts_with("jouroplux_") {
        return Err(DetectError::Rejected(
            "JOUROPLUX recognized, but its parser is pending a sample file — request the feed from CACEIS and provide one sample so the parser can be written.".into()));
    }
    if lower.ends_with(".xlsx") && bytes.starts_with(b"PK\x03\x04") {
        return Ok(Identification { kind: FileKind::NavRecap, fund_code: None });
    }
    Err(DetectError::Unrecognized(filename.to_string()))
}

fn sniff_semicolons(bytes: &[u8], min_fields: usize) -> bool {
    let first_line: Vec<u8> = bytes.iter().copied().take_while(|&b| b != b'\n').collect();
    first_line.iter().filter(|&&b| b == b';').count() + 1 >= min_fields
}

pub fn parse(kind: FileKind, filename: &str, bytes: &[u8]) -> Result<UniversalBatch, crate::ParseFailure> {
    match kind {
        FileKind::NavRecap => crate::parse_workbook(bytes).map(to_batch),
        FileKind::CaceisHisinv => crate::caceis::parse_hisinv(filename, bytes),
        FileKind::CaceisHistovl => crate::caceis::parse_histovl(filename, bytes),
    }
}
