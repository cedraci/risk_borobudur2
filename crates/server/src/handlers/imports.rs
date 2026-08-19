use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Multipart, Path, State};
use axum::{Extension, Json};
use db::auth::marker::{Import, Nav, Positions, Reference, Transactions, View};
use db::auth::{AuthCtx, Domain};
use db::scoped::Scoped;
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
        // `ensure()` cannot produce these — session-only and authz variants,
        // unreachable here.
        AppError::Unauthenticated | AppError::LockedOut(_) => "unauthenticated".to_string(),
        AppError::Forbidden(d) => d.reason(),
    }
}

/// Renders an authorization denial for a per-file import result without
/// naming the target portfolio or revealing whether it exists or is
/// archived. `Denied::reason()` already renders identically for
/// `OutOfScope` and `NotGranted` (both "not permitted: {domain}"), so a
/// principal outside a portfolio's scope entirely cannot distinguish "that
/// portfolio doesn't exist" from "it exists but I can't touch it" — the same
/// non-disclosure a 404 gives at the route level, reproduced here at the
/// per-file level since a per-file result can't itself carry an HTTP status.
fn import_denied_msg(filename: &str, d: &db::auth::Denied) -> String {
    format!("not permitted to import {filename:?}: {}", d.reason())
}

/// Multi-file upload. The URL portfolio is where non-identifying files
/// (NAV Recap) land, and must be active — 404/409 up front, preserving the
/// existing single-file contract. Self-identifying files (CACEIS) route by
/// `portfolio_codes` REGARDLESS of the URL portfolio; problems with an
/// individual file are reported per file, not as a request failure.
///
/// The route-level gate (`Domain::Positions, Action::Import` on the URL id)
/// is a coarse pre-filter only. Per ruling 3, every file's batch write is
/// authorized separately, against the portfolio it actually targets: the
/// URL portfolio for a non-identifying file, or the code-resolved portfolio
/// for a self-identifying one — never assumed from the URL alone.
pub async fn upload(State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(pid): Path<i64>, mut multipart: Multipart) -> Result<Json<Vec<FileImportResult>>, AppError> {
    let scoped = st.db.scope(&ctx);
    scoped.authorize::<Positions, Import>(pid)?;
    let selected = super::portfolios::ensure(&scoped, pid, true).await?;

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
        results.push(import_one(&st, &ctx, &scoped, &selected, filename, &bytes).await);
    }
    Ok(Json(results))
}

async fn import_one(
    st: &AppState, ctx: &AuthCtx, scoped: &Scoped<'_>, selected: &db::repo::Portfolio, filename: String, bytes: &[u8],
) -> FileImportResult {
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
    // Only the id is resolved here — NOT the row (name, archived state):
    // reading the row before authorization is exactly the leak this ordering
    // fixes (see the comment on the authorize block below), so resolution
    // stays limited to the bare id until every Import token is proven.
    let target_id = match &id.fund_code {
        None => selected.id,
        Some((source, code)) => {
            let ref_view = match scoped.authorize_global::<Reference, View>() {
                Ok(a) => a,
                Err(e) => { r.error = Some(err_msg(AppError::from(e))); return r; }
            };
            match scoped.portfolio_by_code(&ref_view, source, code).await {
                Err(e) => { r.error = Some(e.to_string()); return r; }
                Ok(None) => {
                    r.error = Some(format!(
                        "unknown {source} code {code:?} — map it to a portfolio in the Portfolios panel, then re-upload"));
                    return r;
                }
                Ok(Some(tid)) => tid,
            }
        }
    };

    // Authorize the domains this batch actually writes, against the portfolio
    // it actually targets — not just the coarse URL-level gate checked above.
    // A principal who may import into the URL portfolio but not this file's
    // resolved target is refused for this file alone; siblings still run.
    //
    // This runs BEFORE any existence/archived check (`ensure`, below): a
    // self-identifying file can route to a portfolio the caller has no grant
    // on at all, and `ensure`'s errors name the portfolio and its archived
    // state (`"portfolio 'X' is archived"`). Resolving existence first would
    // let an out-of-scope principal learn a target portfolio's name — and
    // whether it exists at all — just by uploading a file coded to it. Only
    // once every token is proven does this handler touch the row at all.
    let positions_a = match scoped.authorize::<Positions, Import>(target_id) {
        Ok(a) => a,
        Err(d) => { r.error = Some(import_denied_msg(&filename, &d)); return r; }
    };
    let nav_a = match scoped.authorize::<Nav, Import>(target_id) {
        Ok(a) => a,
        Err(d) => { r.error = Some(import_denied_msg(&filename, &d)); return r; }
    };
    let transactions_a = match scoped.authorize::<Transactions, Import>(target_id) {
        Ok(a) => a,
        Err(d) => { r.error = Some(import_denied_msg(&filename, &d)); return r; }
    };

    // Existence + archived guard, now that authorization has already proven
    // the principal may act on this portfolio — the row (name included) is
    // only ever read, and only ever put in the response, after that proof.
    let target = match super::portfolios::ensure(scoped, target_id, true).await {
        Ok(p) => p,
        Err(e) => { r.error = Some(err_msg(e)); return r; }
    };
    r.portfolio_id = Some(target.id);
    r.portfolio_name = Some(target.name);

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
    match scoped.import_batch(&positions_a, &nav_a, &transactions_a, &filename, &sha, &batch).await {
        Ok(outcome) => {
            crate::audit::record(st, ctx, "import", Some(Domain::Positions), Some(target.id),
                serde_json::json!({
                    "import_id": outcome.import_id, "filename": filename,
                    "kind": kind_label(id.kind), "duplicate": outcome.duplicate,
                })).await;
            r.outcome = Some(outcome);
        }
        Err(e) => r.error = Some(e.to_string()),
    }
    r
}

pub async fn list(State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(pid): Path<i64>) -> Result<Json<Vec<db::repo::ImportRecord>>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Reference, View>(pid)?;
    super::portfolios::ensure(&scoped, pid, false).await?;
    Ok(Json(scoped.imports_list(&a).await?))
}
