//! THE PRIVILEGED PATH.
//!
//! Every other query in this crate goes through `Scoped` and requires an
//! `AuthCtx`. This module cannot: loading a principal's grants is what *builds*
//! the `AuthCtx`. It is therefore the single hole in the wall, and its only
//! legitimate consumers are identity resolution, grant administration and the
//! audit log. `crates/db/tests/admin_isolation.rs` asserts that.

use crate::auth::{Action, Domain, Grant, GrantSet, Role};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};

/// Never a real Argon2 hash: `PasswordHash::new` fails to parse it, so
/// password verification against it always returns `false`, for any
/// password, forever, until `set_password` replaces it with a real one. A
/// freshly created account (first-administrator enrolment, or any account an
/// administrator creates before setting a password) is stamped with this and
/// starts out unable to log in at all — there is no default password to
/// forget to change.
///
/// It also marks an account as "not yet enrolled" wherever that matters:
/// `POST /api/enrol` will only ever act on a user still carrying this exact
/// hash (`crates/server/src/handlers/admin.rs::enrol`), and cookie session
/// authentication refuses a session row belonging to a user who still
/// carries it (`crates/server/src/auth/local.rs::authenticate`) — otherwise
/// an enrolment token, which is stored as an ordinary `sessions` row, would
/// double as a live administrator cookie for its whole lifetime.
pub const UNUSABLE_PASSWORD_HASH: &str = "!unusable!";

pub struct Admin<'a> {
    pool: &'a PgPool,
}

#[derive(Clone, Debug)]
pub struct UserRow {
    pub id: i64,
    pub email: String,
    pub display_name: String,
    pub password_hash: String,
    pub is_administrator: bool,
    pub disabled: bool,
}

#[derive(Clone, Debug)]
pub struct AuditEvent {
    pub user_id: Option<i64>,
    pub actor_label: String,
    pub action: String,
    pub domain: Option<Domain>,
    pub portfolio_id: Option<i64>,
    pub detail: serde_json::Value,
    pub source_addr: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct AuditRow {
    pub id: i64,
    pub at: DateTime<Utc>,
    pub actor_label: String,
    pub action: String,
    pub domain: Option<String>,
    pub portfolio_id: Option<i64>,
    pub detail: serde_json::Value,
    pub source_addr: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct LockState {
    pub locked: bool,
    pub failures: i32,
    pub retry_after_secs: i64,
}

fn user_from_row(r: &sqlx::postgres::PgRow) -> UserRow {
    UserRow {
        id: r.get("id"),
        email: r.get("email"),
        display_name: r.get("display_name"),
        password_hash: r.get("password_hash"),
        is_administrator: r.get("is_administrator"),
        disabled: r.get("disabled"),
    }
}

impl<'a> Admin<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Admin { pool }
    }

    pub async fn user_count(&self) -> anyhow::Result<i64> {
        Ok(sqlx::query_scalar("SELECT count(*) FROM users").fetch_one(self.pool).await?)
    }

