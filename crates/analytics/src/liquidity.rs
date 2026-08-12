//! Days-to-liquidate. Every function here is pure and takes no database.
//!
//! Days are the primitive: bucket bands exist only as a chart axis. A
//! position's capacity per day comes from Bloomberg 30-day volume where the
//! instrument is exchange-traded and measurable, and from an assumed days
//! figure everywhere else. Both paths produce the same two numbers, so one
//! arithmetic serves the whole portfolio regardless of data quality.

use crate::coupons::Inflow;
use serde::Serialize;

pub const BUCKET_ORDER: [&str; 4] = ["d1", "d2_7", "d8_30", "d30p"];

/// CACEIS market-place codes that are not a trading venue: `FOR` is a forced
/// price (futures, cash, provisions), `260` an unlisted collective investment
/// undertaking, `999` an internal funds publication and `254` a technical
/// quotation place. Every other code in the sample is a real exchange.
pub const NON_MARKET_CODES: [&str; 4] = ["FOR", "260", "999", "254"];

/// Index into `BUCKET_ORDER`. Bands close at their upper edge: 1 day is
/// `d1`, 7 days is `d2_7`, 30 days is `d8_30`.
pub fn band_of_days(days: f64) -> usize {
    if days <= 1.0 { 0 } else if days <= 7.0 { 1 } else if days <= 30.0 { 2 } else { 3 }
}

#[derive(Debug, Clone, Serialize)]
pub struct BucketWeight { pub bucket: String, pub weight: f64 }

#[derive(Debug, Clone)]
pub struct LiqPosition {
    pub code: String,
    pub asset_type: String,
    pub valuation_eur: f64,
    pub quantity: Option<f64>,
    pub adv_30d: Option<f64>,
    /// `adv_asof` is older than `adv_max_age_days`.
    pub adv_stale: bool,
    /// User override of the venue rule. `None` = derive.
    pub adv_eligible: Option<bool>,
    pub market_place: Option<String>,
    /// Per-instrument days override.
    pub liquidity_days: Option<f64>,
    /// Asset-type default, already resolved from settings.
    pub default_days: f64,
}

fn is_cash(asset_type: &str) -> bool {
    matches!(asset_type, "Cash Acc" | "Margin Acc")
}

