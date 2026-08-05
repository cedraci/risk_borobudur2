pub mod bloomberg;
pub mod futures_file;
pub use futures_file::{CtdRow, parse_ctd_file};

use calamine::{Data, Range, Reader, Xlsx};
use chrono::{Days, NaiveDate};
use std::io::Cursor;

#[derive(Debug)]
pub struct ParsedWorkbook {
    pub nav_date: NaiveDate,
    pub aum: f64,
    pub shares: f64,
    pub nav: f64,
    pub positions: Vec<PositionRow>,
    pub nav_history: Vec<NavHistoryRow>,
    pub dividends: Vec<DividendRow>,
    pub operations: Vec<OperationRow>,
}

#[derive(Debug, Clone)]
pub struct PositionRow {
    pub asset_type: String,
    pub isin: String,
    pub name: Option<String>,
    pub currency: Option<String>,
    pub quantity: Option<f64>,
    pub avg_cost: Option<f64>,
    pub price: Option<f64>,
    pub valuation_ccy: Option<f64>,
    pub accrued_interest: Option<f64>,
    pub fx_rate: Option<f64>,
    pub valuation_eur: Option<f64>,
    pub weight: Option<f64>,
    pub ticker: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NavHistoryRow {
    pub date: NaiveDate,
    pub aum: f64,
    pub shares: f64,
    pub nav: f64,
}

#[derive(Debug, Clone)]
pub struct DividendRow {
    pub provision_date: NaiveDate,
    pub payment_date: Option<NaiveDate>,
    pub issuer: String,
    pub amount: f64,
    pub currency: String,
}

#[derive(Debug, Clone)]
pub struct OperationRow {
    pub trade_date: NaiveDate,
    pub side: String,
    pub ticker: Option<String>,
    pub isin: Option<String>,
    pub name: Option<String>,
    pub currency: Option<String>,
    pub quantity: Option<f64>,
    pub price: Option<f64>,
    pub gross_amount: Option<f64>,
    pub fees: Option<f64>,
    pub net_price: Option<f64>,
    pub net_amount: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RowError {
    pub sheet: String,
    /// 1-based Excel row number.
    pub row: u32,
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ParseFailure {
    #[error("workbook error: {0}")]
    Workbook(String),
    #[error("{} row error(s)", .0.len())]
    Rows(Vec<RowError>),
}

struct Ctx {
    errors: Vec<RowError>,
}

impl Ctx {
    fn err(&mut self, sheet: &str, row0: u32, msg: impl Into<String>) {
        self.errors.push(RowError { sheet: sheet.into(), row: row0 + 1, message: msg.into() });
    }
}

pub fn parse_workbook(bytes: &[u8]) -> Result<ParsedWorkbook, ParseFailure> {
    let mut wb: Xlsx<_> = Xlsx::new(Cursor::new(bytes.to_vec()))
        .map_err(|e| ParseFailure::Workbook(e.to_string()))?;
    let pf = sheet(&mut wb, "PORTEFEUILLE_NAV")?;
    let hist = sheet(&mut wb, "HISTO_NAV")?;
    let div = sheet(&mut wb, "DIV")?;
    let ops = sheet(&mut wb, "OPERATIONS")?;

    let mut ctx = Ctx { errors: Vec::new() };

    let nav_date = req_date(&pf, 2, 1, "PORTEFEUILLE_NAV", &mut ctx); // B3
    let aum = req_f64(&pf, 1, 5, "PORTEFEUILLE_NAV", &mut ctx); // F2
    let shares = req_f64(&pf, 2, 5, "PORTEFEUILLE_NAV", &mut ctx); // F3
    let nav = req_f64(&pf, 3, 5, "PORTEFEUILLE_NAV", &mut ctx); // F4

    let positions = parse_positions(&pf, &mut ctx);
    let nav_history = parse_hist(&hist, &mut ctx);
    let dividends = parse_div(&div, &mut ctx);
    let operations = parse_ops(&ops, &mut ctx);

    if let (Some(nd), false) = (nav_date, nav_history.is_empty()) {
        if let Some(bad) = nav_history.iter().find(|r| r.date > nd) {
            ctx.err("HISTO_NAV", 0, format!("date {} is after the file's NAV date {}", bad.date, nd));
        }
    }

    if !ctx.errors.is_empty() {
        return Err(ParseFailure::Rows(ctx.errors));
    }
    Ok(ParsedWorkbook {
        nav_date: nav_date.unwrap(),
        aum: aum.unwrap(),
        shares: shares.unwrap(),
        nav: nav.unwrap(),
        positions,
        nav_history,
        dividends,
        operations,
    })
}

fn sheet(wb: &mut Xlsx<Cursor<Vec<u8>>>, name: &str) -> Result<Range<Data>, ParseFailure> {
    wb.worksheet_range(name)
        .map_err(|e| ParseFailure::Workbook(format!("sheet {name}: {e}")))
}

// ---- cell helpers (absolute 0-based coordinates) ----

fn get<'a>(r: &'a Range<Data>, row: u32, col: u32) -> Option<&'a Data> {
    r.get_value((row, col)).filter(|d| !matches!(d, Data::Empty))
}

fn cell_str(r: &Range<Data>, row: u32, col: u32) -> Option<String> {
    match get(r, row, col) {
        Some(Data::String(s)) => {
            let t = s.trim();
            if t.is_empty() { None } else { Some(t.to_string()) }
        }
        Some(other) => Some(other.to_string()),
        None => None,
    }
}

fn cell_f64(r: &Range<Data>, row: u32, col: u32, sheet: &str, ctx: &mut Ctx) -> Option<f64> {
    match get(r, row, col) {
        Some(Data::Float(f)) => Some(*f),
        Some(Data::Int(i)) => Some(*i as f64),
        Some(Data::String(s)) if s.trim().is_empty() => None,
        // formula artifact (e.g. #VALUE!) left in the source workbook: treat as missing, not garbage.
        Some(Data::Error(_)) => None,
        Some(other) => {
            ctx.err(sheet, row, format!("col {}: expected number, got {other:?}", col + 1));
            None
        }
        None => None,
    }
}

fn cell_date(r: &Range<Data>, row: u32, col: u32, sheet: &str, ctx: &mut Ctx) -> Option<NaiveDate> {
    match get(r, row, col) {
        Some(Data::DateTime(dt)) => match dt.as_datetime() {
            Some(ndt) => Some(ndt.date()),
            None => {
                ctx.err(sheet, row, format!("col {}: invalid Excel datetime", col + 1));
                None
            }
        },
        // raw serial number with date formatting stripped
        Some(Data::Float(f)) => NaiveDate::from_ymd_opt(1899, 12, 30)
            .unwrap()
            .checked_add_days(Days::new(*f as u64)),
        // date stored as plain text (e.g. " 03/11/2025"), a known quirk in this workbook.
        Some(Data::String(s)) => match NaiveDate::parse_from_str(s.trim(), "%d/%m/%Y") {
            Ok(d) => Some(d),
            Err(_) => {
                ctx.err(sheet, row, format!("col {}: expected date, got string {s:?}", col + 1));
                None
            }
        },
        Some(other) => {
            ctx.err(sheet, row, format!("col {}: expected date, got {other:?}", col + 1));
            None
        }
        None => None,
    }
}

fn req_f64(r: &Range<Data>, row: u32, col: u32, sheet: &str, ctx: &mut Ctx) -> Option<f64> {
    let v = cell_f64(r, row, col, sheet, ctx);
    if v.is_none() && ctx.errors.last().map(|e| e.row) != Some(row + 1) {
        ctx.err(sheet, row, format!("col {}: required number missing", col + 1));
    }
    v
}

fn req_date(r: &Range<Data>, row: u32, col: u32, sheet: &str, ctx: &mut Ctx) -> Option<NaiveDate> {
    let v = cell_date(r, row, col, sheet, ctx);
    if v.is_none() && ctx.errors.last().map(|e| e.row) != Some(row + 1) {
        ctx.err(sheet, row, format!("col {}: required date missing", col + 1));
    }
    v
}

// ---- sheet parsers ----

fn parse_positions(r: &Range<Data>, ctx: &mut Ctx) -> Vec<PositionRow> {
    const SHEET: &str = "PORTEFEUILLE_NAV";
    let mut out = Vec::new();
    let mut cash_section = false;
    let end = r.end().map(|(er, _)| er).unwrap_or(0);
    for row in 7..=end {
        let Some(asset_type) = cell_str(r, row, 0) else { continue };
        if asset_type == "Type" {
            cash_section = true; // sub-header switches the column layout
            continue;
        }
        let Some(isin) = cell_str(r, row, 1) else { continue }; // separator rows (e.g. "CASH")
        let p = if !cash_section {
            PositionRow {
                asset_type,
                isin,
                name: cell_str(r, row, 2),
                currency: cell_str(r, row, 3),
                quantity: cell_f64(r, row, 4, SHEET, ctx),
                avg_cost: cell_f64(r, row, 5, SHEET, ctx),
                price: cell_f64(r, row, 6, SHEET, ctx),
                valuation_ccy: cell_f64(r, row, 7, SHEET, ctx),
                accrued_interest: cell_f64(r, row, 8, SHEET, ctx),
                fx_rate: cell_f64(r, row, 9, SHEET, ctx),
                valuation_eur: cell_f64(r, row, 10, SHEET, ctx),
                weight: cell_f64(r, row, 11, SHEET, ctx),
                ticker: cell_str(r, row, 12),
            }
        } else {
            PositionRow {
                asset_type,
                isin,
                name: cell_str(r, row, 2),
                currency: cell_str(r, row, 3),
                quantity: cell_f64(r, row, 4, SHEET, ctx),
                avg_cost: None,
                price: None,
                valuation_ccy: cell_f64(r, row, 5, SHEET, ctx),
                valuation_eur: cell_f64(r, row, 6, SHEET, ctx),
                accrued_interest: cell_f64(r, row, 7, SHEET, ctx),
                weight: cell_f64(r, row, 8, SHEET, ctx),
                fx_rate: None,
                ticker: None,
            }
        };
        out.push(p);
    }
    out
}

fn parse_hist(r: &Range<Data>, ctx: &mut Ctx) -> Vec<NavHistoryRow> {
    const SHEET: &str = "HISTO_NAV";
    let mut out = Vec::new();
    let end = r.end().map(|(er, _)| er).unwrap_or(0);
    for row in 1..=end {
        if get(r, row, 0).is_none() { continue; }
        let date = req_date(r, row, 0, SHEET, ctx);
        let aum = req_f64(r, row, 1, SHEET, ctx);
        let shares = req_f64(r, row, 2, SHEET, ctx);
        let nav = req_f64(r, row, 3, SHEET, ctx);
        if let (Some(date), Some(aum), Some(shares), Some(nav)) = (date, aum, shares, nav) {
            out.push(NavHistoryRow { date, aum, shares, nav });
        }
    }
    out.sort_by_key(|x| x.date);
    out
}

fn parse_div(r: &Range<Data>, ctx: &mut Ctx) -> Vec<DividendRow> {
    const SHEET: &str = "DIV";
    let mut out = Vec::new();
    let end = r.end().map(|(er, _)| er).unwrap_or(0);
    for row in 1..=end {
        if get(r, row, 0).is_none() { continue; }
        let provision_date = req_date(r, row, 0, SHEET, ctx);
        let payment_date = cell_date(r, row, 1, SHEET, ctx);
        let issuer = cell_str(r, row, 2);
        let amount = req_f64(r, row, 3, SHEET, ctx);
        let currency = cell_str(r, row, 4);
        if issuer.is_none() { ctx.err(SHEET, row, "issuer missing"); }
        if currency.is_none() { ctx.err(SHEET, row, "currency missing"); }
        if let (Some(pd), Some(issuer), Some(amount), Some(currency)) = (provision_date, issuer, amount, currency) {
            out.push(DividendRow { provision_date: pd, payment_date, issuer, amount, currency });
        }
    }
    out
}

fn parse_ops(r: &Range<Data>, ctx: &mut Ctx) -> Vec<OperationRow> {
    const SHEET: &str = "OPERATIONS";
    let mut out = Vec::new();
    let end = r.end().map(|(er, _)| er).unwrap_or(0);
    for row in 3..=end {
        if get(r, row, 0).is_none() { continue; }
        let trade_date = req_date(r, row, 0, SHEET, ctx);
        let side = cell_str(r, row, 1);
        if side.is_none() { ctx.err(SHEET, row, "side missing"); }
        let rec = OperationRow {
            trade_date: match trade_date { Some(d) => d, None => continue },
            side: match side { Some(s) => s, None => continue },
            ticker: cell_str(r, row, 2),
            isin: cell_str(r, row, 3),
            name: cell_str(r, row, 4),
            currency: cell_str(r, row, 5),
            quantity: cell_f64(r, row, 6, SHEET, ctx),
            price: cell_f64(r, row, 7, SHEET, ctx),
            gross_amount: cell_f64(r, row, 8, SHEET, ctx),
            fees: cell_f64(r, row, 9, SHEET, ctx),
            net_price: cell_f64(r, row, 10, SHEET, ctx),
            net_amount: cell_f64(r, row, 11, SHEET, ctx),
        };
        out.push(rec);
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct BondStatics {
    pub coupon_pct: f64,
    pub maturity: chrono::NaiveDate,
    pub coupon_freq: i32,
}

/// Parse coupon and maturity from a bond position name like
/// "BRAZILIAN GOVERNMENT INTL BOND 6.625% 15-03-35". The maturity must
/// appear after the coupon; 2-digit years are 20YY. Frequency defaults to
/// 2 (semi-annual) for USD bonds, 1 otherwise.
pub fn parse_bond_statics(name: &str, currency: Option<&str>) -> Option<BondStatics> {
    let coupon_re = regex::Regex::new(r"(\d+(?:[.,]\d+)?)\s*%").unwrap();
    let mat_re = regex::Regex::new(r"(\d{2})-(\d{2})-(\d{2,4})").unwrap();
    let cm = coupon_re.captures(name)?;
    let coupon_pct: f64 = cm.get(1)?.as_str().replace(',', ".").parse().ok()?;
    let tail = &name[cm.get(0)?.end()..];
    let mm = mat_re.captures(tail)?;
    let day: u32 = mm.get(1)?.as_str().parse().ok()?;
    let month: u32 = mm.get(2)?.as_str().parse().ok()?;
    let ytxt = mm.get(3)?.as_str();
    let year: i32 = if ytxt.len() == 2 { 2000 + ytxt.parse::<i32>().ok()? } else { ytxt.parse().ok()? };
    let maturity = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
    let coupon_freq = if currency == Some("USD") { 2 } else { 1 };
    Some(BondStatics { coupon_pct, maturity, coupon_freq })
}

#[cfg(test)]
mod bond_tests {
    use super::*;

    #[test]
    fn parses_standard_us_sovereign() {
        let b = parse_bond_statics("BRAZILIAN GOVERNMENT INTL BOND 6.625% 15-03-35", Some("USD")).unwrap();
        assert!((b.coupon_pct - 6.625).abs() < 1e-12);
        assert_eq!(b.maturity, chrono::NaiveDate::from_ymd_opt(2035, 3, 15).unwrap());
        assert_eq!(b.coupon_freq, 2);
    }

    #[test]
    fn parses_comma_decimal_and_eur_freq() {
        let b = parse_bond_statics("FRANCE OAT 2,50% 25-05-30", Some("EUR")).unwrap();
        assert!((b.coupon_pct - 2.5).abs() < 1e-12);
        assert_eq!(b.maturity, chrono::NaiveDate::from_ymd_opt(2030, 5, 25).unwrap());
        assert_eq!(b.coupon_freq, 1);
    }

    #[test]
    fn parses_four_digit_year() {
        let b = parse_bond_statics("XYZ 5% 01-01-2040", None).unwrap();
        assert_eq!(b.maturity, chrono::NaiveDate::from_ymd_opt(2040, 1, 1).unwrap());
        assert_eq!(b.coupon_freq, 1);
    }

    #[test]
    fn rejects_names_without_both_parts() {
        assert!(parse_bond_statics("NO COUPON HERE 15-03-35", Some("USD")).is_none());
        assert!(parse_bond_statics("COUPON ONLY 5%", Some("USD")).is_none());
        // maturity must come AFTER the coupon
        assert!(parse_bond_statics("15-03-35 THEN 5%", None).is_none());
        // invalid calendar date
        assert!(parse_bond_statics("BAD DATE 5% 32-13-35", None).is_none());
    }
}
