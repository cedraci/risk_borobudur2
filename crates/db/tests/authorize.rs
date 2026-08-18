use db::auth::marker as m;
use db::auth::{Action, AuthCtx, DeniedKind, Domain, Grant, GrantSet};

fn ctx(grants: Vec<Grant>) -> AuthCtx {
    AuthCtx {
        principal_id: 1,
        display_name: "t".into(),
        is_administrator: false,
        grants: GrantSet::from_grants(grants),
    }
}

async fn db_handle() -> (db::Db, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let d = db::Db::connect(&edb.url).await.unwrap();
    std::mem::forget(dir);
    (d, edb)
}

#[tokio::test]
async fn authorize_covers_every_domain_action_and_scope_combination() {
    let (d, edb) = db_handle().await;
    for domain in Domain::ALL {
        for action in Action::ALL {
            // granted on the portfolio we ask about
            let c = ctx(vec![Grant { domain, action, portfolio: Some(7) }]);
            assert!(d.scope(&c).allows(domain, action, Some(7)), "{domain:?}/{action:?} in scope");

            // granted, but on a different portfolio -> the portfolio is invisible
            assert_eq!(d.scope(&c).denial(domain, action, 8).map(|x| x.kind),
                       Some(DeniedKind::OutOfScope),
                       "{domain:?}/{action:?} elsewhere must be out of scope");

            // wildcard reaches everything, including instance-wide resources
            let w = ctx(vec![Grant { domain, action, portfolio: None }]);
            assert!(w.grants.allows(domain, action, Some(999)));
            assert!(d.scope(&w).global_denial(domain, action).is_none());

            // no grant at all
            let n = ctx(vec![]);
            assert_eq!(n.grants.allows(domain, action, Some(7)), false);
        }
    }
    edb.stop().await;
}

#[tokio::test]
async fn a_visible_portfolio_with_the_wrong_domain_is_not_granted_rather_than_out_of_scope() {
    let (d, edb) = db_handle().await;
    let c = ctx(vec![Grant { domain: Domain::Nav, action: Action::View, portfolio: Some(7) }]);
    let s = d.scope(&c);
    let denied = s.authorize::<m::Positions, m::View>(7).unwrap_err();
    assert_eq!(denied.kind, DeniedKind::NotGranted);
    assert_eq!(denied.domain, Domain::Positions);
    assert_eq!(denied.reason(), "not permitted: positions");
    edb.stop().await;
}

#[tokio::test]
async fn an_invisible_portfolio_is_out_of_scope_whatever_the_domain() {
    let (d, edb) = db_handle().await;
    let c = ctx(vec![Grant { domain: Domain::Nav, action: Action::View, portfolio: Some(7) }]);
    let s = d.scope(&c);
    let denied = s.authorize::<m::Nav, m::View>(9).unwrap_err();
    assert_eq!(denied.kind, DeniedKind::OutOfScope);
    edb.stop().await;
}

#[tokio::test]
async fn holding_export_yields_a_view_token() {
    let (d, edb) = db_handle().await;
    let c = ctx(vec![Grant { domain: Domain::Positions, action: Action::Export, portfolio: Some(7) }]);
    let s = d.scope(&c);
    assert!(s.authorize::<m::Positions, m::View>(7).is_ok(),
        "the implication is expanded when grants load, so a view token is obtainable");
    assert!(s.authorize::<m::Positions, m::Import>(7).is_err());
    edb.stop().await;
}

#[tokio::test]
async fn a_portfolio_scoped_grant_never_opens_an_instance_wide_resource() {
    let (d, edb) = db_handle().await;
    let c = ctx(vec![Grant { domain: Domain::Reference, action: Action::Configure, portfolio: Some(7) }]);
    assert!(d.scope(&c).authorize_global::<m::Reference, m::Configure>().is_err());
    let w = ctx(vec![Grant { domain: Domain::Reference, action: Action::Configure, portfolio: None }]);
    assert!(d.scope(&w).authorize_global::<m::Reference, m::Configure>().is_ok());
    edb.stop().await;
}

#[tokio::test]
async fn may_answers_without_producing_a_token() {
    let (d, edb) = db_handle().await;
    let c = ctx(vec![Grant { domain: Domain::Nav, action: Action::View, portfolio: Some(7) }]);
    let s = d.scope(&c);
    assert!(s.may::<m::Nav, m::View>(7));
    assert!(!s.may::<m::Shareholders, m::View>(7));
    edb.stop().await;
}
