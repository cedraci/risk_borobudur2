use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Multipart, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::{Extension, Json};
use analytics::pnl::asset_class_of;
use db::auth::marker::{Configure, Export, Import, MarketData, Nav, Positions, Reference, View};
use db::auth::{AuthCtx, Domain};
use db::scoped::Scoped;
use ingest::bloomberg::{build_adv_request, build_request, market_sector_for, parse_response, region_for, RequestItem};
use std::collections::BTreeSet;

/// One resolved `REFS` sheet classification row: (isin, ticker, country,
/// region, sector, industry).
type ClassificationRow = (String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>);

/// Export the request workbook for everything still unclassified.
pub async fn request(State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>) -> Result<impl IntoResponse, AppError> {
    let scoped = st.db.scope(&ctx);
    scoped.authorize_global::<Positions, Export>()?;
    // Reference is a secondary domain here (this route is gated on
    // Positions) — see routes.rs's comment on this route.
    let refs = match scoped.authorize_global::<Reference, View>() {
        Ok(rv) => scoped.refs_all(&rv).await?,
        Err(_) => Vec::new(),
    };
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
    for pf in scoped.portfolios_list().await?.iter().filter(|p| !p.archived) {
        // The route requires a wildcard Positions/Export grant, which
        // implies Positions/View for every portfolio id — this authorize
        // cannot fail for a principal who reached this handler at all.
        let pv = scoped.authorize::<Positions, View>(pf.id)?;
        let dates = scoped.position_dates(&pv).await?;
        let Some(latest) = dates.first().copied() else { continue };
        latest_any = Some(latest_any.map_or(latest, |d| d.max(latest)));
        // Nav is a secondary domain here too.
        if let Ok(nv) = scoped.authorize::<Nav, View>(pf.id)
            && let Some(first_nav) = scoped.nav_rows(&nv).await?.first().map(|n| n.date) {
            earliest_nav = Some(earliest_nav.map_or(first_nav, |d| d.min(first_nav)));
        }
        for p in scoped.positions_for(&pv, latest).await? {
            if let Some(c) = &p.currency
                && c != "EUR" { currencies.insert(c.clone()); }
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
        &format!("attachment; filename=\"bloomberg_request_{to}.xlsx\"")).map_err(anyhow::Error::from)?);
    crate::audit::record(&st, &ctx, "export", Some(Domain::Positions), None,
        serde_json::json!({"kind": "bloomberg_request", "items": items.len()})).await;
    Ok((StatusCode::OK, h, bytes))
}

