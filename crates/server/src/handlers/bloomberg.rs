use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Multipart, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use ingest::bloomberg::{build_request, parse_response, region_for, RequestItem};
use std::collections::BTreeSet;

/// Export the request workbook for everything still unclassified.
pub async fn request(State(st): State<AppState>) -> Result<impl IntoResponse, AppError> {
    let dates = db::repo::position_dates(&st.pool).await?;
    let latest = dates.first().copied();
    let positions = match latest {
        Some(d) => db::repo::positions_for(&st.pool, d).await?,
        None => Vec::new(),
    };
    let refs = db::repo::refs_all(&st.pool).await?;
    let classified: BTreeSet<&str> = refs.iter()
        .filter(|r| r.country_of_risk.is_some() && r.gics_sector.is_some())
        .map(|r| r.code.as_str())
        .collect();

    let mut items: Vec<RequestItem> = Vec::new();
    let mut currencies: BTreeSet<String> = BTreeSet::new();
    for p in &positions {
        if let Some(c) = &p.currency {
            if c != "EUR" { currencies.insert(c.clone()); }
        }
        // Only instruments a Bloomberg ticker can identify, and only those
        // classification would actually apply to.
        if !matches!(p.asset_type.as_str(), "Action" | "Fonds" | "Obligation") { continue; }
        if classified.contains(p.isin.as_str()) { continue; }
        let Some(ticker) = p.ticker.clone() else { continue };
        items.push(RequestItem { isin: p.isin.clone(), ticker });
    }

    let navs = db::repo::nav_rows(&st.pool).await?;
    let from = navs.first().map(|n| n.date).unwrap_or_else(|| chrono::Utc::now().date_naive());
    let to = latest.unwrap_or_else(|| chrono::Utc::now().date_naive());

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

    let classifications: Vec<(String, Option<String>, Option<String>, Option<String>, Option<String>)> =
        parsed.classifications.iter().map(|c| (
            c.isin.clone(),
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
    // every snapshot date. Disagreement means the pull is upside down.
    let mut fx_check = Vec::new();
    for d in db::repo::position_dates(&st.pool).await? {
        let positions = db::repo::positions_for(&st.pool, d).await?;
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

    Ok(Json(serde_json::json!({
        "classified": classified,
        "fx_rows": fx_stored,
        "skipped": parsed.skipped,
        "fx_check": fx_check,
    })))
}
