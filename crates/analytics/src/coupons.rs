//! Bond coupon and redemption inflows, built from the schedule the
//! depositary sends daily in HISINVLUX rather than reconstructed.
//!
//! The frequency divides the coupon, so a wrong guess scales the inflow
//! directly: a semi-annual bond treated as annual pays double. There is no
//! safe default, and the three sources below are tried in order.

use crate::bizdays::business_days_between;
use chrono::{Datelike, NaiveDate};

#[derive(Debug, Clone)]
pub struct CouponInput {
    pub code: String,
    /// Face amount. CACEIS quotes bond prices per 100 of the nominal, so the
    /// coupon is a percentage of this quantity directly.
    pub quantity: f64,
    pub coupon_pct: Option<f64>,
    pub coupon_type: Option<String>,
    pub next_coupon: Option<NaiveDate>,
    pub maturity: Option<NaiveDate>,
    pub freq: Option<i32>,
    pub accrued_eur: Option<f64>,
    pub fx_rate: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Inflow {
    /// Business-day offset from the snapshot date.
    pub day: u32,
    pub amount_eur: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CouponGap {
    pub code: String,
    pub reason: &'static str,
}

#[derive(Debug, Default)]
pub struct CouponResult {
    pub inflows: Vec<Inflow>,
    pub gaps: Vec<CouponGap>,
}

/// Standard coupon periods in days, paired with their frequency.
const PERIODS: [(i32, f64); 4] = [(1, 365.0), (2, 182.5), (4, 91.25), (12, 30.4167)];
const PERIOD_TOLERANCE: f64 = 0.15;

/// Infer the coupon frequency from accrued interest.
///
/// With `C` the annual coupon, `A` the accrued interest in the same currency,
/// and `g` the calendar days to the next coupon, the elapsed accrual is
/// `365 A / C` and the full period is `elapsed + g`. Snapped to the nearest
/// standard period and accepted only within 15% — wide enough to absorb the
/// day-count convention the file does not disclose, narrow enough to reject a
/// period matching nothing.
pub fn infer_coupon_freq(annual_coupon: f64, accrued: f64, days_to_next_coupon: i64) -> Option<i32> {
    if !annual_coupon.is_finite() || annual_coupon <= 0.0 { return None; }
    if !accrued.is_finite() || accrued < 0.0 { return None; }
    if days_to_next_coupon < 0 { return None; }
    let period = 365.0 * accrued / annual_coupon + days_to_next_coupon as f64;
    if !period.is_finite() || period <= 0.0 { return None; }
    PERIODS.iter()
        .map(|&(f, p)| (f, ((period - p) / p).abs()))
        .filter(|&(_, err)| err <= PERIOD_TOLERANCE)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(f, _)| f)
}

/// Add whole months, clamping the day to the target month's length.
fn add_months(d: NaiveDate, months: u32) -> Option<NaiveDate> {
    let total = d.month0() + months;
    let year = d.year() + (total / 12) as i32;
    let month = total % 12 + 1;
    let last = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 { 29 } else { 28 },
    };
    NaiveDate::from_ymd_opt(year, month, d.day().min(last))
}

fn coupon_schedule(b: &CouponInput, snapshot: NaiveDate, horizon: u32, out: &mut CouponResult) {
    let pct = b.coupon_pct.unwrap_or(0.0);
    let is_fixed = b.coupon_type.as_deref().is_some_and(|t| t.eq_ignore_ascii_case("FIX"));
    // NaN fails every `<= 0.0` / `> 0.0` comparison, so without the explicit
    // `is_finite()` checks a NaN coupon_pct or quantity would slip past this
    // guard and produce a NaN-valued inflow. A NaN input is a data-quality
    // signal, not a coupon, so it is treated the same as a zero coupon.
    if !pct.is_finite() || pct <= 0.0 || !is_fixed
        || !(b.quantity.is_finite() && b.quantity > 0.0)
        || !(b.fx_rate.is_finite() && b.fx_rate > 0.0)
    {
        return; // A zero-coupon instrument is not a gap; it simply pays nothing.
    }
    let Some(first) = b.next_coupon else {
        out.gaps.push(CouponGap { code: b.code.clone(), reason: "no next coupon date" });
        return;
    };
    let annual_eur = b.quantity * pct / 100.0 * b.fx_rate;
    let freq = b.freq.filter(|f| *f > 0).or_else(|| {
        infer_coupon_freq(annual_eur, b.accrued_eur?, (first - snapshot).num_days())
    });
    let Some(f) = freq else {
        out.gaps.push(CouponGap { code: b.code.clone(), reason: "no resolvable frequency" });
        return;
    };
    let amount = annual_eur / f as f64;
    let step = (12 / f).max(1) as u32;
    // Each coupon date is computed from the original anchor, not from the
    // previous one. Re-basing off a clamped intermediate (e.g. a step that
    // lands on Apr 30) would let that clamp permanently drag every later
    // date down, even though the clamp was a property of April, not of the
    // schedule.
    let mut n: u32 = 0;
    loop {
        let Some(date) = add_months(first, n * step) else { break; };
        // A past or same-day coupon yields offset 0 and is already in the
        // position; a coupon past the horizon ends the walk.
        let day = business_days_between(snapshot, date);
        if day == 0 || day > horizon { break; }
        if b.maturity.is_some_and(|m| date > m) { break; }
        out.inflows.push(Inflow { day, amount_eur: amount });
        n += 1;
    }
}

