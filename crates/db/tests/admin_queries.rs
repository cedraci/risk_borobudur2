use db::admin::{Admin, AuditEvent};
use db::auth::{Action, Domain, Grant, Role};

async fn fresh() -> (sqlx::PgPool, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::Db::connect(&edb.url).await.unwrap().test_pool().clone();
    std::mem::forget(dir);
    (pool, edb)
}

#[tokio::test]
async fn users_round_trip() {
    let (pool, edb) = fresh().await;
    let a = Admin::new(&pool);
    assert_eq!(a.user_count().await.unwrap(), 0);
    let id = a.create_user("r@f.lu", "Risk", "hash", false).await.unwrap();
    let u = a.user_by_email("r@f.lu").await.unwrap().expect("user");
    assert_eq!(u.id, id);
    assert_eq!(u.display_name, "Risk");
    assert!(!u.is_administrator);
    assert!(a.user_by_email("R@F.LU").await.unwrap().is_some(), "email lookup is case-insensitive");
    assert!(a.user_by_email("nobody@f.lu").await.unwrap().is_none());
    assert_eq!(a.user_count().await.unwrap(), 1);
    edb.stop().await;
}

#[tokio::test]
async fn grants_load_as_a_resolved_grant_set() {
    let (pool, edb) = fresh().await;
    let a = Admin::new(&pool);
    let id = a.create_user("r@f.lu", "Risk", "hash", false).await.unwrap();
    let pid: i64 = sqlx::query_scalar("INSERT INTO portfolios (name, kind) VALUES ('F','ucits') RETURNING id")
        .fetch_one(&pool).await.unwrap();
    a.grant_add(id, Grant { domain: Domain::Positions, action: Action::Export, portfolio: Some(pid) }, None)
        .await.unwrap();

    let set = a.grants_for(id).await.unwrap();
    assert!(set.allows(Domain::Positions, Action::Export, Some(pid)));
    assert!(set.allows(Domain::Positions, Action::View, Some(pid)), "implication survives the round trip");
    assert!(!set.allows(Domain::Nav, Action::View, Some(pid)));
    edb.stop().await;
}

#[tokio::test]
async fn adding_the_same_grant_twice_is_idempotent() {
    let (pool, edb) = fresh().await;
    let a = Admin::new(&pool);
    let id = a.create_user("r@f.lu", "Risk", "hash", false).await.unwrap();
    let g = Grant { domain: Domain::Nav, action: Action::View, portfolio: None };
    a.grant_add(id, g, None).await.unwrap();
    a.grant_add(id, g, None).await.unwrap();
    assert_eq!(a.grant_rows_for(id).await.unwrap().len(), 1);
    a.grant_remove(id, g).await.unwrap();
    assert!(a.grant_rows_for(id).await.unwrap().is_empty());
    edb.stop().await;
}

#[tokio::test]
async fn assigning_a_role_writes_its_expanded_grants() {
    let (pool, edb) = fresh().await;
    let a = Admin::new(&pool);
    let id = a.create_user("r@f.lu", "Risk", "hash", false).await.unwrap();
    a.role_assign(id, Role::Auditor, None, None).await.unwrap();
    let set = a.grants_for(id).await.unwrap();
    for d in Domain::ALL {
        assert!(set.allows(d, Action::View, Some(1)));
        assert!(!set.allows(d, Action::Export, Some(1)));
    }
    edb.stop().await;
}

#[tokio::test]
async fn sessions_resolve_and_expire() {
    let (pool, edb) = fresh().await;
    let a = Admin::new(&pool);
    let id = a.create_user("r@f.lu", "Risk", "hash", false).await.unwrap();
    a.session_create("hash-1", id, 8).await.unwrap();
    assert_eq!(a.session_user("hash-1").await.unwrap().unwrap().id, id);

    sqlx::query("UPDATE sessions SET expires_at = now() - interval '1 minute'")
        .execute(&pool).await.unwrap();
    assert!(a.session_user("hash-1").await.unwrap().is_none(), "expired sessions resolve to nobody");

    a.session_create("hash-2", id, 8).await.unwrap();
    a.sessions_delete_for(id).await.unwrap();
    assert!(a.session_user("hash-2").await.unwrap().is_none(), "revocation is immediate");
    edb.stop().await;
}

#[tokio::test]
async fn a_disabled_user_never_resolves_from_a_session() {
    let (pool, edb) = fresh().await;
    let a = Admin::new(&pool);
    let id = a.create_user("r@f.lu", "Risk", "hash", false).await.unwrap();
    a.session_create("hash-1", id, 8).await.unwrap();
    a.set_disabled(id, true).await.unwrap();
    assert!(a.session_user("hash-1").await.unwrap().is_none());
    edb.stop().await;
}

#[tokio::test]
async fn login_failures_accumulate_and_lock_then_reset() {
    let (pool, edb) = fresh().await;
    let a = Admin::new(&pool);
    for n in 1..=4 {
        let st = a.login_record_failure("r@f.lu", 5, 900).await.unwrap();
        assert_eq!(st.failures, n);
        assert!(!st.locked, "must not lock before the fifth failure");
    }
    let st = a.login_record_failure("r@f.lu", 5, 900).await.unwrap();
    assert!(st.locked, "the fifth failure locks the account");
    assert!(st.retry_after_secs > 0);
    assert!(a.login_state("r@f.lu").await.unwrap().locked);

    a.login_reset("r@f.lu").await.unwrap();
    let st = a.login_state("r@f.lu").await.unwrap();
    assert!(!st.locked);
    assert_eq!(st.failures, 0);
    edb.stop().await;
}

#[tokio::test]
async fn audit_events_append_and_read_back_newest_first() {
    let (pool, edb) = fresh().await;
    let a = Admin::new(&pool);
    let id = a.create_user("r@f.lu", "Risk", "hash", false).await.unwrap();
    for action in ["login", "export"] {
        a.audit_append(AuditEvent {
            user_id: Some(id),
            actor_label: "Risk".into(),
            action: action.into(),
            domain: Some(Domain::Shareholders),
            portfolio_id: Some(1),
            detail: serde_json::json!({"route": "/x"}),
            source_addr: Some("10.0.0.1".into()),
        }).await.unwrap();
    }
    let rows = a.audit_recent(10).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].action, "export", "newest first");
    assert_eq!(rows[0].domain.as_deref(), Some("shareholders"));
    edb.stop().await;
}