/// Whether Bloomberg volume is meaningful for this instrument.
///
/// Futures are excluded unconditionally, ahead of the override: a margined
/// contract's `valuation_eur` is its mark-to-market rather than its notional,
/// so `valuation_eur / quantity` is not a price that volume can be measured
/// against. That is a structural fact, not a data-quality judgement, so the
/// override does not reach it.
pub fn adv_eligible(p: &LiqPosition) -> bool {
    if p.asset_type == "Future" || is_cash(&p.asset_type) { return false; }
    if let Some(forced) = p.adv_eligible { return forced; }
    match p.market_place.as_deref() {
        // No venue data (a NAV Recap portfolio): the pre-v2 asset-type rule.
        None => p.asset_type == "Action",
        Some(m) => !NON_MARKET_CODES.contains(&m),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Capacity {
    pub code: String,
    pub valuation_eur: f64,
    /// `None` = infinite: cash and margin accounts, and any position whose
    /// resolved days figure is zero.
    pub capacity_eur_day: Option<f64>,
    pub days: f64,
    pub measured: bool,
    /// Why this position is on the fallback path. `None` when measured, and
    /// also `None` for cash, which is not a fallback but a rule.
    pub reason: Option<&'static str>,
}

/// Why an eligible-looking position still cannot be measured.
fn fallback_reason(p: &LiqPosition) -> Option<&'static str> {
    if p.asset_type == "Future" { return Some("future"); }
    if !adv_eligible(p) { return Some("not eligible"); }
    if p.adv_stale { return Some("stale adv"); }
    if !p.adv_30d.is_some_and(|a| a.is_finite() && a > 0.0) { return Some("no adv"); }
    if !p.quantity.is_some_and(|q| q.is_finite() && q > 0.0) { return Some("no quantity"); }
    None
}

/// Days-to-liquidate and euros-per-day for one position.
///
/// Only meaningful for `valuation_eur > 0`; negative positions are an
/// immediate cash need rather than something to be sold, and are handled as a
/// separate term in `available`.
pub fn capacity(p: &LiqPosition, participation: f64, stress: f64) -> Capacity {
    let infinite = |reason| Capacity {
        code: p.code.clone(), valuation_eur: p.valuation_eur,
        capacity_eur_day: None, days: 0.0, measured: false, reason,
    };
    if is_cash(&p.asset_type) { return infinite(None); }

    match fallback_reason(p) {
        None => {
            // Both non-None and positive, checked above.
            let unit_price = p.valuation_eur / p.quantity.unwrap();
            let cap = p.adv_30d.unwrap() * participation * stress * unit_price;
            if cap.is_finite() && cap > 0.0 {
                return Capacity {
                    code: p.code.clone(), valuation_eur: p.valuation_eur,
                    capacity_eur_day: Some(cap), days: p.valuation_eur / cap,
                    measured: true, reason: None,
                };
            }
            // A non-positive capacity from positive inputs means the
            // participation or stress setting is degenerate. Report it rather
            // than dividing by it.
            fallback(p, Some("no adv"))
        }
        Some(reason) => fallback(p, Some(reason)),
    }
}

fn fallback(p: &LiqPosition, reason: Option<&'static str>) -> Capacity {
    let days = p.liquidity_days.filter(|d| d.is_finite() && *d >= 0.0).unwrap_or(p.default_days);
    if !(days.is_finite() && days > 0.0) {
        // Zero assumed days is same-day liquidity, i.e. infinite capacity.
        return Capacity {
            code: p.code.clone(), valuation_eur: p.valuation_eur,
            capacity_eur_day: None, days: 0.0, measured: false, reason,
        };
    }
    Capacity {
        code: p.code.clone(), valuation_eur: p.valuation_eur,
        // Chosen so that `days` comes back out exactly: the two paths agree
        // by construction rather than by coincidence.
        capacity_eur_day: Some(p.valuation_eur / days),
        days, measured: false, reason,
    }
}

/// Cumulative euros available by business day `d`.
///
/// `negatives_eur` is the sum of negative position values (payables, negative
/// cash) and is expected to be <= 0. It applies from day 1: those are an
/// immediate call on liquidity, not a memo.
pub fn available(caps: &[Capacity], inflows: &[Inflow], negatives_eur: f64, d: u32) -> f64 {
    let sellable: f64 = caps.iter().map(|c| match c.capacity_eur_day {
        None => c.valuation_eur,
        Some(cap) => c.valuation_eur.min(cap * d as f64),
    }).sum();
    let inflow: f64 = inflows.iter().filter(|i| i.day <= d).map(|i| i.amount_eur).sum();
    sellable + inflow + negatives_eur
}

#[derive(Debug, Clone, Serialize)]
pub struct Waterfall {
    /// First business day on which `available` reaches the requirement.
    /// `None` when the horizon is never enough.
    pub days: Option<u32>,
    pub unmet_eur: f64,
}

/// Sell the liquid names hardest: the fastest the money could arrive.
pub fn waterfall(
    caps: &[Capacity], inflows: &[Inflow], negatives_eur: f64,
    required: f64, horizon: u32,
) -> Waterfall {
    for d in 1..=horizon {
        if available(caps, inflows, negatives_eur, d) >= required {
            return Waterfall { days: Some(d), unmet_eur: 0.0 };
        }
    }
    let short = required - available(caps, inflows, negatives_eur, horizon);
    Waterfall { days: None, unmet_eur: short.max(0.0) }
}

/// Every position contributes its own proportion, so composition is
/// unchanged. Always the slower of the two orderings, and deliberately blind
/// to inflows so it stays a pure property of the holdings.
pub fn slice_days(caps: &[Capacity], required: f64, nav: f64) -> Option<f64> {
    if !(nav.is_finite() && nav > 0.0) { return None; }
    let f = required / nav;
    let mut worst: Option<f64> = None;
    for c in caps.iter().filter(|c| c.valuation_eur > 0.0) {
        let d = match c.capacity_eur_day {
            None => 0.0,
            Some(cap) => f * c.valuation_eur / cap,
        };
        worst = Some(worst.map_or(d, |w: f64| w.max(d)));
    }
    worst
}

#[derive(Debug, Clone, Serialize)]
pub struct AssetProfile {
    pub buckets: Vec<BucketWeight>,
    pub cumulative: Vec<BucketWeight>,
}

/// Distribution of positive positions across the day bands, by weight.
/// Negative positions are excluded here and reported as `negative_memo`.
pub fn asset_profile(caps: &[Capacity], nav: f64) -> AssetProfile {
    let mut sums = [0.0f64; 4];
    if nav.is_finite() && nav > 0.0 {
        for c in caps.iter().filter(|c| c.valuation_eur > 0.0) {
            sums[band_of_days(c.days)] += c.valuation_eur / nav;
        }
    }
    let buckets: Vec<BucketWeight> = BUCKET_ORDER.iter().zip(sums)
        .map(|(b, w)| BucketWeight { bucket: (*b).into(), weight: w })
        .collect();
    let mut acc = 0.0;
    let cumulative = buckets.iter()
        .map(|b| { acc += b.weight; BucketWeight { bucket: b.bucket.clone(), weight: acc } })
        .collect();
    AssetProfile { buckets, cumulative }
}

#[derive(Debug, Clone, Serialize)]
pub struct Residual {
    pub slow_share_before: f64,
    pub slow_share_after: f64,
}

/// What a waterfall completing at `d_star` leaves behind.
///
/// Sales are allocated in ascending days order and capped at each position's
/// own realisable amount by that day. The reported figures are the share of
/// the fund held in positions slower than 30 days, before the redemption and
/// again against the smaller fund that remains — the dilution imposed on the
/// investors who stayed.
pub fn residual(caps: &[Capacity], required: f64, nav: f64, d_star: u32) -> Residual {
    const SLOW: f64 = 30.0;
    if !(nav.is_finite() && nav > 0.0) {
        return Residual { slow_share_before: 0.0, slow_share_after: 0.0 };
    }
    let slow_before: f64 = caps.iter()
        .filter(|c| c.valuation_eur > 0.0 && c.days > SLOW)
        .map(|c| c.valuation_eur).sum();

    let mut order: Vec<&Capacity> = caps.iter().filter(|c| c.valuation_eur > 0.0).collect();
    order.sort_by(|a, b| a.days.total_cmp(&b.days));

    let mut remaining = required;
    let mut slow_left = slow_before;
    for c in order {
        if remaining <= 0.0 { break; }
        let realisable = match c.capacity_eur_day {
            None => c.valuation_eur,
            Some(cap) => c.valuation_eur.min(cap * d_star as f64),
        };
        let sold = realisable.min(remaining).max(0.0);
        remaining -= sold;
        if c.days > SLOW { slow_left -= sold; }
    }

    let after_nav = nav - required;
    Residual {
        slow_share_before: slow_before / nav,
        slow_share_after: if after_nav > 0.0 { (slow_left.max(0.0)) / after_nav } else { 0.0 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(code: &str, atype: &str, val: f64) -> LiqPosition {
        LiqPosition {
            code: code.into(), asset_type: atype.into(), valuation_eur: val,
            quantity: None, adv_30d: None, adv_stale: false, adv_eligible: None,
            market_place: None, liquidity_days: None, default_days: 1.0,
        }
    }

    fn measured(code: &str, val: f64, qty: f64, adv: f64) -> LiqPosition {
        LiqPosition {
            quantity: Some(qty), adv_30d: Some(adv), market_place: Some("025".into()),
            ..pos(code, "Action", val)
        }
    }

    // ---- eligibility ----

    #[test]
    fn the_venue_rule_admits_listed_etfs_and_etcs() {
        // Amundi MSCI EM Latin America, Euronext Paris, mapped Fonds.
        let mut etf = pos("LU1681045024", "Fonds", 1.0);
        etf.market_place = Some("025".into());
        assert!(adv_eligible(&etf));
        // Gold Bullion Securities, LSE, mapped Obligation by the 13x rule.
        let mut etc = pos("GB00B00FHZ82", "Obligation", 1.0);
        etc.market_place = Some("361".into());
        assert!(adv_eligible(&etc));
    }

    #[test]
    fn the_venue_rule_excludes_unlisted_funds_cash_and_futures() {
        let mut uci = pos("FR0010599399", "Fonds", 1.0);
        uci.market_place = Some("260".into());
        assert!(!adv_eligible(&uci));
        let mut internal = pos("LU1995653893", "Fonds", 1.0);
        internal.market_place = Some("999".into());
        assert!(!adv_eligible(&internal));
        let mut fut = pos("FVSU6", "Future", 1.0);
        fut.market_place = Some("FOR".into());
        assert!(!adv_eligible(&fut));
    }

    #[test]
    fn a_null_venue_degrades_to_the_asset_type_rule() {
        // NAV Recap portfolios carry no market place; behaviour is unchanged there.
        assert!(adv_eligible(&pos("FR0000121014", "Action", 1.0)));
        assert!(!adv_eligible(&pos("LU1112771255", "Fonds", 1.0)));
    }

    #[test]
    fn the_override_forces_both_directions_but_never_enables_a_future() {
        let mut uci = pos("FR0010599399", "Fonds", 1.0);
        uci.market_place = Some("260".into());
        uci.adv_eligible = Some(true);
        assert!(adv_eligible(&uci));

        let mut eq = pos("FR0000121014", "Action", 1.0);
        eq.adv_eligible = Some(false);
        assert!(!adv_eligible(&eq));

        // A margined contract's valuation is mark-to-market, not notional, so
        // volume cannot be measured against it. That is structural, not a
        // data-quality opinion, and the override does not reach it.
        let mut fut = pos("FVSU6", "Future", 1.0);
        fut.adv_eligible = Some(true);
        assert!(!adv_eligible(&fut));
    }

    // ---- capacity ----

    #[test]
    fn the_worked_example_normal_and_stressed() {
        // 500,000 shares against 100,000 ADV at 25% participation.
        let p = measured("X", 5_000_000.0, 500_000.0, 100_000.0);
        let normal = capacity(&p, 0.25, 1.0);
        assert!(normal.measured);
        assert!((normal.days - 20.0).abs() < 1e-9);
        let stressed = capacity(&p, 0.25, 0.30);
        assert!((stressed.days - 500_000.0 / (100_000.0 * 0.25 * 0.30)).abs() < 1e-9);
        assert!((stressed.days - 66.666_666_666).abs() < 1e-6);
    }

    #[test]
    fn both_paths_agree_at_the_fallback_boundary() {
        // The fallback sets capacity so that days equals the assumed figure
        // exactly, so the two paths share one arithmetic.
        let mut p = pos("Y", "Fonds", 700_000.0);
        p.default_days = 7.0;
        let c = capacity(&p, 0.25, 1.0);
        assert!(!c.measured);
        assert!((c.days - 7.0).abs() < 1e-12);
        assert!((c.capacity_eur_day.unwrap() - 100_000.0).abs() < 1e-9);
    }

    #[test]
    fn the_stress_factor_does_not_touch_the_fallback_path() {
        // A fallback days figure is already an assumption; re-stressing it
        // would stack a guess on a guess.
        let mut p = pos("Y", "Fonds", 700_000.0);
        p.default_days = 7.0;
        assert!((capacity(&p, 0.25, 0.30).days - 7.0).abs() < 1e-12);
    }

    #[test]
    fn the_instrument_override_beats_the_asset_type_default() {
        let mut p = pos("Y", "Fonds", 700_000.0);
        p.default_days = 7.0;
        p.liquidity_days = Some(35.0);
        assert!((capacity(&p, 0.25, 1.0).days - 35.0).abs() < 1e-12);
    }

    #[test]
    fn cash_is_infinite_capacity_at_zero_days() {
        for t in ["Cash Acc", "Margin Acc"] {
            let c = capacity(&pos("C", t, 1_000_000.0), 0.25, 1.0);
            assert_eq!(c.capacity_eur_day, None);
            assert_eq!(c.days, 0.0);
            assert_eq!(c.reason, None, "cash is not on the fallback path");
        }
    }

    #[test]
    fn every_fallback_names_its_reason() {
        let mut stale = measured("A", 1_000.0, 100.0, 50.0);
        stale.adv_stale = true;
        assert_eq!(capacity(&stale, 0.25, 1.0).reason, Some("stale adv"));

        let mut no_adv = measured("B", 1_000.0, 100.0, 0.0);
        no_adv.adv_30d = None;
        assert_eq!(capacity(&no_adv, 0.25, 1.0).reason, Some("no adv"));

        let mut zero_adv = measured("C", 1_000.0, 100.0, 0.0);
        zero_adv.adv_30d = Some(0.0);
        assert_eq!(capacity(&zero_adv, 0.25, 1.0).reason, Some("no adv"));

        let mut no_qty = measured("D", 1_000.0, 0.0, 50.0);
        no_qty.quantity = None;
        assert_eq!(capacity(&no_qty, 0.25, 1.0).reason, Some("no quantity"));

        let mut fut = pos("E", "Future", 1_000.0);
        fut.quantity = Some(10.0);
        fut.adv_30d = Some(5_000.0);
        assert_eq!(capacity(&fut, 0.25, 1.0).reason, Some("future"));

        let mut uci = pos("F", "Fonds", 1_000.0);
        uci.market_place = Some("260".into());
        uci.quantity = Some(10.0);
        uci.adv_30d = Some(5_000.0);
        assert_eq!(capacity(&uci, 0.25, 1.0).reason, Some("not eligible"));
    }

    // ---- availability and orderings ----

    fn book() -> Vec<Capacity> {
        // 1m of cash, 2m at 100k/day (20 days), 1m at 25k/day (40 days).
        vec![
            capacity(&pos("CASH", "Cash Acc", 1_000_000.0), 0.25, 1.0),
            capacity(&{ let mut p = pos("FAST", "Fonds", 2_000_000.0); p.default_days = 20.0; p }, 0.25, 1.0),
            capacity(&{ let mut p = pos("SLOW", "Fonds", 1_000_000.0); p.default_days = 40.0; p }, 0.25, 1.0),
        ]
    }

    #[test]
    fn availability_accumulates_and_caps_at_position_value() {
        let c = book();
        // Day 1: 1,000,000 cash + 100,000 + 25,000
        assert!((available(&c, &[], 0.0, 1) - 1_125_000.0).abs() < 1e-6);
        // Day 40: everything, capped at each position's own value.
        assert!((available(&c, &[], 0.0, 40) - 4_000_000.0).abs() < 1e-6);
        // Beyond the last position's days nothing more appears.
        assert!((available(&c, &[], 0.0, 90) - 4_000_000.0).abs() < 1e-6);
    }

    #[test]
    fn negative_positions_reduce_availability_from_day_one() {
        // The defect this design closes: payables were a memo that never
        // counted against the pass/fail test.
        let c = book();
        assert!((available(&c, &[], -500_000.0, 1) - 625_000.0).abs() < 1e-6);
        assert!((available(&c, &[], -500_000.0, 40) - 3_500_000.0).abs() < 1e-6);
    }

    #[test]
    fn inflows_land_on_their_own_day_and_not_before() {
        let c = book();
        let inf = vec![Inflow { day: 10, amount_eur: 60_000.0 }];
        assert!((available(&c, &inf, 0.0, 9) - available(&c, &[], 0.0, 9)).abs() < 1e-9);
        assert!((available(&c, &inf, 0.0, 10) - available(&c, &[], 0.0, 10) - 60_000.0).abs() < 1e-6);
    }

    #[test]
    fn waterfall_sells_the_liquid_names_hardest() {
        let c = book();
        // Need 1,500,000: cash covers 1m, the rest at 125,000/day -> day 4.
        let w = waterfall(&c, &[], 0.0, 1_500_000.0, 60);
        assert_eq!(w.days, Some(4));
        assert_eq!(w.unmet_eur, 0.0);
    }

    #[test]
    fn an_unreachable_requirement_reports_the_shortfall_not_a_pass() {
        let c = book();
        let w = waterfall(&c, &[], 0.0, 6_000_000.0, 60);
        assert_eq!(w.days, None);
        assert!((w.unmet_eur - 2_000_000.0).abs() < 1e-6);
    }

    #[test]
    fn slice_is_the_slower_ordering() {
        let c = book();
        let nav = 4_000_000.0;
        let required = 1_500_000.0;
        // f = 0.375; the 40-day position needs 0.375 x 40 = 15 days.
        let s = slice_days(&c, required, nav).unwrap();
        assert!((s - 15.0).abs() < 1e-9);
        assert!(s >= waterfall(&c, &[], 0.0, required, 60).days.unwrap() as f64);
    }

    // ---- profile and residual ----

    #[test]
    fn the_profile_distributes_weight_across_the_day_bands() {
        let c = book();
        let p = asset_profile(&c, 4_000_000.0);
        assert!((p.buckets[0].weight - 0.25).abs() < 1e-12); // cash, 0 days
        assert!((p.buckets[2].weight - 0.50).abs() < 1e-12); // 20 days
        assert!((p.buckets[3].weight - 0.25).abs() < 1e-12); // 40 days
        assert!((p.cumulative[3].weight - 1.0).abs() < 1e-12);
    }

    #[test]
    fn residual_shows_the_dilution_left_to_the_investors_who_stayed() {
        let c = book();
        // A 1,500,000 waterfall completing at day 4 sells cash first, then the
        // fast name; the slow 1m barely moves, so its share of a smaller fund
        // rises.
        let r = residual(&c, 1_500_000.0, 4_000_000.0, 4);
        assert!((r.slow_share_before - 0.25).abs() < 1e-12);
        assert!(r.slow_share_after > r.slow_share_before);
    }

    #[test]
    fn an_empty_portfolio_is_not_a_pass() {
        let w = waterfall(&[], &[], 0.0, 1_000.0, 60);
        assert_eq!(w.days, None);
        assert!((w.unmet_eur - 1_000.0).abs() < 1e-12);
        assert_eq!(slice_days(&[], 1_000.0, 0.0), None);
        let p = asset_profile(&[], 0.0);
        assert!(p.buckets.iter().all(|b| b.weight == 0.0));
    }
}
