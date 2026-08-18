use crate::auth::middleware::{authenticate, require, require_global};
use crate::state::AppState;
use axum::routing::MethodRouter;
use axum::Router;
use db::auth::{Action, Domain};

/// Every route declares itself portfolio-scoped (`.protected`), instance-wide
/// (`.protected_global`) or public. There is no fourth option, so an endpoint
/// added later cannot quietly ship unguarded — and scope is stated at the
/// call site rather than inferred from whether the path happens to contain a
/// `{id}` segment, so a future route with an unrelated `{id}` (a
/// `/api/admin/users/{id}`, say) cannot be silently mistaken for a portfolio.
pub trait ProtectExt {
    /// Portfolio-scoped. `path` must contain the literal segment `{id}` —
    /// enforced by a panic at router-construction time, not a silent
    /// fallback: a portfolio-scoped route with no portfolio id in its path is
    /// a declaration bug, and this fails the build, not a request at runtime.
    fn protected(self, path: &str, mr: MethodRouter<AppState>, domain: Domain, action: Action) -> Self;
    /// Instance-wide. Its gate never reads path params — the scope checked is
    /// always `None`, matching `Scoped::authorize_global`.
    fn protected_global(self, path: &str, mr: MethodRouter<AppState>, domain: Domain, action: Action) -> Self;
    /// Any authenticated principal — 401 if no `AuthCtx` was resolved, no
    /// grant check beyond that. For a route whose handler filters its own
    /// response to what the principal may see (`GET /api/portfolios`) rather
    /// than gating access to a single resource.
    fn authenticated(self, path: &str, mr: MethodRouter<AppState>) -> Self;
    fn public(self, path: &str, mr: MethodRouter<AppState>) -> Self;
}

impl ProtectExt for Router<AppState> {
    fn protected(self, path: &str, mr: MethodRouter<AppState>, domain: Domain, action: Action) -> Self {
        assert!(
            path.contains("{id}"),
            "`.protected(\"{path}\", ..)` declares a portfolio-scoped route, but its \
             path has no `{{id}}` segment — use `.protected_global` for an \
             instance-wide route, or add `{{id}}` if this route does carry a \
             portfolio id"
        );
        self.route(path, mr.layer(axum::middleware::from_fn(
            move |req, next| require(domain, action, req, next))))
    }

    fn protected_global(self, path: &str, mr: MethodRouter<AppState>, domain: Domain, action: Action) -> Self {
        self.route(path, mr.layer(axum::middleware::from_fn(
            move |req, next| require_global(domain, action, req, next))))
    }

    fn authenticated(self, path: &str, mr: MethodRouter<AppState>) -> Self {
        self.route(path, mr.layer(axum::middleware::from_fn(authenticate)))
    }

    fn public(self, path: &str, mr: MethodRouter<AppState>) -> Self {
        self.route(path, mr)
    }
}
