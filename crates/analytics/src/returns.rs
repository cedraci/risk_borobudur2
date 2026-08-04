use chrono::NaiveDate;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct NavPoint {
    pub date: NaiveDate,
    pub value: f64,
}

/// Daily simple returns. Point dated at the later observation. Zero/negative
/// previous NAV rows are skipped (cannot produce a meaningful return).
pub fn daily_returns(nav: &[NavPoint]) -> Vec<NavPoint> {
    nav.windows(2)
        .filter(|w| w[0].value > 0.0)
        .map(|w| NavPoint { date: w[1].date, value: w[1].value / w[0].value - 1.0 })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn nav(points: &[(i32, u32, u32, f64)]) -> Vec<NavPoint> {
        points.iter().map(|&(y, m, dd, v)| NavPoint { date: d(y, m, dd), value: v }).collect()
    }

    #[test]
    fn daily_returns_basic() {
        let n = nav(&[(2025, 1, 6, 100.0), (2025, 1, 7, 102.0), (2025, 1, 8, 101.0)]);
        let r = daily_returns(&n);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].date, d(2025, 1, 7));
        assert!((r[0].value - 0.02).abs() < 1e-12);
        assert!((r[1].value - (101.0 / 102.0 - 1.0)).abs() < 1e-12);
    }

    #[test]
    fn daily_returns_empty_and_single() {
        assert!(daily_returns(&[]).is_empty());
        assert!(daily_returns(&nav(&[(2025, 1, 6, 100.0)])).is_empty());
    }
}
