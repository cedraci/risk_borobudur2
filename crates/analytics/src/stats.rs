pub fn mean(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() { return None; }
    Some(xs.iter().sum::<f64>() / xs.len() as f64)
}

/// Sample standard deviation (n-1 denominator). None if fewer than 2 values.
pub fn sample_std(xs: &[f64]) -> Option<f64> {
    if xs.len() < 2 { return None; }
    let m = mean(xs)?;
    let var = xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (xs.len() - 1) as f64;
    Some(var.sqrt())
}

/// Population skewness m3 / m2^1.5. None if n<2 or zero variance.
pub fn skewness(xs: &[f64]) -> Option<f64> {
    let (m2, m3, _) = central_moments(xs)?;
    if m2 <= 0.0 { return None; }
    Some(m3 / m2.powf(1.5))
}

/// Population excess kurtosis m4 / m2^2 - 3. None if n<2 or zero variance.
pub fn excess_kurtosis(xs: &[f64]) -> Option<f64> {
    let (m2, _, m4) = central_moments(xs)?;
    if m2 <= 0.0 { return None; }
    Some(m4 / (m2 * m2) - 3.0)
}

fn central_moments(xs: &[f64]) -> Option<(f64, f64, f64)> {
    if xs.len() < 2 { return None; }
    let m = mean(xs)?;
    let n = xs.len() as f64;
    let (mut m2, mut m3, mut m4) = (0.0, 0.0, 0.0);
    for x in xs {
        let d = x - m;
        m2 += d * d;
        m3 += d * d * d;
        m4 += d * d * d * d;
    }
    Some((m2 / n, m3 / n, m4 / n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mean_and_std() {
        assert_eq!(mean(&[]), None);
        assert!((mean(&[1.0, 2.0, 3.0]).unwrap() - 2.0).abs() < 1e-12);
        assert_eq!(sample_std(&[1.0]), None);
        // std of [0.1, -0.1]: mean 0, var = 0.02/(2-1), std = sqrt(0.02)
        assert!((sample_std(&[0.1, -0.1]).unwrap() - 0.02f64.sqrt()).abs() < 1e-12);
    }

    #[test]
    fn skew_kurt_known_values() {
        let sym = [-0.02, -0.01, 0.0, 0.01, 0.02];
        assert!(skewness(&sym).unwrap().abs() < 1e-12);
        // m2=2e-4, m4=6.8e-8 -> kurt=1.7 -> excess=-1.3
        assert!((excess_kurtosis(&sym).unwrap() - (-1.3)).abs() < 1e-9);
        assert_eq!(skewness(&[1.0]), None);
    }
}
