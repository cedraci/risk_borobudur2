use crate::auth::{desktop::DesktopSingleUser, local::LocalAccounts, IdentityProvider};
use crate::config::Mode;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<db::Db>,
    pub identity: Arc<dyn IdentityProvider>,
    pub mode: Mode,
}

impl AppState {
    pub fn desktop(db: db::Db) -> Self {
        AppState {
            db: Arc::new(db),
            identity: Arc::new(DesktopSingleUser),
            mode: Mode::Desktop,
        }
    }

    pub fn server(db: db::Db) -> Self {
        let db = Arc::new(db);
        AppState {
            identity: Arc::new(LocalAccounts::new(db.clone())),
            db,
            mode: Mode::Server,
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