    pub async fn create_user(
        &self, email: &str, display_name: &str, password_hash: &str, is_administrator: bool,
    ) -> anyhow::Result<i64> {
        Ok(sqlx::query_scalar(
            "INSERT INTO users (email, display_name, password_hash, is_administrator)
             VALUES (lower($1), $2, $3, $4) RETURNING id")
            .bind(email).bind(display_name).bind(password_hash).bind(is_administrator)
            .fetch_one(self.pool).await?)
    }

    pub async fn user_by_email(&self, email: &str) -> anyhow::Result<Option<UserRow>> {
        let row = sqlx::query("SELECT * FROM users WHERE email = lower($1)")
            .bind(email).fetch_optional(self.pool).await?;
        Ok(row.as_ref().map(user_from_row))
    }

    pub async fn user_by_id(&self, id: i64) -> anyhow::Result<Option<UserRow>> {
        let row = sqlx::query("SELECT * FROM users WHERE id = $1")
            .bind(id).fetch_optional(self.pool).await?;
        Ok(row.as_ref().map(user_from_row))
    }

    pub async fn users_list(&self) -> anyhow::Result<Vec<UserRow>> {
        let rows = sqlx::query("SELECT * FROM users ORDER BY display_name")
            .fetch_all(self.pool).await?;
        Ok(rows.iter().map(user_from_row).collect())
    }

    pub async fn set_password(&self, user_id: i64, password_hash: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
            .bind(user_id).bind(password_hash).execute(self.pool).await?;
        Ok(())
    }

    pub async fn set_disabled(&self, user_id: i64, disabled: bool) -> anyhow::Result<()> {
        sqlx::query("UPDATE users SET disabled = $2 WHERE id = $1")
            .bind(user_id).bind(disabled).execute(self.pool).await?;
        Ok(())
    }

    /// Administrators other than `excluding` who could actually sign in:
    /// enabled and past enrolment (an account still on the unusable sentinel
    /// hash cannot authenticate, so it must not count as a fallback admin).
    pub async fn other_usable_admin_count(&self, excluding: i64) -> anyhow::Result<i64> {
        let row = sqlx::query(
            "SELECT count(*) FROM users
             WHERE is_administrator AND NOT disabled AND id <> $1 AND password_hash <> $2")
            .bind(excluding).bind(UNUSABLE_PASSWORD_HASH)
            .fetch_one(self.pool).await?;
        Ok(row.get::<i64, _>(0))
    }

    pub async fn grant_rows_for(&self, user_id: i64) -> anyhow::Result<Vec<Grant>> {
        let rows = sqlx::query("SELECT domain, action, portfolio_id FROM grants WHERE user_id = $1")
            .bind(user_id).fetch_all(self.pool).await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let d: String = r.get("domain");
            let a: String = r.get("action");
            // A row failing to parse means the CHECK constraint and this enum
            // have diverged; that is a bug, not user input, so it is loud.
            let domain = Domain::from_str(&d)
                .ok_or_else(|| anyhow::anyhow!("unknown domain in grants: {d}"))?;
            let action = Action::from_str(&a)
                .ok_or_else(|| anyhow::anyhow!("unknown action in grants: {a}"))?;
            out.push(Grant { domain, action, portfolio: r.get("portfolio_id") });
        }
        Ok(out)
    }

    pub async fn grants_for(&self, user_id: i64) -> anyhow::Result<GrantSet> {
        Ok(GrantSet::from_grants(self.grant_rows_for(user_id).await?))
    }

    pub async fn grant_add(&self, user_id: i64, g: Grant, granted_by: Option<i64>) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO grants (user_id, domain, action, portfolio_id, granted_by)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (user_id, domain, action, portfolio_id) DO NOTHING")
            .bind(user_id).bind(g.domain.as_str()).bind(g.action.as_str())
            .bind(g.portfolio).bind(granted_by)
            .execute(self.pool).await?;
        Ok(())
    }

    pub async fn grant_remove(&self, user_id: i64, g: Grant) -> anyhow::Result<()> {
        sqlx::query(
            "DELETE FROM grants WHERE user_id = $1 AND domain = $2 AND action = $3
             AND portfolio_id IS NOT DISTINCT FROM $4")
            .bind(user_id).bind(g.domain.as_str()).bind(g.action.as_str()).bind(g.portfolio)
            .execute(self.pool).await?;
        Ok(())
    }

    /// Re-assigning the same role at the same scope is idempotent, not a
    /// constraint violation — `idx_user_roles_unique` (NULLS NOT DISTINCT, so
    /// two instance-wide rows collide) gives `ON CONFLICT` a real target.
    pub async fn role_assign(
        &self, user_id: i64, role: Role, scope: Option<i64>, granted_by: Option<i64>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO user_roles (user_id, role, portfolio_id) VALUES ($1, $2, $3)
             ON CONFLICT (user_id, role, portfolio_id) DO NOTHING")
            .bind(user_id).bind(role.as_str()).bind(scope)
            .execute(self.pool).await?;
        for g in role.expand(scope) {
            self.grant_add(user_id, g, granted_by).await?;
        }
        Ok(())
    }

    pub async fn session_create(&self, token_hash: &str, user_id: i64, ttl_hours: i64) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO sessions (token_hash, user_id, expires_at)
             VALUES ($1, $2, now() + make_interval(hours => $3::int))")
            .bind(token_hash).bind(user_id).bind(ttl_hours as i32)
            .execute(self.pool).await?;
        Ok(())
    }

    pub async fn session_user(&self, token_hash: &str) -> anyhow::Result<Option<UserRow>> {
        let row = sqlx::query(
            "SELECT u.* FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = $1 AND s.expires_at > now() AND NOT u.disabled")
            .bind(token_hash).fetch_optional(self.pool).await?;
        Ok(row.as_ref().map(user_from_row))
    }

