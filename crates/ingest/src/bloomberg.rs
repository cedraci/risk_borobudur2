//! The Bloomberg round-trip workbook.
//!
//! The Excel add-in resolves `BDP`/`BDH` only inside Excel on a machine with a
//! logged-in Terminal, so a server process cannot query it. The tool therefore
//! writes a workbook of formulas, the user opens and saves it, and uploads it
//! back. Same shape as the weekly CTD companion file.

use crate::{ParseFailure, RowError};
use calamine::{Data, Range, Reader, Xlsx};
use chrono::NaiveDate;
use rust_xlsxwriter::{Format, Formula, Workbook};
use std::io::Cursor;

#[derive(Debug, Clone)]
pub struct RequestItem { pub isin: String, pub ticker: String }

/// Build the request workbook. `items` are instruments still missing a
/// classification; `currencies` are the non-EUR currencies held.
pub fn build_request(
    items: &[RequestItem],
    currencies: &[String],
    from: NaiveDate,
    to: NaiveDate,
) -> anyhow::Result<Vec<u8>> {
    let mut wb = Workbook::new();
    let bold = Format::new().set_bold();

    // ---- REFS ----
    let s = wb.add_worksheet();
    s.set_name("REFS")?;
    for (c, h) in ["isin", "ticker", "country_of_risk", "gics_sector", "gics_industry"].iter().enumerate() {
        s.write_string_with_format(0, c as u16, *h, &bold)?;
    }
    s.set_column_width(0, 16)?;
    s.set_column_width(1, 24)?;
    for (i, it) in items.iter().enumerate() {
        let r = (i + 1) as u32;
        s.write_string(r, 0, &it.isin)?;
        s.write_string(r, 1, &it.ticker)?;
        let row = r + 1; // 1-based for the formula text
        s.write_formula(r, 2, Formula::new(format!("=BDP(B{row},\"CNTRY_OF_RISK\")")))?;
        s.write_formula(r, 3, Formula::new(format!("=BDP(B{row},\"GICS_SECTOR_NAME\")")))?;
        s.write_formula(r, 4, Formula::new(format!("=BDP(B{row},\"GICS_INDUSTRY_GROUP_NAME\")")))?;
    }

    // ---- FX ----
    let f = wb.add_worksheet();
    f.set_name("FX")?;
    f.write_string_with_format(0, 0, "start", &bold)?;
    f.write_string(1, 0, from.to_string())?;
    f.write_string_with_format(2, 0, "end", &bold)?;
    // Dates are written as text and read back as text: Excel locale settings
    // otherwise reinterpret them, and BDH accepts the ISO form.
    f.write_string(3, 0, to.to_string())?;
    for (i, ccy) in currencies.iter().enumerate() {
        let c = (i + 1) as u16;
        f.write_string_with_format(0, c, ccy, &bold)?;
        f.write_formula(1, c, Formula::new(format!(
            "=BDH(\"EUR{ccy} Curncy\",\"PX_LAST\",$A$2,$A$4)"
        )))?;
    }

    // ---- README ----
    let r = wb.add_worksheet();
    r.set_name("README")?;
    r.set_column_width(0, 100)?;
    let lines = [
        "Borobudur Risk - Bloomberg classification request".to_string(),
        format!("Exported {from} to {to}."),
        String::new(),
        "1. Open this file in Excel on a machine with a logged-in Bloomberg Terminal.".into(),
        "2. Wait for every formula to resolve. #N/A cells are reported on upload and not stored.".into(),
        "3. Save the file (keep .xlsx format).".into(),
        "4. Upload it on the Data page, Bloomberg classification panel.".into(),
        String::new(),
        "REFS: one row per instrument still missing a country or GICS classification.".into(),
        "FX:   daily EUR cross rates. The tool inverts these to euros-per-unit and".into(),
        "      cross-checks them against the NAV Recap's own Change column.".into(),
    ];
    for (i, l) in lines.iter().enumerate() {
        r.write_string(i as u32, 0, l)?;
    }

    Ok(wb.save_to_buffer()?)
}

