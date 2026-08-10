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
