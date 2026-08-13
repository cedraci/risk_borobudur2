use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::Json;

#[derive(serde::Deserialize)]
pub struct CreateBody { pub name: String, pub kind: String }

#[derive(serde::Deserialize)]
pub struct UpdateBody { pub name: String, pub archived: bool }

fn valid_name(name: &str) -> Result<String, AppError> {
    let n = name.trim();
    if n.is_empty() {
        return Err(AppError::Unprocessable("name must not be empty".into()));
    }
    Ok(n.to_string())
}

fn valid_kind(kind: &str) -> Result<(), AppError> {
    if !matches!(kind, "ucits" | "mandate") {
        return Err(AppError::Unprocessable("kind must be 'ucits' or 'mandate'".into()));
    }
    Ok(())
}

/// Unique-violation on portfolios.name -> 422 with a helpful message; any
/// other DB error stays a 500.
fn map_name_conflict(e: anyhow::Error) -> AppError {
    let is_unique = e.downcast_ref::<sqlx::Error>()
        .and_then(|se| se.as_database_error())
        .map(|de| de.is_unique_violation())
        .unwrap_or(false);
    if is_unique {
        AppError::Unprocessable("a portfolio with that name already exists".into())
    } else {
        AppError::Internal(e)
    }
}

pub async fn list(State(st): State<AppState>) -> Result<Json<Vec<db::repo::Portfolio>>, AppError> {
    Ok(Json(db::repo::portfolios_list(&st.pool).await?))
}

pub async fn create(State(st): State<AppState>, Json(b): Json<CreateBody>)
    -> Result<Json<db::repo::Portfolio>, AppError>
{
    let name = valid_name(&b.name)?;
    valid_kind(&b.kind)?;
    let p = db::repo::portfolio_create(&st.pool, &name, &b.kind).await
        .map_err(map_name_conflict)?;
    Ok(Json(p))
}

pub async fn update(State(st): State<AppState>, Path(id): Path<i64>, Json(b): Json<UpdateBody>)
    -> Result<Json<db::repo::Portfolio>, AppError>
{
    let name = valid_name(&b.name)?;
    let p = db::repo::portfolio_update(&st.pool, id, &name, b.archived).await
        .map_err(map_name_conflict)?
        .ok_or_else(|| AppError::NotFound(format!("no portfolio {id}")))?;
    Ok(Json(p))
}

/// Every scoped handler's first call. `mutating` requests (imports, CTD
/// upload, KPI puts, settings puts) are refused on an archived portfolio;
/// reads stay available so history remains inspectable.
pub async fn ensure(pool: &sqlx::PgPool, id: i64, mutating: bool)
    -> Result<db::repo::Portfolio, AppError>
{
    let p = db::repo::portfolio_get(pool, id).await?
        .ok_or_else(|| AppError::NotFound(format!("no portfolio {id}")))?;
    if mutating && p.archived {
        return Err(AppError::Conflict(format!("portfolio '{}' is archived", p.name)));
    }
    Ok(p)
}

#[derive(serde::Deserialize)]
pub struct CodeBody {
    pub source: String,
    pub code: String,
}

pub async fn codes_list(State(st): State<AppState>, Path(pid): Path<i64>) -> Result<Json<Vec<db::repo::PortfolioCode>>, AppError> {
    ensure(&st.pool, pid, false).await?;
    Ok(Json(db::repo::portfolio_codes_for(&st.pool, pid).await?))
}

/// Replace the portfolio's full code set. Codes are trimmed; empty entries
/// are 422; a code already claimed by another portfolio is 422 too.
pub async fn codes_put(State(st): State<AppState>, Path(pid): Path<i64>, Json(body): Json<Vec<CodeBody>>) -> Result<Json<Vec<db::repo::PortfolioCode>>, AppError> {
    ensure(&st.pool, pid, true).await?;
    let mut codes: Vec<(String, String)> = Vec::with_capacity(body.len());
    for c in &body {
        let source = c.source.trim().to_lowercase();
        let code = c.code.trim().to_string();
        if source.is_empty() || code.is_empty() {
            return Err(AppError::Unprocessable("source and code must be non-empty".into()));
        }
        codes.push((source, code));
    }
    db::repo::portfolio_codes_replace(&st.pool, pid, &codes).await.map_err(|e| {
        let is_unique = e.downcast_ref::<sqlx::Error>()
            .and_then(|se| se.as_database_error())
            .is_some_and(|de| de.is_unique_violation());
        if is_unique {
            AppError::Unprocessable("one of these codes is already mapped to another portfolio".into())
        } else {
            AppError::Internal(e)
        }
    })?;
    Ok(Json(db::repo::portfolio_codes_for(&st.pool, pid).await?))
}

#[derive(serde::Deserialize)]
pub struct ShareholderBody {
    pub label: String,
    pub pct_of_nav: f64,
    pub as_of: chrono::NaiveDate,
}

pub async fn shareholders_list(
    State(st): State<AppState>, Path(pid): Path<i64>,
) -> Result<Json<Vec<db::repo::Shareholder>>, AppError> {
    ensure(&st.pool, pid, false).await?;
    Ok(Json(db::repo::shareholders_for(&st.pool, pid).await?))
}

/// Replace the portfolio's whole register. Every check runs before any
/// write, so a rejected payload leaves the stored register untouched.
pub async fn shareholders_put(
    State(st): State<AppState>, Path(pid): Path<i64>, Json(body): Json<Vec<ShareholderBody>>,
) -> Result<Json<Vec<db::repo::Shareholder>>, AppError> {
    ensure(&st.pool, pid, true).await?;
    let mut total = 0.0;
    let mut rows = Vec::with_capacity(body.len());
    for b in &body {
        let label = b.label.trim();
        if label.is_empty() {
            return Err(AppError::Unprocessable("label must not be blank".into()));
        }
        if !(b.pct_of_nav.is_finite() && b.pct_of_nav > 0.0 && b.pct_of_nav <= 100.0) {
            return Err(AppError::Unprocessable(format!(
                "{label}: pct_of_nav must be in (0, 100]")));
        }
        total += b.pct_of_nav;
        rows.push((label.to_string(), b.pct_of_nav, b.as_of));
    }
    // A register summing past the whole fund is a typo, not a portfolio.
    if total > 100.0 {
        return Err(AppError::Unprocessable(format!(
            "register totals {total:.2}% of NAV, which exceeds 100%")));
    }
    db::repo::shareholders_replace(&st.pool, pid, &rows).await?;
    Ok(Json(db::repo::shareholders_for(&st.pool, pid).await?))
}
