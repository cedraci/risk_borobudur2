//! Observed subscription and redemption history, from JOURSRLUX.
//!
//! Windows count *loaded observations*, not calendar days: a day that was
//! never uploaded is simply absent, which is honest about what the history
//! actually contains. These numbers inform the configured shock; they never
//! set it.

use chrono::NaiveDate;
use serde::Serialize;

pub const FLOW_WINDOWS: [u32; 3] = [1, 5, 20];
/// Below this, the statistics are reported as unavailable with the count
/// rather than computed from too little history.
pub const MIN_FLOW_OBSERVATIONS: usize = 20;

#[derive(Debug, Clone)]
pub struct FlowObs {
    pub date: NaiveDate,
    /// Subscriptions less redemptions, both as magnitudes.
    pub net_eur: f64,
    /// Fund net assets on that date.
    pub nav_eur: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorstOutflow {
    pub window: u32,
    /// Positive fraction of NAV. Zero when the window never saw a net outflow.
    pub pct_of_nav: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FlowStats {
    pub n_observations: usize,
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub worst: Vec<WorstOutflow>,
}

pub fn flow_stats(obs: &[FlowObs]) -> Option<FlowStats> {
    if obs.len() < MIN_FLOW_OBSERVATIONS { return None; }
    let mut s: Vec<&FlowObs> = obs.iter().collect();
    s.sort_by_key(|o| o.date);

    let worst = FLOW_WINDOWS.iter().map(|&w| {
        let w_us = w as usize;
        let mut worst_ratio = 0.0f64;
        if s.len() >= w_us {
            for end in (w_us - 1)..s.len() {
                let start = end + 1 - w_us;
                // The window is measured against the NAV it opened with.
                let denom = s[start].nav_eur;
                if !(denom.is_finite() && denom > 0.0) { continue; }
                let sum: f64 = s[start..=end].iter().map(|o| o.net_eur).sum();
                let ratio = sum / denom;
                if ratio < worst_ratio { worst_ratio = ratio; }
            }
        }
        // `worst_ratio` starts at 0.0 and only ever moves negative, but a
        // series that never sees a net outflow leaves it at exactly 0.0 —
        // negating that yields -0.0, which `==` treats as equal to 0.0 but
        // which serde_json serialises with the sign bit intact. Normalise so
        // a non-negative accumulator always yields positive zero on output.
        let pct_of_nav = if worst_ratio < 0.0 { -worst_ratio } else { 0.0 };
        WorstOutflow { window: w, pct_of_nav }
    }).collect();

    Some(FlowStats {
        n_observations: s.len(),
        from: s.first().unwrap().date,
        to: s.last().unwrap().date,
        worst,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, NaiveDate};

    /// `n` daily observations at a constant 100m NAV, all zero net flow.
    fn series(n: usize) -> Vec<FlowObs> {
        let start = NaiveDate::from_ymd_opt(2026, 1, 5).unwrap();
        (0..n).map(|i| FlowObs {
            date: start + Duration::days(i as i64),
            net_eur: 0.0,
            nav_eur: 100_000_000.0,
        }).collect()
    }

    #[test]
    fn too_little_history_is_unavailable_not_a_number() {
        assert!(flow_stats(&series(19)).is_none());
        assert!(flow_stats(&[]).is_none());
        assert!(flow_stats(&series(20)).is_some());
    }

    #[test]
    fn the_worst_single_day_outflow_is_reported_as_a_positive_percentage() {
        let mut s = series(40);
        s[10].net_eur = -5_000_000.0;
        let st = flow_stats(&s).unwrap();
        let w1 = st.worst.iter().find(|w| w.window == 1).unwrap();
        assert!((w1.pct_of_nav - 0.05).abs() < 1e-12);
    }

    #[test]
    fn a_run_of_outflows_shows_up_in_the_longer_window() {
        let mut s = series(40);
        for d in &mut s[10..15] { d.net_eur = -2_000_000.0; }
        let st = flow_stats(&s).unwrap();
        let w1 = st.worst.iter().find(|w| w.window == 1).unwrap();
        let w5 = st.worst.iter().find(|w| w.window == 5).unwrap();
        assert!((w1.pct_of_nav - 0.02).abs() < 1e-12);
        assert!((w5.pct_of_nav - 0.10).abs() < 1e-12);
    }

    #[test]
    fn subscriptions_never_produce_a_negative_worst_outflow() {
        let mut s = series(40);
        for o in s.iter_mut() { o.net_eur = 1_000_000.0; }
        let st = flow_stats(&s).unwrap();
        assert!(st.worst.iter().all(|w| w.pct_of_nav == 0.0));
    }

    #[test]
    fn the_range_and_count_describe_the_history_actually_loaded() {
        let s = series(25);
        let st = flow_stats(&s).unwrap();
        assert_eq!(st.n_observations, 25);
        assert_eq!(st.from, NaiveDate::from_ymd_opt(2026, 1, 5).unwrap());
        assert_eq!(st.to, NaiveDate::from_ymd_opt(2026, 1, 29).unwrap());
    }

    #[test]
    fn observations_are_ordered_by_date_regardless_of_input_order() {
        let mut s = series(30);
        s[3].net_eur = -9_000_000.0;
        s.reverse();
        let st = flow_stats(&s).unwrap();
        assert_eq!(st.from, NaiveDate::from_ymd_opt(2026, 1, 5).unwrap());
        assert!((st.worst.iter().find(|w| w.window == 1).unwrap().pct_of_nav - 0.09).abs() < 1e-12);
    }

    /// `==` treats 0.0 and -0.0 as equal, so it cannot catch a regression
    /// here — the check must be sign-sensitive. `serde_json` (used by the
    /// API layer that consumes this struct) preserves the sign bit of a
    /// float, so a leaked -0.0 would render as `"pct_of_nav": -0.0` over the
    /// wire; `f64::is_sign_negative` detects the same defect directly.
    #[test]
    fn an_all_subscriptions_series_reports_worst_outflow_as_positive_zero_not_negative_zero() {
        let mut s = series(40);
        for o in s.iter_mut() { o.net_eur = 1_000_000.0; }
        let st = flow_stats(&s).unwrap();
        for w in &st.worst {
            assert!(!w.pct_of_nav.is_sign_negative(), "window {}: -0.0 leaked", w.window);
        }
    }

    #[test]
    fn a_non_positive_nav_observation_is_skipped_not_divided_by() {
        let mut s = series(30);
        s[5].nav_eur = 0.0;
        s[5].net_eur = -1_000_000.0;
        let st = flow_stats(&s).unwrap();
        assert!(st.worst.iter().all(|w| w.pct_of_nav.is_finite()));
    }
}
