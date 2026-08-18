//! The only route from a `Db` to a query.

use crate::auth::access::{Access, ActionMarker, Denied, DeniedKind, DomainMarker, GlobalAccess};
use crate::auth::{Action, AuthCtx, Domain};
use sqlx::PgPool;

pub struct Scoped<'a> {
    pub(crate) pool: &'a PgPool,
    pub(crate) ctx: &'a AuthCtx,
}

impl crate::Db {
    /// An `AuthCtx` is required, not optional. There is no unscoped constructor.
    pub fn scope<'a>(&'a self, ctx: &'a AuthCtx) -> Scoped<'a> {
        Scoped { pool: self.pool(), ctx }
    }
}

impl<'a> Scoped<'a> {
    pub fn ctx(&self) -> &AuthCtx {
        self.ctx
    }

    /// The single runtime chokepoint. Every portfolio-scoped query passes
    /// through here, which is why it is table-tested exhaustively.
    pub fn authorize<D: DomainMarker, A: ActionMarker>(
        &self, portfolio_id: i64,
    ) -> Result<Access<D, A>, Denied> {
        match self.denial(D::DOMAIN, A::ACTION, portfolio_id) {
            None => Ok(Access::new(portfolio_id)),
            Some(d) => Err(d),
        }
    }

    pub fn authorize_global<D: DomainMarker, A: ActionMarker>(
        &self,
    ) -> Result<GlobalAccess<D, A>, Denied> {
        match self.global_denial(D::DOMAIN, A::ACTION) {
            None => Ok(GlobalAccess::new()),
            Some(d) => Err(d),
        }
    }

    /// Ask without taking a token — used where a denial degrades a component to
    /// `unavailable` rather than failing the request.
    pub fn may<D: DomainMarker, A: ActionMarker>(&self, portfolio_id: i64) -> bool {
        self.denial(D::DOMAIN, A::ACTION, portfolio_id).is_none()
    }

    pub fn may_global<D: DomainMarker, A: ActionMarker>(&self) -> bool {
        self.global_denial(D::DOMAIN, A::ACTION).is_none()
    }

    pub fn allows(&self, domain: Domain, action: Action, portfolio: Option<i64>) -> bool {
        self.ctx.grants.allows(domain, action, portfolio)
    }

    pub fn denial(&self, domain: Domain, action: Action, portfolio_id: i64) -> Option<Denied> {
        if self.ctx.grants.allows(domain, action, Some(portfolio_id)) {
            return None;
        }
        Some(Denied {
            domain,
            action,
            portfolio: Some(portfolio_id),
            kind: if self.ctx.grants.any_domain_on(portfolio_id) {
                DeniedKind::NotGranted
            } else {
                DeniedKind::OutOfScope
            },
        })
    }

    pub fn global_denial(&self, domain: Domain, action: Action) -> Option<Denied> {
        if self.ctx.grants.allows(domain, action, None) {
            return None;
        }
        Some(Denied { domain, action, portfolio: None, kind: DeniedKind::NotGranted })
    }
}