/// Every eligible instrument held in the fleet's latest snapshots, split into
/// those whose stored volume has gone stale and the full held set.
/// Deduplicated by ISIN: an instrument held by three portfolios is requested
/// once.
///
/// Staleness is decided per `(portfolio, ISIN)` pair, then unioned across
/// portfolios — not decided once by whichever portfolio happens to be walked
/// first and then locked in via dedup. `portfolios_list` order is
/// deterministic but arbitrary (`ORDER BY id`), and portfolios can set their
/// own `adv_max_age_days`; if a lax portfolio's threshold silently overrode a
/// stricter one that also holds the instrument, that stricter portfolio's own
/// Limits page would flag the position stale while nothing prompted a fetch
/// to fix it. So an instrument is due if ANY portfolio holding it considers
/// it stale — over-requesting one instrument costs one formula cell.
///
/// Positions is a secondary domain for `adv_due` (gated on MarketData); for
/// `adv_request` it is the route's own primary domain (gated on
/// Positions/Export, which implies View for every portfolio id). Soft-checked
/// either way so a portfolio outside the principal's Positions grants simply
/// contributes nothing to the fleet-wide scope, rather than failing either
/// caller outright.
async fn adv_scope(scoped: &Scoped<'_>) -> Result<(Vec<RequestItem>, Vec<RequestItem>), AppError> {
    let refs = match scoped.authorize_global::<Reference, View>() {
        Ok(rv) => scoped.refs_all(&rv).await?,
        Err(_) => Vec::new(),
    };
    let by: std::collections::HashMap<&str, &db::repo::InstrumentRef> =
        refs.iter().map(|r| (r.code.as_str(), r)).collect();

    // `items` dedups the held set by ISIN (first occurrence wins — the
    // market sector only depends on asset type, which the same instrument
    // does not vary across portfolios). `stale` is the union of every
    // portfolio's own verdict for that ISIN.
    let mut items: std::collections::BTreeMap<String, RequestItem> = std::collections::BTreeMap::new();
    let mut stale: BTreeSet<String> = BTreeSet::new();
    for pf in scoped.portfolios_list().await?.iter().filter(|p| !p.archived) {
        let Ok(pv) = scoped.authorize::<Positions, View>(pf.id) else { continue };
        let dates = scoped.position_dates(&pv).await?;
        let Some(latest) = dates.first().copied() else { continue };
        let settings = scoped.get_settings(pf.id).await?;
        for p in scoped.positions_for(&pv, latest).await? {
            let r = by.get(p.isin.as_str());
            let probe = analytics::LiqPosition {
                code: p.isin.clone(), asset_type: p.asset_type.clone(),
                valuation_eur: p.valuation_eur.unwrap_or(0.0), quantity: p.quantity,
                adv_30d: None, adv_stale: false,
                adv_eligible: r.and_then(|r| r.adv_eligible),
                market_place: r.and_then(|r| r.market_place.clone()),
                liquidity_days: None, default_days: 1.0,
            };
            if !analytics::adv_eligible(&probe) { continue; }
            items.entry(p.isin.clone()).or_insert_with(|| RequestItem {
                isin: p.isin.clone(),
                market_sector: market_sector_for(asset_class_of(&p.asset_type)).to_string(),
            });
            let is_stale = r.and_then(|r| r.adv_asof)
                .map(|d| (latest - d).num_days() > settings.adv_max_age_days as i64)
                .unwrap_or(true);   // never fetched is always due
            if is_stale { stale.insert(p.isin.clone()); }
        }
    }
    let held: Vec<RequestItem> = items.values().cloned().collect();
    let due: Vec<RequestItem> = items.iter()
        .filter(|(isin, _)| stale.contains(isin.as_str()))
        .map(|(_, item)| item.clone())
        .collect();
    Ok((due, held))
}

#[derive(serde::Deserialize, Default)]
pub struct AdvQuery {
    #[serde(default)]
    all: bool,
}

/// Export the ADV request workbook. By default only instruments whose stored
/// volume has gone stale (or was never fetched) — `?all=true` serves the full
/// held set instead. Both come from the same `adv_scope` call so the two
/// endpoints (`adv_request`, `adv_due`) can never disagree.
pub async fn adv_request(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Query(q): Query<AdvQuery>,
) -> Result<impl IntoResponse, AppError> {
    let scoped = st.db.scope(&ctx);
    scoped.authorize_global::<Positions, Export>()?;
    let (due, held) = adv_scope(&scoped).await?;
    let items = if q.all { held } else { due };
    let asof = chrono::Utc::now().date_naive();
    let bytes = build_adv_request(&items, asof)?;

    let mut h = HeaderMap::new();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"));
    h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_str(
        &format!("attachment; filename=\"bloomberg_adv_request_{asof}.xlsx\"")).map_err(anyhow::Error::from)?);
    crate::audit::record(&st, &ctx, "export", Some(Domain::Positions), None,
        serde_json::json!({"kind": "bloomberg_adv_request", "items": items.len(), "all": q.all})).await;
    Ok((StatusCode::OK, h, bytes))
}

/// The cost of the ADV request before it is paid: due and held counts, read
/// from the database alone. Never builds a workbook — a Bloomberg fetch is
/// only ever triggered by the user clicking `adv_request`/`?all=true`.
pub async fn adv_due(State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>) -> Result<Json<serde_json::Value>, AppError> {
    let scoped = st.db.scope(&ctx);
    scoped.authorize_global::<MarketData, View>()?;
    let (due, held) = adv_scope(&scoped).await?;
    Ok(Json(serde_json::json!({ "due": due.len(), "held": held.len() })))
}

