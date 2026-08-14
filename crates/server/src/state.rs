#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::PgPool,
}

impl AppState {
    /// Desktop mode: one principal holding every grant. Task 8 gives this a
    /// real identity provider; until then it is a constructor over the pool so
    /// callers stop using struct-literal syntax.
    pub fn desktop(pool: sqlx::PgPool) -> Self {
        AppState { pool }
    }
}
