//! Administration endpoints and first-administrator enrolment. Every route
//! mounted from here is either checked by `.admin` (requires
//! `ctx.is_administrator`, see `routes/protect.rs`) or, for `enrol`, is the
//! one unauthenticated path that stands up the very first administrator —
//! which is exactly why this file, like `startup.rs`, is on
//! `crates/db/tests/admin_isolation.rs`'s allow-list for reaching
//! `db::admin` directly.

use crate::audit;
use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use db::admin::{AuditRow, UserRow};
use db::auth::{AuthCtx, Grant, Role};

#[derive(serde::Serialize)]
pub struct UserOut {
    pub id: i64,
    pub email: String,
    pub display_name: String,
    pub is_administrator: bool,
    pub disabled: bool,
}

impl From<UserRow> for UserOut {
    fn from(u: UserRow) -> Self {
        UserOut {
            id: u.id,
            email: u.email,
            display_name: u.display_name,
            is_administrator: u.is_administrator,
            disabled: u.disabled,
        }
    }
}

/// `users.email` carries a `UNIQUE` constraint; turn a violation into a
/// friendly 422 instead of a 500, matching `portfolios::map_name_conflict`.
fn map_email_conflict(e: anyhow::Error) -> AppError {
    let is_unique = e.downcast_ref::<sqlx::Error>()
        .and_then(|se| se.as_database_error())
        .map(|de| de.is_unique_violation())
        .unwrap_or(false);
    if is_unique {
        AppError::Unprocessable("a user with that email already exists".into())
    } else {
        AppError::Internal(e)
    }
}

/// `grants.portfolio_id` carries a `REFERENCES portfolios(id)` constraint; a
/// grant for a portfolio id that does not exist would otherwise 500 instead
/// of reporting the actual problem — turn the FK violation into a friendly
/// 422, matching `map_email_conflict`.
fn map_portfolio_fk(e: anyhow::Error) -> AppError {
    let is_fk = e.downcast_ref::<sqlx::Error>()
        .and_then(|se| se.as_database_error())
        .map(|de| de.is_foreign_key_violation())
        .unwrap_or(false);
    if is_fk {
        AppError::Unprocessable("no such portfolio".into())
    } else {
        AppError::Internal(e)
    }
}

async fn require_user(admin: &db::admin::Admin<'_>, id: i64) -> Result<UserRow, AppError> {
    admin.user_by_id(id).await?.ok_or_else(|| AppError::NotFound(format!("no user {id}")))
}

pub async fn users_list(State(st): State<AppState>) -> Result<Json<Vec<UserOut>>, AppError> {
    let rows = st.db.admin().users_list().await?;
    Ok(Json(rows.into_iter().map(UserOut::from).collect()))
}

#[derive(serde::Deserialize)]
pub struct CreateUserBody {
    pub email: String,
    pub display_name: String,
    pub password: String,
    #[serde(default)]
    pub is_administrator: bool,
}

pub async fn users_create(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Json(b): Json<CreateUserBody>,
) -> Result<Json<UserOut>, AppError> {
    let email = b.email.trim();
    if email.is_empty() {
        return Err(AppError::Unprocessable("email must not be empty".into()));
    }
    let display_name = b.display_name.trim();
    if display_name.is_empty() {
        return Err(AppError::Unprocessable("display_name must not be empty".into()));
    }
    if b.password.is_empty() {
        return Err(AppError::Unprocessable("password must not be empty".into()));
    }
    let hash = crate::auth::local::hash_password(&b.password)?;
    let admin = st.db.admin();
    let id = admin.create_user(email, display_name, &hash, b.is_administrator).await
        .map_err(map_email_conflict)?;
    let user = require_user(&admin, id).await?;
    audit::record(&st, &ctx, "user_created", None, None, serde_json::json!({
        "target_user_id": id, "email": user.email, "is_administrator": user.is_administrator,
    })).await;
    Ok(Json(user.into()))
}

#[derive(serde::Deserialize)]
pub struct PasswordBody {
    pub password: String,
}

pub async fn password_set(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(id): Path<i64>, Json(b): Json<PasswordBody>,
) -> Result<StatusCode, AppError> {
    if b.password.is_empty() {
        return Err(AppError::Unprocessable("password must not be empty".into()));
    }
    let admin = st.db.admin();
    require_user(&admin, id).await?;
    let hash = crate::auth::local::hash_password(&b.password)?;
    admin.set_password(id, &hash).await?;
    // An administrator-forced reset must kill any session the old password
    // opened — otherwise a stolen cookie survives its owner's password being
    // changed out from under it.
    admin.sessions_delete_for(id).await?;
    audit::record(&st, &ctx, "password_reset", None, None,
        serde_json::json!({"target_user_id": id})).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Deserialize)]
pub struct DisabledBody {
    pub disabled: bool,
}