pub async fn upload(State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, mut mp: Multipart) -> Result<Json<serde_json::Value>, AppError> {
    let scoped = st.db.scope(&ctx);
    let import = scoped.authorize_global::<MarketData, Import>()?;
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

    let classifications: Vec<ClassificationRow> =
        parsed.classifications.iter().map(|c| (
            c.isin.clone(),
            c.ticker.clone(),
            c.country.clone(),
            c.country.as_deref().and_then(region_for).map(|s| s.to_string()),
            c.sector.clone(),
            c.industry.clone(),
        )).collect();
    // Reference is a secondary domain here (this route is gated on
    // MarketData): a principal without a separate Reference/Configure grant
    // still stores the FX/ADV rows this upload owns, just not the
    // classification columns. A denied grant must not silently read as
    // "nothing to classify" — `classified: 0` is indistinguishable from a
    // response workbook that genuinely resolved zero cells. Mirrors
    // `fx_check_skipped`'s approach: an explicit marker names the denial so
    // a reader can tell "checked, nothing to do" from "could not check".
    let (classified, classification_status) = match scoped.authorize_global::<Reference, Configure>() {
        Ok(rc) => (scoped.classify_upsert_many(&rc, &classifications).await?, serde_json::json!({"status": "ok"})),
        Err(denied) => (0, serde_json::json!({"status": "unavailable", "reason": denied.reason()})),
    };

    let fx_rows: Vec<db::repo::FxRow> = parsed.fx.iter().map(|o| db::repo::FxRow {
        date: o.date, currency: o.currency.clone(), rate_to_eur: o.rate_to_eur,
    }).collect();
    let fx_stored = scoped.fx_upsert_many(&import, &fx_rows).await?;

    // Cross-check the inversion against the workbook's own Change column at
    // every snapshot date, across every non-archived portfolio's positions —
    // a rate mismatch anywhere in the fleet is reported. Refs/FX storage
    // above stays untouched: that data is shared across portfolios.
    // Positions is a secondary domain here too: a principal with the global
    // MarketData/Import grant this route requires need not also hold
    // Positions/View on every portfolio. A portfolio the caller cannot see
    // simply contributes no rows to `fx_check` — an empty result that reads
    // identically to "checked, no drift found". `fx_check_skipped` names
    // every portfolio that was skipped for that reason, so an empty
    // `fx_check` can be told apart from an unchecked fleet.
    let mut fx_check = Vec::new();
    let mut fx_check_skipped = Vec::new();
    for pf in scoped.portfolios_list().await?.iter().filter(|p| !p.archived) {
        let pv = match scoped.authorize::<Positions, View>(pf.id) {
            Ok(pv) => pv,
            Err(denied) => {
                fx_check_skipped.push(serde_json::json!({
                    "portfolio_id": pf.id, "portfolio_name": pf.name, "reason": denied.reason(),
                }));
                continue;
            }
        };
        for d in scoped.position_dates(&pv).await? {
            let positions = scoped.positions_for(&pv, d).await?;
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

    // ADV: stored only for cells Bloomberg actually resolved to a plausible
    // volume — `parse_response` never fabricates a value for an unresolved
    // cell, but a defensive filter here still guards against a stray
    // negative or non-finite number reaching the store. `adv_upsert_many`
    // writes `adv_30d` and `adv_asof` only; the as-of is the upload date.
    let adv_rows: Vec<(String, f64)> = parsed.adv.iter()
        .filter(|a| a.adv_30d.is_finite() && a.adv_30d >= 0.0)
        .map(|a| (a.isin.clone(), a.adv_30d)).collect();
    let adv_stored = scoped.adv_upsert_many(
        &import, &adv_rows, chrono::Utc::now().date_naive()).await?;

    crate::audit::record(&st, &ctx, "import", Some(Domain::MarketData), None,
        serde_json::json!({
            "kind": "bloomberg_response", "classified": classified,
            "fx_rows": fx_stored, "adv_rows": adv_stored,
            "classification_status": classification_status,
        })).await;
    Ok(Json(serde_json::json!({
        "classified": classified,
        "classification_status": classification_status,
        "fx_rows": fx_stored,
        "adv_rows": adv_stored,
        "skipped": parsed.skipped,
        "fx_check": fx_check,
        "fx_check_skipped": fx_check_skipped,
    })))
}
