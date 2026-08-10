//! EMIR threshold-monitoring evidence workbook.
//!
//! The procedure requires the calculation details to be archived (SharePoint);
//! this file IS that artifact: the full month-by-month figures behind the
//! threshold verdicts, the contract inventory with OTC flags, and the manual
//! KPI history. Flat input rows, no dependency on analytics or db — the
//! server maps into them (same pattern as `bloomberg::RequestItem`).

use chrono::NaiveDate;
use rust_xlsxwriter::{Format, Workbook};

pub struct SummaryRow {
    pub label: String,
    pub threshold_eur: f64,
    pub avg_otc_eur: f64,
    pub pct_of_threshold: f64,
    pub verdict: String,
    pub avg_total_eur: f64,
}

pub struct MonthRow {
    pub label: String,
    pub month: String,
    /// `None` renders as "missing" — the month had no snapshot.
    pub snapshot_date: Option<String>,
    pub total_eur: Option<f64>,
    pub otc_eur: Option<f64>,
}

pub struct ContractRow {
    pub root: String,
    pub label: String,
    pub category: String,
    pub otc: bool,
    pub confirmed: bool,
    pub point_value: Option<f64>,
    pub currency: String,
}

pub struct KpiRow {
    pub month: String,
    pub unconfirmed_over_5d: i32,
    pub reconciliation: String,
    pub disputes: i32,
    pub note: String,
}

pub struct EmirEvidence {
    pub anchor: NaiveDate,
    pub months_present: usize,
    pub months_total: usize,
    pub summary: Vec<SummaryRow>,
    pub months: Vec<MonthRow>,
    pub contracts: Vec<ContractRow>,
    pub kpis: Vec<KpiRow>,
    pub warnings: Vec<String>,
}

pub fn build_evidence(e: &EmirEvidence) -> anyhow::Result<Vec<u8>> {
    let mut wb = Workbook::new();
    let bold = Format::new().set_bold();
    let pct_fmt = Format::new().set_num_format("0.0%");

    // ---- Seuils ----
    let s = wb.add_worksheet();
    s.set_name("Seuils")?;
    s.set_column_width(0, 34)?;
    for c in 1..=5u16 {
        s.set_column_width(c, 20)?;
    }
    s.write_string_with_format(0, 0, "EMIR clearing-threshold monitoring — Borobudur", &bold)?;
    s.write_string(1, 0, format!("Anchor date: {}", e.anchor))?;
    s.write_string(2, 0, format!("Months with a snapshot: {} of {}", e.months_present, e.months_total))?;
    s.write_string(3, 0, "Only OTC positions count toward the thresholds; gross notional, no netting. Average of month-end positions.")?;

    let mut row: u32 = 5;
    for (c, h) in ["Class", "Threshold EUR", "Avg OTC notional EUR", "% of threshold", "Verdict", "Avg total notional EUR"].iter().enumerate() {
        s.write_string_with_format(row, c as u16, *h, &bold)?;
    }
    row += 1;
    for r in &e.summary {
        s.write_string(row, 0, &r.label)?;
        s.write_number(row, 1, r.threshold_eur)?;
        s.write_number(row, 2, r.avg_otc_eur)?;
        s.write_number_with_format(row, 3, r.pct_of_threshold, &pct_fmt)?;
        s.write_string(row, 4, &r.verdict)?;
        s.write_number(row, 5, r.avg_total_eur)?;
        row += 1;
    }

    row += 1; // one blank row between the summary and detail tables
    for (c, h) in ["Class", "Month", "Snapshot date", "Total EUR", "OTC EUR"].iter().enumerate() {
        s.write_string_with_format(row, c as u16, *h, &bold)?;
    }
    row += 1;
    for r in &e.months {
        s.write_string(row, 0, &r.label)?;
        s.write_string(row, 1, &r.month)?;
        match &r.snapshot_date {
            Some(d) => s.write_string(row, 2, d)?,
            None => s.write_string(row, 2, "missing")?,
        };
        if let Some(v) = r.total_eur {
            s.write_number(row, 3, v)?;
        }
        if let Some(v) = r.otc_eur {
            s.write_number(row, 4, v)?;
        }
        row += 1;
    }

    row += 1;
    s.write_string_with_format(row, 0, "Warnings", &bold)?;
    row += 1;
    for w in &e.warnings {
        s.write_string(row, 0, w)?;
        row += 1;
    }

    // ---- Contrats ----
    let c = wb.add_worksheet();
    c.set_name("Contrats")?;
    c.set_column_width(0, 10)?;
    c.set_column_width(1, 24)?;
    for (col, h) in ["Root", "Label", "Category", "OTC", "Confirmed", "Point value", "Currency"].iter().enumerate() {
        c.write_string_with_format(0, col as u16, *h, &bold)?;
    }
    for (i, r) in e.contracts.iter().enumerate() {
        let row = (i + 1) as u32;
        c.write_string(row, 0, &r.root)?;
        c.write_string(row, 1, &r.label)?;
        c.write_string(row, 2, &r.category)?;
        c.write_string(row, 3, if r.otc { "true" } else { "false" })?;
        c.write_string(row, 4, if r.confirmed { "true" } else { "false" })?;
        if let Some(pv) = r.point_value {
            c.write_number(row, 5, pv)?;
        }
        c.write_string(row, 6, &r.currency)?;
    }

    // ---- KPI ----
    let k = wb.add_worksheet();
    k.set_name("KPI")?;
    k.set_column_width(4, 60)?;
    for (col, h) in ["Month", "Unconfirmed > 5 days", "Reconciliation", "Disputes", "Note"].iter().enumerate() {
        k.write_string_with_format(0, col as u16, *h, &bold)?;
    }
    for (i, r) in e.kpis.iter().enumerate() {
        let row = (i + 1) as u32;
        k.write_string(row, 0, &r.month)?;
        k.write_number(row, 1, f64::from(r.unconfirmed_over_5d))?;
        k.write_string(row, 2, &r.reconciliation)?;
        k.write_number(row, 3, f64::from(r.disputes))?;
        k.write_string(row, 4, &r.note)?;
    }

    Ok(wb.save_to_buffer()?)
}
