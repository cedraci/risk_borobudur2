use super::{AuthError, IdentityProvider, Principal};
use axum::http::HeaderMap;
use db::auth::GrantSet;

/// Desktop mode's identity. Not a bypass: it satisfies the same trait, travels
/// the same middleware, and produces the same `AuthCtx` shape as a real login.
#[derive(Debug)]
pub struct DesktopSingleUser;

#[async_trait::async_trait]
impl IdentityProvider for DesktopSingleUser {
    async fn authenticate(&self, _headers: &HeaderMap) -> Result<Principal, AuthError> {
        Ok(Principal {
            id: 0,
            display_name: "desktop".to_string(),
            is_administrator: true,
            grants: GrantSet::all_access(),
        })
    }
}
