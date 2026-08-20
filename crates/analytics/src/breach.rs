//! Breach episodes: the pure logic that turns one run's breaching findings
//! into transitions against the episodes already open.
//!
//! An episode, not a row per run: a breach that persists for six weeks is one
//! thing to remediate, not forty-two. Nothing here touches a database or
//! knows about authorization — it takes what is open, takes what this run
//! found, and says what changed.

use std::collections::{HashMap, HashSet};

/// One breaching row from a run: the check, the thing that breached it, and
/// the observed value where the check has one.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub check_key: String,
    pub subject: String,
    pub value: Option<f64>,
}

/// An episode already open on the data (`closed_nav_date IS NULL`).
#[derive(Debug, Clone, PartialEq)]
pub struct LiveEpisode {
    pub id: i64,
    pub check_key: String,
    pub subject: String,
    pub peak_value: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Transition {
    Open { check_key: String, subject: String, value: Option<f64> },
    RaisePeak { id: i64, value: f64 },
    Close { id: i64 },
}

/// What changed between the episodes currently open and what this run found.
///
/// Ordering is deterministic: findings are processed in `findings` order
/// (opens and peaks interleaved as they occur), and all closes follow in
/// `live` order — so a test can assert on the sequence and a reviewer
/// reading the event timeline sees the same order every time.
pub fn transitions(live: &[LiveEpisode], findings: &[Finding]) -> Vec<Transition> {
    let key = |c: &str, s: &str| format!("{c}\u{1f}{s}");
    let open_by_key: HashMap<String, &LiveEpisode> =
        live.iter().map(|e| (key(&e.check_key, &e.subject), e)).collect();
    let found_keys: HashSet<String> =
        findings.iter().map(|f| key(&f.check_key, &f.subject)).collect();

    let mut out = Vec::new();
    for f in findings {
        match open_by_key.get(&key(&f.check_key, &f.subject)) {
            None => out.push(Transition::Open {
                check_key: f.check_key.clone(),
                subject: f.subject.clone(),
                value: f.value,
            }),
            Some(e) => {
                // A worse reading than the episode has ever seen is worth
                // recording; an equal or better one inside an open episode is
                // not news.
                if let Some(v) = f.value {
                    if e.peak_value.is_none_or(|p| v > p) {
                        out.push(Transition::RaisePeak { id: e.id, value: v });
                    }
                }
            }
        }
    }
    for e in live {
        if !found_keys.contains(&key(&e.check_key, &e.subject)) {
            out.push(Transition::Close { id: e.id });
        }
    }
    out
}

/// One instrument belonging to a breaching subject, at one snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct SubjectHolding {
    pub isin: String,
    pub quantity: Option<f64>,
}

/// What the machine thinks caused a breach, and why. `classification` is
/// `None` when the data cannot support a suggestion — never a guess.
#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    pub classification: Option<&'static str>,
    pub reason: String,
}

/// Quantities below this are treated as equal: position files carry rounding,
/// and a 1e-9 drift is not a purchase.
const QTY_EPSILON: f64 = 1e-6;

/// Proposes `active` or `passive` for a breach of `subject`, from the change
/// in its holdings between the previous snapshot and this one.
///
/// Deliberately derived from positions rather than the trade journal:
/// CACEIS-fed portfolios have no journal at all, and a classification that
/// silently skips those funds is worse than one that works everywhere and
/// asks a person to confirm it. Nothing here decides anything — the caller
/// stores this as a *proposal* and a reviewer confirms or overrides it.
pub fn propose(
    subject: &str,
    prev: Option<&[SubjectHolding]>,
    now: &[SubjectHolding],
    prev_weight: Option<f64>,
    now_weight: Option<f64>,
) -> Proposal {
    let Some(prev) = prev else {
        return Proposal {
            classification: None,
            reason: "first snapshot for this portfolio; no prior position to compare".to_string(),
        };
    };
    if prev.is_empty() && now.is_empty() {
        return Proposal {
            classification: None,
            reason: format!("{subject} has no instrument holdings; not derived from positions"),
        };
    }
    let before: std::collections::HashMap<&str, f64> = prev.iter()
        .map(|h| (h.isin.as_str(), h.quantity.unwrap_or(0.0)))
        .collect();
    let bought = now.iter().find(|h| {
        let was = before.get(h.isin.as_str()).copied().unwrap_or(0.0);
        h.quantity.unwrap_or(0.0) > was + QTY_EPSILON
    });
    match bought {
        Some(h) => {
            let was = before.get(h.isin.as_str()).copied().unwrap_or(0.0);
            Proposal {
                classification: Some("active"),
                reason: format!(
                    "quantity of {} rose from {} to {} since the previous snapshot",
                    h.isin, trim(was), trim(h.quantity.unwrap_or(0.0))),
            }
        }
        None => Proposal {
            classification: Some("passive"),
            reason: format!(
                "no purchase in {subject} since the previous snapshot; weight moved from {} to {}",
                pct(prev_weight), pct(now_weight)),
        },
    }
}

fn pct(x: Option<f64>) -> String {
    match x {
        Some(v) => format!("{:.2}%", v * 100.0),
        None => "an unknown weight".to_string(),
    }
}

