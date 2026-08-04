use serde::Serialize;
use std::collections::BTreeMap;

pub const WATCH_FRAC: f64 = 0.8;

#[derive(Debug, Clone)]
pub struct ConPosition {
    pub asset_type: String,
    /// Effective issuer group (override already applied by the caller).
    pub group: String,
    pub weight: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus { Ok, Watch, Breach }

#[derive(Debug, Clone, Serialize)]
pub struct CheckRow { pub group: String, pub weight: f64, pub status: CheckStatus }

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub check: String,
    pub scope_label: String,
    pub limit: f64,
    pub rows: Vec<CheckRow>,
    pub status: CheckStatus,
}

/// Default issuer group: normalized name; for cash/margin accounts the bank
/// code after the last "- " (e.g. "Depositary Bk- CBLU" -> "CBLU").
pub fn default_issuer_group(asset_type: &str, name: &str) -> String {
    let n = name.split_whitespace().collect::<Vec<_>>().join(" ").to_uppercase();
    match asset_type {
        "Cash Acc" | "Margin Acc" => n.rsplit_once("- ")
            .map(|(_, b)| b.trim().to_string())
            .filter(|b| !b.is_empty())
            .unwrap_or(n),
        _ => n,
    }
}

fn status_for(weight: f64, limit: f64) -> CheckStatus {
    if weight > limit { CheckStatus::Breach }
    else if weight >= WATCH_FRAC * limit { CheckStatus::Watch }
    else { CheckStatus::Ok }
}

fn severity(s: CheckStatus) -> u8 {
    match s { CheckStatus::Ok => 0, CheckStatus::Watch => 1, CheckStatus::Breach => 2 }
}

/// Sum weights per group; negatives offset within a group, floored at 0;
/// sorted descending by weight.
fn group_sums<'a>(rows: impl Iterator<Item = &'a ConPosition>) -> Vec<(String, f64)> {
    let mut m: BTreeMap<String, f64> = BTreeMap::new();
    for p in rows { *m.entry(p.group.clone()).or_default() += p.weight; }
    let mut v: Vec<(String, f64)> = m.into_iter().map(|(g, w)| (g, w.max(0.0))).collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1));
    v
}

