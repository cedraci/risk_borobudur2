//! CACEIS Bank Luxembourg adapter. Files are semicolon-delimited,
//! headerless, Latin-1, dates `yyyymmdd`, numbers space-padded with
//! trailing dots ("8336.23333333", "-12."). Column indices come from the
//! depositary's header glossary ("Glossary GP CSV Headers.xlsx") and are
//! the single place to edit if CACEIS changes the layout.

use crate::adapter::{RefFact, RefHint, Snapshot, UniversalBatch};
use crate::{ParseFailure, PositionRow};
use chrono::NaiveDate;

pub const SOURCE: &str = "caceis";

// HISINVLUX columns (0-based).
const H_NAV_DATE: usize = 0;
const H_FUND_CODE: usize = 3;
const H_CATVAL: usize = 5;      // VMOB / FUTU / TRES / CPON
const H_INSTR_CODE: usize = 6;  // fallback code when no ISIN (futures, cash accounts)
const H_NAME: usize = 8;
const H_ASSET_CCY: usize = 9;
const H_GP3: usize = 16;        // detail type: 11101, 12400, 18120, COMPTE, MARGES, FP...
const H_QUANTITY: usize = 25;
const H_MARKET_PRICE: usize = 28;
const H_UNIT_COST: usize = 30;
const H_MV_FUND_CCY: usize = 32;
const H_ACCRUED_FUND_CCY: usize = 33;
const H_WEIGHT_PCT: usize = 35; // percent of TNA; the universal model wants a fraction
const H_RISK_COUNTRY: usize = 41; // ISO alpha-3
const H_ISIN: usize = 45;
const H_MATURITY: usize = 49;      // "Maturity Date"
const H_MV_LOCAL: usize = 51;
const H_NOMINAL: usize = 56;       // "Nominal" — the denomination prices quote against
const H_NEXT_COUPON: usize = 57;   // "Next coupon date"
const H_COUPON_TYPE: usize = 59;   // "Coupon Type" — only FIX yields coupons
const H_COUPON_RATE: usize = 60;   // "Coupon rate"
const H_MARKET_PLACE: usize = 63;  // "Market place"
const H_MARKET_NAME: usize = 64;   // "Market place Description"
const H_BLOOMBERG: usize = 65;
const H_MIN_FIELDS: usize = 66;

/// `HISINVLUX_165878_20260807_20260810130151.csv` -> ("165878", 2026-08-07).
/// Case-insensitive on the prefix; also used for HISTOVLLUX by Task 4.
pub fn filename_meta(filename: &str) -> Option<(String, NaiveDate)> {
    let re = regex::Regex::new(r"(?i)^[A-Z]+_(\d+)_(\d{8})_\d+\.csv$").unwrap();
    let caps = re.captures(filename)?;
    let code = caps.get(1)?.as_str().to_string();
    let date = NaiveDate::parse_from_str(caps.get(2)?.as_str(), "%Y%m%d").ok()?;
    Some((code, date))
}

fn decode_latin1(bytes: &[u8]) -> String {
    // Latin-1 maps byte n to Unicode code point n; no external crate needed.
    bytes.iter().map(|&b| b as char).collect()
}

fn field<'a>(fields: &'a [&str], i: usize) -> &'a str {
    fields.get(i).map(|s| s.trim()).unwrap_or("")
}

fn num(fields: &[&str], i: usize) -> Option<f64> {
    let t = field(fields, i);
    if t.is_empty() { None } else { t.parse::<f64>().ok() }
}

fn text(fields: &[&str], i: usize) -> Option<String> {
    let t = field(fields, i);
    if t.is_empty() { None } else { Some(t.to_string()) }
}

fn date(fields: &[&str], i: usize) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(field(fields, i), "%Y%m%d").ok()
}

/// CACEIS category + detail code -> the closed universal vocabulary.
/// `None` = unmappable; the row is dropped with a warning (signal, don't hide).
fn asset_type_of(catval: &str, gp3: &str) -> Option<&'static str> {
    match catval {
        "CPON" => Some("Dividendes"),
        "VMOB" if gp3.starts_with("111") => Some("Action"),
        "VMOB" if gp3.starts_with("12") => Some("Fonds"),
        "VMOB" if gp3.starts_with("13") => Some("Obligation"),
        "FUTU" if gp3.starts_with("18") => Some("Future"),
        "TRES" => match gp3 {
            "COMPTE" => Some("Cash Acc"),
            "MARGES" => Some("Margin Acc"),
            "FP" | "PF" => Some("Frais provisionnés"),
            "PS" | "PU" => Some("Provisions ordres"),
            _ => None,
        },
        _ => None,
    }
}

