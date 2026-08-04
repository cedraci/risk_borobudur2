use server::routes::router;
use server::state::AppState;
use server::static_assets;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info,sqlx=warn").init();
    let root = dirs::data_local_dir()
        .ok_or_else(|| anyhow::anyhow!("no local data dir"))?
        .join("borobudur-risk");
    std::fs::create_dir_all(&root)?;
    tracing::info!("starting embedded PostgreSQL under {}", root.display());
    let edb = db::embedded::start(&root, false).await?;
    let pool = db::connect(&edb.url).await?;
    let app = router(AppState { pool });
    if static_assets::assets_empty() {
        tracing::warn!("frontend assets are empty — build the frontend first (see build.ps1)");
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8787").await?;
    tracing::info!("listening on http://127.0.0.1:8787");
    let _ = webbrowser::open("http://127.0.0.1:8787");
    axum::serve(listener, app)
        .with_graceful_shutdown(async { let _ = tokio::signal::ctrl_c().await; })
        .await?;
    edb.stop().await; // keep edb alive until server exits
    Ok(())
}
