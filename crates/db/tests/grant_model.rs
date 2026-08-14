use db::auth::{Action, AuthCtx, Domain, Grant, GrantSet, PortfolioScope};

fn g(d: Domain, a: Action, p: Option<i64>) -> Grant {
    Grant { domain: d, action: a, portfolio: p }
}

#[test]
fn absence_of_a_grant_is_denial() {
    let s = GrantSet::from_grants([]);
    for d in Domain::ALL {
        for a in Action::ALL {
            assert!(!s.allows(d, a, Some(1)), "{d:?}/{a:?} must be denied");
            assert!(!s.allows(d, a, None), "{d:?}/{a:?} global must be denied");
        }
    }
}

#[test]
fn export_import_and_configure_each_imply_view() {
    for implying in [Action::Export, Action::Import, Action::Configure] {
        let s = GrantSet::from_grants([g(Domain::Positions, implying, Some(7))]);
        assert!(s.allows(Domain::Positions, Action::View, Some(7)),
            "{implying:?} must imply view");
        assert!(s.allows(Domain::Positions, implying, Some(7)));
    }
}

#[test]
fn view_implies_nothing_else() {
    let s = GrantSet::from_grants([g(Domain::Positions, Action::View, Some(7))]);
    for a in [Action::Export, Action::Import, Action::Configure] {
        assert!(!s.allows(Domain::Positions, a, Some(7)), "view must not imply {a:?}");
    }
}

#[test]
fn implication_does_not_leak_across_domains_or_portfolios() {
    let s = GrantSet::from_grants([g(Domain::Positions, Action::Export, Some(7))]);
    assert!(!s.allows(Domain::Nav, Action::View, Some(7)));
    assert!(!s.allows(Domain::Positions, Action::View, Some(8)));
}

#[test]
fn wildcard_scope_covers_every_portfolio_and_global_resources() {
    let s = GrantSet::from_grants([g(Domain::Reference, Action::Configure, None)]);
    assert!(s.allows(Domain::Reference, Action::Configure, Some(1)));
    assert!(s.allows(Domain::Reference, Action::Configure, Some(999)));
    assert!(s.allows(Domain::Reference, Action::View, None));
}

#[test]
fn a_portfolio_scoped_grant_never_reaches_a_global_resource() {
    let s = GrantSet::from_grants([g(Domain::Reference, Action::Configure, Some(7))]);
    assert!(s.allows(Domain::Reference, Action::Configure, Some(7)));
    assert!(!s.allows(Domain::Reference, Action::Configure, None),
        "global resources require a wildcard grant");
}

#[test]
fn grants_are_additive_across_rows() {
    let s = GrantSet::from_grants([
        g(Domain::Positions, Action::View, Some(1)),
        g(Domain::Positions, Action::View, Some(2)),
        g(Domain::Nav, Action::Export, Some(1)),
    ]);
    assert!(s.allows(Domain::Positions, Action::View, Some(1)));
    assert!(s.allows(Domain::Positions, Action::View, Some(2)));
    assert!(s.allows(Domain::Nav, Action::View, Some(1)));
    assert!(!s.allows(Domain::Nav, Action::View, Some(2)));
}

#[test]
fn any_domain_on_reports_whether_a_portfolio_is_visible_at_all() {
    let s = GrantSet::from_grants([g(Domain::Nav, Action::View, Some(3))]);
    assert!(s.any_domain_on(3));
    assert!(!s.any_domain_on(4));
    let w = GrantSet::from_grants([g(Domain::Nav, Action::View, None)]);
    assert!(w.any_domain_on(4), "a wildcard grant makes every portfolio visible");
}

#[test]
fn visible_portfolios_distinguishes_wildcard_from_an_explicit_set() {
    let s = GrantSet::from_grants([
        g(Domain::Nav, Action::View, Some(3)),
        g(Domain::Positions, Action::View, Some(5)),
    ]);
    match s.visible_portfolios() {
        PortfolioScope::Only(ids) => assert_eq!(ids, std::collections::BTreeSet::from([3, 5])),
        PortfolioScope::All => panic!("expected an explicit set"),
    }
    let w = GrantSet::from_grants([g(Domain::Nav, Action::View, None)]);
    assert!(matches!(w.visible_portfolios(), PortfolioScope::All));
}

#[test]
fn all_access_permits_every_combination() {
    let s = GrantSet::all_access();
    for d in Domain::ALL {
        for a in Action::ALL {
            assert!(s.allows(d, a, Some(42)));
            assert!(s.allows(d, a, None));
        }
    }
}

#[test]
fn domain_and_action_round_trip_through_their_wire_names() {
    for d in Domain::ALL {
        assert_eq!(Domain::from_str(d.as_str()), Some(d));
    }
    for a in Action::ALL {
        assert_eq!(Action::from_str(a.as_str()), Some(a));
    }
    assert_eq!(Domain::MarketData.as_str(), "market_data");
    assert_eq!(Domain::from_str("nonsense"), None);
}

#[test]
fn the_desktop_principal_holds_everything() {
    let ctx = AuthCtx::desktop();
    assert!(ctx.is_administrator);
    assert!(ctx.grants.allows(Domain::Shareholders, Action::Export, Some(1)));
}
