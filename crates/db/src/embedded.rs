use postgresql_embedded::{PostgreSQL, Settings, VersionReq};
use std::path::Path;

pub const DB_NAME: &str = "borobudur";

pub struct EmbeddedDb {
    pg: PostgreSQL,
    pub url: String,
}

/// Start (installing on first run) an embedded PostgreSQL 17.
/// `temporary = true` uses throwaway dirs + random port (tests);
/// `false` persists under `data_root` for the real app.
pub async fn start(data_root: &Path, temporary: bool) -> anyhow::Result<EmbeddedDb> {
    let mut settings = Settings::default();
    settings.version = VersionReq::parse("=17")?;
    settings.temporary = temporary;
    settings.username = "postgres".to_string();
    settings.password = "borobudur-local".to_string();
    if !temporary {
        settings.installation_dir = data_root.join("pg-install");
        settings.data_dir = data_root.join("pg-data");
        settings.password_file = data_root.join(".pgpass");
    }
    let mut pg = PostgreSQL::new(settings);
    pg.setup().await?;
    pg.start().await?;
    if !pg.database_exists(DB_NAME).await? {
        pg.create_database(DB_NAME).await?;
    }
    let url = pg.settings().url(DB_NAME);
    Ok(EmbeddedDb { pg, url })
}

impl EmbeddedDb {
    pub async fn stop(self) {
        let _ = self.pg.stop().await;
    }
}