/// Risk-country ISO alpha-3 -> the full names the Bloomberg pipeline stores
/// (see `bloomberg::region_for`). Unknown codes yield no country hint.
fn country_name(alpha3: &str) -> Option<&'static str> {
    Some(match alpha3 {
        "FRA" => "France", "DEU" => "Germany", "ITA" => "Italy", "ESP" => "Spain",
        "NLD" => "Netherlands", "BEL" => "Belgium", "AUT" => "Austria", "PRT" => "Portugal",
        "IRL" => "Ireland", "LUX" => "Luxembourg", "FIN" => "Finland", "GRC" => "Greece",
        "GBR" => "United Kingdom", "CHE" => "Switzerland", "SWE" => "Sweden", "NOR" => "Norway",
        "DNK" => "Denmark", "POL" => "Poland", "CZE" => "Czech Republic",
        "USA" => "United States", "CAN" => "Canada",
        "BRA" => "Brazil", "MEX" => "Mexico", "CHL" => "Chile", "ARG" => "Argentina",
        "COL" => "Colombia", "PER" => "Peru",
        "JPN" => "Japan", "CHN" => "China", "HKG" => "Hong Kong", "KOR" => "South Korea",
        "TWN" => "Taiwan", "SGP" => "Singapore", "IND" => "India", "AUS" => "Australia",
        "NZL" => "New Zealand", "IDN" => "Indonesia", "THA" => "Thailand", "MYS" => "Malaysia",
        "ZAF" => "South Africa", "ARE" => "United Arab Emirates", "SAU" => "Saudi Arabia",
        "ISR" => "Israel", "TUR" => "Turkey", "QAT" => "Qatar", "EGY" => "Egypt",
        "NGA" => "Nigeria", "MAR" => "Morocco",
        _ => return None,
    })
}

pub fn parse_hisinv(filename: &str, bytes: &[u8]) -> Result<UniversalBatch, ParseFailure> {
    let (fund_code, file_date) = filename_meta(filename)
        .ok_or_else(|| ParseFailure::Workbook(format!("filename {filename:?} does not match HISINVLUX_<fund>_<yyyymmdd>_<ts>.csv")))?;

    let textual = decode_latin1(bytes);
    let mut positions: Vec<PositionRow> = Vec::new();
    let mut ref_hints: Vec<RefHint> = Vec::new();
    let mut ref_facts: Vec<RefFact> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    for (i, line) in textual.lines().enumerate() {
        let lineno = i + 1;
        if line.trim().is_empty() { continue; }
        let fields: Vec<&str> = line.split(';').collect();
        if fields.len() < H_MIN_FIELDS {
            return Err(ParseFailure::Workbook(format!(
                "line {lineno}: {} columns, expected at least {H_MIN_FIELDS} — not a HISINVLUX layout", fields.len())));
        }
        let row_date = NaiveDate::parse_from_str(field(&fields, H_NAV_DATE), "%Y%m%d")
            .map_err(|_| ParseFailure::Workbook(format!("line {lineno}: bad NAV date {:?}", field(&fields, H_NAV_DATE))))?;
        if row_date != file_date {
            return Err(ParseFailure::Workbook(format!(
                "line {lineno}: row date {row_date} differs from filename date {file_date}")));
        }
        if field(&fields, H_FUND_CODE) != fund_code {
            return Err(ParseFailure::Workbook(format!(
                "line {lineno}: fund code {:?} differs from filename code {fund_code:?}", field(&fields, H_FUND_CODE))));
        }

        let catval = field(&fields, H_CATVAL);
        let gp3 = field(&fields, H_GP3);
        let Some(asset_type) = asset_type_of(catval, gp3) else {
            warnings.push(format!("line {lineno}: unmappable asset code {catval}/{gp3} — row dropped ({})",
                field(&fields, H_NAME)));
            continue;
        };

        let isin = text(&fields, H_ISIN).or_else(|| text(&fields, H_INSTR_CODE));
        let Some(isin) = isin else {
            warnings.push(format!("line {lineno}: no ISIN or instrument code — row dropped"));
            continue;
        };

        let valuation_eur = num(&fields, H_MV_FUND_CCY);
        let valuation_ccy = num(&fields, H_MV_LOCAL);
        let currency = text(&fields, H_ASSET_CCY);
        let is_cashlike = matches!(asset_type, "Cash Acc" | "Margin Acc" | "Frais provisionnés" | "Provisions ordres" | "Dividendes");
        let fx_rate = if currency.as_deref() == Some("EUR") {
            Some(1.0)
        } else {
            match (valuation_eur, valuation_ccy) {
                (Some(e), Some(l)) if l.abs() > 1e-12 => Some(e / l),
                _ => None,
            }
        };
        let ticker = text(&fields, H_BLOOMBERG).filter(|t| t != "-1");

        if catval == "VMOB" {
            let country = text(&fields, H_RISK_COUNTRY)
                .and_then(|a3| country_name(&a3).map(str::to_string));
            let region = country.as_deref().and_then(crate::bloomberg::region_for).map(str::to_string);
            if country.is_some() || ticker.is_some() {
                ref_hints.push(RefHint {
                    isin: isin.clone(),
                    country_of_risk: country,
                    region,
                    ticker: ticker.clone(),
                });
            }
        }

        let coupon_type = text(&fields, H_COUPON_TYPE);
        let fixed = coupon_type.as_deref().is_some_and(|t| t.eq_ignore_ascii_case("FIX"));
        let fact = RefFact {
            isin: isin.clone(),
            market_place: text(&fields, H_MARKET_PLACE),
            market_place_name: text(&fields, H_MARKET_NAME),
            // Coupon statics only where the instrument actually carries a
            // fixed coupon; an equity row's blank columns must not write NULLs
            // over a bond's data if the same code ever appears twice.
            bond_maturity: if fixed { date(&fields, H_MATURITY) } else { None },
            bond_next_coupon: if fixed { date(&fields, H_NEXT_COUPON) } else { None },
            bond_coupon_pct: if fixed { num(&fields, H_COUPON_RATE) } else { None },
            bond_nominal: if fixed { num(&fields, H_NOMINAL).filter(|n| *n > 0.0) } else { None },
            bond_coupon_freq: None, // HISINVLUX does not carry it
        };
        if fact.market_place.is_some() || fact.bond_maturity.is_some() {
            ref_facts.push(fact);
        }

        positions.push(PositionRow {
            asset_type: asset_type.to_string(),
            isin,
            name: text(&fields, H_NAME),
            currency,
            quantity: num(&fields, H_QUANTITY),
            avg_cost: if is_cashlike { None } else { num(&fields, H_UNIT_COST) },
            price: if is_cashlike { None } else { num(&fields, H_MARKET_PRICE) },
            valuation_ccy,
            accrued_interest: num(&fields, H_ACCRUED_FUND_CCY),
            fx_rate,
            valuation_eur,
            weight: num(&fields, H_WEIGHT_PCT).map(|w| w / 100.0),
            ticker,
        });
    }

    if positions.is_empty() {
        return Err(ParseFailure::Workbook("no position rows found".into()));
    }

    Ok(UniversalBatch {
        primary_date: file_date,
        nav_points: Vec::new(),
        snapshots: vec![Snapshot { nav_date: file_date, positions }],
        dividends: None,
        operations: None,
        flows: None,
        ref_hints,
        ref_facts,
        warnings,
    })
}

