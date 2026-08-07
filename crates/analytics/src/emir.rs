//! EMIR clearing-threshold monitoring (suivi des seuils de compensation).
//!
//! Average month-end position over the last 12 months per asset class,
//! compared to the clearing thresholds of Delegated Regulation (EU)
//! No 149/2013 as amended. Only OTC positions count toward a threshold —
//! a contract on an EU regulated market or an equivalent third-country
//! market is not OTC — but the total line is reported alongside so the
//! disclosure works under either reading. Gross notional, no netting.

use crate::futures::Category;
use chrono::{Datelike, NaiveDate};
use serde::Serialize;

/// WATCH once the OTC average reaches this fraction of the threshold.
pub const WATCH_FRACTION: f64 = 0.80;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThresholdClass {
    Credit,
    Equity,
    InterestRate,
    Fx,
    CommodityOther,
}

impl ThresholdClass {
    pub const ALL: [ThresholdClass; 5] = [
        ThresholdClass::Credit,
        ThresholdClass::Equity,
        ThresholdClass::InterestRate,
        ThresholdClass::Fx,
        ThresholdClass::CommodityOther,
    ];

    pub fn of(cat: Category) -> Self {
        match cat {
            Category::Credit => Self::Credit,
            Category::Equity => Self::Equity,
            Category::InterestRate => Self::InterestRate,
            Category::Fx => Self::Fx,
            // The regulation's fifth bucket is "commodity and other".
            Category::Commodity | Category::Other => Self::CommodityOther,
        }
    }

    /// EUR notional thresholds per RTS 149/2013 art. 11 as amended.
    pub fn threshold_eur(&self) -> f64 {
        match self {
            Self::Credit | Self::Equity => 1e9,
            Self::InterestRate | Self::Fx => 3e9,
            Self::CommodityOther => 4e9,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Credit => "Credit derivatives",
            Self::Equity => "Equity derivatives",
            Self::InterestRate => "Interest-rate derivatives",
            Self::Fx => "FX derivatives",
            Self::CommodityOther => "Commodity and other derivatives",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Ok,
    Watch,
    Breach,
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Watch => "watch",
            Self::Breach => "breach",
        }
    }
}

pub fn verdict(avg_otc_eur: f64, threshold_eur: f64) -> Verdict {
    let frac = avg_otc_eur / threshold_eur;
    if frac >= 1.0 {
        Verdict::Breach
    } else if frac >= WATCH_FRACTION {
        Verdict::Watch
    } else {
        Verdict::Ok
    }
}

fn month_start(d: NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(d.year(), d.month(), 1).unwrap()
}

fn prev_month_start(s: NaiveDate) -> NaiveDate {
    if s.month() == 1 {
        NaiveDate::from_ymd_opt(s.year() - 1, 12, 1).unwrap()
    } else {
        NaiveDate::from_ymd_opt(s.year(), s.month() - 1, 1).unwrap()
    }
}

/// Last day of the month that starts at `start` (a first-of-month).
fn month_end(start: NaiveDate) -> NaiveDate {
    let next = if start.month() == 12 {
        NaiveDate::from_ymd_opt(start.year() + 1, 1, 1).unwrap()
    } else {
        NaiveDate::from_ymd_opt(start.year(), start.month() + 1, 1).unwrap()
    };
    next.pred_opt().unwrap()
}

/// The 12 calendar months ending with `anchor`'s month, oldest first, each
/// paired with the snapshot date to use: the latest available date that
/// falls INSIDE the month and at or before `min(month end, anchor)`. A date
/// from an earlier month never stands in for a missing month — that would
/// double-count one position as two month-ends. Deterministic from data:
/// no wall clock.
pub fn month_window(anchor: NaiveDate, available: &[NaiveDate]) -> Vec<(NaiveDate, Option<NaiveDate>)> {
    let mut starts = Vec::with_capacity(12);
    let mut s = month_start(anchor);
    for _ in 0..12 {
        starts.push(s);
        s = prev_month_start(s);
    }
    starts.reverse();
    starts
        .into_iter()
        .map(|m| {
            let cutoff = month_end(m).min(anchor);
            let chosen = available.iter().copied().filter(|d| *d >= m && *d <= cutoff).max();
            (m, chosen)
        })
        .collect()
}