pub async fn disabled_set(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(id): Path<i64>, Json(b): Json<DisabledBody>,
) -> Result<StatusCode, AppError> {
    let admin = st.db.admin();
    require_user(&admin, id).await?;
    admin.set_disabled(id, b.disabled).await?;
    if b.disabled {
        // `session_user` already filters out a disabled account's cookie, but
        // the row itself would otherwise linger until it expires — delete it
        // outright so revocation is immediate and the sessions table doesn't
        // carry dead rows for a disabled user's remaining TTL.
        admin.sessions_delete_for(id).await?;
    }
    audit::record(&st, &ctx, "user_disabled", None, None,
        serde_json::json!({"target_user_id": id, "disabled": b.disabled})).await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn grants_list(
    State(st): State<AppState>, Path(id): Path<i64>,
) -> Result<Json<Vec<Grant>>, AppError> {
    let admin = st.db.admin();
    require_user(&admin, id).await?;
    Ok(Json(admin.grant_rows_for(id).await?))
}

pub async fn grant_add(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(id): Path<i64>, Json(g): Json<Grant>,
) -> Result<StatusCode, AppError> {
    let admin = st.db.admin();
    require_user(&admin, id).await?;
    // Desktop mode's principal id is 0 (`auth/desktop.rs`'s `DesktopSingleUser`
    // — not a real `users` row), and `granted_by` carries a
    // `REFERENCES users(id)` constraint: a bare `Some(ctx.principal_id)`
    // there FK-violates (500) the moment an administrator grants anything in
    // desktop mode. Same pattern as `audit::record`'s `user_id`.
    let granted_by = (ctx.principal_id != 0).then_some(ctx.principal_id);
    admin.grant_add(id, g, granted_by).await.map_err(map_portfolio_fk)?;
    audit::record(&st, &ctx, "grant_added", Some(g.domain), g.portfolio, serde_json::json!({
        "target_user_id": id, "domain": g.domain.as_str(), "action": g.action.as_str(), "portfolio_id": g.portfolio,
    })).await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn grant_remove(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(id): Path<i64>, Json(g): Json<Grant>,
) -> Result<StatusCode, AppError> {
    let admin = st.db.admin();
    require_user(&admin, id).await?;
    admin.grant_remove(id, g).await?;
    audit::record(&st, &ctx, "grant_removed", Some(g.domain), g.portfolio, serde_json::json!({
        "target_user_id": id, "domain": g.domain.as_str(), "action": g.action.as_str(), "portfolio_id": g.portfolio,
    })).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Deserialize)]
pub struct RoleBody {
    pub role: String,
    #[serde(default)]
    pub scope: Option<i64>,
}

pub async fn role_assign(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(id): Path<i64>, Json(b): Json<RoleBody>,
) -> Result<StatusCode, AppError> {
    let role = Role::from_str(&b.role)
        .ok_or_else(|| AppError::Unprocessable(format!("unknown role '{}'", b.role)))?;
    let admin = st.db.admin();
    require_user(&admin, id).await?;
    // Same desktop-mode FK trap as `grant_add` above: principal id 0 has no
    // matching `users` row for `granted_by` to reference.
    let granted_by = (ctx.principal_id != 0).then_some(ctx.principal_id);
    admin.role_assign(id, role, b.scope, granted_by).await?;
    audit::record(&st, &ctx, "role_assigned", None, b.scope, serde_json::json!({
        "target_user_id": id, "role": role.as_str(), "scope": b.scope,
    })).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Deserialize)]
pub struct AuditQuery {
    pub limit: Option<i64>,
}

pub async fn audit_list(
    State(st): State<AppState>, Query(q): Query<AuditQuery>,
) -> Result<Json<Vec<AuditRow>>, AppError> {
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    Ok(Json(st.db.admin().audit_recent(limit).await?))
}

#[derive(serde::Deserialize)]
pub struct EnrolBody {
    pub token: String,
    pub password: String,
}

/// Completes first-administrator enrolment: consumes the single-use token
/// `ensure_first_administrator` issued and sets the real password. Not
/// gated by `.admin` — there is no administrator to authenticate as yet;
/// the token itself is the credential, exactly as a session cookie is for
/// `session_user`.
///
/// The token being a valid, live `sessions` row is necessary but never
/// sufficient: it is also how an ordinary login cookie resolves. Without the
/// sentinel check below, a stolen/leaked login cookie would let its holder
/// set that account's password without ever knowing the old one — a public,
/// unauthenticated privilege escalation. Enrolment may only ever act on a
/// user still carrying `UNUSABLE_PASSWORD_HASH`, i.e. one that has never
/// completed enrolment.
pub async fn enrol(
    State(st): State<AppState>, Json(b): Json<EnrolBody>,
) -> Result<StatusCode, AppError> {
    if b.password.is_empty() {
        return Err(AppError::Unprocessable("password must not be empty".into()));
    }
    let admin = st.db.admin();
    let token_hash = crate::auth::local::token_hash(&b.token);
    let user = admin.session_user(&token_hash).await?.ok_or(AppError::Unauthenticated)?;
    if user.password_hash != db::admin::UNUSABLE_PASSWORD_HASH {
        // An ordinary, already-enrolled account's session token — reject it
        // exactly like an unresolved one, so a stolen cookie learns nothing
        // beyond "this didn't work".
        return Err(AppError::Unauthenticated);
    }
    let hash = crate::auth::local::hash_password(&b.password)?;
    // Consume the token BEFORE writing the new password: fail-closed. If
    // `set_password` were first and failed partway (a rare DB error), the
    // token row would survive as a live `sessions` entry for an account
    // whose hash may already have changed — exactly the "usable admin
    // cookie" this whole check exists to prevent. Burning the token first
    // means the worst case of a failure here is a wasted enrolment attempt,
    // never a leftover credential.
    admin.session_delete(&token_hash).await?;
    admin.set_password(user.id, &hash).await?;

    let ctx = AuthCtx {
        principal_id: user.id,
        display_name: user.display_name.clone(),
        is_administrator: user.is_administrator,
        grants: db::auth::GrantSet::default(),
    };
    audit::record(&st, &ctx, "enrolled", None, None, serde_json::json!({"email": user.email})).await;
    Ok(StatusCode::NO_CONTENT)
}
