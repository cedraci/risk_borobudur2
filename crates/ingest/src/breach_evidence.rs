//! Limit-breach register evidence workbook.
//!
//! The artefact a fund hands a regulator or auditor: the episode-by-episode
//! register (who decided what, and when) plus the full run history (every
//! recorded check, every date). Flat input — `serde_json::Value`s built by
//! the handler from exactly the shapes `runs_list`/`register_list` already
//! serve — no dependency on `db` or `server`, same pattern as `emir_file.rs`.

use chrono::Utc;
use rust_xlsxwriter::{Format, Workbook};
use std::collections::{BTreeSet, HashMap};

/// Reads a string field, falling back to `""` rather than `"null"` — the
/// handler passes JSON built from possibly-`None` columns (e.g.
/// `closed_nav_date` on a still-open episode), and a blank cell reads better
/// in an evidence document than the literal word "null".
fn s<'a>(v: &'a serde_json::Value, key: &str) -> &'a str {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("")
}

fn f(v: &serde_json::Value, key: &str) -> Option<f64> {
    v.get(key).and_then(|x| x.as_f64())
}

/// Builds the evidence workbook: `Register` (one row per breach episode) and
/// `Run history` (one row per recorded run, one column per check key, the
/// status in the cell). `runs`/`breaches` are exactly the JSON the register's
/// read endpoints already serve — see `handlers::breaches::runs_list` and
/// `register_list`.
pub fn build(
    portfolio_name: &str, runs: &[serde_json::Value], breaches: &[serde_json::Value],
) -> anyhow::Result<Vec<u8>> {
    let mut wb = Workbook::new();
    let bold = Format::new().set_bold();
    let generated = Utc::now().to_rfc3339();

    // ---- Register ----
    let reg = wb.add_worksheet();
    reg.set_name("Register")?;
    reg.set_column_width(0, 14)?;
    reg.set_column_width(1, 24)?;
    reg.set_column_width(7, 22)?;
    reg.set_column_width(9, 22)?;
    reg.set_column_width(8, 34)?;
    reg.set_column_width(10, 34)?;
    reg.write_string_with_format(0, 0, format!("Limit breach register — {portfolio_name}"), &bold)?;
    reg.write_string(1, 0, format!("Generated: {generated}"))?;

    let mut row: u32 = 3;
    let headers = [
        "Check", "Subject", "Opened", "Peak value", "Cleared", "State", "Classification",
        "Acknowledged at", "Acknowledgement note", "Resolved at", "Resolution note",
    ];
    for (c, h) in headers.iter().enumerate() {
        reg.write_string_with_format(row, c as u16, *h, &bold)?;
    }
    row += 1;
    if breaches.is_empty() {
        reg.write_string(row, 0, "No breaches recorded.")?;
    }
    for b in breaches {
        reg.write_string(row, 0, s(b, "check_key"))?;
        reg.write_string(row, 1, s(b, "subject"))?;
        reg.write_string(row, 2, s(b, "opened_nav_date"))?;
        if let Some(v) = f(b, "peak_value") {
            reg.write_number(row, 3, v)?;
        }
        // A still-open episode has no `closed_nav_date` at all.
        let cleared = b.get("closed_nav_date").and_then(|x| x.as_str()).unwrap_or("open");
        reg.write_string(row, 4, cleared)?;
        reg.write_string(row, 5, s(b, "state"))?;
        reg.write_string(row, 6, s(b, "classification"))?;
        reg.write_string(row, 7, s(b, "acknowledged_at"))?;
        reg.write_string(row, 8, s(b, "acknowledgement_note"))?;
        reg.write_string(row, 9, s(b, "resolved_at"))?;
        reg.write_string(row, 10, s(b, "resolution_note"))?;
        row += 1;
    }

    // ---- Run history ----
    let rh = wb.add_worksheet();
    rh.set_name("Run history")?;
    rh.set_column_width(0, 14)?;
    rh.set_column_width(1, 22)?;
    rh.set_column_width(2, 14)?;
    rh.write_string_with_format(0, 0, format!("Run history — {portfolio_name}"), &bold)?;
    rh.write_string(1, 0, format!("Generated: {generated}"))?;

    // The union of check keys across every run, sorted for a stable column
    // order regardless of the order runs are supplied in (newest-first, per
    // `runs_list`, but a check absent from the newest run must still get its
    // own column).
    let mut keys: BTreeSet<&str> = BTreeSet::new();
    for run in runs {
        if let Some(results) = run.get("results").and_then(|x| x.as_array()) {
            for r in results {
                if let Some(k) = r.get("check_key").and_then(|x| x.as_str()) {
                    keys.insert(k);
                }
            }
        }
    }
    let keys: Vec<&str> = keys.into_iter().collect();

    let mut row: u32 = 3;
    let fixed_headers = ["NAV date", "Run at", "Triggered by"];
    for (c, h) in fixed_headers.iter().enumerate() {
        rh.write_string_with_format(row, c as u16, *h, &bold)?;
    }
    for (i, k) in keys.iter().enumerate() {
        rh.set_column_width(fixed_headers.len() as u16 + i as u16, 14)?;
        rh.write_string_with_format(row, (fixed_headers.len() + i) as u16, *k, &bold)?;
    }
    row += 1;
    if runs.is_empty() {
        rh.write_string(row, 0, "No runs recorded.")?;
    }
    for run in runs {
        rh.write_string(row, 0, s(run, "nav_date"))?;
        rh.write_string(row, 1, s(run, "run_at"))?;
        rh.write_string(row, 2, s(run, "triggered_by"))?;
        let mut status_by_check = HashMap::new();
        if let Some(results) = run.get("results").and_then(|x| x.as_array()) {
            for r in results {
                if let (Some(k), Some(status)) = (
                    r.get("check_key").and_then(|x| x.as_str()),
                    r.get("status").and_then(|x| x.as_str()),
                ) {
                    status_by_check.insert(k, status);
                }
            }
        }
        for (i, k) in keys.iter().enumerate() {
            if let Some(status) = status_by_check.get(k) {
                rh.write_string(row, (fixed_headers.len() + i) as u16, *status)?;
            }
        }
        row += 1;
    }

    Ok(wb.save_to_buffer()?)
}