/// Quantities are whole units far more often than not; render them without a
/// trailing ".00" so the reason reads like something a person wrote.
fn trim(x: f64) -> String {
    if (x - x.round()).abs() < QTY_EPSILON { format!("{}", x.round() as i64) } else { format!("{x:.4}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(check: &str, subject: &str, value: f64) -> Finding {
        Finding { check_key: check.into(), subject: subject.into(), value: Some(value) }
    }

    fn live(id: i64, check: &str, subject: &str, peak: f64) -> LiveEpisode {
        LiveEpisode { id, check_key: check.into(), subject: subject.into(), peak_value: Some(peak) }
    }

    #[test]
    fn a_first_breach_opens_an_episode() {
        let t = transitions(&[], &[finding("issuer_10", "ACME", 0.106)]);
        assert_eq!(t.len(), 1);
        assert!(matches!(&t[0], Transition::Open { check_key, subject, value }
            if check_key == "issuer_10" && subject == "ACME" && *value == Some(0.106)));
    }

    #[test]
    fn a_persisting_breach_does_not_open_a_second_episode() {
        let t = transitions(&[live(1, "issuer_10", "ACME", 0.106)],
                            &[finding("issuer_10", "ACME", 0.104)]);
        assert!(t.is_empty(), "still breaching, no worse: nothing to record, got {t:?}");
    }

    #[test]
    fn a_worsening_breach_raises_the_peak() {
        let t = transitions(&[live(1, "issuer_10", "ACME", 0.106)],
                            &[finding("issuer_10", "ACME", 0.121)]);
        assert_eq!(t.len(), 1);
        assert!(matches!(&t[0], Transition::RaisePeak { id: 1, value } if (*value - 0.121).abs() < 1e-12));
    }

    #[test]
    fn a_subject_that_stops_breaching_closes_its_episode() {
        let t = transitions(&[live(1, "issuer_10", "ACME", 0.106)], &[]);
        assert_eq!(t.len(), 1);
        assert!(matches!(&t[0], Transition::Close { id: 1 }));
    }

    #[test]
    fn episodes_are_keyed_by_check_and_subject_together() {
        // Same issuer breaching a different check is a different episode.
        let t = transitions(&[live(1, "issuer_10", "ACME", 0.106)],
                            &[finding("issuer_10", "ACME", 0.106),
                              finding("group_20", "ACME", 0.21)]);
        assert_eq!(t.len(), 1);
        assert!(matches!(&t[0], Transition::Open { check_key, .. } if check_key == "group_20"));
    }

    #[test]
    fn a_finding_with_no_value_still_opens_an_episode() {
        // The liquidity scenarios have no scalar; the episode is real anyway.
        let t = transitions(&[], &[Finding {
            check_key: "liq_top5".into(), subject: "Top 5 holders".into(), value: None,
        }]);
        assert_eq!(t.len(), 1);
        assert!(matches!(&t[0], Transition::Open { value: None, .. }));
    }

    #[test]
    fn a_first_measured_value_raises_a_peak_that_was_never_set() {
        // An episode can be opened with no value (e.g., liquidity scenarios),
        // then a measurement arrives; it should set the peak.
        let t = transitions(&[LiveEpisode {
            id: 1, check_key: "liq_top5".into(), subject: "Top 5 holders".into(), peak_value: None,
        }], &[Finding {
            check_key: "liq_top5".into(), subject: "Top 5 holders".into(), value: Some(0.35),
        }]);
        assert_eq!(t.len(), 1);
        assert!(matches!(&t[0], Transition::RaisePeak { id: 1, value } if (*value - 0.35).abs() < 1e-12));
    }

    #[test]
    fn a_valueless_finding_never_disturbs_an_existing_peak() {
        // The liquidity scenarios always report None; they should never update
        // an episode's peak, even if it was previously set.
        let t = transitions(&[LiveEpisode {
            id: 1, check_key: "liq_top5".into(), subject: "Top 5 holders".into(), peak_value: Some(0.35),
        }], &[Finding {
            check_key: "liq_top5".into(), subject: "Top 5 holders".into(), value: None,
        }]);
        assert!(t.is_empty(), "valueless finding should not produce any transitions, got {t:?}");
    }

    fn hold(isin: &str, q: f64) -> SubjectHolding {
        SubjectHolding { isin: isin.into(), quantity: Some(q) }
    }

    #[test]
    fn no_purchase_proposes_passive_and_says_how_the_weight_moved() {
        let p = propose("ACME", Some(&[hold("X1", 100.0)]), &[hold("X1", 100.0)],
                        Some(0.094), Some(0.106));
        assert_eq!(p.classification, Some("passive"));
        assert!(p.reason.contains("no purchase in ACME"), "{}", p.reason);
        assert!(p.reason.contains("9.40%") && p.reason.contains("10.60%"), "{}", p.reason);
    }

    #[test]
    fn an_increased_quantity_proposes_active_and_names_the_instrument() {
        let p = propose("ACME", Some(&[hold("X1", 100.0)]), &[hold("X1", 180.0)],
                        Some(0.094), Some(0.106));
        assert_eq!(p.classification, Some("active"));
        assert!(p.reason.contains("X1"), "{}", p.reason);
        assert!(p.reason.contains("100") && p.reason.contains("180"), "{}", p.reason);
    }

    #[test]
    fn an_instrument_that_is_new_this_snapshot_is_a_purchase() {
        let p = propose("ACME", Some(&[]), &[hold("X2", 50.0)], Some(0.0), Some(0.11));
        assert_eq!(p.classification, Some("active"));
        assert!(p.reason.contains("X2"), "{}", p.reason);
    }

    #[test]
    fn with_no_previous_snapshot_nothing_is_proposed() {
        let p = propose("ACME", None, &[hold("X1", 100.0)], None, Some(0.106));
        assert_eq!(p.classification, None);
        assert!(p.reason.contains("no prior position to compare"), "{}", p.reason);
    }

    #[test]
    fn a_subject_with_no_holdings_at_all_proposes_nothing() {
        // Liquidity, VaR and EMIR episodes have no issuer subject.
        let p = propose("Top 5 holders", Some(&[]), &[], None, None);
        assert_eq!(p.classification, None);
        assert!(p.reason.contains("not derived from positions"), "{}", p.reason);
    }
}
