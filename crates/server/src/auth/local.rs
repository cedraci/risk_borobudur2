use super::{AuthError, IdentityProvider, Principal};
use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::http::HeaderMap;
use rand::RngCore;
use std::sync::Arc;

pub const COOKIE_NAME: &str = "borobudur_session";
pub const SESSION_TTL_HOURS: i64 = 12;
const LOCK_AFTER: i32 = 5;
const LOCK_SECS: i64 = 900;

// `db::Db` intentionally does not derive `Debug` (it would invite printing a
// connection pool), so this impl is written by hand rather than derived.
pub struct LocalAccounts {
    db: Arc<db::Db>,
}

impl std::fmt::Debug for LocalAccounts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalAccounts").finish_non_exhaustive()
    }
}

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| anyhow::anyhow!("hashing failed: {e}"))
}

fn verify_password(password: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok(),
        Err(_) => false,
    }
}

/// 256 bits from the OS CSPRNG. Only the SHA-256 of this ever reaches the
/// database, so a stolen dump yields no usable cookie.
fn new_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn token_hash(token: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    headers.get(axum::http::header::COOKIE)?
        .to_str().ok()?
        .split(';')
        .filter_map(|p| p.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE_NAME)
        .map(|(_, v)| v.to_string())
}

/// A successful login: the raw session token (the caller sets the cookie)
/// plus enough of the resolved user to let the caller audit the event
/// against a real principal id.
pub struct LoginSuccess {
    pub token: String,
    pub user_id: i64,
    pub display_name: String,
}

impl LocalAccounts {
    pub fn new(db: Arc<db::Db>) -> Self {
        LocalAccounts { db }
    }

    pub async fn login(&self, email: &str, password: &str) -> Result<LoginSuccess, AuthError> {
        let admin = self.db.admin();
        let state = admin.login_state(email).await?;
        if state.locked {
            return Err(AuthError::LockedOut { retry_after_secs: state.retry_after_secs as u64 });
        }

        let user = admin.user_by_email(email).await?;
        // An unknown account still pays the hashing cost, so response timing
        // does not distinguish "no such user" from "wrong password". A
        // disabled account pays the same cost too — verify unconditionally,
        // then discard the result — so timing cannot distinguish "exists and
        // disabled" from either of the other two cases either.
        let ok = match &user {
            Some(u) => {
                let verified = verify_password(password, &u.password_hash);
                verified && !u.disabled
            }
            None => {
                let _ = verify_password(password, "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHRzYWx0$\
                                                    5Zt7pKQKUwjHhBDUMTLDCcxT4rWmMoTPxSDLbGYwqPo");
                false
            }
        };

        if !ok {
            let st = admin.login_record_failure(email, LOCK_AFTER, LOCK_SECS).await?;
            if st.locked {
                return Err(AuthError::LockedOut { retry_after_secs: st.retry_after_secs as u64 });
            }
            return Err(AuthError::Unauthenticated);
        }

        let user = user.expect("verified above");
        admin.login_reset(email).await?;
        let token = new_token();
        admin.session_create(&token_hash(&token), user.id, SESSION_TTL_HOURS).await?;
        Ok(LoginSuccess { token, user_id: user.id, display_name: user.display_name })
    }

    pub async fn logout(&self, headers: &HeaderMap) -> anyhow::Result<()> {
        if let Some(t) = cookie_token(headers) {
            self.db.admin().session_delete(&token_hash(&t)).await?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl IdentityProvider for LocalAccounts {
    async fn authenticate(&self, headers: &HeaderMap) -> Result<Principal, AuthError> {
        let token = cookie_token(headers).ok_or(AuthError::Unauthenticated)?;
        let admin = self.db.admin();
        let user = admin.session_user(&token_hash(&token)).await?
            .ok_or(AuthError::Unauthenticated)?;
        let grants = admin.grants_for(user.id).await?;
        Ok(Principal {
            id: user.id,
            display_name: user.display_name,
            is_administrator: user.is_administrator,
            grants,
        })
    }
}
