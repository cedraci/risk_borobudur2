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

use serde::Serialize;

/// Derivative category for the exposure disclosure. The six standard
/// regulatory categories; only three have holdings today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Equity,
    InterestRate,
    Fx,
    Credit,
    Commodity,
    Other,
}

impl Category {
    pub const ALL: [Category; 6] = [
        Category::Equity,
        Category::InterestRate,
        Category::Fx,
        Category::Credit,
        Category::Commodity,
        Category::Other,
    ];

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "equity" => Some(Self::Equity),
            "interest_rate" => Some(Self::InterestRate),
            "fx" => Some(Self::Fx),
            "credit" => Some(Self::Credit),
            "commodity" => Some(Self::Commodity),
            "other" => Some(Self::Other),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Equity => "equity",
            Self::InterestRate => "interest_rate",
            Self::Fx => "fx",
            Self::Credit => "credit",
            Self::Commodity => "commodity",
            Self::Other => "other",
        }
    }
}

/// One futures position, with its price already decoded and its spec resolved.
///
/// `qty` and `price` are optional because the source workbook tolerates an
/// empty or `#VALUE!` cell without raising a row error (see `ingest::cell_f64`),
/// and a missing input must never be absorbed into a zero: a zero quantity or a
/// zero price is a real, computable position, whereas an absent one is not.
#[derive(Debug, Clone)]
pub struct FuturePosition {
    pub ticker: String,
    pub name: String,
    pub currency: String,
    pub category: Category,
    pub qty: Option<f64>,
    pub price: Option<f64>,
    pub point_value: Option<f64>,
    pub fx_rate: Option<f64>,
    /// The contract spec exists but the user has not yet verified it, so every
    /// figure derived from it is provisional.
    pub unconfirmed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExposureRow {
    pub ticker: String,
    pub name: String,
    pub currency: String,
    pub category: Category,
    pub qty: Option<f64>,
    pub price: Option<f64>,
    pub point_value: Option<f64>,
    pub notional_ccy: Option<f64>,
    pub notional_eur: Option<f64>,
    pub pct_nav: Option<f64>,
    pub spec_missing: bool,
    /// True when the row's own quantity or price was absent from the workbook,
    /// as opposed to its contract spec being absent (`spec_missing`).
    pub inputs_missing: bool,
    /// True when the contract spec behind this row is still unconfirmed, so the
    /// notional is provisional. Marks the number itself, not just the page.
    pub unconfirmed: bool,
}

/// Long, short and gross for one category, each a fraction of net assets
/// expressed in absolute value (shorts are positive numbers).
#[derive(Debug, Clone, Serialize)]
pub struct CategoryTotals {
    pub category: Category,
    pub long_pct: f64,
    pub short_pct: f64,
    pub gross_pct: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExposureReport {
    pub rows: Vec<ExposureRow>,
    pub categories: Vec<CategoryTotals>,
    /// Sum across categories; `category` is `Other` and carries no meaning.
    pub total: CategoryTotals,
    /// Tickers left out of the totals for want of a spec, an FX rate or an AUM.
    pub excluded: Vec<String>,
}

/// Notional exposure by reference to the underlying, aggregated by category.
///
/// `notional = qty * point_value * price`, converted to EUR at the workbook's
/// own FX rate, then expressed as a fraction of net assets. Long and short are
/// summed separately, each in absolute value, without netting.
pub fn exposure(positions: &[FuturePosition], aum: f64) -> ExposureReport {
    let mut rows = Vec::with_capacity(positions.len());
    let mut excluded = Vec::new();

    for p in positions {
        let notional_ccy = match (p.point_value, p.qty, p.price) {
            (Some(pv), Some(qty), Some(price)) => Some(qty * pv * price),
            _ => None,
        };
        let notional_eur = match (notional_ccy, p.fx_rate) {
            (Some(n), Some(fx)) => Some(n * fx),
            _ => None,
        };
        let pct_nav = match notional_eur {
            Some(n) if aum > 0.0 => Some(n / aum),
            _ => None,
        };
        if pct_nav.is_none() {
            excluded.push(p.ticker.clone());
        }
        rows.push(ExposureRow {
            ticker: p.ticker.clone(),
            name: p.name.clone(),
            currency: p.currency.clone(),
            category: p.category,
            qty: p.qty,
            price: p.price,
            point_value: p.point_value,
            notional_ccy,
            notional_eur,
            pct_nav,
            spec_missing: p.point_value.is_none(),
            inputs_missing: p.qty.is_none() || p.price.is_none(),
            unconfirmed: p.unconfirmed,
        });
    }

    let categories: Vec<CategoryTotals> = Category::ALL
        .iter()
        .map(|cat| {
            let mut long_pct = 0.0;
            let mut short_pct = 0.0;
            for r in rows.iter().filter(|r| r.category == *cat) {
                match r.pct_nav {
                    Some(p) if p > 0.0 => long_pct += p,
                    Some(p) if p < 0.0 => short_pct += -p,
                    _ => {}
                }
            }
            CategoryTotals { category: *cat, long_pct, short_pct, gross_pct: long_pct + short_pct }
        })
        .collect();

    let long_pct: f64 = categories.iter().map(|c| c.long_pct).sum();
    let short_pct: f64 = categories.iter().map(|c| c.short_pct).sum();
    ExposureReport {
        rows,
        categories,
        total: CategoryTotals { category: Category::Other, long_pct, short_pct, gross_pct: long_pct + short_pct },
        excluded,
    }
}

/// Cheapest-to-deliver analytics for one bond future on one NAV date.
/// Supplied weekly; not derivable from the NAV Recap.
#[derive(Debug, Clone)]
pub struct CtdAnalytics {
    pub mod_duration: f64,
    pub clean_price: f64,
    pub accrued: f64,
    pub conversion_factor: f64,
}

/// DV01 of a single contract, in contract currency.
///
/// `mod_duration * dirty_price/100 * face * 1bp / conversion_factor`, where the
/// deliverable face is `point_value * 100` and the price is quoted per 100 —
/// so the two hundreds cancel and only `point_value` remains.
pub fn dv01_contract(a: &CtdAnalytics, point_value: f64) -> Option<f64> {
    if a.conversion_factor <= 0.0 || point_value <= 0.0 {
        return None;
    }
    let dirty = a.clean_price + a.accrued;
    Some(a.mod_duration * dirty * point_value * 1e-4 / a.conversion_factor)
}

/// DV01 of the held position, in EUR. Negative for a short.
pub fn dv01_position(a: &CtdAnalytics, point_value: f64, qty: f64, fx_rate: f64) -> Option<f64> {
    dv01_contract(a, point_value).map(|d| qty * d * fx_rate)
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

    fn fut(ticker: &str, cat: Category, qty: f64, price: f64, pv: Option<f64>, fx: Option<f64>) -> FuturePosition {
        FuturePosition {
            ticker: ticker.into(),
            name: ticker.into(),
            currency: "EUR".into(),
            category: cat,
            qty: Some(qty),
            price: Some(price),
            point_value: pv,
            fx_rate: fx,
            unconfirmed: false,
        }
    }

    #[test]
    fn notional_is_qty_times_point_value_times_price() {
        let rows = [fut("RXU6 Comdty", Category::InterestRate, -8.0, 124.46, Some(1000.0), Some(1.0))];
        let rep = exposure(&rows, 28_332_753.49);
        let r = &rep.rows[0];
        assert!((r.notional_ccy.unwrap() - -995_680.0).abs() < 1e-6);
        assert!((r.notional_eur.unwrap() - -995_680.0).abs() < 1e-6);
        assert!((r.pct_nav.unwrap() - -0.035142).abs() < 1e-6);
        assert!(!r.spec_missing);
    }

    #[test]
    fn categories_report_long_and_short_in_absolute_value() {
        let aum = 1000.0;
        let rows = [
            fut("A Index", Category::Equity, -1.0, 100.0, Some(1.0), Some(1.0)),   // -100 -> short 10%
            fut("B Comdty", Category::InterestRate, 2.0, 100.0, Some(1.0), Some(1.0)), // +200 -> long 20%
            fut("C Comdty", Category::InterestRate, -1.0, 50.0, Some(1.0), Some(1.0)), // -50 -> short 5%
        ];
        let rep = exposure(&rows, aum);
        let ir = rep.categories.iter().find(|c| c.category == Category::InterestRate).unwrap();
        assert!((ir.long_pct - 0.20).abs() < 1e-12);
        assert!((ir.short_pct - 0.05).abs() < 1e-12, "shorts are reported positive");
        assert!((ir.gross_pct - 0.25).abs() < 1e-12);

        let eq = rep.categories.iter().find(|c| c.category == Category::Equity).unwrap();
        assert!((eq.long_pct - 0.0).abs() < 1e-12);
        assert!((eq.short_pct - 0.10).abs() < 1e-12);

        // all six categories are always present, empty ones included
        assert_eq!(rep.categories.len(), 6);
        assert!((rep.total.long_pct - 0.20).abs() < 1e-12);
        assert!((rep.total.short_pct - 0.15).abs() < 1e-12);
        assert!((rep.total.gross_pct - 0.35).abs() < 1e-12);
    }

    #[test]
    fn missing_spec_or_fx_excludes_from_totals_but_keeps_the_row() {
        let rows = [
            fut("A Comdty", Category::InterestRate, -8.0, 124.46, None, Some(1.0)),      // no point value
            fut("B Comdty", Category::InterestRate, -8.0, 124.46, Some(1000.0), None),   // no fx rate
        ];
        let rep = exposure(&rows, 1000.0);
        assert_eq!(rep.rows.len(), 2, "rows are always listed");
        assert!(rep.rows[0].spec_missing);
        assert_eq!(rep.rows[0].notional_ccy, None);
        assert_eq!(rep.rows[1].notional_eur, None, "no fx rate -> no EUR notional");
        assert!(rep.rows[1].notional_ccy.is_some(), "ccy notional still computable");
        assert_eq!(rep.excluded.len(), 2);
        assert!((rep.total.gross_pct - 0.0).abs() < 1e-12, "neither reaches the totals");
    }

    // A missing price or quantity is not a zero. `ingest::cell_f64` returns
    // `None` with no row error for an empty cell and for a `#VALUE!`/`#N/A`
    // formula artifact, which is an ordinary state for a Bloomberg-linked price
    // cell - so this input is reachable, and treating it as 0.0 produced a
    // populated, confident, wrong row: `Qty 0 - Price 0,00 - % NAV 0,00%`, not
    // excluded from any total and flagged nowhere.
    #[test]
    fn missing_price_or_quantity_is_excluded_not_zeroed() {
        let mut no_price = fut("A Comdty", Category::InterestRate, -8.0, 0.0, Some(1000.0), Some(1.0));
        no_price.price = None;
        let mut no_qty = fut("B Comdty", Category::InterestRate, 0.0, 124.46, Some(1000.0), Some(1.0));
        no_qty.qty = None;
        let rep = exposure(&[no_price, no_qty], 1000.0);

        assert_eq!(rep.rows.len(), 2, "rows are always listed");
        for r in &rep.rows {
            assert_eq!(r.notional_ccy, None, "{}", r.ticker);
            assert_eq!(r.pct_nav, None, "{}", r.ticker);
            assert!(r.inputs_missing, "{}", r.ticker);
            assert!(!r.spec_missing, "the spec is fine - the workbook row is not: {}", r.ticker);
        }
        assert_eq!(rep.excluded.len(), 2, "both must surface in `excluded`");
        assert!((rep.total.gross_pct - 0.0).abs() < 1e-12);
    }

    // The other half of the same invariant: a genuine zero must still compute.
    // Together these pin `pct_nav == Some(0.0)` to mean "this position really is
    // flat", never "an input was missing" - the boundary case whose absence let
    // the bug above through.
    #[test]
    fn genuinely_zero_position_reports_zero_rather_than_missing() {
        let rows = [
            fut("A Comdty", Category::InterestRate, 0.0, 124.46, Some(1000.0), Some(1.0)),
            fut("B Comdty", Category::InterestRate, -8.0, 0.0, Some(1000.0), Some(1.0)),
        ];
        let rep = exposure(&rows, 1000.0);
        for r in &rep.rows {
            assert_eq!(r.pct_nav, Some(0.0), "a real zero is Some(0.0): {}", r.ticker);
            assert!(!r.inputs_missing, "{}", r.ticker);
        }
        assert!(rep.excluded.is_empty(), "a computable zero is not an exclusion");
    }

    #[test]
    fn zero_aum_excludes_everything_without_dividing_by_zero() {
        let rows = [fut("A Index", Category::Equity, -1.0, 100.0, Some(1.0), Some(1.0))];
        let rep = exposure(&rows, 0.0);
        assert_eq!(rep.rows[0].pct_nav, None);
        assert_eq!(rep.excluded.len(), 1);
        assert!((rep.total.gross_pct - 0.0).abs() < 1e-12);
    }

    fn ctd() -> CtdAnalytics {
        CtdAnalytics { mod_duration: 8.41, clean_price: 98.72, accrued: 0.63, conversion_factor: 0.782145 }
    }

    #[test]
    fn dv01_per_contract_matches_hand_computation() {
        // dirty = 99.35; 8.41 * 99.35 * 1000 * 1e-4 = 83.55335; / 0.782145 = 106.8259
        let d = dv01_contract(&ctd(), 1000.0).unwrap();
        assert!((d - 106.8259).abs() < 1e-3, "got {d}");
    }

    #[test]
    fn dv01_position_scales_by_quantity_and_fx() {
        let per = dv01_contract(&ctd(), 1000.0).unwrap();
        let pos = dv01_position(&ctd(), 1000.0, -8.0, 1.0).unwrap();
        assert!((pos - -8.0 * per).abs() < 1e-9, "a short is negative DV01");
        let usd = dv01_position(&ctd(), 1000.0, -6.0, 0.87881185).unwrap();
        assert!((usd - -6.0 * per * 0.87881185).abs() < 1e-9);
    }

    #[test]
    fn dv01_rejects_degenerate_inputs() {
        let mut a = ctd();
        a.conversion_factor = 0.0;
        assert_eq!(dv01_contract(&a, 1000.0), None);
        assert_eq!(dv01_contract(&ctd(), 0.0), None);
        assert_eq!(dv01_position(&ctd(), 0.0, -8.0, 1.0), None);
    }
}
