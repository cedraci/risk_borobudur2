//! Roles are templates, not indirection. `expand` produces concrete grant rows
//! that are stored against the user; nothing at request time knows a role
//! existed. The cost is that editing a role does not retroactively change
//! people already holding it — the administration screen re-applies instead.

use super::model::{Action, Domain, Grant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    RiskAnalyst,
    HeadOfRisk,
    Operations,
    Auditor,
}

impl Role {
    pub const ALL: [Role; 4] = [Role::RiskAnalyst, Role::HeadOfRisk, Role::Operations, Role::Auditor];

    pub fn as_str(&self) -> &'static str {
        match self {
            Role::RiskAnalyst => "risk_analyst",
            Role::HeadOfRisk => "head_of_risk",
            Role::Operations => "operations",
            Role::Auditor => "auditor",
        }
    }

    /// See `Domain::from_str` (`auth/model.rs`) for why this is not `FromStr`.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.as_str() == s)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Role::RiskAnalyst => "Risk Analyst",
            Role::HeadOfRisk => "Head of Risk",
            Role::Operations => "Operations",
            Role::Auditor => "Auditor",
        }
    }

    pub fn expand(&self, scope: Option<i64>) -> Vec<Grant> {
        let mk = |domain: Domain, action: Action| Grant { domain, action, portfolio: scope };
        match self {
            Role::RiskAnalyst => {
                let domains = [Domain::Positions, Domain::Nav, Domain::Transactions,
                               Domain::MarketData, Domain::Reference];
                domains.into_iter()
                    .flat_map(|d| [mk(d, Action::View), mk(d, Action::Export)])
                    .collect()
            }
            Role::HeadOfRisk => {
                let mut g: Vec<Grant> = Domain::ALL.into_iter()
                    .flat_map(|d| [mk(d, Action::View), mk(d, Action::Export)])
                    .collect();
                g.push(mk(Domain::Reference, Action::Configure));
                g
            }
            Role::Operations => {
                let domains = [Domain::Positions, Domain::Nav, Domain::Transactions, Domain::MarketData];
                let mut g: Vec<Grant> = domains.into_iter().map(|d| mk(d, Action::Import)).collect();
                g.push(mk(Domain::Reference, Action::View));
                g
            }
            Role::Auditor => Domain::ALL.into_iter().map(|d| mk(d, Action::View)).collect(),
        }
    }
}
