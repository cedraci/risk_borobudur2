use crate::auth::middleware::require;
use crate::state::AppState;
use axum::routing::MethodRouter;
use axum::Router;
use db::auth::{Action, Domain};

/// Every route declares itself protected or public. There is no third option,
/// so an endpoint added later cannot quietly ship unguarded.
pub trait ProtectExt {
    fn protected(self, path: &str, mr: MethodRouter<AppState>, domain: Domain, action: Action) -> Self;
    fn public(self, path: &str, mr: MethodRouter<AppState>) -> Self;
}

impl ProtectExt for Router<AppState> {
    fn protected(self, path: &str, mr: MethodRouter<AppState>, domain: Domain, action: Action) -> Self {
        self.route(path, mr.layer(axum::middleware::from_fn(
            move |req, next| require(domain, action, req, next))))
    }

    fn public(self, path: &str, mr: MethodRouter<AppState>) -> Self {
        self.route(path, mr)
    }
}
