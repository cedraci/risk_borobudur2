//! The capability token. `Access<D, A>` cannot be constructed outside
//! `Scoped::authorize`, so a repository method that demands one is a method
//! whose permission check provably ran. The marker types make the domain and
//! action part of the type, so a NAV authorization cannot be handed to a
//! positions query.

use super::model::{Action, Domain};
use std::marker::PhantomData;

pub trait DomainMarker {
    const DOMAIN: Domain;
}

pub trait ActionMarker {
    const ACTION: Action;
}

macro_rules! domain_marker {
    ($name:ident, $variant:ident) => {
        #[derive(Debug, Clone, Copy)]
        pub struct $name;
        impl DomainMarker for $name {
            const DOMAIN: Domain = Domain::$variant;
        }
    };
}

macro_rules! action_marker {
    ($name:ident, $variant:ident) => {
        #[derive(Debug, Clone, Copy)]
        pub struct $name;
        impl ActionMarker for $name {
            const ACTION: Action = Action::$variant;
        }
    };
}

pub mod marker {
    use super::{ActionMarker, DomainMarker};
    use crate::auth::model::{Action, Domain};

    domain_marker!(Positions, Positions);
    domain_marker!(Nav, Nav);
    domain_marker!(Transactions, Transactions);
    domain_marker!(Shareholders, Shareholders);
    domain_marker!(MarketData, MarketData);
    domain_marker!(Reference, Reference);

    action_marker!(View, View);
    action_marker!(Export, Export);
    action_marker!(Import, Import);
    action_marker!(Configure, Configure);
}

/// Proof that `(D, A)` was authorized for this portfolio.
#[derive(Debug)]
pub struct Access<D: DomainMarker, A: ActionMarker> {
    portfolio_id: i64,
    _d: PhantomData<D>,
    _a: PhantomData<A>,
}

impl<D: DomainMarker, A: ActionMarker> Access<D, A> {
    pub(crate) fn new(portfolio_id: i64) -> Self {
        Access { portfolio_id, _d: PhantomData, _a: PhantomData }
    }

    pub fn portfolio_id(&self) -> i64 {
        self.portfolio_id
    }
}

/// Proof that `(D, A)` was authorized instance-wide.
#[derive(Debug)]
pub struct GlobalAccess<D: DomainMarker, A: ActionMarker> {
    _d: PhantomData<D>,
    _a: PhantomData<A>,
}

impl<D: DomainMarker, A: ActionMarker> GlobalAccess<D, A> {
    pub(crate) fn new() -> Self {
        GlobalAccess { _d: PhantomData, _a: PhantomData }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeniedKind {
    /// The principal holds no grant of any domain on this portfolio, so the
    /// portfolio is not theirs to know about — the caller renders 404.
    OutOfScope,
    /// The portfolio is visible, this domain or action is not — 403.
    NotGranted,
}

#[derive(Debug, Clone)]
pub struct Denied {
    pub domain: Domain,
    pub action: Action,
    pub portfolio: Option<i64>,
    pub kind: DeniedKind,
}

impl Denied {
    /// The phrasing that travels in an `unavailable` component. It must stay
    /// distinguishable from a missing-data reason such as
    /// "no shareholder register".
    pub fn reason(&self) -> String {
        format!("not permitted: {}", self.domain.label())
    }
}

impl std::fmt::Display for Denied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({:?} on {:?})", self.reason(), self.action, self.portfolio)
    }
}

// DELIBERATELY NOT `impl std::error::Error for Denied`.
//
// `anyhow::Error` absorbs anything implementing `StdError + Send + Sync`, and
// the server's `AppError` converts from `anyhow::Error`. If `Denied` were a
// std error, a `?` in a handler would quietly turn a 403 into a 500 and the
// permission model would look like a bug report instead of a denial. Keeping
// `Denied` outside the std error hierarchy makes that conversion impossible.
