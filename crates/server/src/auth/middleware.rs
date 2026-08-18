use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use db::auth::{Action, AuthCtx, Domain};

/// Resolves the principal and inserts an `AuthCtx` extension. Runs for every
/// route, protected or public — a public route may still want to know who is
/// calling, and resolving in one place keeps the two modes identical.
pub async fn resolve_principal(
    State(st): State<AppState>, mut req: Request, next: Next,
) -> Result<Response, AppError> {
    if let Ok(p) = st.identity.authenticate(req.headers()).await {
        req.extensions_mut().insert(AuthCtx {
            principal_id: p.id,
            display_name: p.display_name,
            is_administrator: p.is_administrator,
            grants: p.grants,
        });
    }
    Ok(next.run(req).await)
}

/// Enforces one route's declared primary requirement. Attached per route by
/// `.protected`.
pub async fn require(
    domain: Domain, action: Action, req: Request, next: Next,
) -> Result<Response, AppError> {
    let ctx = req.extensions().get::<AuthCtx>().cloned().ok_or(AppError::Unauthenticated)?;
    let (portfolio, req) = portfolio_id_from_path(req).await;
    let allowed = match portfolio {
        Some(id) => ctx.grants.allows(domain, action, Some(id)),
        None => ctx.grants.allows(domain, action, None),
    };
    if allowed {
        return Ok(next.run(req).await);
    }
    Err(AppError::Forbidden(db::auth::Denied {
        domain,
        action,
        portfolio,
        kind: match portfolio {
            Some(id) if !ctx.grants.any_domain_on(id) => db::auth::DeniedKind::OutOfScope,
            _ => db::auth::DeniedKind::NotGranted,
        },
    }))
}

/// Routes name the portfolio parameter `{id}` throughout.
///
/// `RawPathParams` reads a private extension through `FromRequestParts`, so it
/// cannot be fetched with `extensions().get()` — the request has to be split
/// and reassembled. Returning the request avoids a clone.
async fn portfolio_id_from_path(req: Request) -> (Option<i64>, Request) {
    use axum::extract::{FromRequestParts, RawPathParams};
    let (mut parts, body) = req.into_parts();
    let id = RawPathParams::from_request_parts(&mut parts, &()).await.ok()
        .and_then(|params| {
            params.iter()
                .find(|(k, _)| *k == "id")
                .and_then(|(_, v)| v.parse::<i64>().ok())
        });
    (id, Request::from_parts(parts, body))
}
