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

/// Reads a string field, falling back to `""` — the handler passes JSON
/// built from possibly-`None` columns (e.g. `resolved_at` on a still-open
/// episode), which `serde_json::to_value` emits as a present key holding
/// `null`, not an absent key; `.as_str()` returns `None` for that `null`
/// either way, and a blank cell reads better in an evidence document than
/// the literal word "null".
fn s<'a>(v: &'a serde_json::Value, key: &str) -> &'a str {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("")
}

fn f(v: &serde_json::Value, key: &str) -> Option<f64> {
    v.get(key).and_then(|x| x.as_f64())
}

/// `input_notes` — the map naming each check that could NOT be evaluated and
/// why — flattened into one cell. Sorted by key so the same run always renders
/// the same string. An absent or non-object value renders blank, matching `s`.
///
/// Before M4 this never reached the workbook at all: a check skipped because
/// an input was missing produced a blank status cell, indistinguishable from a
/// check key that did not exist for that run. The whole point of `input_notes`
/// is that "a check that could not run must never appear as one that passed" —
/// and the artefact the auditor actually receives is this file.
fn notes(v: &serde_json::Value) -> String {
    let Some(map) = v.get("input_notes").and_then(|x| x.as_object()) else { return String::new() };
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    keys.into_iter()
        .map(|k| format!("{k}: {}", map[k].as_str().unwrap_or_default()))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Builds the evidence workbook: `Register` (one row per breach episode) and
/// `Run history` (one row per recorded run, one column per check key, the
/// status in the cell). `runs`/`breaches` are exactly the JSON the register's
/// read endpoints already serve — see `handlers::breaches::runs_list` and
/// `register_list`. The caller is responsible for `runs` being the FULL
/// history (`runs_all`, not the paged `runs_for`): this function has no way
/// to tell a genuinely short history from one truncated upstream, so it
/// writes whatever it is given as if it were complete.
pub fn build(
    portfolio_name: &str, runs: &[serde_json::Value], breaches: &[serde_json::Value],
) -> anyhow::Result<Vec<u8>> {
    let mut wb = Workbook::new();
    let bold = Format::new().set_bold();
    let generated = Utc::now().to_rfc3339();

    // ---- Register ----
    let reg = wb.add_worksheet();
    reg.set_name("Register")?;
    reg.set_column_width(0, 14)?; // Check
    reg.set_column_width(1, 24)?; // Subject
    reg.set_column_width(7, 22)?; // Acknowledged at
    reg.set_column_width(8, 20)?; // Acknowledged by
    reg.set_column_width(9, 34)?; // Acknowledgement note
    reg.set_column_width(10, 22)?; // Resolved at
    reg.set_column_width(11, 20)?; // Resolved by
    reg.set_column_width(12, 34)?; // Resolution note
    reg.set_column_width(14, 40)?; // Proposal reason
    reg.write_string_with_format(0, 0, format!("Limit breach register — {portfolio_name}"), &bold)?;
    reg.write_string(1, 0, format!("Generated: {generated}"))?;

    let mut row: u32 = 3;
    let headers = [
        "Check", "Subject", "Opened", "Peak value", "Cleared", "State", "Classification",
        "Acknowledged at", "Acknowledged by", "Acknowledgement note",
        "Resolved at", "Resolved by", "Resolution note",
        // M4: the machine's proposal and the remediation deadline the human's
        // decision was made against. Without them the workbook showed the
        // decision with nothing to judge it by.
        "Proposed", "Proposal reason", "Deadline",
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
        // A still-open episode's `closed_nav_date` is JSON `null` (see `s`'s
        // doc comment) — `.as_str()` returns `None` for that, the same as it
        // would for an absent key, so this falls through to "open" either way.
        let cleared = b.get("closed_nav_date").and_then(|x| x.as_str()).unwrap_or("open");
        reg.write_string(row, 4, cleared)?;
        reg.write_string(row, 5, s(b, "state"))?;
        reg.write_string(row, 6, s(b, "classification"))?;
        reg.write_string(row, 7, s(b, "acknowledged_at"))?;
        reg.write_string(row, 8, s(b, "acknowledged_by_label"))?;
        reg.write_string(row, 9, s(b, "acknowledgement_note"))?;
        reg.write_string(row, 10, s(b, "resolved_at"))?;
        reg.write_string(row, 11, s(b, "resolved_by_label"))?;
        reg.write_string(row, 12, s(b, "resolution_note"))?;
        reg.write_string(row, 13, s(b, "proposed_classification"))?;
        reg.write_string(row, 14, s(b, "proposal_reason"))?;
        reg.write_string(row, 15, s(b, "deadline_date"))?;
        row += 1;
    }

    // ---- Run history ----
    let rh = wb.add_worksheet();
    rh.set_name("Run history")?;
    rh.set_column_width(0, 14)?;
    rh.set_column_width(1, 22)?;
    rh.set_column_width(2, 14)?;
    rh.set_column_width(3, 15)?;
    rh.set_column_width(4, 52)?;
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
    // `Inputs complete` and `Input notes` sit with the run's own metadata,
    // before the per-check columns: an auditor reading a blank status cell has
    // to be able to tell "this check could not run, and here is why" from
    // "this check did not exist for this run" without leaving the row (M4).
    let fixed_headers = ["NAV date", "Run at", "Triggered by", "Inputs complete", "Input notes"];
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
        // Spelled out rather than TRUE/FALSE: this is a document a person
        // reads, and a bare FALSE beside a row of "ok" cells is exactly the
        // thing that gets skimmed past.
        rh.write_string(row, 3, match run.get("inputs_complete").and_then(|x| x.as_bool()) {
            Some(true) => "yes",
            Some(false) => "no",
            None => "",
        })?;
        rh.write_string(row, 4, notes(run))?;
        let mut status_by_check = HashMap::new();
        if let Some(results) = run.get("results").and_then(|x| x.as_array()) {
            for r in results {
                if let (Some(k), Some(status)) = (
                    r.get("check_key").and_then(|x| x.as_str()),
                    r.get("status").and_then(|x| x.as_str()),
                ) {
                    // Last-wins on a repeated `check_key` within one run.
                    // `limit_check_results` has a unique index on
                    // `(run_id, check_key)`, so `runs_for`/`runs_all` can
                    // never actually hand this a run with a duplicate key —
                    // documented here as the assumption, not left implicit.
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
