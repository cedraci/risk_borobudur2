//! Period P&L attribution.
//!
//! Pure functions over plain structs: the caller reads the database and passes
//! trades, snapshot positions and FX rates in. Money is `f64` and local
//! currency unless a name says `_eur`.

use chrono::NaiveDate;

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
}
