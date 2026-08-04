use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Multipart, State};
use axum::Json;
use sha2::Digest;

pub async fn upload(State(st): State<AppState>, mut multipart: Multipart) -> Result<Json<db::repo::ImportOutcome>, AppError> {
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart error: {e}")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field.file_name().unwrap_or("upload.xlsx").to_string();
        let bytes = field
            .bytes()
            .await
            .map_err(|e| AppError::BadRequest(format!("read error: {e}")))?;
        let sha = hex::encode(sha2::Sha256::digest(&bytes));
        let parsed = ingest::parse_workbook(&bytes).map_err(|e| match e {
            ingest::ParseFailure::Workbook(m) => AppError::BadRequest(m),
            ingest::ParseFailure::Rows(rows) => AppError::UnprocessableRows(rows),
        })?;
        let outcome = db::repo::import_workbook(&st.pool, &filename, &sha, &parsed).await?;
        return Ok(Json(outcome));
    }
    Err(AppError::BadRequest("missing multipart field 'file'".into()))
}

pub async fn list(State(st): State<AppState>) -> Result<Json<Vec<db::repo::ImportRecord>>, AppError> {
    Ok(Json(db::repo::imports_list(&st.pool).await?))
}
