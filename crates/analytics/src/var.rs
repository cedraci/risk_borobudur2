use crate::{excess_kurtosis, mean, rolling, sample_std, skewness, NavPoint};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VarMethod {
    Historical,
    Gaussian,
    CornishFisher,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct VarEs {
    /// Positive = loss, decimal fraction of NAV, horizon-scaled.
    pub var: f64,
    pub es: f64,
}

/// 1-day VaR/ES from daily returns, scaled by sqrt(horizon_days).
/// Needs n>=2, confidence in (0.5, 1), horizon >= 1.
pub fn var_es(returns: &[f64], method: VarMethod, confidence: f64, horizon_days: f64) -> Option<VarEs> {
    if returns.len() < 2 || confidence <= 0.5 || confidence >= 1.0 || horizon_days < 1.0 {
        return None;
    }
    let (var1, es1) = match method {
        VarMethod::Historical => historical_var_es(returns, confidence)?,
        VarMethod::Gaussian => {
            gaussian_var_es(mean(returns)?, sample_std(returns)?, confidence)?
        }
        VarMethod::CornishFisher => {
            let s = skewness(returns)?;
            let k = excess_kurtosis(returns)?;
            let r = var_es_with_moments(returns, confidence, 1.0, s, k)?;
            (r.var, r.es)
        }
    };
    let scale = horizon_days.sqrt();
    Some(VarEs { var: var1 * scale, es: es1 * scale })
}

/// Empirical quantile with linear interpolation at index p*(n-1) on the
/// ascending-sorted returns; ES = mean of the worst ceil(p*n) observations.
fn historical_var_es(returns: &[f64], confidence: f64) -> Option<(f64, f64)> {
    let mut sorted = returns.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let n = sorted.len();
    let p = 1.0 - confidence;
    let idx = p * (n - 1) as f64;
    let lo = idx.floor() as usize;
    let hi = idx.ceil() as usize;
    let q = sorted[lo] + (idx - lo as f64) * (sorted[hi] - sorted[lo]);
    let tail_n = ((p * n as f64).ceil() as usize).clamp(1, n);
    let es = -(sorted[..tail_n].iter().sum::<f64>() / tail_n as f64);
    Some((-q, es))
}

/// Analytic Gaussian 1-day (VaR, ES): (z_c*sigma - mu, sigma*phi(z_c)/(1-c) - mu).
pub fn gaussian_var_es(mu: f64, sigma: f64, confidence: f64) -> Option<(f64, f64)> {
    if sigma <= 0.0 { return None; }
    let z = inverse_normal_cdf(confidence);
    let var = z * sigma - mu;
    let es = sigma * normal_pdf(z) / (1.0 - confidence) - mu;
    Some((var, es))
}

/// Cornish-Fisher VaR/ES with explicit skew/kurtosis (exposed for tests).
/// ES via 200-step midpoint integration of the CF quantile over the tail.
pub fn var_es_with_moments(returns: &[f64], confidence: f64, horizon_days: f64, s: f64, k: f64) -> Option<VarEs> {
    let mu = mean(returns)?;
    let sd = sample_std(returns)?;
    if sd <= 0.0 { return None; }
    let z = inverse_normal_cdf(1.0 - confidence); // negative tail z
    let var1 = -(mu + sd * cornish_fisher_z(z, s, k));
    let p_tail = 1.0 - confidence;
    const STEPS: usize = 200;
    let mut acc = 0.0;
    for i in 0..STEPS {
        let p = p_tail * (i as f64 + 0.5) / STEPS as f64;
        acc += mu + sd * cornish_fisher_z(inverse_normal_cdf(p), s, k);
    }
    let es1 = -(acc / STEPS as f64);
    let scale = horizon_days.sqrt();
    Some(VarEs { var: var1 * scale, es: es1 * scale })
}

fn cornish_fisher_z(z: f64, s: f64, k: f64) -> f64 {
    z + (z * z - 1.0) * s / 6.0 + (z * z * z - 3.0 * z) * k / 24.0
        - (2.0 * z * z * z - 5.0 * z) * s * s / 36.0
}

fn normal_pdf(z: f64) -> f64 {
    (-0.5 * z * z).exp() / (2.0 * std::f64::consts::PI).sqrt()
}

/// Acklam's inverse normal CDF approximation, |relative error| < 1.15e-9.
/// Reference: https://web.archive.org/web/20151110174102/http://home.online.no/~pjacklam/notes/invnorm/
pub fn inverse_normal_cdf(p: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969683028665376e+01, 2.209460984245205e+02, -2.759285104469687e+02,
        1.383577518672690e+02, -3.066479806614716e+01, 2.506628277459239e+00,
    ];
    const B: [f64; 5] = [
        -5.447609879822406e+01, 1.615858368580409e+02, -1.556989798598866e+02,
        6.680131188771972e+01, -1.328068155288572e+01,
    ];
    const C: [f64; 6] = [
        -7.784894002430293e-03, -3.223964580411365e-01, -2.400758277161838e+00,
        -2.549732539343734e+00, 4.374664141464968e+00, 2.938163982698783e+00,
    ];
    const D: [f64; 4] = [
        7.784695709041462e-03, 3.224671290700398e-01, 2.445134137142996e+00,
        3.754408661907416e+00,
    ];
    const P_LOW: f64 = 0.02425;
    if !(0.0..=1.0).contains(&p) || p == 0.0 || p == 1.0 {
        return f64::NAN;
    }
    if p < P_LOW {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p <= 1.0 - P_LOW {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        -((((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0))
    }
}

/// Standard normal CDF via the Abramowitz–Stegun 7.1.26 erf approximation
/// (|error| < 1.5e-7).
pub fn normal_cdf(z: f64) -> f64 {
    let x = z / std::f64::consts::SQRT_2;
    let (sign, x) = if x < 0.0 { (-1.0, -x) } else { (1.0, x) };
    let t = 1.0 / (1.0 + 0.3275911 * x);
    let y = 1.0
        - (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736) * t
            + 0.254829592)
            * t
            * (-x * x).exp();
    0.5 * (1.0 + sign * y)
}

/// Rolling VaR over trailing `window` daily returns; value = horizon-scaled VaR.
pub fn rolling_var(nav: &[NavPoint], window: usize, method: VarMethod, confidence: f64, horizon_days: f64) -> Vec<NavPoint> {
    rolling(nav, window, move |r| var_es(r, method, confidence, horizon_days).map(|v| v.var))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NavPoint;
    use chrono::NaiveDate;

    const RETS: [f64; 10] = [-0.05, -0.03, -0.02, -0.01, 0.0, 0.01, 0.02, 0.03, 0.04, 0.06];

    #[test]
    fn inverse_normal_cdf_known_values() {
        assert!(inverse_normal_cdf(0.5).abs() < 1e-9);
        assert!((inverse_normal_cdf(0.975) - 1.959964).abs() < 1e-5);
        assert!((inverse_normal_cdf(0.99) - 2.326348).abs() < 1e-5);
        assert!((inverse_normal_cdf(0.01) + 2.326348).abs() < 1e-5);
        assert!(inverse_normal_cdf(0.0).is_nan());
    }

    #[test]
    fn historical_var_es() {
        // p=0.1, idx=0.9 -> quantile between -0.05 and -0.03 = -0.032
        let v = var_es(&RETS, VarMethod::Historical, 0.90, 1.0).unwrap();
        assert!((v.var - 0.032).abs() < 1e-12);
        // ES = mean of worst ceil(0.1*10)=1 obs = 0.05
        assert!((v.es - 0.05).abs() < 1e-12);
        // horizon scaling sqrt(4)=2
        let v4 = var_es(&RETS, VarMethod::Historical, 0.90, 4.0).unwrap();
        assert!((v4.var - 0.064).abs() < 1e-12);
        assert!((v4.es - 0.10).abs() < 1e-12);
    }

    #[test]
    fn gaussian_var_es_from_moments() {
        // mu=0, sigma=0.01 via symmetric two-point sample is invalid (std uses n-1);
        // instead test the internal on a crafted sample: use returns with known
        // sample stats: [-0.01, 0.01] -> mean 0, sample std = sqrt(2e-4/1)=0.0141421...
        // Simpler: call the moment-level helper directly.
        let g = gaussian_var_es(0.0, 0.01, 0.99).unwrap();
        assert!((g.0 - 0.023263479).abs() < 1e-6); // z_.99 * sigma - mu
        assert!((g.1 - 0.026652142).abs() < 1e-5); // sigma * phi(z)/(1-c) - mu
    }

    #[test]
    fn cornish_fisher_reduces_to_gaussian_when_normal() {
        // symmetric sample -> skew 0; CF with s=0,k=0 must match Gaussian closely
        // (CF ES uses numerical tail integration; allow 1e-3 relative slack)
        let sample = [-0.02, -0.01, 0.0, 0.01, 0.02];
        let g = var_es(&sample, VarMethod::Gaussian, 0.99, 1.0).unwrap();
        let cf = var_es_with_moments(&sample, 0.99, 1.0, 0.0, 0.0).unwrap();
        assert!((g.var - cf.var).abs() < 1e-9);
        assert!((g.es - cf.es).abs() / g.es < 2e-3);
    }

    #[test]
    fn rolling_var_mechanics() {
        let mut nav = Vec::new();
        let mut v = 100.0;
        for i in 0..40u32 {
            v *= if i % 2 == 0 { 1.01 } else { 0.995 };
            nav.push(NavPoint {
                date: NaiveDate::from_ymd_opt(2025, 1, 1).unwrap() + chrono::Days::new(i as u64),
                value: v,
            });
        }
        let out = rolling_var(&nav, 20, VarMethod::Historical, 0.99, 1.0);
        assert_eq!(out.len(), nav.len() - 1 - 20 + 1); // returns n-1, windows
        assert!(out.iter().all(|p| p.value.is_finite()));
    }
}
