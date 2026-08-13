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
pub struct RequestItem {
    pub isin: String,
    /// Bloomberg market sector ("yellow key") joined to the ISIN in every
    /// BDP formula: "{ISIN} Equity", "{ISIN} Corp", ...
    pub market_sector: String,
}

/// The Bloomberg market sector that resolves an ISIN of the given asset
/// class (as named by `analytics::asset_class_of`). Bloomberg has no Fund
/// sector — funds resolve under the Equity yellow key; bonds under Corp.
/// The sector is written as plain text next to each row, so a wrong guess
/// (e.g. a sovereign needing Govt) is a one-cell edit in Excel, not a
/// broken formula.
pub fn market_sector_for(asset_class: &str) -> &'static str {
    match asset_class {
        "Bonds" => "Corp",
        _ => "Equity",
    }
}

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
    for (c, h) in ["isin", "ticker", "country_of_risk", "gics_sector", "gics_industry", "market_sector"].iter().enumerate() {
        s.write_string_with_format(0, c as u16, *h, &bold)?;
    }
    s.set_column_width(0, 16)?;
    s.set_column_width(1, 24)?;
    s.set_column_width(5, 14)?;
    // The NAV Recap has no Bloomberg ticker column, so every BDP keys off
    // the ISIN in column A joined with the row's own market sector in
    // column F ("FR0000121014 Equity", "US105756CL22 Corp"). A hardcoded
    // Equity suffix only resolved equities and funds; keeping the sector in
    // a plain cell lets the user correct a row in Excel (e.g. Corp -> Govt
    // for a sovereign) without touching the formulas. Column B pulls the
    // ticker itself, stored on upload for later use.
    for (i, it) in items.iter().enumerate() {
        let r = (i + 1) as u32;
        s.write_string(r, 0, &it.isin)?;
        s.write_string(r, 5, &it.market_sector)?;
        let row = r + 1; // 1-based for the formula text
        let key = format!("A{row}&\" \"&F{row}");
        s.write_formula(r, 1, Formula::new(format!("=BDP({key},\"PARSEKYABLE_DES\")")))?;
        s.write_formula(r, 2, Formula::new(format!("=BDP({key},\"CNTRY_OF_RISK\")")))?;
        s.write_formula(r, 3, Formula::new(format!("=BDP({key},\"GICS_SECTOR_NAME\")")))?;
        s.write_formula(r, 4, Formula::new(format!("=BDP({key},\"GICS_INDUSTRY_GROUP_NAME\")")))?;
    }

    // ---- FX ----
    // One BDH per currency, each owning a two-column block: the formula spills
    // dates into its anchor column and values into the next. Blocks are two
    // columns apart so no spill lands inside its neighbour.
    //
    // Dates are inlined as YYYYMMDD rather than referenced from cells, and
    // Dts=S / Sort=A are passed explicitly: the add-in's default spill shape
    // varies with user settings, and this parser depends on it.
    let f = wb.add_worksheet();
    f.set_name("FX")?;
    for (i, ccy) in currencies.iter().enumerate() {
        let a = (i * 2) as u16;
        f.write_string_with_format(0, a, ccy, &bold)?;
        f.write_string_with_format(0, a + 1, "rate", &bold)?;
        f.write_formula(1, a, Formula::new(format!(
            "=BDH(\"EUR{ccy} Curncy\",\"PX_LAST\",\"{}\",\"{}\",\"Dts=S\",\"Sort=A\")",
            from.format("%Y%m%d"), to.format("%Y%m%d")
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
        "      Every column queries Bloomberg by \"{ISIN} {market sector}\", with the market sector".into(),
        "      (Equity for equities and funds, Corp for bonds) written per row in column F.".into(),
        "      If a bond does not resolve, edit its column F cell (e.g. Corp -> Govt) and let the".into(),
        "      formulas recalculate. The resolved ticker is stored on upload.".into(),
        "FX:   daily EUR cross rates. The tool inverts these to euros-per-unit and".into(),
        "      cross-checks them against the NAV Recap's own Change column.".into(),
    ];
    for (i, l) in lines.iter().enumerate() {
        r.write_string(i as u32, 0, l)?;
    }

    Ok(wb.save_to_buffer()?)
}

/// The ADV request workbook. Separate from `build_request` on purpose:
/// country and GICS are one-and-done, so that sheet shrinks toward empty,
/// while ADV decays daily and never drops out. Bundling them would turn every
/// classification export into a fleet-wide volume request.
///
/// One `BDP` cell per instrument — a point value, not a `BDH` history series.
/// That is the smallest possible footprint per instrument, and it makes a
/// typical daily refresh a handful of formulas rather than a sweep.
pub fn build_adv_request(items: &[RequestItem], asof: NaiveDate) -> anyhow::Result<Vec<u8>> {
    let mut wb = Workbook::new();
    let bold = Format::new().set_bold();
    let s = wb.add_worksheet();
    s.set_name("ADV")?;
    for (c, h) in ["isin", "adv_30d", "market_sector"].iter().enumerate() {
        s.write_string_with_format(0, c as u16, *h, &bold)?;
    }
    s.set_column_width(0, 16)?;
    s.set_column_width(2, 14)?;
    for (i, it) in items.iter().enumerate() {
        let r = (i + 1) as u32;
        s.write_string(r, 0, &it.isin)?;
        s.write_string(r, 2, &it.market_sector)?;
        let row = r + 1;
        s.write_formula(r, 1, Formula::new(
            format!("=BDP(A{row}&\" \"&C{row},\"VOLUME_AVG_30D\")")))?;
    }

    let r = wb.add_worksheet();
    r.set_name("README")?;
    r.set_column_width(0, 100)?;
    for (i, l) in [
        "Borobudur Risk - Bloomberg 30-day average volume request".to_string(),
        format!("Exported {asof}. {} instrument(s).", items.len()),
        String::new(),
        "1. Open in Excel on a machine with a logged-in Bloomberg Terminal.".into(),
        "2. Wait for every formula to resolve. #N/A cells are reported on upload and not stored.".into(),
        "3. Save as .xlsx and upload on the Data page, Bloomberg panel.".into(),
        String::new(),
        "Volumes are stored with the upload date as their as-of. A volume older than the".into(),
        "configured maximum age is treated as stale: the position falls back to its assumed".into(),
        "days figure and is flagged, and nothing in the tool ever refreshes it on your behalf.".into(),
    ].iter().enumerate() {
        r.write_string(i as u32, 0, l)?;
    }
    Ok(wb.save_to_buffer()?)
}

#[derive(Debug, Clone)]
pub struct ClassificationRow {
    pub isin: String,
    pub ticker: Option<String>,
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

#[derive(Debug, Clone)]
pub struct AdvRow {
    pub isin: String,
    pub adv_30d: f64,
}

#[derive(Debug, Default)]
pub struct ParsedResponse {
    pub classifications: Vec<ClassificationRow>,
    pub fx: Vec<FxObservation>,
    pub adv: Vec<AdvRow>,
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

    let refs_sheet = wb.worksheet_range("REFS");
    let fx_sheet = wb.worksheet_range("FX");
    let adv_sheet = wb.worksheet_range("ADV");
    if refs_sheet.is_err() && fx_sheet.is_err() && adv_sheet.is_err() {
        return Err(ParseFailure::Workbook(
            "workbook has neither a REFS nor an FX sheet; not a Bloomberg response file".into(),
        ));
    }

    if let Ok(refs) = refs_sheet {
        let end = refs.end().map(|(r, _)| r).unwrap_or(0);
        for row in 1..=end {
            let Some(isin) = text(&refs, row, 0) else { continue };
            let ticker = text(&refs, row, 1);
            let country = text(&refs, row, 2);
            let sector = text(&refs, row, 3);
            let industry = text(&refs, row, 4);
            for (col, name) in [(1u32, "ticker"), (2, "country_of_risk"), (3, "gics_sector"), (4, "gics_industry")] {
                if unresolved(refs.get_value((row, col))) {
                    out.skipped.push(RowError {
                        sheet: "REFS".into(),
                        row: row + 1,
                        message: format!("{isin}: {name} did not resolve; not stored"),
                    });
                }
            }
            if ticker.is_some() || country.is_some() || sector.is_some() || industry.is_some() {
                out.classifications.push(ClassificationRow { isin, ticker, country, sector, industry });
            }
        }
    }

    if let Ok(fx) = fx_sheet {
        let end = fx.end().map(|(r, _)| r).unwrap_or(0);
        let width = fx.end().map(|(_, c)| c).unwrap_or(0);
        // Each currency owns a two-column block: the anchor column (even
        // offset) carries the BDH-spilled date, the next column its rate.
        let currencies: Vec<(u32, String)> = (0..=width)
            .step_by(2)
            .filter_map(|c| text(&fx, 0, c).map(|n| (c, n)))
            .collect();

        for (a, ccy) in &currencies {
            let rate_col = a + 1;
            for row in 1..=end {
                let Some(dtxt) = text(&fx, row, *a) else { break }; // series ended
                let Some(date) = parse_any_date(&dtxt) else {
                    out.skipped.push(RowError {
                        sheet: "FX".into(), row: row + 1,
                        message: format!("{ccy}: date expected YYYY-MM-DD, got {dtxt:?}"),
                    });
                    continue;
                };
                let Some(v) = fx.get_value((row, rate_col)) else { continue };
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

    if let Ok(adv) = adv_sheet {
        let end = adv.end().map(|(r, _)| r).unwrap_or(0);
        for row in 1..=end {
            let Some(isin) = text(&adv, row, 0) else { continue };
            if unresolved(adv.get_value((row, 1))) {
                out.skipped.push(RowError {
                    sheet: "ADV".into(),
                    row: row + 1,
                    message: format!("{isin}: adv_30d did not resolve; not stored"),
                });
                continue;
            }
            let raw = match adv.get_value((row, 1)) {
                Some(Data::Float(f)) => *f,
                Some(Data::Int(i)) => *i as f64,
                _ => continue,
            };
            out.adv.push(AdvRow { isin, adv_30d: raw });
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
