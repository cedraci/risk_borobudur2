use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use db::auth::{Action, AuthCtx, Domain};

/// The request's client address, inserted for every request whether or not a
/// principal resolved. Handlers that audit an unauthenticated event (a failed
/// sign-in) read it from here; everything else finds it on `AuthCtx`.
#[derive(Clone, Debug)]
pub struct ClientAddr(pub Option<String>);

/// Resolves the principal and inserts an `AuthCtx` extension. Runs for every
/// route, protected or public — a public route may still want to know who is
/// calling, and resolving in one place keeps the two modes identical.
pub async fn resolve_principal(
    State(st): State<AppState>, mut req: Request, next: Next,
) -> Result<Response, AppError> {
    // Resolved before the principal so the audit log can attribute an
    // unauthenticated request too — `session::login` reads it back out of the
    // extensions to stamp `login_failed`/`login_locked` rows, which are
    // precisely the ones with no principal to name.
    let source_addr = crate::auth::client_addr::from_request(
        req.headers(),
        req.extensions().get::<axum::extract::ConnectInfo<std::net::SocketAddr>>().map(|ci| ci.0),
    );
    req.extensions_mut().insert(ClientAddr(source_addr.clone()));
    if let Ok(p) = st.identity.authenticate(req.headers()).await {
        req.extensions_mut().insert(AuthCtx {
            principal_id: p.id,
            display_name: p.display_name,
            is_administrator: p.is_administrator,
            grants: p.grants,
            source_addr,
        });
    }
    Ok(next.run(req).await)
}

/// Enforces only that a principal was resolved — no grant check. Attached by
/// `.authenticated`, for a route any signed-in principal may call and whose
/// handler itself narrows the response to what that principal's grants cover
/// (see `handlers::portfolios::list`).
pub async fn authenticate(req: Request, next: Next) -> Result<Response, AppError> {
    if req.extensions().get::<AuthCtx>().is_none() {
        return Err(AppError::Unauthenticated);
    }
    Ok(next.run(req).await)
}

/// Enforces that the resolved principal is an administrator. Attached by
/// `.admin`, for every `/api/admin/*` route — administrator status is a flag
/// on the principal, not a grant, so it has no `Domain`/`Action` pair to check
/// through `require`/`require_global`.
pub async fn require_administrator(req: Request, next: Next) -> Result<Response, AppError> {
    let ctx = req.extensions().get::<AuthCtx>().cloned().ok_or(AppError::Unauthenticated)?;
    if !ctx.is_administrator {
        return Err(AppError::AdministratorRequired);
    }
    Ok(next.run(req).await)
}

/// Enforces one route's declared primary requirement on a portfolio-scoped
/// route. Attached per route by `.protected`, which requires `{id}` in the
/// route's path — scope is declared at the call site, never inferred here.
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

/// Enforces one route's declared primary requirement on an instance-wide
/// route. Attached per route by `.protected_global` — never reads path
/// params, so a `{id}` segment that happens to name something other than a
/// portfolio (a future `/api/admin/users/{id}`, say) cannot be mistaken for
/// portfolio scope. Mirrors `Scoped::global_denial`: there is no "out of
/// scope" for an instance-wide resource, only granted or not.
pub async fn require_global(
    domain: Domain, action: Action, req: Request, next: Next,
) -> Result<Response, AppError> {
    let ctx = req.extensions().get::<AuthCtx>().cloned().ok_or(AppError::Unauthenticated)?;
    if ctx.grants.allows(domain, action, None) {
        return Ok(next.run(req).await);
    }
    Err(AppError::Forbidden(db::auth::Denied {
        domain,
        action,
        portfolio: None,
        kind: db::auth::DeniedKind::NotGranted,
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
