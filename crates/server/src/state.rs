use crate::auth::throttle::Throttle;
use crate::auth::{desktop::DesktopSingleUser, local::LocalAccounts, IdentityProvider};
use crate::config::Mode;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<db::Db>,
    pub identity: Arc<dyn IdentityProvider>,
    pub mode: Mode,
    /// Per-source sign-in throttling. Shared across the process (hence the
    /// `Arc`) because it is the whole point: the counter follows the origin,
    /// not the connection or the account.
    pub login_throttle: Arc<Throttle>,
}

impl AppState {
    pub fn desktop(db: db::Db) -> Self {
        AppState {
            db: Arc::new(db),
            identity: Arc::new(DesktopSingleUser),
            mode: Mode::Desktop,
            login_throttle: Arc::new(Throttle::new()),
        }
    }

    pub fn server(db: db::Db) -> Self {
        let db = Arc::new(db);
        AppState {
            identity: Arc::new(LocalAccounts::new(db.clone())),
            db,
            mode: Mode::Server,
            login_throttle: Arc::new(Throttle::new()),
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
