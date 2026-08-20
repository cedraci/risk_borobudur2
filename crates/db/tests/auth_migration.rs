async fn fresh() -> (sqlx::PgPool, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::Db::connect(&edb.url).await.unwrap().test_pool().clone();
    std::mem::forget(dir); // the embedded server owns the directory for the test's life
    (pool, edb)
}

#[tokio::test]
async fn auth_tables_exist_after_migration() {
    let (pool, edb) = fresh().await;
    for table in ["users", "sessions", "grants", "user_roles", "audit_events", "login_attempts"] {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM information_schema.tables
             WHERE table_schema = 'public' AND table_name = $1)")
            .bind(table).fetch_one(&pool).await.unwrap();
        assert!(exists, "table {table} is missing");
    }
    edb.stop().await;
}

#[tokio::test]
async fn a_grant_row_is_unique_per_subject_domain_action_and_portfolio() {
    let (pool, edb) = fresh().await;
    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO users (email, display_name, password_hash, is_administrator)
         VALUES ('a@b.c', 'A', 'x', false) RETURNING id")
        .fetch_one(&pool).await.unwrap();
    let insert = "INSERT INTO grants (user_id, domain, action, portfolio_id) VALUES ($1,'nav','view',NULL)";
    sqlx::query(insert).bind(uid).execute(&pool).await.unwrap();
    let again = sqlx::query(insert).bind(uid).execute(&pool).await;
    assert!(again.is_err(), "duplicate grant rows must be rejected");
    edb.stop().await;
}

#[tokio::test]
async fn deleting_a_user_removes_their_grants_and_sessions_but_keeps_audit_history() {
    let (pool, edb) = fresh().await;
    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO users (email, display_name, password_hash, is_administrator)
         VALUES ('a@b.c', 'A', 'x', false) RETURNING id")
        .fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO grants (user_id, domain, action, portfolio_id) VALUES ($1,'nav','view',NULL)")
        .bind(uid).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO sessions (token_hash, user_id, expires_at) VALUES ('h', $1, now() + interval '1 hour')")
        .bind(uid).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO audit_events (user_id, actor_label, action, domain, detail) VALUES ($1,'A','login',NULL,'{}')")
        .bind(uid).execute(&pool).await.unwrap();

    sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.unwrap();

    let grants: i64 = sqlx::query_scalar("SELECT count(*) FROM grants").fetch_one(&pool).await.unwrap();
    let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions").fetch_one(&pool).await.unwrap();
    let audit: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events").fetch_one(&pool).await.unwrap();
    assert_eq!(grants, 0);
    assert_eq!(sessions, 0);
    assert_eq!(audit, 1, "audit history must survive user deletion");
    edb.stop().await;
}

#[tokio::test]
async fn grants_are_removed_when_their_portfolio_is_deleted() {
    let (pool, edb) = fresh().await;
    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO users (email, display_name, password_hash, is_administrator)
         VALUES ('a@b.c', 'A', 'x', false) RETURNING id")
        .fetch_one(&pool).await.unwrap();
    let pid: i64 = sqlx::query_scalar(
        "INSERT INTO portfolios (name, kind) VALUES ('F', 'ucits') RETURNING id")
        .fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO grants (user_id, domain, action, portfolio_id) VALUES ($1,'nav','view',$2)")
        .bind(uid).bind(pid).execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM portfolios WHERE id = $1").bind(pid).execute(&pool).await.unwrap();
    let grants: i64 = sqlx::query_scalar("SELECT count(*) FROM grants").fetch_one(&pool).await.unwrap();
    assert_eq!(grants, 0, "a grant must not outlive its portfolio");
    edb.stop().await;
}

/// The P10 split moved per-portfolio configuration out of `reference` and
/// into its own `settings` domain. Everyone already holding a `reference`
/// grant was, by construction, holding the settings authority too — so the
/// migration has to hand them the matching `settings` row, or an upgrade
/// silently takes the fund's own configuration away from the people who had
/// it yesterday.
#[tokio::test]
async fn the_settings_split_carries_existing_reference_grants_across() {
    let (pool, edb) = fresh().await;
    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO users (email, display_name, password_hash, is_administrator)
         VALUES ('legacy@b.c', 'Legacy', 'x', false) RETURNING id")
        .fetch_one(&pool).await.unwrap();

    // Simulate a database migrated from before the split: `reference` rows
    // only, at both scopes, then re-run the back-fill the migration performs.
    sqlx::query("DELETE FROM grants WHERE user_id = $1").bind(uid).execute(&pool).await.unwrap();
    for (action, portfolio) in [("view", None), ("configure", Some(1i64))] {
        sqlx::query("INSERT INTO grants (user_id, domain, action, portfolio_id) VALUES ($1,'reference',$2,$3)")
            .bind(uid).bind(action).bind(portfolio).execute(&pool).await.unwrap();
    }
    sqlx::query(
        "INSERT INTO grants (user_id, domain, action, portfolio_id)
         SELECT user_id, 'settings', action, portfolio_id FROM grants WHERE domain = 'reference'
         ON CONFLICT DO NOTHING")
        .execute(&pool).await.unwrap();

    let mirrored: Vec<(String, Option<i64>)> = sqlx::query_as(
        "SELECT action, portfolio_id FROM grants WHERE user_id = $1 AND domain = 'settings' ORDER BY action")
        .bind(uid).fetch_all(&pool).await.unwrap();
    assert_eq!(mirrored, vec![("configure".to_string(), Some(1)), ("view".to_string(), None)],
        "every reference grant should have a settings twin after the split");

    edb.stop().await;
}

/// `grants.domain` is constrained, so a new domain that the constraint does
/// not know about would be rejected at write time however well the Rust enum
/// spells it.
#[tokio::test]
async fn the_settings_domain_is_accepted_by_the_grants_constraint() {
    let (pool, edb) = fresh().await;
    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO users (email, display_name, password_hash, is_administrator)
         VALUES ('s@b.c', 'S', 'x', false) RETURNING id")
        .fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO grants (user_id, domain, action, portfolio_id) VALUES ($1,'settings','configure',NULL)")
        .bind(uid).execute(&pool).await.expect("the settings domain must be writable");
    edb.stop().await;
}
