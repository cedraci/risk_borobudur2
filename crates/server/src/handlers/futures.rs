use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Multipart, Path, Query, State};
use axum::{Extension, Json};
use chrono::NaiveDate;
use db::auth::marker::{Configure, Import, MarketData, Positions, Reference, View};
use db::auth::AuthCtx;

#[derive(serde::Deserialize)]
pub struct DateQuery {
    date: Option<String>,
}

pub async fn contracts(State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>) -> Result<Json<Vec<db::repo::FuturesContract>>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize_global::<Reference, View>()?;
    Ok(Json(scoped.contracts_all(&a).await?))
}

#[derive(serde::Deserialize)]
pub struct ContractBody {
    pub label: String,
    pub category: String,
    pub point_value: Option<f64>,
    pub currency: String,
    pub curve: Option<String>,
    pub price_convention: String,
    pub confirmed: bool,
    pub otc: bool,
}

pub async fn put_contract(
    State(st): State<AppState>,
    Extension(ctx): Extension<AuthCtx>,
    Path(root): Path<String>,
    Json(b): Json<ContractBody>,
) -> Result<Json<db::repo::FuturesContract>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize_global::<Reference, Configure>()?;
    if analytics::Category::parse(&b.category).is_none() {
        return Err(AppError::Unprocessable(format!(
            "category must be one of equity, interest_rate, fx, credit, commodity, other (got {:?})",
            b.category
        )));
    }
    if analytics::PriceConvention::parse(&b.price_convention).is_none() {
        return Err(AppError::Unprocessable("price_convention must be 'decimal' or 'th32'".into()));
    }
    if let Some(pv) = b.point_value {
        if !(pv.is_finite() && pv > 0.0) {
            return Err(AppError::Unprocessable("point_value must be a positive number".into()));
        }
    }
    if b.label.trim().is_empty() || b.currency.trim().is_empty() {
        return Err(AppError::Unprocessable("label and currency must not be blank".into()));
    }
    let c = db::repo::FuturesContract {
        contract_root: root,
        label: b.label.trim().to_string(),
        category: b.category,
        point_value: b.point_value,
        currency: b.currency.trim().to_string(),
        curve: b.curve.map(|c| c.trim().to_string()).filter(|c| !c.is_empty()),
        price_convention: b.price_convention,
        confirmed: b.confirmed,
        otc: b.otc,
    };
    scoped.contracts_upsert(&a, &c).await?;
    Ok(Json(c))
}

#[derive(serde::Serialize)]
pub struct CtdUploadOutcome {
    pub nav_date: NaiveDate,
    pub rows: usize,
    pub replaced: bool,
}

pub async fn upload_ctd(
    State(st): State<AppState>,
    Extension(ctx): Extension<AuthCtx>,
    Path(pid): Path<i64>,
    mut multipart: Multipart,
) -> Result<Json<CtdUploadOutcome>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<MarketData, Import>(pid)?;
    super::portfolios::ensure(&scoped, pid, true).await?;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart error: {e}")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field.file_name().unwrap_or("ctd.csv").to_string();
        let bytes = field
            .bytes()
            .await
            .map_err(|e| AppError::BadRequest(format!("read error: {e}")))?;
        let rows = ingest::parse_ctd_file(&bytes, &filename).map_err(|e| match e {
            ingest::ParseFailure::Workbook(m) => AppError::BadRequest(m),
            ingest::ParseFailure::Rows(rows) => AppError::UnprocessableRows(rows),
        })?;

        let date = rows[0].nav_date;
        // Positions is a secondary domain here (this route is gated on
        // MarketData): the known-tickers cross-check degrades to "nothing
        // known" — which surfaces as every row being unknown — for a
        // principal without a separate Positions grant, rather than
        // failing the whole upload outright.
        let known = match scoped.authorize::<Positions, View>(pid) {
            Ok(pv) => scoped.positions_for(&pv, date).await?,
            Err(_) => Vec::new(),
        };
        if known.is_empty() {
            return Err(AppError::Unprocessable(format!(
                "no NAV snapshot for {date}; upload the NAV Recap first"
            )));
        }
        let tickers: Vec<&str> = known
            .iter()
            .filter(|p| p.asset_type == "Future")
            .filter_map(|p| p.ticker.as_deref())
            .collect();
        let unknown: Vec<ingest::RowError> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| !tickers.contains(&r.ticker.as_str()))
            .map(|(i, r)| ingest::RowError {
                sheet: "CTD".into(),
                row: (i + 2) as u32,
                message: format!("{} is not a future in the {date} snapshot", r.ticker),
            })
            .collect();
        if !unknown.is_empty() {
            return Err(AppError::UnprocessableRows(unknown));
        }

        // `a` (Import) already implies View in the grant set, so this
        // authorize cannot fail for a principal who reached this handler.
        let view = scoped.authorize::<MarketData, View>(pid)?;
        let replaced = !scoped.ctd_for(&view, date).await?.is_empty();
        let n = scoped.ctd_replace(&a, date, &filename, &rows).await?;
        return Ok(Json(CtdUploadOutcome { nav_date: date, rows: n, replaced }));
    }
    Err(AppError::BadRequest("missing multipart field 'file'".into()))
}

pub async fn list_ctd(
    State(st): State<AppState>,
    Extension(ctx): Extension<AuthCtx>,
    Path(pid): Path<i64>,
    Query(q): Query<DateQuery>,
) -> Result<Json<Vec<db::repo::CtdRecord>>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<MarketData, View>(pid)?;
    super::portfolios::ensure(&scoped, pid, false).await?;
    let date = match &q.date {
        Some(s) => s.parse::<NaiveDate>().map_err(|_| AppError::BadRequest(format!("bad date: {s}")))?,
        None => {
            // Positions is a secondary domain here too — see `upload_ctd`.
            let dates = match scoped.authorize::<Positions, View>(pid) {
                Ok(pv) => scoped.position_dates(&pv).await?,
                Err(_) => Vec::new(),
            };
            match dates.first().copied() {
                Some(d) => d,
                None => return Ok(Json(Vec::new())),
            }
        }
    };
    Ok(Json(scoped.ctd_for(&a, date).await?))
}
