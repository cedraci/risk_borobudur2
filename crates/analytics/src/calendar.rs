use crate::NavPoint;
use chrono::Datelike;

#[derive(Debug, Clone, serde::Serialize)]
pub struct PeriodReturn {
    pub year: i32,
    pub period: u32,
    pub value: f64,
}

/// Generic period-end returns: `key` maps a point to its (year, period)
/// bucket; return = last NAV of bucket / last NAV of previous bucket - 1
/// (first bucket: vs first point of the series).
fn period_returns(nav: &[NavPoint], key: impl Fn(&NavPoint) -> (i32, u32)) -> Vec<PeriodReturn> {
    let Some(first) = nav.first() else { return Vec::new(); };
    let mut ends: Vec<(i32, u32, f64)> = Vec::new();
    for p in nav {
        let (y, per) = key(p);
        match ends.last_mut() {
            Some(last) if (last.0, last.1) == (y, per) => last.2 = p.value,
            _ => ends.push((y, per, p.value)),
        }
    }
    let mut prev = first.value;
    ends.into_iter()
        .map(|(year, period, v)| {
            let r = PeriodReturn { year, period, value: v / prev - 1.0 };
            prev = v;
            r
        })
        .collect()
}

pub fn monthly_returns(nav: &[NavPoint]) -> Vec<PeriodReturn> {
    period_returns(nav, |p| (p.date.year(), p.date.month()))
}

pub fn quarterly_returns(nav: &[NavPoint]) -> Vec<PeriodReturn> {
    period_returns(nav, |p| (p.date.year(), (p.date.month() - 1) / 3 + 1))
}

/// Annual totals; `period` is always 0.
pub fn annual_returns(nav: &[NavPoint]) -> Vec<PeriodReturn> {
    period_returns(nav, |p| (p.date.year(), 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, day).unwrap() }
    fn nav(points: &[(i32, u32, u32, f64)]) -> Vec<NavPoint> {
        points.iter().map(|&(y, m, dd, v)| NavPoint { date: d(y, m, dd), value: v }).collect()
    }

    fn fixture() -> Vec<NavPoint> {
        nav(&[
            (2025, 1, 31, 100.0), (2025, 2, 15, 103.0),
            (2025, 2, 28, 104.0), (2025, 3, 31, 102.0),
        ])
    }

    #[test]
    fn monthly_last_of_month_vs_prev() {
        let m = monthly_returns(&fixture());
        assert_eq!(m.len(), 3);
        assert_eq!((m[0].year, m[0].period), (2025, 1));
        assert!((m[0].value - 0.0).abs() < 1e-12); // inception month: 100/100-1
        assert!((m[1].value - 0.04).abs() < 1e-12); // 104/100-1 (mid-month 103 ignored)
        assert!((m[2].value - (102.0 / 104.0 - 1.0)).abs() < 1e-12);
    }

    #[test]
    fn quarterly_compounds_across_months() {
        let q = quarterly_returns(&fixture());
        assert_eq!(q.len(), 1);
        assert_eq!((q[0].year, q[0].period), (2025, 1));
        assert!((q[0].value - 0.02).abs() < 1e-12); // 102/100-1
    }

    #[test]
    fn annual_totals() {
        let n = nav(&[(2024, 12, 31, 100.0), (2025, 3, 31, 104.0), (2025, 6, 30, 106.0)]);
        let a = annual_returns(&n);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].year, 2024);
        assert!((a[0].value - 0.0).abs() < 1e-12);
        assert_eq!(a[1].year, 2025);
        assert!((a[1].value - 0.06).abs() < 1e-12); // 106/100-1
    }

    #[test]
    fn empty_series() {
        assert!(monthly_returns(&[]).is_empty());
    }
}
