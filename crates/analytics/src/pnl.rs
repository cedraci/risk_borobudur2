//! Period P&L attribution.
//!
//! Pure functions over plain structs: the caller reads the database and passes
//! trades, snapshot positions and FX rates in. Money is `f64` and local
//! currency unless a name says `_eur`.

use chrono::NaiveDate;
use serde::Serialize;
use std::collections::BTreeMap;

/// A trade as the engine needs it. `net_price` includes fees, matching the
/// administrator's PAM convention; `net_amount` is signed, negative for a buy.
#[derive(Debug, Clone)]
pub struct Trade {
    pub trade_date: NaiveDate,
    pub isin: String,
    pub is_buy: bool,
    pub quantity: f64,
    pub net_price: f64,
    pub net_amount: f64,
    pub currency: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Basis { pub qty: f64, pub avg_cost: f64 }

#[derive(Debug, Clone, Copy)]
pub struct Flow { pub date: NaiveDate, pub amount_local: f64 }

#[derive(Debug, Clone)]
pub struct Walk {
    pub basis_start: Basis,
    pub basis_end: Basis,
    /// Realized in (t0, t1], local currency.
    pub realized_local: f64,
    pub buys: Vec<Flow>,
    pub sells: Vec<Flow>,
    /// A sell exceeded the running quantity: history is incomplete.
    pub oversold: bool,
}

/// `Achat` -> buy, `Vente` -> sell, case- and whitespace-insensitive.
/// `None` for anything else, which the caller reports rather than guesses.
pub fn is_buy(side: &str) -> Option<bool> {
    match side.trim().to_lowercase().as_str() {
        "achat" => Some(true),
        "vente" => Some(false),
        _ => None,
    }
}

/// Roll weighted-average cost over `trades` (which must be sorted by date).
/// Trades on or before `t0` build the opening basis; trades in `(t0, t1]`
/// accumulate realized P&L and flows. Trades after `t1` are ignored.
pub fn walk_instrument(trades: &[Trade], t0: NaiveDate, t1: NaiveDate) -> Walk {
    let mut b = Basis::default();
    let mut basis_start = None;
    let mut realized = 0.0;
    let (mut buys, mut sells) = (Vec::new(), Vec::new());
    let mut oversold = false;

    for t in trades {
        if t.trade_date > t1 { break; }
        if basis_start.is_none() && t.trade_date > t0 {
            basis_start = Some(b);
        }
        let in_window = t.trade_date > t0;

        if t.is_buy {
            let total = b.avg_cost * b.qty + t.quantity * t.net_price;
            b.qty += t.quantity;
            b.avg_cost = if b.qty.abs() > f64::EPSILON { total / b.qty } else { 0.0 };
            if in_window { buys.push(Flow { date: t.trade_date, amount_local: t.net_amount }); }
        } else {
            let q = if t.quantity > b.qty + 1e-9 { oversold = true; b.qty } else { t.quantity };
            if in_window { realized += q * (t.net_price - b.avg_cost); }
            b.qty = (b.qty - q).max(0.0);
            if b.qty <= 1e-9 { b.qty = 0.0; b.avg_cost = 0.0; }
            if in_window { sells.push(Flow { date: t.trade_date, amount_local: t.net_amount }); }
        }
    }

    Walk {
        basis_start: basis_start.unwrap_or(b),
        basis_end: b,
        realized_local: realized,
        buys, sells, oversold,
    }
}

/// FX rates for one currency over one period. `f0`/`f1` are the snapshot rates
/// at the period endpoints; `at_trade` carries daily rates for trade dates.
/// For EUR, every rate is 1.0 and the map is empty.
#[derive(Debug, Clone)]
pub struct FxLookup {
    pub f0: f64,
    pub f1: f64,
    pub at_trade: BTreeMap<NaiveDate, f64>,
}

impl FxLookup {
    pub fn eur() -> Self { Self { f0: 1.0, f1: 1.0, at_trade: BTreeMap::new() } }

