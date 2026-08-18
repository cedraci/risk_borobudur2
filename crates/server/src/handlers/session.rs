use crate::auth::local::{LocalAccounts, COOKIE_NAME, SESSION_TTL_HOURS};
use crate::auth::{AuthError, Principal};
use crate::config::Mode;
use crate::error::AppError;
use crate::state::AppState;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

#[derive(serde::Deserialize)]
pub struct LoginBody {
    pub email: String,
    pub password: String,
}

pub async fn login(
    State(st): State<AppState>, Json(body): Json<LoginBody>,
) -> Result<Response, AppError> {
    let Some(local) = st.local_accounts() else {
        // Desktop mode has no accounts to log in to.
        return Err(AppError::NotFound("login is not available in desktop mode".into()));
    };
    match local.login(&body.email, &body.password).await {
        Ok(token) => {
            let secure = if st.mode == Mode::Server { "; Secure" } else { "" };
            let cookie = format!(
                "{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Strict{secure}; Max-Age={}",
                SESSION_TTL_HOURS * 3600);
            Ok(([(header::SET_COOKIE, cookie)], Json(serde_json::json!({"ok": true}))).into_response())
        }
        Err(AuthError::LockedOut { retry_after_secs }) => Err(AppError::LockedOut(retry_after_secs)),
        Err(AuthError::Unauthenticated) => Err(AppError::Unauthenticated),
        Err(AuthError::Internal(e)) => Err(AppError::Internal(e)),
    }
}

pub async fn logout(State(st): State<AppState>, headers: HeaderMap) -> Result<Response, AppError> {
    if let Some(local) = st.local_accounts() {
        local.logout(&headers).await?;
    }
    let cookie = format!("{COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0");
    Ok(([(header::SET_COOKIE, cookie)], StatusCode::NO_CONTENT).into_response())
}

#[derive(serde::Serialize)]
pub struct MeResponse {
    pub display_name: String,
    pub is_administrator: bool,
    pub capabilities: Vec<Capability>,
}

#[derive(serde::Serialize)]
pub struct Capability {
    pub domain: &'static str,
    pub action: &'static str,
    pub portfolio_id: Option<i64>,
}

pub async fn me(
    State(st): State<AppState>, headers: HeaderMap,
) -> Result<Json<MeResponse>, AppError> {
    let p: Principal = st.identity.authenticate(&headers).await.map_err(AppError::from)?;
    Ok(Json(MeResponse {
        display_name: p.display_name,
        is_administrator: p.is_administrator,
        capabilities: p.grants.iter().map(|g| Capability {
            domain: g.domain.as_str(),
            action: g.action.as_str(),
            portfolio_id: g.portfolio,
        }).collect(),
    }))
}
