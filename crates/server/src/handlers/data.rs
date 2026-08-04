use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::Json;
use chrono::NaiveDate;

pub async fn nav(State(st): State<AppState>) -> Result<Json<Vec<db::repo::NavRow>>, AppError> {
    Ok(Json(db::repo::nav_rows(&st.pool).await?))
}

#[derive(serde::Deserialize)]
pub struct PositionsQuery {
    date: Option<String>,
}

#[derive(serde::Serialize)]
pub struct PositionsResponse {
    dates: Vec<NaiveDate>,
    date: Option<NaiveDate>,
    rows: Vec<db::repo::PositionRecord>,
}

pub async fn positions(
    State(st): State<AppState>,
    Query(q): Query<PositionsQuery>,
) -> Result<Json<PositionsResponse>, AppError> {
    let dates = db::repo::position_dates(&st.pool).await?;
    let date = match q.date {
        Some(s) => Some(s.parse::<NaiveDate>().map_err(|_| AppError::BadRequest(format!("bad date: {s}")))?),
        None => dates.first().copied(),
    };
    let rows = match date {
        Some(d) => db::repo::positions_for(&st.pool, d).await?,
        None => Vec::new(),
    };
    Ok(Json(PositionsResponse { dates, date, rows }))
}
