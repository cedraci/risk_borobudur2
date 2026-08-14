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

    let pool = db::connect(&url).await?;
    let app = router(AppState::desktop(pool));
    if static_assets::assets_empty() {
        tracing::warn!("frontend assets are empty — build the frontend first (see build.ps1)");
    }
    let listener = tokio::net::TcpListener::bind(&cfg.bind).await?;
    tracing::info!("listening on http://{}", cfg.bind);
    if cfg.open_browser {
        let _ = webbrowser::open(&format!("http://{}", cfg.bind));
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(async { let _ = tokio::signal::ctrl_c().await; })
        .await?;
    if let Some(edb) = embedded {
        edb.stop().await;
    }
    Ok(())
}