/// One derivative position at one month-end, with its EUR notional already
/// computed by the exposure path. `notional_eur` is `None` when the spec, an
/// input or the FX rate was missing — excluded from the sums and warned
/// about, never silently zeroed.
#[derive(Debug, Clone)]
pub struct EmirPosition {
    pub ticker: String,
    pub category: Category,
    pub notional_eur: Option<f64>,
    pub otc: bool,
    pub unconfirmed: bool,
}

#[derive(Debug, Clone)]
pub struct MonthSnapshot {
    /// First day of the calendar month.
    pub month: NaiveDate,
    /// `None` when no snapshot falls inside the month.
    pub snapshot: Option<(NaiveDate, Vec<EmirPosition>)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MonthCell {
    pub month: NaiveDate,
    pub snapshot_date: Option<NaiveDate>,
    pub total_eur: Option<f64>,
    pub otc_eur: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClassReport {
    pub class: ThresholdClass,
    pub label: &'static str,
    pub threshold_eur: f64,
    pub months: Vec<MonthCell>,
    pub avg_total_eur: f64,
    pub avg_otc_eur: f64,
    pub pct_of_threshold: f64,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, Serialize)]
pub struct ThresholdReport {
    pub classes: Vec<ClassReport>,
    pub months_present: usize,
    pub months_total: usize,
    pub warnings: Vec<String>,
}

/// Gross notional per threshold class per month, averaged over the months
/// present. Shorts count in absolute value; long and short are never netted.
pub fn thresholds(months: &[MonthSnapshot]) -> ThresholdReport {
    let months_present = months.iter().filter(|m| m.snapshot.is_some()).count();

    let mut warnings = Vec::new();
    for m in months {
        match &m.snapshot {
            None => warnings.push(format!(
                "{}: no snapshot in this month; excluded from the average",
                m.month.format("%Y-%m")
            )),
            Some((date, ps)) => {
                for p in ps {
                    if p.notional_eur.is_none() {
                        warnings.push(format!(
                            "{date}: {} notional unavailable (missing spec, quantity, price or FX rate); excluded from the sums",
                            p.ticker
                        ));
                    } else if p.unconfirmed {
                        warnings.push(format!(
                            "{date}: {} contract spec unconfirmed; its notional is provisional",
                            p.ticker
                        ));
                    }
                }
            }
        }
    }

    let classes = ThresholdClass::ALL
        .iter()
        .map(|cls| {
            let cells: Vec<MonthCell> = months
                .iter()
                .map(|m| match &m.snapshot {
                    None => MonthCell { month: m.month, snapshot_date: None, total_eur: None, otc_eur: None },
                    Some((date, ps)) => {
                        let mut total = 0.0;
                        let mut otc = 0.0;
                        for p in ps.iter().filter(|p| ThresholdClass::of(p.category) == *cls) {
                            if let Some(n) = p.notional_eur {
                                let n = n.abs();
                                total += n;
                                if p.otc {
                                    otc += n;
                                }
                            }
                        }
                        MonthCell { month: m.month, snapshot_date: Some(*date), total_eur: Some(total), otc_eur: Some(otc) }
                    }
                })
                .collect();
            // Average over the months that have data; max(1) only guards the
            // no-data case, where the sums are zero anyway.
            let n = months_present.max(1) as f64;
            let avg_total_eur = cells.iter().filter_map(|c| c.total_eur).sum::<f64>() / n;
            let avg_otc_eur = cells.iter().filter_map(|c| c.otc_eur).sum::<f64>() / n;
            let threshold_eur = cls.threshold_eur();
            ClassReport {
                class: *cls,
                label: cls.label(),
                threshold_eur,
                months: cells,
                avg_total_eur,
                avg_otc_eur,
                pct_of_threshold: avg_otc_eur / threshold_eur,
                verdict: verdict(avg_otc_eur, threshold_eur),
            }
        })
        .collect();

    ThresholdReport { classes, months_present, months_total: months.len(), warnings }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationTier {
    NotTriggered,
    Quarterly,
    Weekly,
    Daily,
}

#[derive(Debug, Clone, Serialize)]
pub struct Monitors {
    pub otc_open_contracts: usize,
    pub reconciliation: ReconciliationTier,
    /// Semiannual portfolio-compression analysis required (>= 500 OTC
    /// contracts outstanding with one counterparty, RTS 149/2013 art. 14).
    pub compression_required: bool,
}

/// Reconciliation tiers for a financial counterparty (RTS 149/2013 art. 13):
/// daily above 500 contracts, weekly 51-499, quarterly 50 or fewer. The tool
/// has no counterparty data, so the count assumes a single counterparty —
/// the strictest possible tier assignment.
pub fn monitors(anchor_positions: &[EmirPosition]) -> Monitors {
    let n = anchor_positions.iter().filter(|p| p.otc).count();
    let reconciliation = match n {
        0 => ReconciliationTier::NotTriggered,
        1..=50 => ReconciliationTier::Quarterly,
        51..=499 => ReconciliationTier::Weekly,
        _ => ReconciliationTier::Daily,
    };
    Monitors { otc_open_contracts: n, reconciliation, compression_required: n >= 500 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::futures::Category;
    use chrono::NaiveDate;

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn pos(ticker: &str, cat: Category, notional: Option<f64>, otc: bool, unconfirmed: bool) -> EmirPosition {
        EmirPosition { ticker: ticker.into(), category: cat, notional_eur: notional, otc, unconfirmed }
    }

    #[test]
    fn window_is_twelve_months_oldest_first_within_month_capped_at_anchor() {
        let available = [
            d("2026-07-31"), // after the anchor: must never be chosen
            d("2026-07-24"),
            d("2026-07-10"),
            d("2026-06-26"),
            d("2026-04-30"),
            d("2025-08-29"),
            d("2025-07-31"), // before the window: must not leak into 2025-08
        ];
        let w = month_window(d("2026-07-24"), &available);
        assert_eq!(w.len(), 12);
        assert_eq!(w[0].0, d("2025-08-01"));
        assert_eq!(w[11].0, d("2026-07-01"));
        assert_eq!(w[0].1, Some(d("2025-08-29")));
        assert_eq!(w[1].1, None); // 2025-09: no snapshot IN that month
        assert_eq!(w[8].1, Some(d("2026-04-30")));
        assert_eq!(w[9].1, None); // 2026-05
        assert_eq!(w[10].1, Some(d("2026-06-26")));
        // Anchor month: capped at the anchor itself, so 2026-07-31 is skipped.
        assert_eq!(w[11].1, Some(d("2026-07-24")));
    }

    #[test]
    fn window_handles_year_boundary() {
        let w = month_window(d("2026-01-15"), &[d("2026-01-15")]);
        assert_eq!(w[0].0, d("2025-02-01"));
        assert_eq!(w[11].0, d("2026-01-01"));
    }

    #[test]
    fn category_mapping_and_threshold_amounts() {
        assert_eq!(ThresholdClass::of(Category::Equity), ThresholdClass::Equity);
        assert_eq!(ThresholdClass::of(Category::Credit), ThresholdClass::Credit);
        assert_eq!(ThresholdClass::of(Category::InterestRate), ThresholdClass::InterestRate);
        assert_eq!(ThresholdClass::of(Category::Fx), ThresholdClass::Fx);
        assert_eq!(ThresholdClass::of(Category::Commodity), ThresholdClass::CommodityOther);
        assert_eq!(ThresholdClass::of(Category::Other), ThresholdClass::CommodityOther);
        assert_eq!(ThresholdClass::Credit.threshold_eur(), 1e9);
        assert_eq!(ThresholdClass::Equity.threshold_eur(), 1e9);
        assert_eq!(ThresholdClass::InterestRate.threshold_eur(), 3e9);
        assert_eq!(ThresholdClass::Fx.threshold_eur(), 3e9);
        assert_eq!(ThresholdClass::CommodityOther.threshold_eur(), 4e9);
    }

    #[test]
    fn averages_divide_by_months_present_and_shorts_count_absolute() {
        // Two present months out of a 3-slot window; shorts enter the gross
        // sum in absolute value; only OTC-flagged notional feeds the OTC line.
        let months = [
            MonthSnapshot {
                month: d("2026-05-01"),
                snapshot: Some((d("2026-05-29"), vec![
                    pos("A Index", Category::Equity, Some(100.0), false, false),
                    pos("B Index", Category::Equity, Some(-40.0), true, false), // short, OTC
                ])),
            },
            MonthSnapshot { month: d("2026-06-01"), snapshot: None },
            MonthSnapshot {
                month: d("2026-07-01"),
                snapshot: Some((d("2026-07-24"), vec![
                    pos("A Index", Category::Equity, Some(300.0), false, false),
                    pos("B Index", Category::Equity, Some(-60.0), true, false),
                ])),
            },
        ];
        let r = thresholds(&months);
        assert_eq!(r.months_present, 2);
        assert_eq!(r.months_total, 3);
        let eq = r.classes.iter().find(|c| c.class == ThresholdClass::Equity).unwrap();
        assert_eq!(eq.months[0].total_eur, Some(140.0));
        assert_eq!(eq.months[0].otc_eur, Some(40.0));
        assert_eq!(eq.months[1].total_eur, None);
        assert_eq!(eq.months[1].snapshot_date, None);
        assert!((eq.avg_total_eur - 250.0).abs() < 1e-9); // (140+360)/2
        assert!((eq.avg_otc_eur - 50.0).abs() < 1e-9); // (40+60)/2
        assert!(r.warnings.iter().any(|w| w.contains("2026-06") && w.contains("no snapshot")));
        // A class with no positions averages to zero, verdict OK.
        let fx = r.classes.iter().find(|c| c.class == ThresholdClass::Fx).unwrap();
        assert_eq!(fx.avg_otc_eur, 0.0);
        assert_eq!(fx.verdict, Verdict::Ok);
    }

    #[test]
    fn verdict_boundaries() {
        assert_eq!(verdict(0.799_999e9, 1e9), Verdict::Ok);
        assert_eq!(verdict(0.8e9, 1e9), Verdict::Watch);
        assert_eq!(verdict(0.999e9, 1e9), Verdict::Watch);
        assert_eq!(verdict(1.0e9, 1e9), Verdict::Breach);
        assert_eq!(verdict(2.5e9, 3e9), Verdict::Watch); // 83% of the 3bn tier
    }

    #[test]
    fn warnings_name_the_contract_and_date() {
        let months = [MonthSnapshot {
            month: d("2026-07-01"),
            snapshot: Some((d("2026-07-24"), vec![
                pos("TYU6 Comdty", Category::InterestRate, None, false, false),
                pos("RXU6 Comdty", Category::InterestRate, Some(1000.0), false, true),
            ])),
        }];
        let r = thresholds(&months);
        assert!(r.warnings.iter().any(|w| w.contains("TYU6 Comdty") && w.contains("2026-07-24") && w.contains("excluded")));
        assert!(r.warnings.iter().any(|w| w.contains("RXU6 Comdty") && w.contains("provisional")));
        // The missing notional is excluded, not zeroed: the sum still counts RX.
        let ir = r.classes.iter().find(|c| c.class == ThresholdClass::InterestRate).unwrap();
        assert_eq!(ir.months[0].total_eur, Some(1000.0));
    }

    #[test]
    fn monitor_tiers() {
        let mk = |n: usize| -> Vec<EmirPosition> {
            (0..n).map(|i| pos(&format!("C{i}"), Category::Fx, Some(1.0), true, false)).collect()
        };
        let m = monitors(&[]);
        assert_eq!(m.otc_open_contracts, 0);
        assert_eq!(m.reconciliation, ReconciliationTier::NotTriggered);
        assert!(!m.compression_required);
        assert_eq!(monitors(&mk(1)).reconciliation, ReconciliationTier::Quarterly);
        assert_eq!(monitors(&mk(50)).reconciliation, ReconciliationTier::Quarterly);
        assert_eq!(monitors(&mk(51)).reconciliation, ReconciliationTier::Weekly);
        assert_eq!(monitors(&mk(499)).reconciliation, ReconciliationTier::Weekly);
        let m = monitors(&mk(500));
        assert_eq!(m.reconciliation, ReconciliationTier::Daily);
        assert!(m.compression_required);
        // Non-OTC positions never count.
        let mut ps = mk(2);
        ps.push(pos("LISTED", Category::Fx, Some(1.0), false, false));
        assert_eq!(monitors(&ps).otc_open_contracts, 2);
    }
}