pub fn bond_inflows(inputs: &[CouponInput], snapshot: NaiveDate, horizon_days: u32) -> CouponResult {
    let mut out = CouponResult::default();
    for b in inputs {
        coupon_schedule(b, snapshot, horizon_days, &mut out);
        if let Some(m) = b.maturity {
            let day = business_days_between(snapshot, m);
            if day > 0 && day <= horizon_days && b.quantity > 0.0
                && b.fx_rate.is_finite() && b.fx_rate > 0.0
            {
                out.inflows.push(Inflow { day, amount_eur: b.quantity * b.fx_rate });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, day).unwrap() }

    // Brazil 6.625% 15-03-35, from the real HISINVLUX sample: 2,000,000 face,
    // accrued 45,236.41 EUR, next coupon 2026-09-15, snapshot 2026-08-07,
    // market value 1,764,365.78 EUR against 2,038,460 USD local.
    const FX: f64 = 1_764_365.78 / 2_038_460.0;

    fn brazil() -> CouponInput {
        CouponInput {
            code: "US105756CL22".into(),
            quantity: 2_000_000.0,
            coupon_pct: Some(6.625),
            coupon_type: Some("FIX".into()),
            next_coupon: Some(d(2026, 9, 15)),
            maturity: Some(d(2035, 3, 15)),
            freq: None,
            accrued_eur: Some(45_236.41),
            fx_rate: FX,
        }
    }

    #[test]
    fn infers_semi_annual_from_accrued_interest() {
        let annual_eur = 2_000_000.0 * 6.625 / 100.0 * FX;
        assert_eq!(infer_coupon_freq(annual_eur, 45_236.41, 39), Some(2));
    }

    #[test]
    fn inference_survives_a_30_360_accrual() {
        // The day-count convention is not visible in the file. Recomputing the
        // same position on a 30/360 basis must not change the answer.
        let annual_eur = 2_000_000.0 * 6.625 / 100.0 * FX;
        let accrued_30_360 = annual_eur * (142.0 / 360.0);
        assert_eq!(infer_coupon_freq(annual_eur, accrued_30_360, 39), Some(2));
    }

    #[test]
    fn an_out_of_tolerance_accrual_infers_nothing() {
        // An accrual implying a 240-day period matches no standard frequency.
        let annual = 100_000.0;
        assert_eq!(infer_coupon_freq(annual, annual * (200.0 / 365.0), 40), None);
        // Degenerate inputs never guess.
        assert_eq!(infer_coupon_freq(0.0, 100.0, 40), None);
        assert_eq!(infer_coupon_freq(100_000.0, -1.0, 40), None);
    }

    #[test]
    fn credits_one_coupon_at_its_business_day_offset() {
        let r = bond_inflows(&[brazil()], d(2026, 8, 7), 60);
        assert!(r.gaps.is_empty(), "{:?}", r.gaps);
        assert_eq!(r.inflows.len(), 1);
        assert_eq!(r.inflows[0].day, 27);
        // 2,000,000 x 6.625% / 2 = 66,250 USD, converted at the position rate.
        let expected = 2_000_000.0 * 6.625 / 100.0 / 2.0 * FX;
        assert!((r.inflows[0].amount_eur - expected).abs() < 1e-6);
    }

    #[test]
    fn an_explicit_frequency_beats_the_inference() {
        let mut b = brazil();
        b.freq = Some(4);
        let r = bond_inflows(&[b], d(2026, 8, 7), 60);
        let expected = 2_000_000.0 * 6.625 / 100.0 / 4.0 * FX;
        assert!((r.inflows[0].amount_eur - expected).abs() < 1e-6);
    }

    #[test]
    fn an_unresolvable_frequency_credits_nothing_and_reports_why() {
        let mut b = brazil();
        b.accrued_eur = None;
        let r = bond_inflows(&[b], d(2026, 8, 7), 60);
        assert!(r.inflows.is_empty());
        assert_eq!(r.gaps.len(), 1);
        assert_eq!(r.gaps[0].reason, "no resolvable frequency");
    }

    #[test]
    fn a_missing_next_coupon_date_is_reported_not_reconstructed() {
        let mut b = brazil();
        b.next_coupon = None;
        let r = bond_inflows(&[b], d(2026, 8, 7), 60);
        assert!(r.inflows.is_empty());
        assert_eq!(r.gaps[0].reason, "no next coupon date");
    }

    #[test]
    fn zero_coupon_and_far_placeholder_maturity_contribute_nothing() {
        // The sample's ETCs: 0.00% coupon, CACEIS placeholder maturity 2049-12-31.
        let etc = CouponInput {
            code: "GB00B00FHZ82".into(),
            quantity: 1_000.0,
            coupon_pct: Some(0.0),
            coupon_type: Some("FIX".into()),
            next_coupon: Some(d(2049, 12, 31)),
            maturity: Some(d(2049, 12, 31)),
            freq: None,
            accrued_eur: Some(0.0),
            fx_rate: 1.0,
        };
        let r = bond_inflows(&[etc], d(2026, 8, 7), 60);
        assert!(r.inflows.is_empty());
        assert!(r.gaps.is_empty(), "a zero-coupon instrument is not a gap");
    }

    #[test]
    fn a_maturity_inside_the_horizon_redeems_the_face() {
        let b = CouponInput {
            code: "XS0000000001".into(),
            quantity: 500_000.0,
            coupon_pct: Some(0.0),
            coupon_type: Some("FIX".into()),
            next_coupon: None,
            maturity: Some(d(2026, 8, 21)),
            freq: Some(1),
            accrued_eur: Some(0.0),
            fx_rate: 1.0,
        };
        let r = bond_inflows(&[b], d(2026, 8, 7), 60);
        assert_eq!(r.inflows.len(), 1);
        assert_eq!(r.inflows[0].day, 10);
        assert!((r.inflows[0].amount_eur - 500_000.0).abs() < 1e-9);
    }

    #[test]
    fn a_monthly_payer_fits_several_coupons_in_the_horizon() {
        let b = CouponInput {
            code: "XS0000000002".into(),
            quantity: 1_200_000.0,
            coupon_pct: Some(12.0),
            coupon_type: Some("FIX".into()),
            next_coupon: Some(d(2026, 8, 20)),
            maturity: Some(d(2030, 8, 20)),
            freq: Some(12),
            accrued_eur: Some(0.0),
            fx_rate: 1.0,
        };
        let r = bond_inflows(&[b], d(2026, 8, 7), 60);
        // 2026-08-20, 2026-09-20 and 2026-10-20 all land inside 60 business days.
        assert_eq!(r.inflows.len(), 3);
        let each = 1_200_000.0 * 12.0 / 100.0 / 12.0;
        assert!(r.inflows.iter().all(|i| (i.amount_eur - each).abs() < 1e-9));
    }

    #[test]
    fn a_quarterly_coupon_anchored_on_a_month_end_does_not_drift() {
        // Anchored Jan 31, quarterly: the walk must land on Apr 30 (a real
        // clamp, since April has 30 days) and then recover to Jul 31 (no
        // clamp on July) rather than dragging the April clamp forward to
        // land on Jul 30.
        let snapshot = d(2026, 1, 1);
        let third_coupon = d(2026, 7, 31);
        let horizon = business_days_between(snapshot, third_coupon);
        let b = CouponInput {
            code: "XS0000000003".into(),
            quantity: 1_000_000.0,
            coupon_pct: Some(8.0),
            coupon_type: Some("FIX".into()),
            next_coupon: Some(d(2026, 1, 31)),
            maturity: Some(d(2030, 1, 31)),
            freq: Some(4),
            accrued_eur: Some(0.0),
            fx_rate: 1.0,
        };
        let r = bond_inflows(&[b], snapshot, horizon);
        assert_eq!(r.inflows.len(), 3, "{:?}", r.inflows);
        // The horizon was set to exactly the correct third coupon's offset,
        // so the third inflow must land there.
        assert_eq!(r.inflows[2].day, horizon);
        // A drifted walk would instead land one business day earlier, on
        // 2026-07-30.
        let wrong = business_days_between(snapshot, d(2026, 7, 30));
        assert_ne!(r.inflows[2].day, wrong);
    }

    #[test]
    fn a_coupon_and_a_maturity_in_the_same_horizon_both_land() {
        // Interest and principal are not double counting: a coupon inside
        // the horizon and a maturity redemption inside the horizon are two
        // distinct inflows for the same bond.
        let snapshot = d(2026, 8, 7);
        let b = CouponInput {
            code: "XS0000000004".into(),
            quantity: 1_000_000.0,
            coupon_pct: Some(6.0),
            coupon_type: Some("FIX".into()),
            next_coupon: Some(d(2026, 8, 20)),
            maturity: Some(d(2026, 9, 15)),
            freq: Some(2),
            accrued_eur: Some(0.0),
            fx_rate: 1.0,
        };
        let r = bond_inflows(&[b], snapshot, 60);
        assert_eq!(r.inflows.len(), 2, "{:?}", r.inflows);
        let coupon_day = business_days_between(snapshot, d(2026, 8, 20));
        let maturity_day = business_days_between(snapshot, d(2026, 9, 15));
        let coupon = r.inflows.iter().find(|i| i.day == coupon_day).expect("coupon inflow");
        let redemption = r.inflows.iter().find(|i| i.day == maturity_day).expect("redemption inflow");
        let expected_coupon = 1_000_000.0 * 6.0 / 100.0 / 2.0;
        assert!((coupon.amount_eur - expected_coupon).abs() < 1e-9);
        assert!((redemption.amount_eur - 1_000_000.0).abs() < 1e-9);
    }
}
