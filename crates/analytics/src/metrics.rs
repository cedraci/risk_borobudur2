use crate::{daily_returns, sample_std, NavPoint};
use chrono::{Datelike, NaiveDate};

pub const TRADING_DAYS: f64 = 252.0;

pub fn annualized_vol(returns: &[f64]) -> Option<f64> {
    Some(sample_std(returns)? * TRADING_DAYS.sqrt())
}

/// Geometric annualization of a window of daily returns:
/// (prod(1+r))^(252/n) - 1. None on empty input or wipeout (growth <= 0).
pub fn annualized_return_from_returns(returns: &[f64]) -> Option<f64> {
    if returns.is_empty() { return None; }
    let growth = returns.iter().fold(1.0, |a, r| a * (1.0 + r));
    if growth <= 0.0 { return None; }
    Some(growth.powf(TRADING_DAYS / returns.len() as f64) - 1.0)
}

pub fn sharpe_ratio(ann_return: f64, ann_vol: f64, risk_free: f64) -> Option<f64> {
    if ann_vol <= 0.0 { return None; }
    Some((ann_return - risk_free) / ann_vol)
}

/// Yield/vol ratio = annualized return / annualized vol (no risk-free deduction).
pub fn yield_vol_ratio(ann_return: f64, ann_vol: f64) -> Option<f64> {
    if ann_vol <= 0.0 { return None; }
    Some(ann_return / ann_vol)
}

/// NAV_last / NAV_(latest date in a prior year) - 1; inception fallback.
pub fn ytd_performance(nav: &[NavPoint], as_of: NaiveDate) -> Option<f64> {
    let last = nav.iter().rev().find(|p| p.date <= as_of)?;
    let base = nav
        .iter()
        .rev()
        .find(|p| p.date.year() < as_of.year())
        .map(|p| p.value)
        .unwrap_or(nav.first()?.value);
    if base <= 0.0 { return None; }
    Some(last.value / base - 1.0)
}

/// Rolling window over DAILY RETURNS of `nav`. Each output point is dated at
/// the last return date of its window. Windows with f(...)==None are skipped.
pub fn rolling(nav: &[NavPoint], window: usize, f: impl Fn(&[f64]) -> Option<f64>) -> Vec<NavPoint> {
    let rets = daily_returns(nav);
    if window < 2 || rets.len() < window { return Vec::new(); }
    let values: Vec<f64> = rets.iter().map(|p| p.value).collect();
    (window..=values.len())
        .filter_map(|end| {
            f(&values[end - window..end]).map(|v| NavPoint { date: rets[end - 1].date, value: v })
        })
        .collect()
}

pub fn rolling_vol(nav: &[NavPoint], window: usize) -> Vec<NavPoint> {
    rolling(nav, window, annualized_vol)
}

pub fn rolling_yield_vol(nav: &[NavPoint], window: usize) -> Vec<NavPoint> {
    rolling(nav, window, |r| {
        yield_vol_ratio(annualized_return_from_returns(r)?, annualized_vol(r)?)
    })
}

pub fn rolling_sharpe(nav: &[NavPoint], window: usize, risk_free: f64) -> Vec<NavPoint> {
    rolling(nav, window, move |r| {
        sharpe_ratio(annualized_return_from_returns(r)?, annualized_vol(r)?, risk_free)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NavPoint;
    use chrono::NaiveDate;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, day).unwrap() }
    fn nav(points: &[(i32, u32, u32, f64)]) -> Vec<NavPoint> {
        points.iter().map(|&(y, m, dd, v)| NavPoint { date: d(y, m, dd), value: v }).collect()
    }

    #[test]
    fn annualized_vol_known_value() {
        // returns [0.1, -0.1]: sample std sqrt(0.02) -> ann vol sqrt(0.02*252) = sqrt(5.04)
        let v = annualized_vol(&[0.1, -0.1]).unwrap();
        assert!((v - 5.04f64.sqrt()).abs() < 1e-9);
        assert_eq!(annualized_vol(&[0.1]), None);
    }

    #[test]
    fn annualized_return_round_trip() {
        // 252 equal returns compounding to +10% over one year -> annualized 10%
        let r = 1.1f64.powf(1.0 / 252.0) - 1.0;
        let a = annualized_return_from_returns(&vec![r; 252]).unwrap();
        assert!((a - 0.1).abs() < 1e-9);
        assert_eq!(annualized_return_from_returns(&[]), None);
    }

    #[test]
    fn sharpe_known_value() {
        assert!((sharpe_ratio(0.10, 0.20, 0.02).unwrap() - 0.4).abs() < 1e-12);
        assert_eq!(sharpe_ratio(0.10, 0.0, 0.02), None);
    }

    #[test]
    fn ytd_uses_prior_year_close() {
        let n = nav(&[
            (2024, 12, 30, 100.0), (2024, 12, 31, 102.0),
            (2025, 1, 2, 105.0), (2025, 1, 3, 107.0),
        ]);
        let y = ytd_performance(&n, d(2025, 1, 3)).unwrap();
        assert!((y - (107.0 / 102.0 - 1.0)).abs() < 1e-12);
    }

    #[test]
    fn ytd_falls_back_to_inception() {
        let n = nav(&[(2025, 3, 1, 100.0), (2025, 3, 5, 104.0)]);
        assert!((ytd_performance(&n, d(2025, 3, 5)).unwrap() - 0.04).abs() < 1e-12);
        assert_eq!(ytd_performance(&n, d(2025, 2, 1)), None); // as_of before series
    }

    #[test]
    fn rolling_windows_mechanics() {
        let n = nav(&[
            (2025, 1, 6, 100.0), (2025, 1, 7, 102.0), (2025, 1, 8, 101.0),
            (2025, 1, 9, 103.0), (2025, 1, 10, 104.0),
        ]);
        // 4 returns, window 2 -> 3 output points dated at each window's last return
        let out = rolling_vol(&n, 2);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].date, d(2025, 1, 8));
        assert_eq!(out[2].date, d(2025, 1, 10));
        // each value must equal annualized_vol of that trailing slice
        let rets: Vec<f64> = crate::daily_returns(&n).iter().map(|p| p.value).collect();
        assert!((out[0].value - annualized_vol(&rets[0..2]).unwrap()).abs() < 1e-12);
        assert!((out[2].value - annualized_vol(&rets[2..4]).unwrap()).abs() < 1e-12);
        // window larger than returns -> empty
        assert!(rolling_vol(&n, 5).is_empty());
    }
}
