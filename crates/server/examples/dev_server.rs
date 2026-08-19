//! A throwaway SERVER-MODE instance on an embedded PostgreSQL, for exercising
//! accounts and access rights locally without provisioning a real Postgres.
//!
//! Desktop mode (`cargo run -p server`) has no login and a single
//! all-powerful principal, so none of the authorization surface is reachable
//! from it. This example stands up the same router in `Mode::Server` on a
//! `temporary = true` embedded database (throwaway dirs, random port —
//! nothing touches the desktop installation under %LOCALAPPDATA%), enrols
//! nobody, and prints the first administrator's single-use enrolment token
//! so a test run can bootstrap itself.
//!
//!     cargo run -p server --example dev_server
//!
//! Environment:
//! - `BOROBUDUR_DEV_BIND`          bind address (default 127.0.0.1:8788)
//! - `BOROBUDUR_ADMIN_EMAIL`       first administrator (default admin@dev.local)
//! - `BOROBUDUR_DEV_SHUTDOWN_SECS` auto-stop after N seconds (for unattended
//!   runs; default: run until Ctrl-C)
//!
//! Everything is discarded on shutdown. Note the server-mode session cookie
//! carries `Secure`; browsers treat 127.0.0.1 as a trustworthy origin, so
//! signing in to the served UI over plain http on this loopback bind still
//! works in Chrome/Edge/Firefox.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info,sqlx=warn").init();
    let bind = std::env::var("BOROBUDUR_DEV_BIND")
        .unwrap_or_else(|_| "127.0.0.1:8788".to_string());
    let admin_email = std::env::var("BOROBUDUR_ADMIN_EMAIL")
        .unwrap_or_else(|_| "admin@dev.local".to_string());

    let edb = db::embedded::start(&std::env::temp_dir(), true).await?;
    let dbh = db::Db::connect(&edb.url).await?;

    let token = server::startup::ensure_first_administrator(&dbh, &admin_email)
        .await?
        .expect("a temporary embedded database always starts with zero users");

    let app = server::routes::router(server::state::AppState::server(dbh));
    let listener = tokio::net::TcpListener::bind(&bind).await?;

    println!("dev server up (server mode, throwaway embedded PostgreSQL)");
    println!("  url:             http://{bind}");
    println!("  admin email:     {admin_email}");
    println!("  enrolment token: {token}");
    println!("  bootstrap + test suite:");
    println!("    pwsh scripts/test-access-rights.ps1 -BaseUrl http://{bind} \\");
    println!("      -AdminEmail {admin_email} -AdminPassword <choose one> -EnrolToken {token}");
    println!("  stop with Ctrl-C (the embedded database is discarded)");

    let ttl: Option<u64> = std::env::var("BOROBUDUR_DEV_SHUTDOWN_SECS")
        .ok()
        .and_then(|v| v.parse().ok());
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            match ttl {
                Some(secs) => {
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(secs),
                        tokio::signal::ctrl_c(),
                    )
                    .await;
                }
                None => {
                    let _ = tokio::signal::ctrl_c().await;
                }
            }
        })
        .await?;
    edb.stop().await;
    Ok(())
}
