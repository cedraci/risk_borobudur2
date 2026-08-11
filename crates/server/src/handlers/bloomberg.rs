use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Multipart, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use analytics::pnl::asset_class_of;
use ingest::bloomberg::{build_request, market_sector_for, parse_response, region_for, RequestItem};
use std::collections::BTreeSet;

/// Export the request workbook for everything still unclassified.
pub async fn request(State(st): State<AppState>) -> Result<impl IntoResponse, AppError> {
    let refs = db::repo::refs_all(&st.pool).await?;
    // (has country, has sector) per instrument code.
    let ref_state: std::collections::BTreeMap<&str, (bool, bool)> = refs.iter()
        .map(|r| (r.code.as_str(), (r.country_of_risk.is_some(), r.gics_sector.is_some())))
        .collect();

    // One request workbook serves the whole fleet: walk every non-archived
    // portfolio at its own latest snapshot and union the still-unclassified
    // instruments and non-EUR currencies. Dedup by ISIN via `seen` — refs
    // are global, so an instrument classified via one portfolio's holdings
    // is classified for every portfolio that also holds it.
    let mut items: Vec<RequestItem> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut currencies: BTreeSet<String> = BTreeSet::new();
    let mut latest_any: Option<chrono::NaiveDate> = None;
    let mut earliest_nav: Option<chrono::NaiveDate> = None;
    for pf in db::repo::portfolios_list(&st.pool).await?.iter().filter(|p| !p.archived) {
        let dates = db::repo::position_dates(&st.pool, pf.id).await?;
        let Some(latest) = dates.first().copied() else { continue };
        latest_any = Some(latest_any.map_or(latest, |d| d.max(latest)));
        if let Some(first_nav) = db::repo::nav_rows(&st.pool, pf.id).await?.first().map(|n| n.date) {
            earliest_nav = Some(earliest_nav.map_or(first_nav, |d| d.min(first_nav)));
        }
        for p in db::repo::positions_for(&st.pool, pf.id, latest).await? {
            if let Some(c) = &p.currency {
                if c != "EUR" { currencies.insert(c.clone()); }
            }
            // Only instrument types classification applies to. Every one
            // still unclassified is exported — the workbook resolves its
            // own ticker from the ISIN, so nothing is skipped for lack of
            // one.
            if !matches!(p.asset_type.as_str(), "Action" | "Fonds" | "Obligation") { continue; }
            // Bloomberg publishes no GICS classification for Corp/Govt
            // securities, so a bond is fully classified once its country is
            // known; requiring a sector would re-list every bond forever.
            let (has_country, has_sector) =
                ref_state.get(p.isin.as_str()).copied().unwrap_or((false, false));
            if has_country && (has_sector || p.asset_type == "Obligation") { continue; }
            if !seen.insert(p.isin.clone()) { continue; }
            items.push(RequestItem {
                isin: p.isin.clone(),
                market_sector: market_sector_for(asset_class_of(&p.asset_type)).to_string(),
            });
        }
    }
    let from = earliest_nav.unwrap_or_else(|| chrono::Utc::now().date_naive());
    let to = latest_any.unwrap_or_else(|| chrono::Utc::now().date_naive());

    let bytes = build_request(&items, &currencies.into_iter().collect::<Vec<_>>(), from, to)?;

    let mut h = HeaderMap::new();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"));
    h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_str(
        &format!("attachment; filename=\"bloomberg_request_{to}.xlsx\""))?);
    Ok((StatusCode::OK, h, bytes))
}

pub async fn upload(State(st): State<AppState>, mut mp: Multipart) -> Result<Json<serde_json::Value>, AppError> {
    let mut bytes: Option<Vec<u8>> = None;
    while let Some(f) = mp.next_field().await.map_err(|e| AppError::BadRequest(e.to_string()))? {
        if f.name() == Some("file") {
            bytes = Some(f.bytes().await.map_err(|e| AppError::BadRequest(e.to_string()))?.to_vec());
        }
    }
    let bytes = bytes.ok_or_else(|| AppError::BadRequest("no file field".into()))?;

    let parsed = parse_response(&bytes).map_err(|e| match e {
        ingest::ParseFailure::Workbook(m) => AppError::BadRequest(m),
        ingest::ParseFailure::Rows(rows) => AppError::UnprocessableRows(rows),
    })?;

    let classifications: Vec<(String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>)> =
        parsed.classifications.iter().map(|c| (
            c.isin.clone(),
            c.ticker.clone(),
            c.country.clone(),
            c.country.as_deref().and_then(region_for).map(|s| s.to_string()),
            c.sector.clone(),
            c.industry.clone(),
        )).collect();
    let classified = db::repo::classify_upsert_many(&st.pool, &classifications).await?;

    let fx_rows: Vec<db::repo::FxRow> = parsed.fx.iter().map(|o| db::repo::FxRow {
        date: o.date, currency: o.currency.clone(), rate_to_eur: o.rate_to_eur,
    }).collect();
    let fx_stored = db::repo::fx_upsert_many(&st.pool, &fx_rows).await?;

    // Cross-check the inversion against the workbook's own Change column at
    // every snapshot date, across every non-archived portfolio's positions —
    // a rate mismatch anywhere in the fleet is reported. Refs/FX storage
    // above stays untouched: that data is shared across portfolios.
    let mut fx_check = Vec::new();
    for pf in db::repo::portfolios_list(&st.pool).await?.iter().filter(|p| !p.archived) {
        for d in db::repo::position_dates(&st.pool, pf.id).await? {
            let positions = db::repo::positions_for(&st.pool, pf.id, d).await?;
            for o in parsed.fx.iter().filter(|o| o.date == d) {
                let Some(book) = positions.iter()
                    .find(|p| p.currency.as_deref() == Some(o.currency.as_str()) && p.fx_rate.is_some_and(|f| f > 0.0))
                    .and_then(|p| p.fx_rate) else { continue };
                let drift = (book - o.rate_to_eur).abs() / book;
                if drift > 0.01 {
                    fx_check.push(serde_json::json!({
                        "currency": o.currency, "date": d,
                        "workbook": book, "bloomberg": o.rate_to_eur, "drift": drift,
                    }));
                }
            }
        }
    }

    Ok(Json(serde_json::json!({
        "classified": classified,
        "fx_rows": fx_stored,
        "skipped": parsed.skipped,
        "fx_check": fx_check,
    })))
}
