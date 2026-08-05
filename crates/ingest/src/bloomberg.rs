//! The Bloomberg round-trip workbook.
//!
//! The Excel add-in resolves `BDP`/`BDH` only inside Excel on a machine with a
//! logged-in Terminal, so a server process cannot query it. The tool therefore
//! writes a workbook of formulas, the user opens and saves it, and uploads it
//! back. Same shape as the weekly CTD companion file.

use chrono::NaiveDate;
use rust_xlsxwriter::{Format, Formula, Workbook};

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