#[derive(Debug, Clone)]
pub struct ClassificationRow {
    pub isin: String,
    pub country: Option<String>,
    pub sector: Option<String>,
    pub industry: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FxObservation {
    pub date: NaiveDate,
    pub currency: String,
    pub rate_to_eur: f64,
}

#[derive(Debug, Default)]
pub struct ParsedResponse {
    pub classifications: Vec<ClassificationRow>,
    pub fx: Vec<FxObservation>,
    /// Cells that did not resolve, reported so the user can fix and re-upload.
    pub skipped: Vec<RowError>,
}

/// True for a cell Bloomberg did not resolve.
fn unresolved(d: Option<&Data>) -> bool {
    match d {
        None | Some(Data::Empty) => true,
        Some(Data::Error(_)) => true,
        Some(Data::String(s)) => {
            let t = s.trim();
            t.is_empty() || t.starts_with("#N/A") || t == "#VALUE!" || t == "#NAME?"
        }
        _ => false,
    }
}

fn text(r: &Range<Data>, row: u32, col: u32) -> Option<String> {
    let v = r.get_value((row, col));
    if unresolved(v) { return None; }
    v.map(|d| d.to_string().trim().to_string()).filter(|s| !s.is_empty())
}

/// Parse the workbook the user saved out of Excel. Values only — a file still
/// holding formulas has not been resolved and its cells read as unresolved.
pub fn parse_response(bytes: &[u8]) -> Result<ParsedResponse, ParseFailure> {
    let mut wb: Xlsx<_> = Xlsx::new(Cursor::new(bytes.to_vec()))
        .map_err(|e| ParseFailure::Workbook(e.to_string()))?;
    let mut out = ParsedResponse::default();

    if let Ok(refs) = wb.worksheet_range("REFS") {
        let end = refs.end().map(|(r, _)| r).unwrap_or(0);
        for row in 1..=end {
            let Some(isin) = text(&refs, row, 0) else { continue };
            let country = text(&refs, row, 2);
            let sector = text(&refs, row, 3);
            let industry = text(&refs, row, 4);
            for (col, name) in [(2u32, "country_of_risk"), (3, "gics_sector"), (4, "gics_industry")] {
                if unresolved(refs.get_value((row, col))) {
                    out.skipped.push(RowError {
                        sheet: "REFS".into(),
                        row: row + 1,
                        message: format!("{isin}: {name} did not resolve; not stored"),
                    });
                }
            }
            if country.is_some() || sector.is_some() || industry.is_some() {
                out.classifications.push(ClassificationRow { isin, country, sector, industry });
            }
        }
    }

    if let Ok(fx) = wb.worksheet_range("FX") {
        let end = fx.end().map(|(r, _)| r).unwrap_or(0);
        let width = fx.end().map(|(_, c)| c).unwrap_or(0);
        let currencies: Vec<(u32, String)> = (1..=width)
            .filter_map(|c| text(&fx, 0, c).map(|n| (c, n)))
            .collect();

        for row in 1..=end {
            let Some(dtxt) = text(&fx, row, 0) else { continue };
            let Some(date) = parse_any_date(&dtxt) else {
                out.skipped.push(RowError {
                    sheet: "FX".into(), row: row + 1,
                    message: format!("date: expected YYYY-MM-DD, got {dtxt:?}"),
                });
                continue;
            };
            for (col, ccy) in &currencies {
                let Some(v) = fx.get_value((row, *col)) else { continue };
                if unresolved(Some(v)) { continue; }
                let raw = match v {
                    Data::Float(f) => *f,
                    Data::Int(i) => *i as f64,
                    _ => continue,
                };
                if !(raw.is_finite() && raw > 0.0) {
                    out.skipped.push(RowError {
                        sheet: "FX".into(), row: row + 1,
                        message: format!("{ccy}: rate must be positive, got {raw}"),
                    });
                    continue;
                }
                // Bloomberg quotes EURXXX as units of XXX per EUR; the tool
                // needs EUR per unit, so invert.
                out.fx.push(FxObservation { date, currency: ccy.clone(), rate_to_eur: 1.0 / raw });
            }
        }
    }

    Ok(out)
}

fn parse_any_date(s: &str) -> Option<NaiveDate> {
    let t = s.trim();
    NaiveDate::parse_from_str(t, "%Y-%m-%d")
        .or_else(|_| NaiveDate::parse_from_str(t, "%d/%m/%Y"))
        .ok()
        .or_else(|| {
            // Excel serial left with its formatting stripped.
            let f: f64 = t.parse().ok()?;
            NaiveDate::from_ymd_opt(1899, 12, 30)?
                .checked_add_days(chrono::Days::new(f as u64))
        })
}

/// Region from country of risk. A fixed table, not fetched: it is reporting
/// policy, not market data. Unknown countries return None and group as
/// "Unclassified" rather than being forced into a wrong bucket.
pub fn region_for(country: &str) -> Option<&'static str> {
    let c = country.trim().to_ascii_uppercase();
    Some(match c.as_str() {
        "FRANCE" | "GERMANY" | "ITALY" | "SPAIN" | "NETHERLANDS" | "BELGIUM" | "AUSTRIA"
        | "PORTUGAL" | "IRELAND" | "LUXEMBOURG" | "FINLAND" | "GREECE" | "UNITED KINGDOM"
        | "SWITZERLAND" | "SWEDEN" | "NORWAY" | "DENMARK" | "POLAND" | "CZECH REPUBLIC" => "Europe",
        "UNITED STATES" | "CANADA" => "North America",
        "BRAZIL" | "MEXICO" | "CHILE" | "ARGENTINA" | "COLOMBIA" | "PERU" => "Latin America",
        "JAPAN" | "CHINA" | "HONG KONG" | "SOUTH KOREA" | "TAIWAN" | "SINGAPORE" | "INDIA"
        | "AUSTRALIA" | "NEW ZEALAND" | "INDONESIA" | "THAILAND" | "MALAYSIA" => "Asia Pacific",
        "SOUTH AFRICA" | "UNITED ARAB EMIRATES" | "SAUDI ARABIA" | "ISRAEL" | "TURKEY"
        | "QATAR" | "EGYPT" | "NIGERIA" | "MOROCCO" => "Middle East & Africa",
        _ => return None,
    })
}
