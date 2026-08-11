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

// (`detect` and `parse` dispatchers arrive in Task 4 with the CACEIS side;
// this task only establishes the types and the NAV Recap conversion.)
