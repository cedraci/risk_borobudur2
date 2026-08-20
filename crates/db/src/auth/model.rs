//! The grant model. Pure — no sqlx, no I/O — so it can be tested exhaustively
//! without a database.

use std::collections::{BTreeSet, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Domain {
    Positions,
    Nav,
    Transactions,
    Shareholders,
    MarketData,
    /// Instrument data shared by every portfolio: classifications, issuer
    /// groups, liquidity overrides, bond statics, futures contract specs, FX
    /// rates. Fleet-wide by nature — a grant on it is a grant over data that
    /// belongs to no single fund.
    Reference,
    /// One portfolio's own configuration: its VaR limit and window, its
    /// redemption stress and liquidity parameters, its depositary code
    /// mapping, its monthly EMIR KPI records, its import ledger. Split out of
    /// `Reference` (finding P10) because the two were never the same
    /// authority: curating the shared instrument tables is an instance-wide
    /// act, and moving a fund's VaR limit is emphatically not.
    Settings,
}

impl Domain {
    pub const ALL: [Domain; 7] = [
        Domain::Positions, Domain::Nav, Domain::Transactions,
        Domain::Shareholders, Domain::MarketData, Domain::Reference,
        Domain::Settings,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Domain::Positions => "positions",
            Domain::Nav => "nav",
            Domain::Transactions => "transactions",
            Domain::Shareholders => "shareholders",
            Domain::MarketData => "market_data",
            Domain::Reference => "reference",
            Domain::Settings => "settings",
        }
    }

    // Named `from_str` (not `FromStr::from_str`) throughout this module:
    // there is no `Err` to report for an unrecognized wire value, only
    // `None`, so implementing the trait would mean inventing an error type
    // with nothing to say. Every call site already matches on `Option`.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Domain> {
        Domain::ALL.into_iter().find(|d| d.as_str() == s)
    }

    /// Human phrasing used in denial reasons: "not permitted: {label}".
    pub fn label(&self) -> &'static str {
        match self {
            Domain::Positions => "positions",
            Domain::Nav => "NAV history",
            Domain::Transactions => "transactions",
            Domain::Shareholders => "shareholder register",
            Domain::MarketData => "market data",
            Domain::Reference => "reference data",
            Domain::Settings => "portfolio settings",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    View,
    Export,
    Import,
    Configure,
}

impl Action {
    pub const ALL: [Action; 4] = [Action::View, Action::Export, Action::Import, Action::Configure];

    pub fn as_str(&self) -> &'static str {
        match self {
            Action::View => "view",
            Action::Export => "export",
            Action::Import => "import",
            Action::Configure => "configure",
        }
    }

    /// See `Domain::from_str` above for why this is not `FromStr`.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|a| a.as_str() == s)
    }
}

/// One stored permission row. `portfolio: None` means every portfolio, and is
/// also the only thing that reaches instance-wide resources.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Grant {
    pub domain: Domain,
    pub action: Action,
    pub portfolio: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortfolioScope {
    All,
    Only(BTreeSet<i64>),
}

/// A principal's resolved permissions. The `view` implication is expanded here,
/// once, so `allows` is a set lookup and cannot drift between call sites.
#[derive(Clone, Debug, Default)]
pub struct GrantSet {
    entries: HashSet<Grant>,
    wildcard_any: bool,
}

impl GrantSet {
    pub fn from_grants(grants: impl IntoIterator<Item = Grant>) -> Self {
        let mut entries = HashSet::new();
        let mut wildcard_any = false;
        for g in grants {
            if g.portfolio.is_none() {
                wildcard_any = true;
            }
            entries.insert(g);
            if g.action != Action::View {
                entries.insert(Grant { action: Action::View, ..g });
            }
        }
        GrantSet { entries, wildcard_any }
    }

    /// `portfolio: None` asks about an instance-wide resource and is answered
    /// only by a wildcard grant. `Some(id)` is answered by either a wildcard
    /// grant or an explicit row for that id.
    pub fn allows(&self, domain: Domain, action: Action, portfolio: Option<i64>) -> bool {
        if self.entries.contains(&Grant { domain, action, portfolio: None }) {
            return true;
        }
        match portfolio {
            None => false,
            Some(id) => self.entries.contains(&Grant { domain, action, portfolio: Some(id) }),
        }
    }

    /// Is this portfolio visible under *any* domain? Used to choose between a
    /// 404 (the portfolio is not this principal's to know about) and a 403.
    pub fn any_domain_on(&self, portfolio_id: i64) -> bool {
        self.wildcard_any
            || self.entries.iter().any(|g| g.portfolio == Some(portfolio_id))
    }

    pub fn visible_portfolios(&self) -> PortfolioScope {
        if self.wildcard_any {
            return PortfolioScope::All;
        }
        PortfolioScope::Only(self.entries.iter().filter_map(|g| g.portfolio).collect())
    }

    pub fn all_access() -> Self {
        let mut grants = Vec::new();
        for domain in Domain::ALL {
            for action in Action::ALL {
                grants.push(Grant { domain, action, portfolio: None });
            }
        }
        GrantSet::from_grants(grants)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Grant> {
        self.entries.iter()
    }
}

/// Everything a request knows about who is making it. Constructed once by the
/// middleware; the only key that opens `Scoped`.
#[derive(Clone, Debug)]
pub struct AuthCtx {
    pub principal_id: i64,
    pub display_name: String,
    pub is_administrator: bool,
    pub grants: GrantSet,
    /// Where the request came from, as the front-end proxy reported it.
    /// Carried here rather than passed separately because every audit row is
    /// written from a handler that already holds an `AuthCtx` and nothing
    /// else about the request. `None` in desktop mode, and for any caller
    /// the deployment could not attribute an address to.
    pub source_addr: Option<String>,
}

impl AuthCtx {
    /// The desktop principal. Not a bypass — a configured identity that happens
    /// to hold every grant, travelling the same code path as everyone else.
    pub fn desktop() -> Self {
        AuthCtx {
            principal_id: 0,
            display_name: "desktop".to_string(),
            is_administrator: true,
            grants: GrantSet::all_access(),
            source_addr: None,
        }
    }

    /// The context a *recorded* computation runs under.
    ///
    /// The breach register is the fund's compliance record, not a transcript
    /// of what one user could see: a run computed on fallback issuer groups
    /// because the importer lacked reference access would be a denial
    /// rendering as data, persisted. Constructed only by
    /// `server::recorder`; it never reaches a request handler, and it writes
    /// nothing a principal with full grants could not have computed.
    pub fn system() -> Self {
        AuthCtx {
            principal_id: 0,
            display_name: "system".to_string(),
            is_administrator: false,
            grants: GrantSet::all_access(),
            source_addr: None,
        }
    }
}
