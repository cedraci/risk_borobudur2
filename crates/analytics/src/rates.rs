use chrono::NaiveDate;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct BondMetrics {
    pub ytm: f64,
    pub macaulay: f64,
    pub modified: f64,
}

/// YTM (nominal, compounded `freq` times/yr), Macaulay and modified duration
/// from a clean price per 100 face. Coupons are laid out backwards from
/// maturity every 1/freq years (ACT/365.25 year fractions). Bisection on
/// y in [-0.5, 1.0]. None if maturity <= asof, freq not in {1, 2},
/// price <= 0, coupon < 0, or the price is outside the bracketed range.
pub fn bond_metrics(clean_price: f64, coupon_pct: f64, freq: u32, asof: NaiveDate, maturity: NaiveDate) -> Option<BondMetrics> {
    if !(freq == 1 || freq == 2) || clean_price <= 0.0 || coupon_pct < 0.0 {
        return None;
    }
    let t_mat = (maturity - asof).num_days() as f64 / 365.25;
    if t_mat <= 0.0 { return None; }
    let f = freq as f64;
    let n = (t_mat * f).ceil() as usize;
    let times: Vec<f64> = (0..n).map(|k| t_mat - (n - 1 - k) as f64 / f).collect();
    let cpn = coupon_pct / f; // per-period coupon per 100 face

    let price_at = |y: f64| -> f64 {
        let per = 1.0 + y / f;
        times.iter().enumerate().map(|(k, t)| {
            let cf = if k == n - 1 { cpn + 100.0 } else { cpn };
            cf / per.powf(f * t)
        }).sum()
    };

    let (mut lo, mut hi) = (-0.5f64, 1.0f64);
    if price_at(lo) < clean_price || price_at(hi) > clean_price { return None; }
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if price_at(mid) > clean_price { lo = mid; } else { hi = mid; }
    }
    let y = 0.5 * (lo + hi);
    let per = 1.0 + y / f;
    let p = price_at(y);
    let macaulay = times.iter().enumerate().map(|(k, t)| {
        let cf = if k == n - 1 { cpn + 100.0 } else { cpn };
        t * cf / per.powf(f * t)
    }).sum::<f64>() / p;
    Some(BondMetrics { ytm: y, macaulay, modified: macaulay / per })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, dd: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, dd).unwrap() }

    /// Independent PV for round-trip tests (same schedule convention).
    fn pv(coupon_pct: f64, f: f64, t_mat: f64, y: f64) -> f64 {
        let n = (t_mat * f).ceil() as usize;
        let cpn = coupon_pct / f;
        let per = 1.0 + y / f;
        (0..n).map(|k| {
            let t = t_mat - (n - 1 - k) as f64 / f;
            let cf = if k == n - 1 { cpn + 100.0 } else { cpn };
            cf / per.powf(f * t)
        }).sum()
    }

    #[test]
    fn round_trip_recovers_yield_annual() {
        // ~3y annual 5% bond priced at y = 4%
        let (asof, mat) = (d(2026, 8, 1), d(2029, 8, 1));
        let t_mat = (mat - asof).num_days() as f64 / 365.25;
        let price = pv(5.0, 1.0, t_mat, 0.04);
        let m = bond_metrics(price, 5.0, 1, asof, mat).unwrap();
        assert!((m.ytm - 0.04).abs() < 1e-6);
    }

    #[test]
    fn round_trip_recovers_yield_semiannual() {
        let (asof, mat) = (d(2026, 8, 1), d(2035, 3, 15));
        let t_mat = (mat - asof).num_days() as f64 / 365.25;
        let price = pv(6.625, 2.0, t_mat, 0.07);
        let m = bond_metrics(price, 6.625, 2, asof, mat).unwrap();
        assert!((m.ytm - 0.07).abs() < 1e-6);
    }

    #[test]
    fn par_bond_duration_close_to_textbook() {
        // 3-year 5% annual bond at par: Macaulay ~ 2.859, modified ~ 2.723.
        // Dates give t_mat ~ 2.9952 years, so allow a small tolerance.
        let (asof, mat) = (d(2026, 8, 3), d(2029, 8, 1));
        let m = bond_metrics(100.0, 5.0, 1, asof, mat).unwrap();
        assert!((m.ytm - 0.05).abs() < 5e-3);
        assert!((m.macaulay - 2.859).abs() < 0.02);
        assert!((m.modified - 2.723).abs() < 0.02);
        assert!(m.modified < m.macaulay);
    }

    #[test]
    fn rejects_invalid_inputs() {
        let (asof, mat) = (d(2026, 8, 1), d(2029, 8, 1));
        assert!(bond_metrics(100.0, 5.0, 4, asof, mat).is_none());   // bad freq
        assert!(bond_metrics(0.0, 5.0, 1, asof, mat).is_none());     // bad price
        assert!(bond_metrics(100.0, -1.0, 1, asof, mat).is_none());  // bad coupon
        assert!(bond_metrics(100.0, 5.0, 1, mat, asof).is_none());   // matured
        assert!(bond_metrics(1e-3, 0.0, 1, asof, mat).is_none());    // out of bracket
    }
}
