use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::{Extension, Json};
use chrono::NaiveDate;
use db::auth::marker::{Nav, Positions, View};
use db::auth::AuthCtx;

pub async fn nav(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(pid): Path<i64>,
) -> Result<Json<Vec<db::repo::NavRow>>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Nav, View>(pid)?;
    // Authorization does not subsume existence: a wildcard grant answers
    // "yes" for any portfolio id, including one that was never created, and
    // `authorize` alone would 200 an empty result for it instead of 404ing.
    // `ensure` also still guards an archived portfolio for mutating calls
    // elsewhere; this is a read, so `mutating: false`.
    super::portfolios::ensure(&scoped, pid, false).await?;
    Ok(Json(scoped.nav_rows(&a).await?))
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
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>,
    Path(pid): Path<i64>, Query(q): Query<PositionsQuery>,
) -> Result<Json<PositionsResponse>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Positions, View>(pid)?;
    // See the comment in `nav` above: `authorize` checks grants, not
    // existence, so a nonexistent (or archived-and-mutating) portfolio still
    // needs this explicit guard.
    super::portfolios::ensure(&scoped, pid, false).await?;
    let dates = scoped.position_dates(&a).await?;
    let date = match q.date {
        Some(s) => Some(s.parse::<NaiveDate>().map_err(|_| AppError::BadRequest(format!("bad date: {s}")))?),
        None => dates.first().copied(),
    };
    let rows = match date {
        Some(d) => scoped.positions_for(&a, d).await?,
        None => Vec::new(),
    };
    Ok(Json(PositionsResponse { dates, date, rows }))
}
