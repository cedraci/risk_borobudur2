use db::auth::{Action, Domain, GrantSet, Role};

fn set_for(role: Role, scope: Option<i64>) -> GrantSet {
    GrantSet::from_grants(role.expand(scope))
}

#[test]
fn risk_analyst_reads_and_exports_but_never_sees_shareholders() {
    let s = set_for(Role::RiskAnalyst, Some(7));
    for d in [Domain::Positions, Domain::Nav, Domain::Transactions, Domain::MarketData, Domain::Reference] {
        assert!(s.allows(d, Action::View, Some(7)), "{d:?} view");
        assert!(s.allows(d, Action::Export, Some(7)), "{d:?} export");
        assert!(!s.allows(d, Action::Configure, Some(7)), "{d:?} must not configure");
        assert!(!s.allows(d, Action::Import, Some(7)), "{d:?} must not import");
    }
    assert!(!s.allows(Domain::Shareholders, Action::View, Some(7)));
}

#[test]
fn head_of_risk_configures_reference_only() {
    let s = set_for(Role::HeadOfRisk, Some(7));
    assert!(s.allows(Domain::Shareholders, Action::View, Some(7)));
    assert!(s.allows(Domain::Shareholders, Action::Export, Some(7)));
    assert!(s.allows(Domain::Reference, Action::Configure, Some(7)));
    for d in Domain::ALL.into_iter().filter(|d| *d != Domain::Reference) {
        assert!(!s.allows(d, Action::Configure, Some(7)),
            "configure is only meaningful on reference, not {d:?}");
    }
}

#[test]
fn operations_imports_but_cannot_export() {
    let s = set_for(Role::Operations, Some(7));
    for d in [Domain::Positions, Domain::Nav, Domain::Transactions, Domain::MarketData] {
        assert!(s.allows(d, Action::Import, Some(7)), "{d:?} import");
        assert!(s.allows(d, Action::View, Some(7)), "import implies view");
        assert!(!s.allows(d, Action::Export, Some(7)), "{d:?} must not export");
    }
    assert!(!s.allows(Domain::Shareholders, Action::Import, Some(7)));
}

#[test]
fn auditor_sees_everything_and_takes_nothing_out() {
    let s = set_for(Role::Auditor, None);
    for d in Domain::ALL {
        assert!(s.allows(d, Action::View, Some(123)), "{d:?} view anywhere");
        assert!(!s.allows(d, Action::Export, Some(123)), "{d:?} must not export");
        assert!(!s.allows(d, Action::Import, Some(123)));
        assert!(!s.allows(d, Action::Configure, Some(123)));
    }
}

#[test]
fn expansion_carries_the_requested_scope() {
    assert!(Role::RiskAnalyst.expand(Some(9)).iter().all(|g| g.portfolio == Some(9)));
    assert!(Role::RiskAnalyst.expand(None).iter().all(|g| g.portfolio.is_none()));
}

#[test]
fn roles_round_trip_through_their_wire_names() {
    for r in Role::ALL {
        assert_eq!(Role::from_str(r.as_str()), Some(r));
    }
    assert_eq!(Role::from_str("nonsense"), None);
}