/// The five v2 concentration checks: issuer_10, forty, group_20 on
/// transferable securities (+ dividend receivables), fund_20 per target
/// fund, deposit_20 per bank on net-positive cash+margin.
pub fn concentration(positions: &[ConPosition]) -> Vec<Check> {
    let sec_groups = group_sums(positions.iter()
        .filter(|p| matches!(p.asset_type.as_str(), "Action" | "Obligation" | "Dividendes")));
    let issuer_rows: Vec<CheckRow> = sec_groups.iter()
        .map(|(g, w)| CheckRow { group: g.clone(), weight: *w, status: status_for(*w, 0.10) })
        .collect();
    let over5: f64 = sec_groups.iter().filter(|(_, w)| *w > 0.05).map(|(_, w)| w).sum();
    let forty_rows = vec![CheckRow {
        group: "Sum of issuer exposures > 5%".into(), weight: over5, status: status_for(over5, 0.40),
    }];
    let group_rows: Vec<CheckRow> = sec_groups.iter()
        .map(|(g, w)| CheckRow { group: g.clone(), weight: *w, status: status_for(*w, 0.20) })
        .collect();
    let fund_rows: Vec<CheckRow> = group_sums(positions.iter().filter(|p| p.asset_type == "Fonds"))
        .iter()
        .map(|(g, w)| CheckRow { group: g.clone(), weight: *w, status: status_for(*w, 0.20) })
        .collect();
    let dep_rows: Vec<CheckRow> = group_sums(positions.iter()
        .filter(|p| matches!(p.asset_type.as_str(), "Cash Acc" | "Margin Acc")))
        .iter()
        .filter(|(_, w)| *w > 0.0)
        .map(|(g, w)| CheckRow { group: g.clone(), weight: *w, status: status_for(*w, 0.20) })
        .collect();

    let mk = |check: &str, scope_label: &str, limit: f64, rows: Vec<CheckRow>| {
        let status = rows.iter().map(|r| r.status).max_by_key(|s| severity(*s)).unwrap_or(CheckStatus::Ok);
        Check { check: check.into(), scope_label: scope_label.into(), limit, rows, status }
    };
    vec![
        mk("issuer_10", "Issuer <= 10% NAV (equities + bonds)", 0.10, issuer_rows),
        mk("forty", "Sum of issuers > 5% <= 40% NAV", 0.40, forty_rows),
        mk("group_20", "Connected group <= 20% NAV", 0.20, group_rows),
        mk("fund_20", "Target fund <= 20% NAV", 0.20, fund_rows),
        mk("deposit_20", "Deposits per bank <= 20% NAV", 0.20, dep_rows),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(t: &str, g: &str, w: f64) -> ConPosition {
        ConPosition { asset_type: t.into(), group: g.into(), weight: w }
    }

    #[test]
    fn default_groups() {
        assert_eq!(default_issuer_group("Action", "  Kering  SA "), "KERING SA");
        assert_eq!(default_issuer_group("Cash Acc", "Depositary Bk- CBLU"), "CBLU");
        assert_eq!(default_issuer_group("Margin Acc", "Managed acc - CABK"), "CABK");
        assert_eq!(default_issuer_group("Cash Acc", "NO SEPARATOR"), "NO SEPARATOR");
    }

    #[test]
    fn five_checks_toy_portfolio() {
        let positions = vec![
            pos("Action", "ALPHA", 0.09),        // watch on issuer_10 (>= 0.08)
            pos("Action", "BETA", 0.11),          // breach on issuer_10
            pos("Action", "GAMMA", 0.04),
            pos("Dividendes", "GAMMA", 0.02),     // folds into GAMMA -> 0.06
            pos("Future", "IGNORED", 0.50),       // excluded from all checks
            pos("Fonds", "F1", 0.19),             // watch on fund_20
            pos("Fonds", "F2", 0.05),             // ok
            pos("Cash Acc", "CBLU", 0.05),
            pos("Margin Acc", "CBLU", -0.01),     // nets to 0.04
            pos("Cash Acc", "NEGBANK", -0.02),    // floored to 0, dropped from rows
        ];
        let checks = concentration(&positions);
        assert_eq!(checks.len(), 5);

        let issuer = &checks[0];
        assert_eq!(issuer.check, "issuer_10");
        assert_eq!(issuer.status, CheckStatus::Breach);
        assert_eq!(issuer.rows[0].group, "BETA"); // sorted desc
        assert_eq!(issuer.rows[0].status, CheckStatus::Breach);
        assert_eq!(issuer.rows[1].group, "ALPHA");
        assert_eq!(issuer.rows[1].status, CheckStatus::Watch);
        let gamma = issuer.rows.iter().find(|r| r.group == "GAMMA").unwrap();
        assert!((gamma.weight - 0.06).abs() < 1e-12);
        assert!(!issuer.rows.iter().any(|r| r.group == "IGNORED"));

        let forty = &checks[1];
        assert!((forty.rows[0].weight - 0.26).abs() < 1e-12); // 0.09 + 0.11 + 0.06
        assert_eq!(forty.status, CheckStatus::Ok);

        let group = &checks[2];
        assert_eq!(group.check, "group_20");
        assert_eq!(group.status, CheckStatus::Ok); // 0.11 < 0.16 watch threshold

        let fund = &checks[3];
        assert_eq!(fund.rows[0].group, "F1");
        assert_eq!(fund.rows[0].status, CheckStatus::Watch);
        assert_eq!(fund.rows.len(), 2);

        let dep = &checks[4];
        assert_eq!(dep.rows.len(), 1); // NEGBANK floored to 0 and dropped
        assert_eq!(dep.rows[0].group, "CBLU");
        assert!((dep.rows[0].weight - 0.04).abs() < 1e-12);
        assert_eq!(dep.status, CheckStatus::Ok);
    }

    #[test]
    fn empty_input_yields_five_ok_checks() {
        let checks = concentration(&[]);
        assert_eq!(checks.len(), 5);
        assert!(checks.iter().all(|c| c.status == CheckStatus::Ok));
    }
}
