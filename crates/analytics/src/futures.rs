//! Futures exposure: contract identification, notional, and bond-future DV01.
//!
//! The NAV Recap reports a future's `Valorisation` as variation margin, not
//! market value, so exposure has to be rebuilt from quantity, price and the
//! contract's point value.

/// How a contract's price is quoted in `PORTEFEUILLE_NAV`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceConvention {
    /// The quoted number is the price.
    Decimal,
    /// Thirty-seconds: `108.105` means `108-10.5`, i.e. 108 + 10.5/32.
    Th32,
}

impl PriceConvention {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "decimal" => Some(Self::Decimal),
            "th32" => Some(Self::Th32),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Decimal => "decimal",
            Self::Th32 => "th32",
        }
    }
}

/// Decode a quoted futures price. Under `Th32` the three fractional digits
/// are thirty-seconds multiplied by ten (`.105` = 10.5/32), so dividing by
/// 320 recovers the true price.
pub fn decode_price(raw: f64, conv: PriceConvention) -> f64 {
    match conv {
        PriceConvention::Decimal => raw,
        PriceConvention::Th32 => {
            let whole = raw.trunc();
            let ticks = ((raw - whole) * 1000.0).round();
            whole + ticks / 320.0
        }
    }
}

/// Contract root from a Bloomberg ticker: the symbol before the space, minus
/// its trailing month letter and year digit. `"RXU6 Comdty"` -> `"RX"`.
/// Stable across quarterly rolls, unlike the workbook's futures ISINs.
pub fn contract_root(ticker: &str) -> Option<String> {
    let sym = ticker.split_whitespace().next()?;
    if !sym.is_ascii() || sym.len() < 3 {
        return None;
    }
    Some(sym[..sym.len() - 2].to_string())
}

/// Point value implied by the workbook's own identity
/// `valorisation = (price - avg_cost) * qty * point_value`.
///
/// `None` when the position is marked at its average cost (the denominator
/// vanishes), when quantity is zero, or when the result is not a positive
/// finite number. Prices must already be decoded.
pub fn implied_point_value(price: f64, avg_cost: f64, qty: f64, valuation_ccy: f64) -> Option<f64> {
    let scale = price.abs().max(1.0);
    if (price - avg_cost).abs() < 1e-6 * scale || qty == 0.0 {
        return None;
    }
    let pv = valuation_ccy / ((price - avg_cost) * qty);
    (pv.is_finite() && pv > 0.0).then_some(pv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_thirty_seconds_quotes() {
        // 109.145 is the "109-14.5" 32nds quote written without its hyphen.
        // Proven against the workbook: TYU6's only trade was 6 @ 109.453125.
        assert!((decode_price(109.145, PriceConvention::Th32) - 109.453125).abs() < 1e-12);
        assert!((decode_price(108.105, PriceConvention::Th32) - 108.328125).abs() < 1e-12);
        assert!((decode_price(108.0, PriceConvention::Th32) - 108.0).abs() < 1e-12);
        // Decimal contracts pass through untouched.
        assert!((decode_price(124.46, PriceConvention::Decimal) - 124.46).abs() < 1e-12);
        assert!((decode_price(8388.0, PriceConvention::Decimal) - 8388.0).abs() < 1e-12);
    }

    #[test]
    fn parses_convention_names() {
        assert_eq!(PriceConvention::parse("decimal"), Some(PriceConvention::Decimal));
        assert_eq!(PriceConvention::parse("th32"), Some(PriceConvention::Th32));
        assert_eq!(PriceConvention::parse("32nds"), None);
        assert_eq!(PriceConvention::Th32.as_str(), "th32");
    }

    #[test]
    fn derives_contract_root() {
        assert_eq!(contract_root("RXU6 Comdty").as_deref(), Some("RX"));
        assert_eq!(contract_root("OATU6 Comdty").as_deref(), Some("OAT"));
        assert_eq!(contract_root("KOAU6 Comdty").as_deref(), Some("KOA"));
        assert_eq!(contract_root("TYU6 Comdty").as_deref(), Some("TY"));
        assert_eq!(contract_root("CFQ6 Index").as_deref(), Some("CF"));
        assert_eq!(contract_root("NQU6 Index").as_deref(), Some("NQ"));
        assert_eq!(contract_root("RYU6 Curncy").as_deref(), Some("RY"));
        assert_eq!(contract_root(""), None);
        assert_eq!(contract_root("AB"), None); // nothing left after the suffix
    }

    #[test]
    fn recovers_exchange_point_values() {
        // (price, avg_cost, qty, valuation_ccy, expected) - the eight real contracts,
        // with the TY line already decoded out of 32nds.
        let cases = [
            (8388.0, 8336.23333333, -12.0, -6212.0, 10.0),
            (6301.0, 6287.0, -9.0, -1260.0, 10.0),
            (108.328125, 109.453125, -6.0, 6750.0, 1000.0),
            (185.93, 184.13, -7.0, -1575000.0, 125000.0),
            (124.46, 125.83, -8.0, 10960.0, 1000.0),
            (117.12, 118.918, -15.0, 26970.0, 1000.0),
            (28282.25, 28982.5, -1.0, 14005.0, 20.0),
            (119.82, 121.16625, 8.0, -10770.0, 1000.0),
        ];
        for (price, pam, qty, val, want) in cases {
            let got = implied_point_value(price, pam, qty, val).unwrap();
            assert!((got - want).abs() < 1e-6, "price {price}: got {got}, want {want}");
        }
    }

    #[test]
    fn point_value_undeterminable_cases() {
        // marked at average cost -> denominator vanishes
        assert_eq!(implied_point_value(124.46, 124.46, -8.0, 0.0), None);
        // zero quantity
        assert_eq!(implied_point_value(124.46, 125.83, 0.0, 0.0), None);
        // a negative implied value is nonsense, not a spec
        assert_eq!(implied_point_value(124.46, 125.83, -8.0, -10960.0), None);
    }
}
