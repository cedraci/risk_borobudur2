//! Parser for the weekly cheapest-to-deliver companion file.
//!
//! One row per bond future. The header is matched by name so column order is
//! free; `nav_date` repeats on every row and all rows must agree.

use crate::{ParseFailure, RowError};
use calamine::{Data, Reader, Xlsx};
use chrono::NaiveDate;
use std::io::Cursor;

const SHEET: &str = "CTD";
const COLUMNS: [&str; 7] = [
    "nav_date",
    "ticker",
    "ctd_isin",
    "ctd_mod_duration",
    "ctd_clean_price",
    "ctd_accrued",
    "conversion_factor",
];

#[derive(Debug, Clone)]
pub struct CtdRow {
    pub nav_date: NaiveDate,
    pub ticker: String,
    pub ctd_isin: String,
    pub ctd_mod_duration: f64,
    pub ctd_clean_price: f64,
    pub ctd_accrued: f64,
    pub conversion_factor: f64,
}

/// Parse the companion file. `.xlsx` is read from its first worksheet;
/// anything else is treated as CSV.
pub fn parse_ctd_file(bytes: &[u8], filename: &str) -> Result<Vec<CtdRow>, ParseFailure> {
    let grid = if filename.to_ascii_lowercase().ends_with(".xlsx") {
        read_xlsx(bytes)?
    } else {
        read_csv(bytes)?
    };
    rows_from_grid(grid)
}

fn read_csv(bytes: &[u8]) -> Result<Vec<Vec<String>>, ParseFailure> {
    let mut rdr = csv::ReaderBuilder::new().has_headers(false).flexible(true).from_reader(bytes);
    let mut out = Vec::new();
    for rec in rdr.records() {
        let rec = rec.map_err(|e| ParseFailure::Workbook(format!("CSV error: {e}")))?;
        out.push(rec.iter().map(|s| s.trim().to_string()).collect());
    }
    Ok(out)
}

fn read_xlsx(bytes: &[u8]) -> Result<Vec<Vec<String>>, ParseFailure> {
    let mut wb: Xlsx<_> =
        Xlsx::new(Cursor::new(bytes.to_vec())).map_err(|e| ParseFailure::Workbook(e.to_string()))?;
    let name = wb
        .sheet_names()
        .iter()
        .find(|n| n.eq_ignore_ascii_case(SHEET))
        .cloned()
        .or_else(|| wb.sheet_names().first().cloned())
        .ok_or_else(|| ParseFailure::Workbook("workbook has no sheets".into()))?;
    let range = wb
        .worksheet_range(&name)
        .map_err(|e| ParseFailure::Workbook(format!("sheet {name}: {e}")))?;
    Ok(range
        .rows()
        .map(|r| {
            r.iter()
                .map(|c| match c {
                    Data::Empty => String::new(),
                    Data::DateTime(dt) => dt
                        .as_datetime()
                        .map(|d| d.date().to_string())
                        .unwrap_or_default(),
                    other => other.to_string().trim().to_string(),
                })
                .collect()
        })
        .collect())
}

fn rows_from_grid(grid: Vec<Vec<String>>) -> Result<Vec<CtdRow>, ParseFailure> {
    let header = grid
        .first()
        .ok_or_else(|| ParseFailure::Workbook("file is empty".into()))?;
    let norm: Vec<String> = header.iter().map(|h| h.trim().to_ascii_lowercase()).collect();

    let mut idx = [0usize; 7];
    for (i, want) in COLUMNS.iter().enumerate() {
        idx[i] = norm
            .iter()
            .position(|h| h == want)
            .ok_or_else(|| ParseFailure::Workbook(format!("missing required column '{want}'")))?;
    }

    let body: Vec<&Vec<String>> = grid
        .iter()
        .skip(1)
        .filter(|r| r.iter().any(|c| !c.trim().is_empty()))
        .collect();
    if body.is_empty() {
        return Err(ParseFailure::Workbook("file has no data rows".into()));
    }

    let mut errors: Vec<RowError> = Vec::new();
    let mut out: Vec<CtdRow> = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    for (n, r) in body.iter().enumerate() {
        let row0 = (n + 1) as u32; // 0-based for RowError, which adds 1; header is row 1
        let cell = |i: usize| r.get(idx[i]).map(|s| s.trim()).unwrap_or("");

        let mut err = |msg: String| errors.push(RowError { sheet: "CTD".into(), row: row0 + 1, message: msg });

        let date = match NaiveDate::parse_from_str(cell(0), "%Y-%m-%d") {
            Ok(d) => Some(d),
            Err(_) => {
                err(format!("nav_date: expected YYYY-MM-DD, got {:?}", cell(0)));
                None
            }
        };
        let ticker = cell(1).to_string();
        if ticker.is_empty() {
            err("ticker: must not be blank".into());
        }
        if !ticker.is_empty() {
            if seen.contains(&ticker) {
                err(format!("duplicate ticker {ticker}"));
            } else {
                seen.push(ticker.clone());
            }
        }
        let isin = cell(2).to_string();
        if isin.is_empty() {
            err("ctd_isin: must not be blank".into());
        }

        let mut num = |i: usize, name: &str, allow_zero: bool| -> Option<f64> {
            match cell(i).replace(',', ".").parse::<f64>() {
                Ok(v) if v.is_finite() && (v > 0.0 || (allow_zero && v == 0.0)) => Some(v),
                Ok(_) => {
                    errors.push(RowError {
                        sheet: "CTD".into(),
                        row: row0 + 1,
                        message: format!("{name}: must be {}", if allow_zero { "zero or positive" } else { "positive" }),
                    });
                    None
                }
                Err(_) => {
                    errors.push(RowError {
                        sheet: "CTD".into(),
                        row: row0 + 1,
                        message: format!("{name}: expected a number, got {:?}", cell(i)),
                    });
                    None
                }
            }
        };
        let dur = num(3, "ctd_mod_duration", false);
        let clean = num(4, "ctd_clean_price", false);
        let accrued = num(5, "ctd_accrued", true);
        let cf = num(6, "conversion_factor", false);

        if let (Some(nav_date), false, false, Some(d), Some(c), Some(a), Some(f)) =
            (date, ticker.is_empty(), isin.is_empty(), dur, clean, accrued, cf)
        {
            out.push(CtdRow {
                nav_date,
                ticker,
                ctd_isin: isin,
                ctd_mod_duration: d,
                ctd_clean_price: c,
                ctd_accrued: a,
                conversion_factor: f,
            });
        }
    }

    if !errors.is_empty() {
        return Err(ParseFailure::Rows(errors));
    }
    let first = out[0].nav_date;
    if out.iter().any(|r| r.nav_date != first) {
        return Err(ParseFailure::Workbook("rows disagree on nav_date".into()));
    }
    Ok(out)
}
