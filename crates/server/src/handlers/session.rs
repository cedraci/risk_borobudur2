use crate::auth::local::{COOKIE_NAME, SESSION_TTL_HOURS};
use crate::auth::middleware::ClientAddr;
use crate::auth::{AuthError, Principal};
use crate::config::Mode;
use crate::error::AppError;
use crate::state::AppState;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use db::auth::{AuthCtx, GrantSet};

/// A login attempt has no `AuthCtx` yet — that's what a successful login
/// produces. This builds just enough of one for `audit::record` to log the
/// attempt against: the real principal id and display name once login has
/// actually succeeded, or (a failed/locked attempt may not resolve to a real
/// user id at all) principal id `0` and the attempted email as the actor
/// label instead.
fn login_actor(principal_id: i64, label: &str, source_addr: Option<String>) -> AuthCtx {
    AuthCtx {
        principal_id,
        display_name: label.to_string(),
        is_administrator: false,
        grants: GrantSet::default(),
        source_addr,
    }
}

#[derive(serde::Deserialize)]
pub struct LoginBody {
    pub email: String,
    pub password: String,
}

pub async fn login(
    State(st): State<AppState>, Extension(addr): Extension<ClientAddr>, Json(body): Json<LoginBody>,
) -> Result<Response, AppError> {
    let Some(local) = st.local_accounts() else {
        // Desktop mode has no accounts to log in to.
        return Err(AppError::NotFound("login is not available in desktop mode".into()));
    };
    // Checked before any password work: the point of throttling a source is
    // to stop paying for its attempts, and an Argon2 verification is the most
    // expensive thing an unauthenticated caller can make this process do.
    let source = addr.0.as_deref();
    let now = std::time::Instant::now();
    if let Some(retry) = st.login_throttle.retry_after(source, now) {
        let ctx = login_actor(0, &body.email, addr.0.clone());
        crate::audit::record(&st, &ctx, "login_throttled", None, None,
            serde_json::json!({"email": body.email, "retry_after_secs": retry.as_secs()})).await;
        return Err(AppError::LockedOut(retry.as_secs()));
    }
    match local.login(&body.email, &body.password).await {
        Ok(success) => {
            st.login_throttle.reset(source);
            let ctx = login_actor(success.user_id, &success.display_name, addr.0.clone());
            crate::audit::record(&st, &ctx, "login", None, None,
                serde_json::json!({"email": body.email})).await;
            let secure = if st.mode == Mode::Server { "; Secure" } else { "" };
            let cookie = format!(
                "{COOKIE_NAME}={}; Path=/; HttpOnly; SameSite=Strict{secure}; Max-Age={}",
                success.token, SESSION_TTL_HOURS * 3600);
            Ok(([(header::SET_COOKIE, cookie)], Json(serde_json::json!({"ok": true}))).into_response())
        }
        Err(AuthError::LockedOut { retry_after_secs }) => {
            st.login_throttle.record_failure(source, now);
            let ctx = login_actor(0, &body.email, addr.0.clone());
            crate::audit::record(&st, &ctx, "login_locked", None, None,
                serde_json::json!({"email": body.email, "retry_after_secs": retry_after_secs})).await;
            Err(AppError::LockedOut(retry_after_secs))
        }
        Err(AuthError::Unauthenticated) => {
            st.login_throttle.record_failure(source, now);
            let ctx = login_actor(0, &body.email, addr.0.clone());
            crate::audit::record(&st, &ctx, "login_failed", None, None,
                serde_json::json!({"email": body.email})).await;
            // If this failure is the one that tips the source over, say so on
            // the spot rather than answering 401 and 429ing the next attempt —
            // "too many attempts, wait" is the useful thing to tell whoever is
            // actually at the keyboard.
            match st.login_throttle.retry_after(source, now) {
                Some(retry) => Err(AppError::LockedOut(retry.as_secs())),
                None => Err(AppError::Unauthenticated),
            }
        }
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
