pub mod desktop;
pub mod local;
pub mod middleware;

use axum::http::HeaderMap;
use db::auth::GrantSet;

/// Who is making this request, and what they may do. Everything downstream sees
/// only this — which is what lets OIDC become a third provider later with no
/// change below the seam.
#[derive(Clone, Debug)]
pub struct Principal {
    pub id: i64,
    pub display_name: String,
    pub is_administrator: bool,
    pub grants: GrantSet,
}

#[derive(Debug)]
pub enum AuthError {
    Unauthenticated,
    LockedOut { retry_after_secs: u64 },
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for AuthError {
    fn from(e: anyhow::Error) -> Self {
        AuthError::Internal(e)
    }
}

#[async_trait::async_trait]
pub trait IdentityProvider: Send + Sync + std::fmt::Debug {
    async fn authenticate(&self, headers: &HeaderMap) -> Result<Principal, AuthError>;
}