// HISTOVLLUX columns (0-based).
const V_FUND_CODE: usize = 0;
const V_NAV_DATE: usize = 2;
const V_SHARE_CLASS: usize = 3;
const V_NAV: usize = 5;
const V_TNA: usize = 6;
const V_OUTSTANDING: usize = 7;
const V_MIN_FIELDS: usize = 20;

pub fn parse_histovl(filename: &str, bytes: &[u8]) -> Result<UniversalBatch, ParseFailure> {
    let (fund_code, file_date) = filename_meta(filename)
        .ok_or_else(|| ParseFailure::Workbook(format!("filename {filename:?} does not match HISTOVLLUX_<fund>_<yyyymmdd>_<ts>.csv")))?;

    let textual = decode_latin1(bytes);
    let mut rows: Vec<(String, crate::NavHistoryRow)> = Vec::new();
    for (i, line) in textual.lines().enumerate() {
        let lineno = i + 1;
        if line.trim().is_empty() { continue; }
        let fields: Vec<&str> = line.split(';').collect();
        if fields.len() < V_MIN_FIELDS {
            return Err(ParseFailure::Workbook(format!(
                "line {lineno}: {} columns, expected at least {V_MIN_FIELDS} — not a HISTOVLLUX layout", fields.len())));
        }
        if field(&fields, V_FUND_CODE) != fund_code {
            return Err(ParseFailure::Workbook(format!(
                "line {lineno}: fund code {:?} differs from filename code {fund_code:?}", field(&fields, V_FUND_CODE))));
        }
        let date = NaiveDate::parse_from_str(field(&fields, V_NAV_DATE), "%Y%m%d")
            .map_err(|_| ParseFailure::Workbook(format!("line {lineno}: bad NAV date {:?}", field(&fields, V_NAV_DATE))))?;
        if date != file_date {
            return Err(ParseFailure::Workbook(format!(
                "line {lineno}: row date {date} differs from filename date {file_date}")));
        }
        let (Some(nav), Some(aum), Some(shares)) = (num(&fields, V_NAV), num(&fields, V_TNA), num(&fields, V_OUTSTANDING)) else {
            return Err(ParseFailure::Workbook(format!("line {lineno}: NAV/TNA/outstanding missing or unparsable")));
        };
        rows.push((field(&fields, V_SHARE_CLASS).to_string(), crate::NavHistoryRow { date, aum, shares, nav }));
    }

    match rows.len() {
        0 => Err(ParseFailure::Workbook("no NAV rows found".into())),
        1 => {
            let (_, nav_point) = rows.into_iter().next().unwrap();
            Ok(UniversalBatch {
                primary_date: file_date,
                nav_points: vec![nav_point],
                snapshots: Vec::new(),
                dividends: None,
                operations: None,
                flows: None,
                ref_hints: Vec::new(),
                ref_facts: Vec::new(),
                warnings: Vec::new(),
            })
        }
        _ => {
            let classes: Vec<String> = rows.iter().map(|(c, _)| c.clone()).collect();
            Err(ParseFailure::Workbook(format!(
                "multi share class not supported yet (classes {classes:?}) — a silent sum would make NAV-per-share analytics meaningless")))
        }
    }
}

