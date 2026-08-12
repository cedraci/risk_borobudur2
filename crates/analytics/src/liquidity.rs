use serde::Serialize;

pub const BUCKET_ORDER: [&str; 4] = ["d1", "d2_7", "d8_30", "d30p"];

/// Index into `BUCKET_ORDER` for a days figure. Bands are closed at the
/// upper edge: 1 day is `d1`, 7 days is `d2_7`, 30 days is `d8_30`.
pub fn band_of_days(days: f64) -> usize {
    if days <= 1.0 { 0 } else if days <= 7.0 { 1 } else if days <= 30.0 { 2 } else { 3 }
}

#[derive(Debug, Clone)]
pub struct LiqPosition {
    pub weight: f64,
    pub bucket: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BucketWeight { pub bucket: String, pub weight: f64 }

#[derive(Debug, Clone, Serialize)]
pub struct LiquidityReport {
    pub buckets: Vec<BucketWeight>,
    pub cumulative: Vec<BucketWeight>,
    /// Sum of negative weights (payables, negative cash) — reported, not netted.
    pub negative_memo: f64,
    /// True when assets liquidatable in <= 7 days (d1 + d2_7) cover `shock`.
    pub stress_ok: bool,
}

/// Aggregate long weights per bucket. Unknown bucket names count as d30p
/// (conservative).
pub fn liquidity(rows: &[LiqPosition], shock: f64) -> LiquidityReport {
    let mut sums = [0.0f64; 4];
    let mut neg = 0.0;
    for r in rows {
        if r.weight < 0.0 { neg += r.weight; continue; }
        let idx = BUCKET_ORDER.iter().position(|b| *b == r.bucket).unwrap_or(3);
        sums[idx] += r.weight;
    }
    let buckets: Vec<BucketWeight> = BUCKET_ORDER.iter().zip(sums)
        .map(|(b, w)| BucketWeight { bucket: (*b).into(), weight: w })
        .collect();
    let mut acc = 0.0;
    let cumulative: Vec<BucketWeight> = buckets.iter()
        .map(|b| { acc += b.weight; BucketWeight { bucket: b.bucket.clone(), weight: acc } })
        .collect();
    let stress_ok = cumulative[1].weight >= shock;
    LiquidityReport { buckets, cumulative, negative_memo: neg, stress_ok }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lp(w: f64, b: &str) -> LiqPosition { LiqPosition { weight: w, bucket: b.into() } }

    #[test]
    fn buckets_cumulative_and_stress() {
        let rows = vec![
            lp(0.40, "d1"), lp(0.10, "d1"),
            lp(0.25, "d2_7"),
            lp(0.15, "d8_30"),
            lp(0.05, "d30p"),
            lp(0.02, "bogus"),   // unknown -> d30p
            lp(-0.03, "d1"),     // negative -> memo only
        ];
        let r = liquidity(&rows, 0.30);
        assert!((r.buckets[0].weight - 0.50).abs() < 1e-12);
        assert!((r.buckets[1].weight - 0.25).abs() < 1e-12);
        assert!((r.buckets[2].weight - 0.15).abs() < 1e-12);
        assert!((r.buckets[3].weight - 0.07).abs() < 1e-12);
        assert!((r.cumulative[3].weight - 0.97).abs() < 1e-12);
        assert!((r.negative_memo - (-0.03)).abs() < 1e-12);
        assert!(r.stress_ok); // 0.75 >= 0.30
        assert!(!liquidity(&rows, 0.80).stress_ok); // 0.75 < 0.80
    }

    #[test]
    fn empty_is_all_zero() {
        let r = liquidity(&[], 0.30);
        assert!(r.buckets.iter().all(|b| b.weight == 0.0));
        assert!(!r.stress_ok);
        assert!(liquidity(&[], 0.0).stress_ok);
    }
}
