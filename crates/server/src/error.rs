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
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        AppError::Internal(e.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::Internal(e) => {
                tracing::error!("internal error: {e:#}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"title": "Internal Server Error", "status": 500, "detail": e.to_string()})),
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
        }
    }
}