// JOURSRLUX columns (0-based), from the depositary glossary.
const R_FUND_CODE: usize = 0;
const R_NAV_DATE: usize = 1;
const R_SHARE_CLASS: usize = 2;
const R_OUTSTANDING: usize = 3;
const R_NAV_PER_SHARE: usize = 4;
const R_SUB_AMOUNT: usize = 6;
const R_RED_AMOUNT: usize = 8;
const R_MIN_FIELDS: usize = 15;

/// Daily subscriptions/redemptions per share class. Unlike HISTOVLLUX,
/// nothing here divides by a fund-level share count, so multiple share
/// classes on one date are normal and all stored — no multi-class rejection.
pub fn parse_joursr(filename: &str, bytes: &[u8]) -> Result<UniversalBatch, ParseFailure> {
    let (fund_code, file_date) = filename_meta(filename)
        .ok_or_else(|| ParseFailure::Workbook(format!(
            "filename {filename:?} does not match JOURSRLUX_<fund>_<yyyymmdd>_<ts>.csv")))?;

    let textual = decode_latin1(bytes);
    let mut rows: Vec<crate::ShareClassFlowRow> = Vec::new();
    for (i, line) in textual.lines().enumerate() {
        let lineno = i + 1;
        if line.trim().is_empty() { continue; }
        let fields: Vec<&str> = line.split(';').collect();
        if fields.len() < R_MIN_FIELDS {
            return Err(ParseFailure::Workbook(format!(
                "line {lineno}: {} columns, expected at least {R_MIN_FIELDS} — not a JOURSRLUX layout",
                fields.len())));
        }
        if field(&fields, R_FUND_CODE) != fund_code {
            return Err(ParseFailure::Workbook(format!(
                "line {lineno}: fund code {:?} differs from filename code {fund_code:?}",
                field(&fields, R_FUND_CODE))));
        }
        let row_date = NaiveDate::parse_from_str(field(&fields, R_NAV_DATE), "%Y%m%d")
            .map_err(|_| ParseFailure::Workbook(format!(
                "line {lineno}: bad NAV date {:?}", field(&fields, R_NAV_DATE))))?;
        if row_date != file_date {
            return Err(ParseFailure::Workbook(format!(
                "line {lineno}: row date {row_date} differs from filename date {file_date}")));
        }
        let share_class = field(&fields, R_SHARE_CLASS).to_string();
        if share_class.is_empty() {
            return Err(ParseFailure::Workbook(format!("line {lineno}: blank share class code")));
        }
        rows.push(crate::ShareClassFlowRow {
            flow_date: row_date,
            share_class,
            outstanding_shares: num(&fields, R_OUTSTANDING),
            nav_per_share: num(&fields, R_NAV_PER_SHARE),
            subscription_amount: num(&fields, R_SUB_AMOUNT).unwrap_or(0.0).abs(),
            redemption_amount: num(&fields, R_RED_AMOUNT).unwrap_or(0.0).abs(),
        });
    }
    if rows.is_empty() {
        return Err(ParseFailure::Workbook("no flow rows found".into()));
    }
    Ok(UniversalBatch {
        primary_date: file_date,
        nav_points: Vec::new(),
        snapshots: Vec::new(),
        dividends: None,
        operations: None,
        flows: Some(rows),
        ref_hints: Vec::new(),
        ref_facts: Vec::new(),
        warnings: Vec::new(),
    })
}
