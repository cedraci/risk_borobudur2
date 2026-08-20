use server::config::{Mode, ServerConfig};
use server::routes::router;
use server::state::AppState;
use server::static_assets;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info,sqlx=warn").init();
    let cfg = ServerConfig::from_env()?;

    // Held for the process lifetime in desktop mode; `None` in server mode.
    let mut embedded: Option<db::embedded::EmbeddedDb> = None;
    let url = match cfg.mode {
        Mode::Server => cfg.database_url.clone().expect("server mode implies a url"),
        Mode::Desktop => {
            let root = dirs::data_local_dir()
                .ok_or_else(|| anyhow::anyhow!("no local data dir"))?
                .join("borobudur-risk");
            std::fs::create_dir_all(&root)?;
            tracing::info!("starting embedded PostgreSQL under {}", root.display());
            let edb = db::embedded::start(&root, false).await?;
            let url = edb.url.clone();
            embedded = Some(edb);
            url
        }
    };

    let dbh = db::Db::connect(&url).await?;
    let state = match cfg.mode {
        Mode::Server => {
            match &cfg.admin_email {
                Some(email) => {
                    if let Some(token) = server::startup::ensure_first_administrator(&dbh, email).await? {
                        tracing::info!("no users exist yet — issued a single-use enrolment token for {email}");
                        tracing::info!("{token}");
                        tracing::info!(
                            "complete enrolment within 1 hour: POST /api/enrol with {{\"token\": \"<above>\", \"password\": \"<new password>\"}}"
                        );
                    }
                }
                None => {
                    if server::startup::no_users_exist(&dbh).await? {
                        tracing::warn!(
                            "server mode is starting with zero users and no BOROBUDUR_ADMIN_EMAIL set — \
                             there is no way to sign in; set BOROBUDUR_ADMIN_EMAIL and restart to enrol \
                             the first administrator"
                        );
                    }
                }
            }
            // Server mode always sets `Secure` on the session cookie
            // (`handlers/session.rs::login`), which a browser only ever
            // sends back over HTTPS — TLS termination is assumed to sit in
            // front of this process (a reverse proxy, load balancer, etc.).
            // If it doesn't, the cookie is silently dropped and every login
            // looks like it never happened.
            tracing::warn!(
                "server mode sets the Secure cookie flag — plain HTTP access will silently drop the \
                 session cookie unless TLS terminates in front of this process"
            );
            AppState::server(dbh)
        }
        Mode::Desktop => AppState::desktop(dbh),
    };
    let app = router(state);
    if static_assets::assets_empty() {
        tracing::warn!("frontend assets are empty — build the frontend first (see build.ps1)");
    }
    let listener = tokio::net::TcpListener::bind(&cfg.bind).await?;
    tracing::info!("listening on http://{}", cfg.bind);
    if cfg.open_browser {
        let _ = webbrowser::open(&format!("http://{}", cfg.bind));
    }
    // `into_make_service_with_connect_info` is what puts the peer address in
    // the request extensions; `auth::client_addr` prefers the proxy's
    // forwarded header and falls back to it, so a deployment without a proxy
    // still gets a real address into the audit log.
    axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
        .with_graceful_shutdown(async { let _ = tokio::signal::ctrl_c().await; })
        .await?;
    if let Some(edb) = embedded {
        edb.stop().await;
    }
    Ok(())
}
