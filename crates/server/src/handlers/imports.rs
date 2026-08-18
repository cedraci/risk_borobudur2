use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Multipart, Path, State};
use axum::Json;
use sha2::Digest;

#[derive(serde::Serialize)]
pub struct FileImportResult {
    pub filename: String,
    /// "nav_recap" | "caceis_hisinv" | "caceis_histovl" | "caceis_joursr" |
    /// "caceis_invjcp"; None when detection failed.
    pub kind: Option<String>,
    pub portfolio_id: Option<i64>,
    pub portfolio_name: Option<String>,
    pub outcome: Option<db::repo::ImportOutcome>,
    pub error: Option<String>,
    pub error_rows: Option<Vec<ingest::RowError>>,
}

fn kind_label(k: ingest::adapter::FileKind) -> &'static str {
    match k {
        ingest::adapter::FileKind::NavRecap => "nav_recap",
        ingest::adapter::FileKind::CaceisHisinv => "caceis_hisinv",
        ingest::adapter::FileKind::CaceisHistovl => "caceis_histovl",
        ingest::adapter::FileKind::CaceisJoursr => "caceis_joursr",
        ingest::adapter::FileKind::CaceisInvjcp => "caceis_invjcp",
    }
}

/// `AppError` has no `Display`/`ToString` impl (it renders straight to an
/// HTTP response), but per-file entries need a plain message string. Extract
/// the inner message for the variants `ensure()` can return.
fn err_msg(e: AppError) -> String {
    match e {
        AppError::Internal(e) => e.to_string(),
        AppError::BadRequest(m) | AppError::Unprocessable(m) | AppError::NotFound(m) | AppError::Conflict(m) => m,
        AppError::UnprocessableRows(rows) => format!("{} row error(s)", rows.len()),
        // `ensure()` cannot produce these — session-only variants, unreachable here.
        AppError::Unauthenticated | AppError::LockedOut(_) => "unauthenticated".to_string(),
    }
}

/// Multi-file upload. The URL portfolio is where non-identifying files
/// (NAV Recap) land, and must be active — 404/409 up front, preserving the
/// existing single-file contract. Self-identifying files (CACEIS) route by
/// `portfolio_codes` REGARDLESS of the URL portfolio; problems with an
/// individual file are reported per file, not as a request failure.
pub async fn upload(State(st): State<AppState>, Path(pid): Path<i64>, mut multipart: Multipart) -> Result<Json<Vec<FileImportResult>>, AppError> {
    let selected = super::portfolios::ensure(&st.pool, pid, true).await?;

    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart error: {e}")))?
    {
        if field.name() != Some("file") { continue; }
        let filename = field.file_name().unwrap_or("upload.bin").to_string();
        let bytes = field.bytes().await
            .map_err(|e| AppError::BadRequest(format!("read error: {e}")))?;
        files.push((filename, bytes.to_vec()));
    }
    if files.is_empty() {
        return Err(AppError::BadRequest("missing multipart field 'file'".into()));
    }

    let mut results = Vec::with_capacity(files.len());
    for (filename, bytes) in files {
        results.push(import_one(&st, &selected, filename, &bytes).await);
    }
    Ok(Json(results))
}

async fn import_one(st: &AppState, selected: &db::repo::Portfolio, filename: String, bytes: &[u8]) -> FileImportResult {
    let mut r = FileImportResult {
        filename: filename.clone(), kind: None, portfolio_id: None,
        portfolio_name: None, outcome: None, error: None, error_rows: None,
    };

    let id = match ingest::adapter::detect(&filename, bytes) {
        Ok(id) => id,
        Err(e) => { r.error = Some(e.to_string()); return r; }
    };
    r.kind = Some(kind_label(id.kind).to_string());

    // Route: self-identifying files by code lookup; others to the URL portfolio.
    let (target_id, target_name) = match &id.fund_code {
        None => (selected.id, selected.name.clone()),
        Some((source, code)) => {
            match db::repo::portfolio_by_code(&st.pool, source, code).await {
                Err(e) => { r.error = Some(e.to_string()); return r; }
                Ok(None) => {
                    r.error = Some(format!(
                        "unknown {source} code {code:?} — map it to a portfolio in the Portfolios panel, then re-upload"));
                    return r;
                }
                Ok(Some(tid)) => match super::portfolios::ensure(&st.pool, tid, true).await {
                    Ok(p) => (p.id, p.name),
                    Err(e) => { r.error = Some(err_msg(e)); return r; }
                },
            }
        }
    };
    r.portfolio_id = Some(target_id);
    r.portfolio_name = Some(target_name);

    let batch = match ingest::adapter::parse(id.kind, &filename, bytes) {
        Ok(b) => b,
        Err(ingest::ParseFailure::Workbook(m)) => { r.error = Some(m); return r; }
        Err(ingest::ParseFailure::Rows(rows)) => {
            r.error = Some(format!("{} row error(s)", rows.len()));
            r.error_rows = Some(rows);
            return r;
        }
    };

    let sha = hex::encode(sha2::Sha256::digest(bytes));
    match db::repo::import_batch(&st.pool, target_id, &filename, &sha, &batch).await {
        Ok(outcome) => r.outcome = Some(outcome),
        Err(e) => r.error = Some(e.to_string()),
    }
    r
}

pub async fn list(State(st): State<AppState>, Path(pid): Path<i64>) -> Result<Json<Vec<db::repo::ImportRecord>>, AppError> {
    super::portfolios::ensure(&st.pool, pid, false).await?;
    Ok(Json(db::repo::imports_list(&st.pool, pid).await?))
}
