//! Startup configuration. Read once in `main`, never consulted again — the
//! chosen mode becomes concrete values (a pool, a bind address, an identity
//! provider) so no request path ever branches on "are we desktop or server".

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Embedded PostgreSQL under the user's local data directory, loopback
    /// bind, browser opened, a single all-powerful principal.
    Desktop,
    /// Externally configured PostgreSQL, real accounts, no browser.
    Server,
}

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub mode: Mode,
    pub database_url: Option<String>,
    pub bind: String,
    pub open_browser: bool,
    pub admin_email: Option<String>,
}

pub const DEFAULT_BIND: &str = "127.0.0.1:8787";

/// Blank and whitespace-only values are treated as unset: an operator who
/// writes `BOROBUDUR_DATABASE_URL=` in a systemd unit means "not set", and
/// silently entering server mode with an empty URL would be a confusing failure.
fn get(f: &impl Fn(&str) -> Option<String>, key: &str) -> Option<String> {
    f(key).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

impl ServerConfig {
    pub fn from_vars(f: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let database_url = get(&f, "BOROBUDUR_DATABASE_URL");
        let mode = if database_url.is_some() { Mode::Server } else { Mode::Desktop };
        Ok(ServerConfig {
            bind: get(&f, "BOROBUDUR_BIND").unwrap_or_else(|| DEFAULT_BIND.to_string()),
            open_browser: mode == Mode::Desktop,
            admin_email: match mode {
                Mode::Server => get(&f, "BOROBUDUR_ADMIN_EMAIL"),
                Mode::Desktop => None,
            },
            mode,
            database_url,
        })
    }

    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_vars(|k| std::env::var(k).ok())
    }
}
