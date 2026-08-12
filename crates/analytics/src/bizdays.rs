//! Business days, Monday to Friday. No holiday calendar — a deliberate
//! simplification stated in the UI parameters strip.

use chrono::{Datelike, Duration, NaiveDate, Weekday};

pub fn is_business_day(d: NaiveDate) -> bool {
    !matches!(d.weekday(), Weekday::Sat | Weekday::Sun)
}

/// Business days in `(from, to]`. Zero when `to <= from`, so a cash flow
/// dated on or before the snapshot is never credited to a future day.
pub fn business_days_between(from: NaiveDate, to: NaiveDate) -> u32 {
    let mut n = 0;
    let mut d = from;
    while d < to {
        d += Duration::days(1);
        if is_business_day(d) { n += 1; }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, day).unwrap() }

    #[test]
    fn weekends_are_not_business_days() {
        assert!(is_business_day(d(2026, 8, 7)));   // Friday
        assert!(!is_business_day(d(2026, 8, 8)));  // Saturday
        assert!(!is_business_day(d(2026, 8, 9)));  // Sunday
        assert!(is_business_day(d(2026, 8, 10)));  // Monday
    }

    #[test]
    fn offset_skips_the_weekend() {
        // From Friday, the next business day is Monday: offset 1, not 3.
        assert_eq!(business_days_between(d(2026, 8, 7), d(2026, 8, 10)), 1);
        assert_eq!(business_days_between(d(2026, 8, 7), d(2026, 8, 14)), 5);
    }

    #[test]
    fn same_day_and_past_dates_are_zero() {
        assert_eq!(business_days_between(d(2026, 8, 7), d(2026, 8, 7)), 0);
        assert_eq!(business_days_between(d(2026, 8, 7), d(2026, 8, 1)), 0);
    }

    #[test]
    fn the_real_bond_coupon_offset() {
        // Brazil 6.625% 2035 pays 2026-09-15; the sample snapshot is
        // 2026-08-07. Inside the default 60-business-day horizon.
        assert_eq!(business_days_between(d(2026, 8, 7), d(2026, 9, 15)), 27);
    }
}
