//! First-administrator enrolment. `db::admin` is the privileged path
//! (`crates/db/tests/admin_isolation.rs` enforces that only a short allow-list
//! of files may reach it); this module is on that allow-list alongside
//! `handlers/admin.rs`, because ensuring the very first account exists is the
//! same chicken-and-egg problem as resolving identity in the first place.

use db::admin::UNUSABLE_PASSWORD_HASH;
use rand::RngCore;

const ENROLMENT_TOKEN_TTL_HOURS: i64 = 1;

/// On first start in server mode with an empty users table, create one
/// administrator and return a single-use enrolment token. No default password
/// exists at any point, so there is nothing to forget to change.
///
/// The enrolment token is stored in `sessions` — the same table cookie
/// sessions use — under a 1-hour TTL. Reusing it means expiry and single-use
/// (a consumed row is deleted, exactly like a logout) are already
/// implemented; `POST /api/enrol` looks the token up the same way
/// `session_user` resolves a cookie.
pub async fn ensure_first_administrator(
    db: &db::Db, admin_email: &str,
) -> anyhow::Result<Option<String>> {
    let admin = db.admin();
    if admin.user_count().await? > 0 {
        return Ok(None);
    }
    let user_id = admin.create_user(admin_email, "Administrator", UNUSABLE_PASSWORD_HASH, true).await?;

    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let token = hex::encode(bytes);
    admin.session_create(&crate::auth::local::token_hash(&token), user_id, ENROLMENT_TOKEN_TTL_HOURS).await?;

    Ok(Some(token))
}

/// Whether the `users` table is currently empty. `main` uses this to warn
/// when server mode starts with no way to sign in at all — no users exist
/// and no `BOROBUDUR_ADMIN_EMAIL` was set to enrol one.
pub async fn no_users_exist(db: &db::Db) -> anyhow::Result<bool> {
    Ok(db.admin().user_count().await? == 0)
}