    /// Exact-date lookup. No nearest-date fallback: a silently approximated
    /// rate is precisely the error the reconciliation exists to catch.
    pub fn rate(&self, d: NaiveDate) -> Option<f64> {
        if self.f0 == 1.0 && self.f1 == 1.0 && self.at_trade.is_empty() {
            return Some(1.0); // EUR
        }
        self.at_trade.get(&d).copied()
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Decomp {
    pub realized_price: f64,
    pub unrealized_price: f64,
    pub realized_fx: f64,
    pub unrealized_fx: f64,
    /// A partial sale followed a mid-period purchase: weighted-average costing
    /// cannot attribute the purchase's FX exactly. Disclosed, never smoothed.
    pub fx_split_imprecise: bool,
    /// Trade dates with no FX rate available.
    pub fx_missing: Vec<NaiveDate>,
}

impl Decomp {
    pub fn realized(&self) -> f64 { self.realized_price + self.realized_fx }
    pub fn unrealized(&self) -> f64 { self.unrealized_price + self.unrealized_fx }
    pub fn fx(&self) -> f64 { self.realized_fx + self.unrealized_fx }
    pub fn total(&self) -> f64 { self.realized() + self.unrealized() }
}

/// Decompose one instrument's period P&L into price and FX, each split into
/// realized and unrealized.
///
/// ```text
/// LocalP&L      = (v1 - v0) + Σ flows
/// Price         = LocalP&L × f0                       (split realized/unrealized
///                                                      by the walk's realized figure)
/// RealizedFX    = Σ_sells     CF × [F(trade) - f0]
/// UnrealizedFX  = v1 × [f1 - f0] + Σ_buys CF × [F(trade) - f0]
/// ```
///
/// The four sum to `v1·f1 - v0·f0 + Σ CF·F(trade)` exactly, so period
/// additivity — and therefore the reconciliation to ΔNAV — is preserved. If a
/// flow's trade date has no rate in `fx`, that flow is translated at `f0`
/// instead (and its date is listed in `fx_missing`), so the identity above
/// holds with `f0` substituted for the missing `F(trade)`.
///
/// When the position is closed at `t1`, purchase-flow FX moves to realized:
/// reporting unrealized FX on a holding of nothing is meaningless. Whether the
/// position is closed is decided by the snapshot (`v1_local`), not by the
/// walk's own `basis_end.qty`: a walk built from a trade history missing this
/// instrument (ISIN mismatch, truncated history) reports zero quantity even
/// though the fund still holds it, and that must not misroute its
/// translation FX to realized.
pub fn decompose(w: &Walk, v0_local: f64, v1_local: f64, fx: &FxLookup) -> Decomp {
    let mut out = Decomp::default();

    let flow_sum: f64 = w.buys.iter().chain(w.sells.iter()).map(|f| f.amount_local).sum();
    let local_pnl = (v1_local - v0_local) + flow_sum;
    out.realized_price = w.realized_local * fx.f0;
    out.unrealized_price = (local_pnl - w.realized_local) * fx.f0;

    let mut missing: Vec<NaiveDate> = Vec::new();
    let fx_on = |flows: &[Flow], missing: &mut Vec<NaiveDate>| -> f64 {
        flows.iter().map(|f| match fx.rate(f.date) {
            Some(r) => f.amount_local * (r - fx.f0),
            None => { if !missing.contains(&f.date) { missing.push(f.date); } 0.0 }
        }).sum()
    };
    let sells_fx = fx_on(&w.sells, &mut missing);
    let buys_fx = fx_on(&w.buys, &mut missing);
    missing.sort();
    out.fx_missing = missing;

    out.realized_fx = sells_fx;
    out.unrealized_fx = v1_local * (fx.f1 - fx.f0) + buys_fx;

    if v1_local.abs() <= 1e-9 {
        // "Closed" means the snapshot shows nothing held at t1. The walk's own
        // quantity is not authoritative here: an instrument with no trades in
        // the file (ISIN mismatch, truncated history) leaves basis_end.qty at
        // zero while the position is still held, and routing its translation
        // FX to realized would misstate the split without moving the total.
        out.realized_fx += out.unrealized_fx;
        out.unrealized_fx = 0.0;
    } else if let (Some(first_buy), Some(last_sell)) = (w.buys.first(), w.sells.last()) {
        // Only a sale drawing on a basis that includes a mid-period purchase is
        // ambiguous. A sale that precedes every purchase is attributed exactly.
        out.fx_split_imprecise = last_sell.date >= first_buy.date;
    }

    out
}

/// P&L for one futures contract over a period.
///
/// `v0_ccy`/`v1_ccy` are the contract's `Valorisation Dev` at each snapshot,
/// which is variation margin — accumulated unrealized P&L — not market value.
/// `realized_ccy` is the result of contracts closed inside the period, which
/// the caller derives from `OPERATIONS`; pass 0.0 when none were closed.
///
/// There is no cost basis here: a future has no acquisition cost to average,
/// and the fund holds short futures, for which an average cost is meaningless.
pub fn futures_pnl(v0_ccy: f64, v1_ccy: f64, realized_ccy: f64, fx: &FxLookup) -> Decomp {
    let total_local = (v1_ccy - v0_ccy) + realized_ccy;
    Decomp {
        realized_price: realized_ccy * fx.f0,
        unrealized_price: (total_local - realized_ccy) * fx.f0,
        realized_fx: 0.0,
        unrealized_fx: v1_ccy * (fx.f1 - fx.f0),
        fx_split_imprecise: false,
        fx_missing: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, dd: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, dd).unwrap() }

    fn trade(day: u32, buy: bool, qty: f64, px: f64) -> Trade {
        Trade {
            trade_date: d(2026, 6, day),
            isin: "X".into(),
            is_buy: buy,
            quantity: qty,
            net_price: px,
            net_amount: if buy { -qty * px } else { qty * px },
            currency: "EUR".into(),
        }
    }

    #[test]
    fn weighted_average_matches_worked_example() {
        // Spec: buy 5000 @ 40.76, buy 3000 @ 44.20 -> avg 42.05,
        // sell 2000 @ 46.00 -> realized 7900, avg unchanged.
        let t = vec![trade(1, true, 5000.0, 40.76), trade(2, true, 3000.0, 44.20), trade(3, false, 2000.0, 46.00)];
        let w = walk_instrument(&t, d(2026, 5, 31), d(2026, 6, 30));
        assert!((w.basis_end.avg_cost - 42.05).abs() < 1e-9);
        assert!((w.basis_end.qty - 6000.0).abs() < 1e-9);
        assert!((w.realized_local - 2000.0 * (46.00 - 42.05)).abs() < 1e-6);
    }

    #[test]
    fn trades_on_or_before_t0_build_the_opening_basis_only() {
        let t = vec![trade(1, true, 1000.0, 10.0), trade(20, false, 400.0, 12.0)];
        let w = walk_instrument(&t, d(2026, 6, 10), d(2026, 6, 30));
        assert!((w.basis_start.qty - 1000.0).abs() < 1e-9);
        assert!((w.basis_start.avg_cost - 10.0).abs() < 1e-9);
        // Only the sell falls inside the window.
        assert!((w.realized_local - 400.0 * 2.0).abs() < 1e-9);
        assert_eq!(w.buys.len(), 0);
        assert_eq!(w.sells.len(), 1);
    }

    #[test]
    fn a_trade_exactly_on_t0_is_in_the_opening_basis() {
        let t = vec![trade(10, true, 100.0, 5.0)];
        let w = walk_instrument(&t, d(2026, 6, 10), d(2026, 6, 30));
        assert!((w.basis_start.qty - 100.0).abs() < 1e-9);
        assert_eq!(w.buys.len(), 0);
    }

    #[test]
    fn overselling_is_flagged_and_does_not_go_negative() {
        let t = vec![trade(1, true, 100.0, 5.0), trade(2, false, 250.0, 6.0)];
        let w = walk_instrument(&t, d(2026, 5, 31), d(2026, 6, 30));
        assert!(w.oversold);
        assert!(w.basis_end.qty >= 0.0);
    }

    #[test]
    fn side_parsing_is_case_insensitive() {
        assert_eq!(is_buy("Achat"), Some(true));
        assert_eq!(is_buy("Vente"), Some(false));
        assert_eq!(is_buy("VENTE"), Some(false));
        assert_eq!(is_buy("  achat "), Some(true));
        assert_eq!(is_buy("Nonsense"), None);
    }

    fn eur_fx() -> FxLookup { FxLookup { f0: 1.0, f1: 1.0, at_trade: BTreeMap::new() } }

    #[test]
    fn eur_position_has_no_fx_effect_and_totals_correctly() {
        // Held throughout, no trades: value 1000 -> 1150.
        let w = walk_instrument(&[], d(2026, 6, 1), d(2026, 6, 30));
        let dec = decompose(&w, 1000.0, 1150.0, &eur_fx());
        assert!((dec.total() - 150.0).abs() < 1e-9);
        assert!(dec.fx().abs() < 1e-12);
        assert!((dec.unrealized() - 150.0).abs() < 1e-9);
    }

    #[test]
    fn price_plus_fx_equals_total_exactly() {
        let t = vec![trade(15, false, 100.0, 12.0)];
        let mut at = BTreeMap::new();
        at.insert(d(2026, 6, 15), 0.90);
        let fx = FxLookup { f0: 0.88, f1: 0.92, at_trade: at };
        // Opening basis needs a prior buy.
        let mut all = vec![trade(1, true, 300.0, 10.0)];
        all.extend(t);
        let w = walk_instrument(&all, d(2026, 6, 10), d(2026, 6, 30));
        let dec = decompose(&w, 3000.0, 2400.0, &fx);

        let expected_total = 2400.0 * 0.92 - 3000.0 * 0.88 + (100.0 * 12.0) * 0.90;
        assert!((dec.total() - expected_total).abs() < 1e-9);
        assert!((dec.realized_price + dec.unrealized_price + dec.realized_fx + dec.unrealized_fx
                 - expected_total).abs() < 1e-9);
    }

    #[test]
    fn fully_exited_position_puts_all_fx_in_realized() {
        let mut at = BTreeMap::new();
        at.insert(d(2026, 6, 15), 0.90);
        let fx = FxLookup { f0: 0.88, f1: 0.92, at_trade: at };
        let all = vec![trade(1, true, 100.0, 10.0), trade(15, false, 100.0, 12.0)];
        let w = walk_instrument(&all, d(2026, 6, 10), d(2026, 6, 30));
        let dec = decompose(&w, 1000.0, 0.0, &fx);
        assert_eq!(w.basis_end.qty, 0.0);
        assert!(dec.unrealized_fx.abs() < 1e-12, "unrealized FX on an empty holding is nonsense");
        assert!(dec.realized_fx.abs() > 0.0);
    }

    #[test]
    fn held_throughout_with_no_trades_reports_translation_fx_as_unrealized() {
        // ISIN mismatch or truncated history: no trades in the file for an
        // instrument the fund still holds. basis_end.qty is zero, but the
        // snapshot (v1) says the position is very much still held, so the
        // translation FX must land in unrealized, not realized.
        let fx = FxLookup { f0: 0.88, f1: 0.92, at_trade: BTreeMap::new() };
        let w = walk_instrument(&[], d(2026, 6, 10), d(2026, 6, 30));
        let dec = decompose(&w, 1000.0, 1150.0, &fx);
        assert!(dec.unrealized_fx.abs() > 0.0);
        assert_eq!(dec.realized_fx, 0.0);
    }

    #[test]
    fn sell_then_rebuy_is_not_flagged_as_imprecise() {
        // A sale that precedes every purchase draws only on the opening basis
        // at the opening average cost: no weighted-average ambiguity.
        let mut at = BTreeMap::new();
        at.insert(d(2026, 6, 12), 0.89);
        at.insert(d(2026, 6, 20), 0.93);
        let fx = FxLookup { f0: 0.88, f1: 0.92, at_trade: at };
        let all = vec![trade(1, true, 100.0, 10.0), trade(12, false, 30.0, 11.0), trade(20, true, 50.0, 12.0)];
        let w = walk_instrument(&all, d(2026, 6, 10), d(2026, 6, 30));
        let dec = decompose(&w, 1000.0, 1400.0, &fx);
        assert!(!dec.fx_split_imprecise);
    }

    #[test]
    fn opened_and_closed_inside_the_period_reports_no_unrealized_fx() {
        let mut at = BTreeMap::new();
        at.insert(d(2026, 6, 12), 0.89);
        at.insert(d(2026, 6, 20), 0.93);
        let fx = FxLookup { f0: 0.88, f1: 0.92, at_trade: at };
        let all = vec![trade(12, true, 50.0, 20.0), trade(20, false, 50.0, 22.0)];
        let w = walk_instrument(&all, d(2026, 6, 10), d(2026, 6, 30));
        let dec = decompose(&w, 0.0, 0.0, &fx);
        assert!(dec.unrealized_fx.abs() < 1e-12);
        assert!(!dec.fx_split_imprecise, "a closed round trip is exact, not imprecise");
    }

    #[test]
    fn partial_sale_after_a_mid_period_purchase_is_flagged() {
        let mut at = BTreeMap::new();
        at.insert(d(2026, 6, 12), 0.89);
        at.insert(d(2026, 6, 20), 0.93);
        let fx = FxLookup { f0: 0.88, f1: 0.92, at_trade: at };
        let all = vec![trade(1, true, 100.0, 10.0), trade(12, true, 50.0, 11.0), trade(20, false, 30.0, 12.0)];
        let w = walk_instrument(&all, d(2026, 6, 10), d(2026, 6, 30));
        let dec = decompose(&w, 1000.0, 1400.0, &fx);
        assert!(dec.fx_split_imprecise);
    }

    #[test]
    fn a_trade_date_with_no_fx_rate_is_reported() {
        let fx = FxLookup { f0: 0.88, f1: 0.92, at_trade: BTreeMap::new() };
        let all = vec![trade(1, true, 100.0, 10.0), trade(15, false, 40.0, 12.0)];
        let w = walk_instrument(&all, d(2026, 6, 10), d(2026, 6, 30));
        let dec = decompose(&w, 1000.0, 700.0, &fx);
        assert_eq!(dec.fx_missing, vec![d(2026, 6, 15)]);

        // The flow with no rate is translated at f0, so the identity holds
        // with f0 substituted for the missing F(trade).
        let cf = 40.0 * 12.0;
        let expected_total = 700.0 * 0.92 - 1000.0 * 0.88 + cf * 0.88;
        assert!((dec.total() - expected_total).abs() < 1e-9);
    }

    /// Exhaustive small-grid check that the four components always sum to the
    /// EUR total. This is the identity the whole reconciliation rests on.
    #[test]
    fn decomposition_is_exact_across_a_grid() {
        for &f0 in &[0.5, 0.88, 1.0, 1.4] {
            for &f1 in &[0.5, 0.92, 1.0, 1.4] {
                for &ft in &[0.5, 0.90, 1.0, 1.4] {
                    for &(qty_buy, qty_sell) in &[(100.0, 0.0), (100.0, 40.0), (100.0, 100.0), (0.0, 0.0), (0.0, 200.0)] {
                        let ft_buy = ft;
                        let ft_sell = ft * 1.05;
                        let mut at = BTreeMap::new();
                        at.insert(d(2026, 6, 15), ft_buy);
                        at.insert(d(2026, 6, 16), ft_sell);
                        let fx = FxLookup { f0, f1, at_trade: at };

                        let mut all = vec![trade(1, true, 200.0, 10.0)];
                        if qty_buy > 0.0 { all.push(trade(15, true, qty_buy, 11.0)); }
                        if qty_sell > 0.0 { all.push(trade(16, false, qty_sell, 12.0)); }

                        let w = walk_instrument(&all, d(2026, 6, 10), d(2026, 6, 30));
                        let v0 = 2000.0;
                        let v1 = w.basis_end.qty * 12.5;
                        let dec = decompose(&w, v0, v1, &fx);

                        let flows: f64 = w.buys.iter().map(|f| f.amount_local * ft_buy).sum::<f64>()
                            + w.sells.iter().map(|f| f.amount_local * ft_sell).sum::<f64>();
                        let expected = v1 * f1 - v0 * f0 + flows;
                        assert!((dec.total() - expected).abs() < 1e-6,
                            "f0={f0} f1={f1} ft={ft} buy={qty_buy} sell={qty_sell}: {} vs {}",
                            dec.total(), expected);
                        if v1.abs() <= 1e-9 {
                            assert_eq!(dec.unrealized_fx, 0.0,
                                "f0={f0} f1={f1} ft={ft} buy={qty_buy} sell={qty_sell}: closed position must have zero unrealized FX");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn futures_pnl_is_the_change_in_variation_margin() {
        let dec = futures_pnl(6750.0, 9100.0, 0.0, &eur_fx());
        assert!((dec.total() - 2350.0).abs() < 1e-9);
        assert!((dec.unrealized() - 2350.0).abs() < 1e-9);
        assert!(dec.realized().abs() < 1e-12);
    }

    #[test]
    fn closed_futures_contract_reports_its_result_as_realized() {
        // Margin ran to zero because the contract was closed for +1200.
        let dec = futures_pnl(800.0, 0.0, 1200.0, &eur_fx());
        assert!((dec.realized() - 1200.0).abs() < 1e-9);
        assert!((dec.total() - (1200.0 - 800.0)).abs() < 1e-9);
    }

    #[test]
    fn futures_fx_uses_the_closing_margin_balance() {
        let fx = FxLookup { f0: 0.88, f1: 0.92, at_trade: BTreeMap::new() };
        let dec = futures_pnl(1000.0, 1500.0, 0.0, &fx);
        // price effect at f0, FX on the closing balance
        assert!((dec.unrealized_price - 500.0 * 0.88).abs() < 1e-9);
        assert!((dec.unrealized_fx - 1500.0 * (0.92 - 0.88)).abs() < 1e-9);
    }
}