    pub async fn session_delete(&self, token_hash: &str) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(token_hash).execute(self.pool).await?;
        Ok(())
    }

    pub async fn sessions_delete_for(&self, user_id: i64) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(user_id).execute(self.pool).await?;
        Ok(())
    }

    /// Opportunistic hygiene: `session_user`'s own `expires_at > now()` check
    /// already keeps an expired row from ever authenticating anything, so
    /// this is not a security fix — it just keeps the table from growing a
    /// dead row for every session that ever expired instead of being logged
    /// out. Called from the login path (`LocalAccounts::login`) rather than
    /// on a timer: there is no background scheduler in this process, and a
    /// sweep on every login is cheap and frequent enough not to matter.
    pub async fn sessions_delete_expired(&self) -> anyhow::Result<u64> {
        let res = sqlx::query("DELETE FROM sessions WHERE expires_at <= now()")
            .execute(self.pool).await?;
        Ok(res.rows_affected())
    }

    pub async fn audit_append(&self, e: AuditEvent) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO audit_events (user_id, actor_label, action, domain, portfolio_id, detail, source_addr)
             VALUES ($1, $2, $3, $4, $5, $6, $7)")
            .bind(e.user_id).bind(&e.actor_label).bind(&e.action)
            .bind(e.domain.map(|d| d.as_str())).bind(e.portfolio_id)
            .bind(&e.detail).bind(&e.source_addr)
            .execute(self.pool).await?;
        Ok(())
    }

    pub async fn audit_recent(&self, limit: i64) -> anyhow::Result<Vec<AuditRow>> {
        let rows = sqlx::query(
            "SELECT id, at, actor_label, action, domain, portfolio_id, detail, source_addr
             FROM audit_events ORDER BY at DESC, id DESC LIMIT $1")
            .bind(limit).fetch_all(self.pool).await?;
        Ok(rows.iter().map(|r| AuditRow {
            id: r.get("id"),
            at: r.get("at"),
            actor_label: r.get("actor_label"),
            action: r.get("action"),
            domain: r.get("domain"),
            portfolio_id: r.get("portfolio_id"),
            detail: r.get("detail"),
            source_addr: r.get("source_addr"),
        }).collect())
    }

    pub async fn login_state(&self, email: &str) -> anyhow::Result<LockState> {
        let row = sqlx::query(
            "SELECT failures, GREATEST(0, EXTRACT(EPOCH FROM (locked_until - now()))::bigint) AS retry
             FROM login_attempts WHERE email = lower($1)")
            .bind(email).fetch_optional(self.pool).await?;
        Ok(match row {
            None => LockState { locked: false, failures: 0, retry_after_secs: 0 },
            Some(r) => {
                let retry: i64 = r.get("retry");
                LockState { locked: retry > 0, failures: r.get("failures"), retry_after_secs: retry }
            }
        })
    }

    /// Records one failure and returns the resulting state. Locking is applied
    /// on the `lock_after`-th failure and every failure beyond it, so an
    /// attacker who keeps guessing keeps extending their own lockout.
    pub async fn login_record_failure(
        &self, email: &str, lock_after: i32, lock_secs: i64,
    ) -> anyhow::Result<LockState> {
        let row = sqlx::query(
            "INSERT INTO login_attempts (email, failures, last_failure_at)
             VALUES (lower($1), 1, now())
             ON CONFLICT (email) DO UPDATE
               SET failures = login_attempts.failures + 1,
                   last_failure_at = now(),
                   locked_until = CASE
                     WHEN login_attempts.failures + 1 >= $2
                     THEN now() + make_interval(secs => $3::double precision)
                     ELSE login_attempts.locked_until END
             RETURNING failures,
                       GREATEST(0, EXTRACT(EPOCH FROM (locked_until - now()))::bigint) AS retry")
            .bind(email).bind(lock_after).bind(lock_secs as f64)
            .fetch_one(self.pool).await?;
        let retry: i64 = row.get("retry");
        Ok(LockState { locked: retry > 0, failures: row.get("failures"), retry_after_secs: retry })
    }

    pub async fn login_reset(&self, email: &str) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM login_attempts WHERE email = lower($1)")
            .bind(email).execute(self.pool).await?;
        Ok(())
    }
}
