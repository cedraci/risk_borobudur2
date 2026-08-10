use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Multipart, Path, Query, State};
use axum::Json;
use chrono::NaiveDate;

#[derive(serde::Deserialize)]
pub struct DateQuery {
    date: Option<String>,
}

pub async fn contracts(State(st): State<AppState>) -> Result<Json<Vec<db::repo::FuturesContract>>, AppError> {
    Ok(Json(db::repo::contracts_all(&st.pool).await?))
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
    Path(root): Path<String>,
    Json(b): Json<ContractBody>,
) -> Result<Json<db::repo::FuturesContract>, AppError> {
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
    db::repo::contracts_upsert(&st.pool, &c).await?;
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
    Path(pid): Path<i64>,
    mut multipart: Multipart,
) -> Result<Json<CtdUploadOutcome>, AppError> {
    super::portfolios::ensure(&st.pool, pid, true).await?;
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
        let known = db::repo::positions_for(&st.pool, pid, date).await?;
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

        let replaced = !db::repo::ctd_for(&st.pool, pid, date).await?.is_empty();
        let n = db::repo::ctd_replace(&st.pool, pid, date, &filename, &rows).await?;
        return Ok(Json(CtdUploadOutcome { nav_date: date, rows: n, replaced }));
    }
    Err(AppError::BadRequest("missing multipart field 'file'".into()))
}

pub async fn list_ctd(
    State(st): State<AppState>,
    Path(pid): Path<i64>,
    Query(q): Query<DateQuery>,
) -> Result<Json<Vec<db::repo::CtdRecord>>, AppError> {
    super::portfolios::ensure(&st.pool, pid, false).await?;
    let date = match &q.date {
        Some(s) => s.parse::<NaiveDate>().map_err(|_| AppError::BadRequest(format!("bad date: {s}")))?,
        None => match db::repo::position_dates(&st.pool, pid).await?.first().copied() {
            Some(d) => d,
            None => return Ok(Json(Vec::new())),
        },
    };
    Ok(Json(db::repo::ctd_for(&st.pool, pid, date).await?))
}
