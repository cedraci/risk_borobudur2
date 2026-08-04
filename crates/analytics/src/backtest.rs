use crate::{daily_returns, normal_cdf, var_es, NavPoint, VarMethod};
use chrono::NaiveDate;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct BacktestPoint {
    /// Date of the realized return being compared.
    pub date: NaiveDate,
    pub ret: f64,
    pub var_hist: Option<f64>,
    pub var_gauss: Option<f64>,
    pub var_cf: Option<f64>,
    pub exc_hist: bool,
    pub exc_gauss: bool,
    pub exc_cf: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MethodSummary {
    pub exceptions: u32,
    pub n: u32,
    /// "green" (<=4), "yellow" (5-9), "red" (>=10) over trailing min(250, n).
    pub zone: String,
    pub kupiec_lr: Option<f64>,
    pub kupiec_p: Option<f64>,
    pub reject: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct BacktestReport {
    pub points: Vec<BacktestPoint>,
    pub historical: MethodSummary,
    pub gaussian: MethodSummary,
    pub cornish_fisher: MethodSummary,
}

/// Kupiec proportion-of-failures test: LR statistic and chi²(1 df) p-value.
/// None if n == 0, x > n, or p outside (0, 1).
pub fn kupiec_pof(n: u32, x: u32, p: f64) -> Option<(f64, f64)> {
    if n == 0 || x > n || p <= 0.0 || p >= 1.0 { return None; }
    let (nf, xf) = (n as f64, x as f64);
    let ln_null = (nf - xf) * (1.0 - p).ln() + xf * p.ln();
    let phat = xf / nf;
    let ln_alt = if x == 0 {
        0.0
    } else if x == n {
        xf * phat.ln() // = 0; kept explicit to avoid 0 * ln(0) in the general form
    } else {
        (nf - xf) * (1.0 - phat).ln() + xf * phat.ln()
    };
    let lr = (-2.0 * (ln_null - ln_alt)).max(0.0);
    // chi²(1) survival: P(X > lr) = 2 * (1 - Phi(sqrt(lr)))
    let pval = 2.0 * (1.0 - normal_cdf(lr.sqrt()));
    Some((lr, pval))
}

/// Regulatory back-test: for each date with `window` prior returns, 1-day
/// VaR at `confidence` from the trailing window vs that date's realized
/// return. Empty `points` when history is insufficient.
pub fn backtest(nav: &[NavPoint], window: usize, confidence: f64) -> BacktestReport {
    let rets = daily_returns(nav);
    let mut points: Vec<BacktestPoint> = Vec::new();
    if window >= 2 && rets.len() > window {
        for i in window..rets.len() {
            let w: Vec<f64> = rets[i - window..i].iter().map(|p| p.value).collect();
            let r = rets[i].value;
            let vh = var_es(&w, VarMethod::Historical, confidence, 1.0).map(|v| v.var);
            let vg = var_es(&w, VarMethod::Gaussian, confidence, 1.0).map(|v| v.var);
            let vc = var_es(&w, VarMethod::CornishFisher, confidence, 1.0).map(|v| v.var);
            let exc = |v: Option<f64>| v.map(|v| r < -v).unwrap_or(false);
            points.push(BacktestPoint {
                date: rets[i].date,
                ret: r,
                exc_hist: exc(vh), exc_gauss: exc(vg), exc_cf: exc(vc),
                var_hist: vh, var_gauss: vg, var_cf: vc,
            });
        }
    }
    let tail: &[BacktestPoint] = if points.len() > 250 { &points[points.len() - 250..] } else { &points };
    let p_tail = 1.0 - confidence;
    let summarize = |get: fn(&BacktestPoint) -> bool| -> MethodSummary {
        let n = tail.len() as u32;
        let x = tail.iter().filter(|pt| get(pt)).count() as u32;
        let zone = if x <= 4 { "green" } else if x <= 9 { "yellow" } else { "red" };
        let kp = kupiec_pof(n, x, p_tail);
        MethodSummary {
            exceptions: x,
            n,
            zone: zone.into(),
            kupiec_lr: kp.map(|(lr, _)| lr),
            kupiec_p: kp.map(|(_, p)| p),
            reject: kp.map(|(_, p)| p < 0.05).unwrap_or(false),
        }
    };
    let historical = summarize(|p| p.exc_hist);
    let gaussian = summarize(|p| p.exc_gauss);
    let cornish_fisher = summarize(|p| p.exc_cf);
    BacktestReport { points, historical, gaussian, cornish_fisher }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn normal_cdf_known_values() {
        assert!((normal_cdf(0.0) - 0.5).abs() < 1e-7);
        assert!((normal_cdf(1.96) - 0.9750).abs() < 1e-4);
        assert!((normal_cdf(-1.96) - 0.0250).abs() < 1e-4);
    }

    #[test]
    fn kupiec_published_values() {
        // n=250, x=5, p=1%: LR ~ 1.9569, p-value ~ 0.1618
        let (lr, p) = kupiec_pof(250, 5, 0.01).unwrap();
        assert!((lr - 1.9569).abs() < 1e-3);
        assert!((p - 0.1618).abs() < 1e-3);
        // n=250, x=0: LR = -2 * 250 * ln(0.99) ~ 5.0252, p ~ 0.0250 -> reject
        let (lr0, p0) = kupiec_pof(250, 0, 0.01).unwrap();
        assert!((lr0 - 5.0252).abs() < 1e-3);
        assert!((p0 - 0.0250).abs() < 1e-3);
        assert!(kupiec_pof(0, 0, 0.01).is_none());
        assert!(kupiec_pof(10, 11, 0.01).is_none());
    }

    #[test]
    fn counts_engineered_exceptions() {
        // 31 nav points -> 30 returns: +0.001 everywhere except two -5%
        // spikes at return indices 15 and 20; window = 10.
        let mut nav = Vec::new();
        let mut v = 100.0;
        nav.push(NavPoint { date: NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(), value: v });
        for i in 0..30u32 {
            let r = if i == 15 || i == 20 { -0.05 } else { 0.001 };
            v *= 1.0 + r;
            nav.push(NavPoint {
                date: NaiveDate::from_ymd_opt(2025, 1, 1).unwrap() + chrono::Days::new(i as u64 + 1),
                value: v,
            });
        }
        let report = backtest(&nav, 10, 0.99);
        assert_eq!(report.points.len(), 20); // 30 returns - window 10
        assert_eq!(report.historical.exceptions, 2);
        assert_eq!(report.historical.n, 20);
        assert_eq!(report.historical.zone, "green");
        let exc_dates: Vec<NaiveDate> = report.points.iter().filter(|p| p.exc_hist).map(|p| p.date).collect();
        assert_eq!(exc_dates.len(), 2);
        // insufficient history -> empty points, n = 0, no kupiec
        let short = backtest(&nav[..5], 10, 0.99);
        assert!(short.points.is_empty());
        assert_eq!(short.historical.n, 0);
        assert!(short.historical.kupiec_p.is_none());
    }
}
