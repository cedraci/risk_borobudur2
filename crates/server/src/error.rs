use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

pub enum AppError {
    Internal(anyhow::Error),
    BadRequest(String),
    UnprocessableRows(Vec<ingest::RowError>),
    Unprocessable(String),
    NotFound(String),
    Conflict(String),
    Unauthenticated,
    LockedOut(u64),
    Forbidden(db::auth::Denied),
    /// A route under `/api/admin`, reached by a principal that resolved but
    /// is not an administrator. Distinct from `Forbidden`, which always
    /// carries a `Denied` tied to one `Domain`/`Action` pair — administrator
    /// status is not a grant, so it has no domain to name.
    AdministratorRequired,
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError::Internal(e)
    }
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        AppError::Internal(e.into())
    }
}

impl From<db::auth::Denied> for AppError {
    fn from(d: db::auth::Denied) -> Self {
        AppError::Forbidden(d)
    }
}

impl From<crate::auth::AuthError> for AppError {
    fn from(e: crate::auth::AuthError) -> Self {
        match e {
            crate::auth::AuthError::Unauthenticated => AppError::Unauthenticated,
            crate::auth::AuthError::LockedOut { retry_after_secs } => AppError::LockedOut(retry_after_secs),
            crate::auth::AuthError::Internal(e) => AppError::Internal(e),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::Internal(e) => {
                // The real error (and its full `anyhow` chain, via `{:#}`) is
                // logged, never handed to the client: `e.to_string()` in the
                // response body can leak internals (a query fragment, a file
                // path, a dependency's error phrasing) to anyone who can
                // trigger a 500, and gives an attacker a diagnostic they
                // should not get for free. The body says only that something
                // failed; the operator reads why from the logs.
                tracing::error!("internal error: {e:#}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"title": "Internal Server Error", "status": 500, "detail": "internal error"})),
                )
                    .into_response()
            }
            AppError::BadRequest(msg) => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"title": "Bad Request", "status": 400, "detail": msg})),
            )
                .into_response(),
            AppError::UnprocessableRows(rows) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({"title": "File rejected", "status": 422, "rows": rows})),
            )
                .into_response(),
            AppError::Unprocessable(msg) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({"title": "Unprocessable Entity", "status": 422, "detail": msg})),
            )
                .into_response(),
            AppError::NotFound(msg) => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"title": "Not Found", "status": 404, "detail": msg})),
            )
                .into_response(),
            AppError::Conflict(msg) => (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"title": "Conflict", "status": 409, "detail": msg})),
            )
                .into_response(),
            AppError::Unauthenticated => (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"title": "Unauthorized", "status": 401, "detail": "authentication required"})),
            )
                .into_response(),
            AppError::LockedOut(secs) => (
                StatusCode::TOO_MANY_REQUESTS,
                [(axum::http::header::RETRY_AFTER, secs.to_string())],
                Json(serde_json::json!({"title": "Too Many Requests", "status": 429,
                                        "detail": "too many failed sign-in attempts"})),
            )
                .into_response(),
            AppError::AdministratorRequired => (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"title": "Forbidden", "status": 403,
                    "detail": "administrator required"})),
            )
                .into_response(),
            AppError::Forbidden(d) => match d.kind {
                db::auth::DeniedKind::OutOfScope => (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({"title": "Not Found", "status": 404, "detail": "no such portfolio"})),
                ).into_response(),
                db::auth::DeniedKind::NotGranted => (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({"title": "Forbidden", "status": 403,
                        "detail": d.reason(), "domain": d.domain.as_str(), "action": d.action.as_str(),
                        "portfolio_id": d.portfolio})),
                ).into_response(),
            },
        }
    }
}
