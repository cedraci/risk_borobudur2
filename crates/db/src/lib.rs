pub mod admin;
pub mod auth;
pub mod embedded;
pub mod repo;
pub mod settings;

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

pub async fn connect(url: &str) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new().max_connections(5).connect(url).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}

/// Owns the connection pool. The pool is private: from Task 10 onward the only
/// routes out of this type are `scope` (which demands an `AuthCtx`) and `admin`
/// (the declared privileged path).
#[derive(Clone)]
pub struct Db {
    pool: PgPool,
}

impl Db {
    pub async fn connect(url: &str) -> anyhow::Result<Db> {
        Ok(Db { pool: connect(url).await? })
    }

    pub fn from_pool(pool: PgPool) -> Db {
        Db { pool }
    }

    pub fn admin(&self) -> crate::admin::Admin<'_> {
        crate::admin::Admin::new(self.pool())
    }

    pub(crate) fn pool(&self) -> &PgPool {
        &self.pool
    }
}
