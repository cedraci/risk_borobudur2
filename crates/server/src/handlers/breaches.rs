//! The shared computation behind a recorded limit-check run.
//!
//! `compute` is what `server::recorder` records; `holdings_at` is the same
//! grouping applied to an earlier snapshot so a proposal can compare
//! quantities. Both call the one `subject_group` helper below — two copies of
//! the grouping rule drifting apart would silently change what a "subject"
//! means between the run and its proposals, breaking every active/passive
//! classification.
//!
//! Task 5 computes concentration only; Task 6 adds liquidity, VaR and EMIR to
//! `compute`.

use analytics::breach::{Finding, SubjectHolding};
use analytics::{concentration, default_issuer_group, CheckStatus, ConPosition};
use chrono::NaiveDate;
use db::auth::marker::{Positions, Reference, View};
use db::repo::CheckResultRow;
use db::scoped::Scoped;
use std::collections::HashMap;

/// One run's worth of computed checks, plus what the proposal step needs and
/// what the run row needs to say about its inputs.
pub struct ComputedRun {
    pub results: Vec<CheckResultRow>,
    /// One per breaching row.
    pub findings: Vec<Finding>,
    /// Subject (issuer group) -> its instruments at THIS date.
    pub holdings: HashMap<String, Vec<SubjectHolding>>,
    /// Subject -> its weight at THIS date.
    pub weights: HashMap<String, f64>,
    /// Inputs that were genuinely ABSENT — never a permission, since this
    /// runs under the system context. A check that could not be evaluated is
    /// omitted from `results` and named here; a check that could not run must
    /// never appear in the register as one that passed.
    pub input_notes: serde_json::Map<String, serde_json::Value>,
}

/// THE grouping rule: `issuer_group` override, falling back to
/// `default_issuer_group`, with `Fonds` never regrouped (fund_20 is per
/// target fund). The single definition of what a subject is.
fn subject_group(
    p: &db::repo::PositionRecord, by: &HashMap<&str, &db::repo::InstrumentRef>,
) -> String {
    let name = p.name.as_deref().unwrap_or_default();
    if p.asset_type == "Fonds" {
        default_issuer_group(&p.asset_type, name)
    } else {
        by.get(p.isin.as_str())
            .and_then(|r| r.issuer_group.clone())
            .unwrap_or_else(|| default_issuer_group(&p.asset_type, name))
    }
}

/// The `ConPosition` assembly lifted out of `limits::concentration_h`, which
/// now calls this too — the grouping applied to a check must be the grouping
/// recorded in the register.
pub(crate) fn con_positions(
    rows: &[db::repo::PositionRecord], by: &HashMap<&str, &db::repo::InstrumentRef>,
) -> Vec<ConPosition> {
    rows.iter().filter_map(|p| {
        let w = p.weight?;
        Some(ConPosition {
            asset_type: p.asset_type.clone(),
            group: subject_group(p, by),
            weight: w,
        })
    }).collect()
}

fn status_str(s: CheckStatus) -> &'static str {
    match s {
        CheckStatus::Ok => "ok",
        CheckStatus::Watch => "watch",
        CheckStatus::Breach => "breach",
    }
}

/// Groups one snapshot's rows by subject: every instrument (for quantity
/// comparison) and the summed weight where the snapshot reports one.
fn group_holdings(
    rows: &[db::repo::PositionRecord], by: &HashMap<&str, &db::repo::InstrumentRef>,
) -> (HashMap<String, Vec<SubjectHolding>>, HashMap<String, f64>) {
    let mut holdings: HashMap<String, Vec<SubjectHolding>> = HashMap::new();
    let mut weights: HashMap<String, f64> = HashMap::new();
    for p in rows {
        let subject = subject_group(p, by);
        holdings.entry(subject.clone()).or_default().push(SubjectHolding {
            isin: p.isin.clone(),
            quantity: p.quantity,
        });
        if let Some(w) = p.weight {
            *weights.entry(subject).or_default() += w;
        }
    }
    (holdings, weights)
}

async fn snapshot_grouped(
    scoped: &Scoped<'_>, pid: i64, date: NaiveDate,
) -> anyhow::Result<(Vec<db::repo::PositionRecord>, Vec<db::repo::InstrumentRef>)> {
    let a = scoped.authorize::<Positions, View>(pid)
        .map_err(|d| anyhow::anyhow!("system context refused: {d}"))?;
    let rows = scoped.positions_for(&a, date).await?;
    let rv = scoped.authorize_global::<Reference, View>()
        .map_err(|d| anyhow::anyhow!("system context refused: {d}"))?;
    let refs = scoped.refs_all(&rv).await?;
    Ok((rows, refs))
}

/// Computes the checks for one portfolio at one date, as rows to record.
/// Concentration only in this task; Task 6 adds the rest.
pub async fn compute(
    scoped: &Scoped<'_>, pid: i64, nav_date: NaiveDate,
) -> anyhow::Result<ComputedRun> {
    let (rows, refs) = snapshot_grouped(scoped, pid, nav_date).await?;
    let by = super::limits::ref_map(&refs);
    let cons = con_positions(&rows, &by);
    let checks = concentration(&cons);

    let mut results = Vec::with_capacity(checks.len());
    let mut findings = Vec::new();
    for c in &checks {
        // Rows come sorted descending by weight, but take the max explicitly:
        // the register must not depend on a presentation ordering.
        let observed = c.rows.iter().map(|r| r.weight)
            .max_by(|x, y| x.total_cmp(y));
        results.push(CheckResultRow {
            check_key: c.check.clone(),
            scope_label: c.scope_label.clone(),
            limit_value: Some(c.limit),
            observed_value: observed,
            status: status_str(c.status).to_string(),
            detail: serde_json::to_value(c)?,
        });
        for r in &c.rows {
            if r.status == CheckStatus::Breach {
                findings.push(Finding {
                    check_key: c.check.clone(),
                    subject: r.group.clone(),
                    value: Some(r.weight),
                });
            }
        }
    }

    let (holdings, weights) = group_holdings(&rows, &by);
    Ok(ComputedRun {
        results,
        findings,
        holdings,
        weights,
        input_notes: serde_json::Map::new(),
    })
}

/// Just the groupings for one date — no checks. Used for the previous
/// snapshot, where only the quantities and weights matter.
pub async fn holdings_at(
    scoped: &Scoped<'_>, pid: i64, date: NaiveDate,
) -> anyhow::Result<(
    HashMap<String, Vec<SubjectHolding>>,
    HashMap<String, f64>,
)> {
    let (rows, refs) = snapshot_grouped(scoped, pid, date).await?;
    let by = super::limits::ref_map(&refs);
    Ok(group_holdings(&rows, &by))
}
