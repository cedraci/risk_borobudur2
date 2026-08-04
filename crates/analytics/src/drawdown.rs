use crate::NavPoint;
use chrono::{Datelike, NaiveDate};

#[derive(Debug, Clone, serde::Serialize)]
pub struct YearlyDrawdown {
    pub year: i32,
    pub max_drawdown: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DrawdownEpisode {
    pub peak_date: NaiveDate,
    pub trough_date: NaiveDate,
    pub depth: f64,
    pub duration_days: i64,
    pub recovery_date: Option<NaiveDate>,
}

/// NAV_t / running_peak - 1 (values <= 0).
pub fn drawdown_series(nav: &[NavPoint]) -> Vec<NavPoint> {
    let mut peak = f64::NEG_INFINITY;
    nav.iter()
        .map(|p| {
            peak = peak.max(p.value);
            NavPoint { date: p.date, value: p.value / peak - 1.0 }
        })
        .collect()
}

/// Deepest drawdown per calendar year; the running peak RESETS at each
/// year start (per spec). Years in ascending order.
pub fn yearly_max_drawdowns(nav: &[NavPoint]) -> Vec<YearlyDrawdown> {
    let mut out: Vec<YearlyDrawdown> = Vec::new();
    let mut peak = f64::NEG_INFINITY;
    for p in nav {
        let year = p.date.year();
        if out.last().map(|y| y.year) != Some(year) {
            out.push(YearlyDrawdown { year, max_drawdown: 0.0 });
            peak = p.value;
        }
        peak = peak.max(p.value);
        let dd = p.value / peak - 1.0;
        let cur = out.last_mut().unwrap();
        if dd < cur.max_drawdown { cur.max_drawdown = dd; }
    }
    out
}

/// Distinct peak->trough episodes. An episode opens when NAV drops below the
/// running peak and closes at the first NAV >= that peak (recovery) or at
/// series end (recovery_date = None).
pub fn drawdown_episodes(nav: &[NavPoint]) -> Vec<DrawdownEpisode> {
    let mut episodes = Vec::new();
    let Some(first) = nav.first() else { return episodes; };
    let mut peak = first.clone();
    let mut trough: Option<NavPoint> = None;
    for p in &nav[1..] {
        if p.value >= peak.value {
            if let Some(t) = trough.take() {
                episodes.push(make_episode(&peak, &t, Some(p.date)));
            }
            peak = p.clone();
        } else if trough.as_ref().is_none_or(|t| p.value < t.value) {
            trough = Some(p.clone());
        }
    }
    if let Some(t) = trough {
        episodes.push(make_episode(&peak, &t, None));
    }
    episodes
}

fn make_episode(peak: &NavPoint, trough: &NavPoint, recovery: Option<NaiveDate>) -> DrawdownEpisode {
    DrawdownEpisode {
        peak_date: peak.date,
        trough_date: trough.date,
        depth: trough.value / peak.value - 1.0,
        duration_days: (trough.date - peak.date).num_days(),
        recovery_date: recovery,
    }
}

/// Episodes with peak->trough duration <= max_calendar_days, deepest first.
pub fn top_short_drawdowns(nav: &[NavPoint], max_calendar_days: i64, top_n: usize) -> Vec<DrawdownEpisode> {
    let mut eps: Vec<DrawdownEpisode> = drawdown_episodes(nav)
        .into_iter()
        .filter(|e| e.duration_days <= max_calendar_days && e.duration_days >= 1)
        .collect();
    eps.sort_by(|a, b| a.depth.total_cmp(&b.depth));
    eps.truncate(top_n);
    eps
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

    // 100,110,99,105,120,108 on Jan 1..6
    fn fixture() -> Vec<NavPoint> {
        nav(&[
            (2025, 1, 1, 100.0), (2025, 1, 2, 110.0), (2025, 1, 3, 99.0),
            (2025, 1, 4, 105.0), (2025, 1, 5, 120.0), (2025, 1, 6, 108.0),
        ])
    }

    #[test]
    fn underwater_series() {
        let dd = drawdown_series(&fixture());
        let vals: Vec<f64> = dd.iter().map(|p| p.value).collect();
        let expect = [0.0, 0.0, -0.1, 105.0 / 110.0 - 1.0, 0.0, -0.1];
        for (v, e) in vals.iter().zip(expect) { assert!((v - e).abs() < 1e-12); }
        assert_eq!(dd[2].date, d(2025, 1, 3));
    }

    #[test]
    fn episodes_detected() {
        let eps = drawdown_episodes(&fixture());
        assert_eq!(eps.len(), 2);
        assert_eq!(eps[0].peak_date, d(2025, 1, 2));
        assert_eq!(eps[0].trough_date, d(2025, 1, 3));
        assert!((eps[0].depth - (-0.1)).abs() < 1e-12);
        assert_eq!(eps[0].duration_days, 1);
        assert_eq!(eps[0].recovery_date, Some(d(2025, 1, 5)));
        assert_eq!(eps[1].peak_date, d(2025, 1, 5));
        assert_eq!(eps[1].recovery_date, None); // ongoing
    }

    #[test]
    fn top_short_filters_and_ranks() {
        let eps = top_short_drawdowns(&fixture(), 50, 5);
        assert_eq!(eps.len(), 2);
        // both -10%; deeper-or-equal first, stable by date
        assert!(eps[0].depth <= eps[1].depth);
        // duration filter: max 0 days excludes both
        assert!(top_short_drawdowns(&fixture(), 0, 5).is_empty());
    }

    #[test]
    fn yearly_max_resets_peak_at_year_start() {
        let n = nav(&[
            (2024, 12, 30, 100.0), (2024, 12, 31, 90.0),
            (2025, 1, 2, 95.0), (2025, 1, 3, 85.0),
        ]);
        let y = yearly_max_drawdowns(&n);
        assert_eq!(y.len(), 2);
        assert_eq!(y[0].year, 2024);
        assert!((y[0].max_drawdown - (-0.10)).abs() < 1e-12);
        assert_eq!(y[1].year, 2025);
        // peak resets to 95 in 2025 -> 85/95-1, NOT 85/100-1
        assert!((y[1].max_drawdown - (85.0 / 95.0 - 1.0)).abs() < 1e-12);
    }
}
