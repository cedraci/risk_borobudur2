use crate::auth::{desktop::DesktopSingleUser, local::LocalAccounts, IdentityProvider};
use crate::config::Mode;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<db::Db>,
    pub identity: Arc<dyn IdentityProvider>,
    pub mode: Mode,
    // Handlers pre-dating this task still take `&PgPool` directly (they go
    // through `db::repo`'s free functions, not `Db`). `Db::pool()` is
    // deliberately `pub(crate)` to the `db` crate, so this crate cannot reach
    // it through `db`. Keeping a clone here — the same pool `Db` wraps, and
    // just as cheap to clone — lets those handlers keep working unmodified
    // until Task 8 moves them onto `Scoped`. Not part of this task's declared
    // `AppState` surface; see the Task 7 report for why it was added.
    pub pool: sqlx::PgPool,
}

impl AppState {
    pub fn desktop(pool: sqlx::PgPool) -> Self {
        AppState {
            db: Arc::new(db::Db::from_pool(pool.clone())),
            identity: Arc::new(DesktopSingleUser),
            mode: Mode::Desktop,
            pool,
        }
    }

    pub fn server(pool: sqlx::PgPool) -> Self {
        let db = Arc::new(db::Db::from_pool(pool.clone()));
        AppState {
            identity: Arc::new(LocalAccounts::new(db.clone())),
            db,
            mode: Mode::Server,
            pool,
        }
    }

    /// `Some` only in server mode — desktop mode has no accounts.
    pub fn local_accounts(&self) -> Option<LocalAccounts> {
        match self.mode {
            Mode::Server => Some(LocalAccounts::new(self.db.clone())),
            Mode::Desktop => None,
        }
    }
}
