# Server Deployment, Identity and Authorization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn Borobudur from a single-user desktop app with no concept of identity into a self-hostable server where every query is scoped to an authenticated principal's grants, enforced by the type system rather than by discipline.

**Architecture:** A startup mode selects between today's embedded PostgreSQL (`desktop`) and a configured connection string (`server`). An `IdentityProvider` trait resolves each request to a `Principal`; `DesktopSingleUser` returns a principal holding every grant, so desktop mode is a configured identity rather than a bypass. The `db` crate stops exporting `PgPool`: all repository functions become methods on a `Scoped` handle obtainable only from an `AuthCtx`, and each takes an `Access<Domain, Action>` token that can only be produced by `authorize`. Denials degrade whole result components to `status: "unavailable"` rather than trimming inputs to a computation.

**Tech Stack:** Rust 2024 edition, axum 0.8, sqlx 0.8 (runtime queries — no compile-time macros, no `DATABASE_URL` needed at build time), PostgreSQL, argon2 for password hashing, trybuild for compile-fail tests, React + TypeScript + Vite frontend.

**Spec:** `docs/superpowers/specs/2026-08-14-authorization-and-deployment-design.md`

## Global Constraints

- **Desktop parity is the acceptance bar.** Every existing test in `crates/db/tests/` and `crates/server/tests/` must pass unchanged in behaviour under `DesktopSingleUser`. Changing an assertion to accommodate the refactor is a defect, not a fix.
- **`analytics` must not be modified.** It depends only on `chrono` and `serde`. If a task appears to need a change there, the design is wrong — stop and report.
- **Never run `cargo test --workspace`.** It has previously run for 15 hours without terminating. Run per-crate, per-target, with an explicit timeout: `timeout 600 cargo test -p db --test <name>`.
- **Six domains, fixed:** `positions`, `nav`, `transactions`, `shareholders`, `market_data`, `reference`. Four actions, fixed: `view`, `export`, `import`, `configure`.
- **`view` is implied by `export`, `import` and `configure`.** The implication is expanded once when a `GrantSet` is built, never at check time.
- **Grants are additive only.** There are no deny rules. Absence of a grant is denial.
- **Anything not portfolio-scoped requires a grant whose scope is all portfolios** (`portfolio_id IS NULL`).
- **A computation whose inputs are partially denied returns `unavailable`.** It never computes on a subset. This is the safety rule; a task that computes a limit result over a filtered position set is a defect.
- **Missing data and denied permission are different reasons in the same envelope.** `"no shareholder register"` and `"not permitted: shareholder register"` must never surface as one another.
- **No new client data files are ever committed.** The repository has untracked `.csv`/`.xlsx` sample files; `git add -A` is forbidden. Stage named paths only.
- **Commit message trailer:** every commit ends with `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.

## File Structure

**New in `crates/db`:**

| File | Responsibility |
|---|---|
| `src/auth/mod.rs` | Re-exports; module root |
| `src/auth/model.rs` | `Domain`, `Action`, `Grant`, `GrantSet`, `PortfolioScope`, `AuthCtx` — pure, no sqlx |
| `src/auth/roles.rs` | `Role` and its expansion into grants — pure |
| `src/auth/access.rs` | Domain/action marker types, `Access<D,A>`, `GlobalAccess<D,A>`, `Denied` |
| `src/scoped.rs` | `Db`, `Scoped`, `authorize`, `authorize_global`, `may` |
| `src/admin.rs` | The single privileged path: users, sessions, grants, audit, login attempts |
| `src/repo/mod.rs` + six domain modules | `repo.rs` (1,285 lines) split by domain |
| `migrations/0012_auth.sql` | New tables only |

**New in `crates/server`:**

| File | Responsibility |
|---|---|
| `src/config.rs` | `ServerConfig` — startup mode, bind address, database URL |
| `src/auth/mod.rs` | `Principal`, `IdentityProvider`, `AuthError` |
| `src/auth/local.rs` | `LocalAccounts` — password verification, sessions, throttling |
| `src/auth/desktop.rs` | `DesktopSingleUser` |
| `src/auth/middleware.rs` | Principal resolution, `AuthCtx` construction, per-route requirement enforcement |
| `src/routes/protect.rs` | `.protected()` / `.public()` router constructors |
| `src/handlers/session.rs` | `POST /api/login`, `POST /api/logout`, `GET /api/me` |
| `src/handlers/admin.rs` | User, role and grant administration; audit log read |

**New in `frontend/src`:** `auth.ts`, `pages/LoginPage.tsx`, `components/Unavailable.tsx`, `pages/AdminPage.tsx`.

---

### Task 1: Startup configuration

**Files:**
- Create: `crates/server/src/config.rs`
- Modify: `crates/server/src/lib.rs`, `crates/server/src/main.rs`
- Test: `crates/server/tests/config.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `server::config::{ServerConfig, Mode}`; `ServerConfig::from_vars(f: impl Fn(&str) -> Option<String>) -> anyhow::Result<ServerConfig>`; fields `mode: Mode`, `database_url: Option<String>`, `bind: String`, `open_browser: bool`, `admin_email: Option<String>`. `Mode` is `Mode::Desktop | Mode::Server`.

- [ ] **Step 1: Write the failing test**

Create `crates/server/tests/config.rs`:

```rust
use server::config::{Mode, ServerConfig};
use std::collections::HashMap;

fn cfg(pairs: &[(&str, &str)]) -> anyhow::Result<ServerConfig> {
    let map: HashMap<String, String> =
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    ServerConfig::from_vars(|k| map.get(k).cloned())
}

#[test]
fn defaults_to_desktop_mode() {
    let c = cfg(&[]).unwrap();
    assert_eq!(c.mode, Mode::Desktop);
    assert_eq!(c.database_url, None);
    assert_eq!(c.bind, "127.0.0.1:8787");
    assert!(c.open_browser);
}

#[test]
fn database_url_selects_server_mode() {
    let c = cfg(&[("BOROBUDUR_DATABASE_URL", "postgres://u@h/db")]).unwrap();
    assert_eq!(c.mode, Mode::Server);
    assert_eq!(c.database_url.as_deref(), Some("postgres://u@h/db"));
    assert_eq!(c.bind, "127.0.0.1:8787", "bind default is unchanged by mode");
    assert!(!c.open_browser, "server mode never opens a browser");
}

#[test]
fn bind_is_overridable_in_both_modes() {
    assert_eq!(cfg(&[("BOROBUDUR_BIND", "0.0.0.0:9000")]).unwrap().bind, "0.0.0.0:9000");
    let c = cfg(&[
        ("BOROBUDUR_DATABASE_URL", "postgres://u@h/db"),
        ("BOROBUDUR_BIND", "0.0.0.0:9000"),
    ]).unwrap();
    assert_eq!(c.bind, "0.0.0.0:9000");
}

#[test]
fn blank_values_are_treated_as_unset() {
    let c = cfg(&[("BOROBUDUR_DATABASE_URL", "   "), ("BOROBUDUR_BIND", "")]).unwrap();
    assert_eq!(c.mode, Mode::Desktop);
    assert_eq!(c.bind, "127.0.0.1:8787");
}

#[test]
fn admin_email_is_read_only_in_server_mode() {
    let c = cfg(&[("BOROBUDUR_ADMIN_EMAIL", "risk@firm.lu")]).unwrap();
    assert_eq!(c.admin_email, None, "desktop mode never enrols anyone");
    let c = cfg(&[
        ("BOROBUDUR_DATABASE_URL", "postgres://u@h/db"),
        ("BOROBUDUR_ADMIN_EMAIL", "risk@firm.lu"),
    ]).unwrap();
    assert_eq!(c.admin_email.as_deref(), Some("risk@firm.lu"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 600 cargo test -p server --test config`
Expected: FAIL — `unresolved import server::config`.

- [ ] **Step 3: Write the implementation**

Create `crates/server/src/config.rs`:

```rust
//! Startup configuration. Read once in `main`, never consulted again — the
//! chosen mode becomes concrete values (a pool, a bind address, an identity
//! provider) so no request path ever branches on "are we desktop or server".

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Embedded PostgreSQL under the user's local data directory, loopback
    /// bind, browser opened, a single all-powerful principal.
    Desktop,
    /// Externally configured PostgreSQL, real accounts, no browser.
    Server,
}

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub mode: Mode,
    pub database_url: Option<String>,
    pub bind: String,
    pub open_browser: bool,
    pub admin_email: Option<String>,
}

pub const DEFAULT_BIND: &str = "127.0.0.1:8787";

/// Blank and whitespace-only values are treated as unset: an operator who
/// writes `BOROBUDUR_DATABASE_URL=` in a systemd unit means "not set", and
/// silently entering server mode with an empty URL would be a confusing failure.
fn get(f: &impl Fn(&str) -> Option<String>, key: &str) -> Option<String> {
    f(key).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

impl ServerConfig {
    pub fn from_vars(f: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let database_url = get(&f, "BOROBUDUR_DATABASE_URL");
        let mode = if database_url.is_some() { Mode::Server } else { Mode::Desktop };
        Ok(ServerConfig {
            bind: get(&f, "BOROBUDUR_BIND").unwrap_or_else(|| DEFAULT_BIND.to_string()),
            open_browser: mode == Mode::Desktop,
            admin_email: match mode {
                Mode::Server => get(&f, "BOROBUDUR_ADMIN_EMAIL"),
                Mode::Desktop => None,
            },
            mode,
            database_url,
        })
    }

    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_vars(|k| std::env::var(k).ok())
    }
}
```

Add `pub mod config;` to `crates/server/src/lib.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `timeout 600 cargo test -p server --test config`
Expected: PASS, 5 tests.

- [ ] **Step 5: Rewrite `main.rs` to use it**

Replace the body of `crates/server/src/main.rs`:

```rust
use server::config::{Mode, ServerConfig};
use server::routes::router;
use server::state::AppState;
use server::static_assets;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info,sqlx=warn").init();
    let cfg = ServerConfig::from_env()?;

    // Held for the process lifetime in desktop mode; `None` in server mode.
    let mut embedded: Option<db::embedded::EmbeddedDb> = None;
    let url = match cfg.mode {
        Mode::Server => cfg.database_url.clone().expect("server mode implies a url"),
        Mode::Desktop => {
            let root = dirs::data_local_dir()
                .ok_or_else(|| anyhow::anyhow!("no local data dir"))?
                .join("borobudur-risk");
            std::fs::create_dir_all(&root)?;
            tracing::info!("starting embedded PostgreSQL under {}", root.display());
            let edb = db::embedded::start(&root, false).await?;
            let url = edb.url.clone();
            embedded = Some(edb);
            url
        }
    };

    let pool = db::connect(&url).await?;
    let app = router(AppState::desktop(pool));
    if static_assets::assets_empty() {
        tracing::warn!("frontend assets are empty — build the frontend first (see build.ps1)");
    }
    let listener = tokio::net::TcpListener::bind(&cfg.bind).await?;
    tracing::info!("listening on http://{}", cfg.bind);
    if cfg.open_browser {
        let _ = webbrowser::open(&format!("http://{}", cfg.bind));
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(async { let _ = tokio::signal::ctrl_c().await; })
        .await?;
    if let Some(edb) = embedded {
        edb.stop().await;
    }
    Ok(())
}
```

`AppState::desktop` does not exist yet. Add it now as a temporary shim in `crates/server/src/state.rs` so this compiles; Task 8 replaces its body:

```rust
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
```

- [ ] **Step 6: Verify the build and one existing suite**

Run: `timeout 900 cargo build -p server && timeout 900 cargo test -p server --test api_portfolios`
Expected: build succeeds; `api_portfolios` passes unchanged.

- [ ] **Step 7: Commit**

```bash
git add crates/server/src/config.rs crates/server/src/lib.rs crates/server/src/main.rs crates/server/src/state.rs crates/server/tests/config.rs
git commit -m "feat(server): startup modes for desktop and configured server deployment

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: The grant model

**Files:**
- Create: `crates/db/src/auth/mod.rs`, `crates/db/src/auth/model.rs`
- Modify: `crates/db/src/lib.rs`
- Test: `crates/db/tests/grant_model.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `db::auth::{Domain, Action, Grant, GrantSet, PortfolioScope, AuthCtx}`. Key signatures: `GrantSet::from_grants(impl IntoIterator<Item = Grant>) -> GrantSet`; `GrantSet::allows(&self, Domain, Action, Option<i64>) -> bool`; `GrantSet::any_domain_on(&self, i64) -> bool`; `GrantSet::visible_portfolios(&self) -> PortfolioScope`; `GrantSet::all_access() -> GrantSet`; `Domain::ALL: [Domain; 6]`; `Action::ALL: [Action; 4]`; `Domain::as_str`/`from_str`/`label`; `Action::as_str`/`from_str`; `AuthCtx { principal_id: i64, display_name: String, is_administrator: bool, grants: GrantSet }` and `AuthCtx::desktop()`.

- [ ] **Step 1: Write the failing test**

Create `crates/db/tests/grant_model.rs`:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 600 cargo test -p db --test grant_model`
Expected: FAIL — `unresolved import db::auth`.

- [ ] **Step 3: Write the implementation**

Create `crates/db/src/auth/model.rs`:

```rust
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
    Reference,
}

impl Domain {
    pub const ALL: [Domain; 6] = [
        Domain::Positions, Domain::Nav, Domain::Transactions,
        Domain::Shareholders, Domain::MarketData, Domain::Reference,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Domain::Positions => "positions",
            Domain::Nav => "nav",
            Domain::Transactions => "transactions",
            Domain::Shareholders => "shareholders",
            Domain::MarketData => "market_data",
            Domain::Reference => "reference",
        }
    }

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
        }
    }
}
```

Create `crates/db/src/auth/mod.rs`:

```rust
pub mod model;

pub use model::{Action, AuthCtx, Domain, Grant, GrantSet, PortfolioScope};
```

Add `pub mod auth;` to `crates/db/src/lib.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `timeout 600 cargo test -p db --test grant_model`
Expected: PASS, 12 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/db/src/auth crates/db/src/lib.rs crates/db/tests/grant_model.rs
git commit -m "feat(db): domain x portfolio x action grant model

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: Role templates

**Files:**
- Create: `crates/db/src/auth/roles.rs`
- Modify: `crates/db/src/auth/mod.rs`
- Test: `crates/db/tests/roles.rs`

**Interfaces:**
- Consumes: `Domain`, `Action`, `Grant` from Task 2.
- Produces: `db::auth::Role` with variants `RiskAnalyst`, `HeadOfRisk`, `Operations`, `Auditor`; `Role::ALL: [Role; 4]`; `Role::as_str`/`from_str`/`label`; `Role::expand(&self, scope: Option<i64>) -> Vec<Grant>`.

- [ ] **Step 1: Write the failing test**

Create `crates/db/tests/roles.rs`:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 600 cargo test -p db --test roles`
Expected: FAIL — `no Role in db::auth`.

- [ ] **Step 3: Write the implementation**

Create `crates/db/src/auth/roles.rs`:

```rust
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
```

Add to `crates/db/src/auth/mod.rs`:

```rust
pub mod roles;
pub use roles::Role;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `timeout 600 cargo test -p db --test roles`
Expected: PASS, 6 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/db/src/auth/roles.rs crates/db/src/auth/mod.rs crates/db/tests/roles.rs
git commit -m "feat(db): role templates expanding into concrete grants

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: Migration — authentication and authorization tables

**Files:**
- Create: `crates/db/migrations/0012_auth.sql`
- Test: `crates/db/tests/auth_migration.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: tables `users`, `sessions`, `grants`, `user_roles`, `audit_events`, `login_attempts`. No existing table is altered.

- [ ] **Step 1: Write the failing test**

Create `crates/db/tests/auth_migration.rs`:

```rust
async fn fresh() -> (sqlx::PgPool, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 900 cargo test -p db --test auth_migration`
Expected: FAIL — `table users is missing`.

- [ ] **Step 3: Write the migration**

Create `crates/db/migrations/0012_auth.sql`:

```sql
-- Authentication and authorization. New tables only: an existing desktop
-- database upgrades by running this and continues to work as a single-user
-- install, because desktop mode never reads any of it.

CREATE TABLE users (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  email TEXT NOT NULL UNIQUE,
  display_name TEXT NOT NULL,
  password_hash TEXT NOT NULL,
  is_administrator BOOLEAN NOT NULL DEFAULT false,
  disabled BOOLEAN NOT NULL DEFAULT false,
  must_change_password BOOLEAN NOT NULL DEFAULT false,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Sessions are server-side so revocation is immediate. Only the hash of the
-- token is stored: a stolen database gives no usable cookie.
CREATE TABLE sessions (
  token_hash TEXT PRIMARY KEY,
  user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX idx_sessions_user ON sessions(user_id);
CREATE INDEX idx_sessions_expiry ON sessions(expires_at);

-- One row per (subject, domain, action, portfolio). NULL portfolio_id means
-- every portfolio, and is the only thing that reaches instance-wide resources.
CREATE TABLE grants (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  domain TEXT NOT NULL CHECK (domain IN
    ('positions','nav','transactions','shareholders','market_data','reference')),
  action TEXT NOT NULL CHECK (action IN ('view','export','import','configure')),
  portfolio_id BIGINT REFERENCES portfolios(id) ON DELETE CASCADE,
  granted_by BIGINT REFERENCES users(id) ON DELETE SET NULL,
  granted_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- NULLS NOT DISTINCT so a second wildcard row for the same pair collides.
CREATE UNIQUE INDEX idx_grants_unique
  ON grants(user_id, domain, action, portfolio_id) NULLS NOT DISTINCT;
CREATE INDEX idx_grants_user ON grants(user_id);

-- Which template a user was given, and at what scope. Kept only so the
-- administration screen can offer "re-apply this role"; never read at request
-- time, because roles expand into grant rows at assignment.
CREATE TABLE user_roles (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  role TEXT NOT NULL CHECK (role IN ('risk_analyst','head_of_risk','operations','auditor')),
  portfolio_id BIGINT REFERENCES portfolios(id) ON DELETE CASCADE,
  assigned_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_user_roles_user ON user_roles(user_id);

-- Append-only. There is deliberately no delete path in the application.
-- actor_label denormalises the display name so history stays readable after a
-- user is deleted; user_id goes NULL rather than taking the row with it.
CREATE TABLE audit_events (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  at TIMESTAMPTZ NOT NULL DEFAULT now(),
  user_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
  actor_label TEXT NOT NULL,
  action TEXT NOT NULL,
  domain TEXT,
  portfolio_id BIGINT,
  detail JSONB NOT NULL DEFAULT '{}'::jsonb,
  source_addr TEXT
);
CREATE INDEX idx_audit_at ON audit_events(at DESC);
CREATE INDEX idx_audit_user ON audit_events(user_id);

-- Per account, not per IP: a corporate NAT must not lock out a whole floor.
CREATE TABLE login_attempts (
  email TEXT PRIMARY KEY,
  failures INT NOT NULL DEFAULT 0,
  last_failure_at TIMESTAMPTZ,
  locked_until TIMESTAMPTZ
);
```

- [ ] **Step 4: Run test to verify it passes**

Run: `timeout 900 cargo test -p db --test auth_migration`
Expected: PASS, 4 tests.

- [ ] **Step 5: Verify an existing suite still migrates cleanly**

Run: `timeout 900 cargo test -p db --test settings_roundtrip`
Expected: PASS — proves the new migration does not disturb the existing schema.

- [ ] **Step 6: Commit**

```bash
git add crates/db/migrations/0012_auth.sql crates/db/tests/auth_migration.rs
git commit -m "feat(db): migration for users, sessions, grants, roles and audit

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 5: The privileged admin path

**Files:**
- Create: `crates/db/src/admin.rs`
- Modify: `crates/db/src/lib.rs`
- Test: `crates/db/tests/admin_queries.rs`

**Interfaces:**
- Consumes: `Domain`, `Action`, `Grant`, `GrantSet`, `Role`.
- Produces: `db::admin::Admin<'a>` wrapping `&'a PgPool`, constructed by `db::admin::Admin::new(&PgPool)`. Methods (all `async` returning `anyhow::Result`):
  - `create_user(email, display_name, password_hash, is_administrator) -> i64`
  - `user_by_email(email) -> Option<UserRow>`
  - `user_by_id(id) -> Option<UserRow>`
  - `users_list() -> Vec<UserRow>`
  - `set_password(user_id, password_hash) -> ()`
  - `set_disabled(user_id, bool) -> ()`
  - `grants_for(user_id) -> GrantSet`
  - `grant_rows_for(user_id) -> Vec<Grant>`
  - `grant_add(user_id, Grant, granted_by: Option<i64>) -> ()`
  - `grant_remove(user_id, Grant) -> ()`
  - `role_assign(user_id, Role, scope: Option<i64>, granted_by: Option<i64>) -> ()`
  - `session_create(token_hash, user_id, ttl_hours: i64) -> ()`
  - `session_user(token_hash) -> Option<UserRow>`
  - `session_delete(token_hash) -> ()`
  - `sessions_delete_for(user_id) -> ()`
  - `audit_append(AuditEvent) -> ()`
  - `audit_recent(limit: i64) -> Vec<AuditRow>`
  - `login_record_failure(email, lock_after: i32, lock_secs: i64) -> LockState`
  - `login_reset(email) -> ()`
  - `login_state(email) -> LockState`
  - `user_count() -> i64`
- Types: `UserRow { id, email, display_name, password_hash, is_administrator, disabled }`, `AuditEvent { user_id: Option<i64>, actor_label: String, action: String, domain: Option<Domain>, portfolio_id: Option<i64>, detail: serde_json::Value, source_addr: Option<String> }`, `AuditRow` (the stored shape plus `at: DateTime<Utc>`), `LockState { locked: bool, failures: i32, retry_after_secs: i64 }`.

- [ ] **Step 1: Write the failing test**

Create `crates/db/tests/admin_queries.rs`:

```rust
use db::admin::{Admin, AuditEvent};
use db::auth::{Action, Domain, Grant, Role};

async fn fresh() -> (sqlx::PgPool, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 900 cargo test -p db --test admin_queries`
Expected: FAIL — `unresolved import db::admin`.

- [ ] **Step 3: Write the implementation**

Create `crates/db/src/admin.rs`. This is the one privileged path in the crate — it takes a raw pool because loading a principal's grants necessarily happens before an `AuthCtx` exists. Its module doc must say so, because Task 10 adds a test asserting nothing else uses it.

```rust
//! THE PRIVILEGED PATH.
//!
//! Every other query in this crate goes through `Scoped` and requires an
//! `AuthCtx`. This module cannot: loading a principal's grants is what *builds*
//! the `AuthCtx`. It is therefore the single hole in the wall, and its only
//! legitimate consumers are identity resolution, grant administration and the
//! audit log. `crates/db/tests/admin_isolation.rs` asserts that.

use crate::auth::{Action, Domain, Grant, GrantSet, Role};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};

pub struct Admin<'a> {
    pool: &'a PgPool,
}

#[derive(Clone, Debug)]
pub struct UserRow {
    pub id: i64,
    pub email: String,
    pub display_name: String,
    pub password_hash: String,
    pub is_administrator: bool,
    pub disabled: bool,
}

#[derive(Clone, Debug)]
pub struct AuditEvent {
    pub user_id: Option<i64>,
    pub actor_label: String,
    pub action: String,
    pub domain: Option<Domain>,
    pub portfolio_id: Option<i64>,
    pub detail: serde_json::Value,
    pub source_addr: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct AuditRow {
    pub id: i64,
    pub at: DateTime<Utc>,
    pub actor_label: String,
    pub action: String,
    pub domain: Option<String>,
    pub portfolio_id: Option<i64>,
    pub detail: serde_json::Value,
    pub source_addr: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct LockState {
    pub locked: bool,
    pub failures: i32,
    pub retry_after_secs: i64,
}

fn user_from_row(r: &sqlx::postgres::PgRow) -> UserRow {
    UserRow {
        id: r.get("id"),
        email: r.get("email"),
        display_name: r.get("display_name"),
        password_hash: r.get("password_hash"),
        is_administrator: r.get("is_administrator"),
        disabled: r.get("disabled"),
    }
}

impl<'a> Admin<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Admin { pool }
    }

    pub async fn user_count(&self) -> anyhow::Result<i64> {
        Ok(sqlx::query_scalar("SELECT count(*) FROM users").fetch_one(self.pool).await?)
    }

    pub async fn create_user(
        &self, email: &str, display_name: &str, password_hash: &str, is_administrator: bool,
    ) -> anyhow::Result<i64> {
        Ok(sqlx::query_scalar(
            "INSERT INTO users (email, display_name, password_hash, is_administrator)
             VALUES (lower($1), $2, $3, $4) RETURNING id")
            .bind(email).bind(display_name).bind(password_hash).bind(is_administrator)
            .fetch_one(self.pool).await?)
    }

    pub async fn user_by_email(&self, email: &str) -> anyhow::Result<Option<UserRow>> {
        let row = sqlx::query("SELECT * FROM users WHERE email = lower($1)")
            .bind(email).fetch_optional(self.pool).await?;
        Ok(row.as_ref().map(user_from_row))
    }

    pub async fn user_by_id(&self, id: i64) -> anyhow::Result<Option<UserRow>> {
        let row = sqlx::query("SELECT * FROM users WHERE id = $1")
            .bind(id).fetch_optional(self.pool).await?;
        Ok(row.as_ref().map(user_from_row))
    }

    pub async fn users_list(&self) -> anyhow::Result<Vec<UserRow>> {
        let rows = sqlx::query("SELECT * FROM users ORDER BY display_name")
            .fetch_all(self.pool).await?;
        Ok(rows.iter().map(user_from_row).collect())
    }

    pub async fn set_password(&self, user_id: i64, password_hash: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE users SET password_hash = $2, must_change_password = false WHERE id = $1")
            .bind(user_id).bind(password_hash).execute(self.pool).await?;
        Ok(())
    }

    pub async fn set_disabled(&self, user_id: i64, disabled: bool) -> anyhow::Result<()> {
        sqlx::query("UPDATE users SET disabled = $2 WHERE id = $1")
            .bind(user_id).bind(disabled).execute(self.pool).await?;
        Ok(())
    }

    pub async fn grant_rows_for(&self, user_id: i64) -> anyhow::Result<Vec<Grant>> {
        let rows = sqlx::query("SELECT domain, action, portfolio_id FROM grants WHERE user_id = $1")
            .bind(user_id).fetch_all(self.pool).await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let d: String = r.get("domain");
            let a: String = r.get("action");
            // A row failing to parse means the CHECK constraint and this enum
            // have diverged; that is a bug, not user input, so it is loud.
            let domain = Domain::from_str(&d)
                .ok_or_else(|| anyhow::anyhow!("unknown domain in grants: {d}"))?;
            let action = Action::from_str(&a)
                .ok_or_else(|| anyhow::anyhow!("unknown action in grants: {a}"))?;
            out.push(Grant { domain, action, portfolio: r.get("portfolio_id") });
        }
        Ok(out)
    }

    pub async fn grants_for(&self, user_id: i64) -> anyhow::Result<GrantSet> {
        Ok(GrantSet::from_grants(self.grant_rows_for(user_id).await?))
    }

    pub async fn grant_add(&self, user_id: i64, g: Grant, granted_by: Option<i64>) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO grants (user_id, domain, action, portfolio_id, granted_by)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (user_id, domain, action, portfolio_id) DO NOTHING")
            .bind(user_id).bind(g.domain.as_str()).bind(g.action.as_str())
            .bind(g.portfolio).bind(granted_by)
            .execute(self.pool).await?;
        Ok(())
    }

    pub async fn grant_remove(&self, user_id: i64, g: Grant) -> anyhow::Result<()> {
        sqlx::query(
            "DELETE FROM grants WHERE user_id = $1 AND domain = $2 AND action = $3
             AND portfolio_id IS NOT DISTINCT FROM $4")
            .bind(user_id).bind(g.domain.as_str()).bind(g.action.as_str()).bind(g.portfolio)
            .execute(self.pool).await?;
        Ok(())
    }

    pub async fn role_assign(
        &self, user_id: i64, role: Role, scope: Option<i64>, granted_by: Option<i64>,
    ) -> anyhow::Result<()> {
        sqlx::query("INSERT INTO user_roles (user_id, role, portfolio_id) VALUES ($1, $2, $3)")
            .bind(user_id).bind(role.as_str()).bind(scope)
            .execute(self.pool).await?;
        for g in role.expand(scope) {
            self.grant_add(user_id, g, granted_by).await?;
        }
        Ok(())
    }

    pub async fn session_create(&self, token_hash: &str, user_id: i64, ttl_hours: i64) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO sessions (token_hash, user_id, expires_at)
             VALUES ($1, $2, now() + make_interval(hours => $3::int))")
            .bind(token_hash).bind(user_id).bind(ttl_hours as i32)
            .execute(self.pool).await?;
        Ok(())
    }

    pub async fn session_user(&self, token_hash: &str) -> anyhow::Result<Option<UserRow>> {
        let row = sqlx::query(
            "SELECT u.* FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = $1 AND s.expires_at > now() AND NOT u.disabled")
            .bind(token_hash).fetch_optional(self.pool).await?;
        Ok(row.as_ref().map(user_from_row))
    }

    pub async fn session_delete(&self, token_hash: &str) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(token_hash).execute(self.pool).await?;
        Ok(())
    }

    pub async fn sessions_delete_for(&self, user_id: i64) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(user_id).execute(self.pool).await?;
        Ok(())
    }

    pub async fn audit_append(&self, e: AuditEvent) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO audit_events (user_id, actor_label, action, domain, portfolio_id, detail, source_addr)
             VALUES ($1, $2, $3, $4, $5, $6, $7)")
            .bind(e.user_id).bind(&e.actor_label).bind(&e.action)
            .bind(e.domain.map(|d| d.as_str())).bind(e.portfolio_id)
            .bind(&e.detail).bind(&e.source_addr)
            .execute(self.pool).await?;
        Ok(())
    }

    pub async fn audit_recent(&self, limit: i64) -> anyhow::Result<Vec<AuditRow>> {
        let rows = sqlx::query(
            "SELECT id, at, actor_label, action, domain, portfolio_id, detail, source_addr
             FROM audit_events ORDER BY at DESC, id DESC LIMIT $1")
            .bind(limit).fetch_all(self.pool).await?;
        Ok(rows.iter().map(|r| AuditRow {
            id: r.get("id"),
            at: r.get("at"),
            actor_label: r.get("actor_label"),
            action: r.get("action"),
            domain: r.get("domain"),
            portfolio_id: r.get("portfolio_id"),
            detail: r.get("detail"),
            source_addr: r.get("source_addr"),
        }).collect())
    }

    pub async fn login_state(&self, email: &str) -> anyhow::Result<LockState> {
        let row = sqlx::query(
            "SELECT failures, GREATEST(0, EXTRACT(EPOCH FROM (locked_until - now()))::bigint) AS retry
             FROM login_attempts WHERE email = lower($1)")
            .bind(email).fetch_optional(self.pool).await?;
        Ok(match row {
            None => LockState { locked: false, failures: 0, retry_after_secs: 0 },
            Some(r) => {
                let retry: i64 = r.get("retry");
                LockState { locked: retry > 0, failures: r.get("failures"), retry_after_secs: retry }
            }
        })
    }

    /// Records one failure and returns the resulting state. Locking is applied
    /// on the `lock_after`-th failure and every failure beyond it, so an
    /// attacker who keeps guessing keeps extending their own lockout.
    pub async fn login_record_failure(
        &self, email: &str, lock_after: i32, lock_secs: i64,
    ) -> anyhow::Result<LockState> {
        let row = sqlx::query(
            "INSERT INTO login_attempts (email, failures, last_failure_at)
             VALUES (lower($1), 1, now())
             ON CONFLICT (email) DO UPDATE
               SET failures = login_attempts.failures + 1,
                   last_failure_at = now(),
                   locked_until = CASE
                     WHEN login_attempts.failures + 1 >= $2
                     THEN now() + make_interval(secs => $3::double precision)
                     ELSE login_attempts.locked_until END
             RETURNING failures,
                       GREATEST(0, EXTRACT(EPOCH FROM (locked_until - now()))::bigint) AS retry")
            .bind(email).bind(lock_after).bind(lock_secs as f64)
            .fetch_one(self.pool).await?;
        let retry: i64 = row.get("retry");
        Ok(LockState { locked: retry > 0, failures: row.get("failures"), retry_after_secs: retry })
    }

    pub async fn login_reset(&self, email: &str) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM login_attempts WHERE email = lower($1)")
            .bind(email).execute(self.pool).await?;
        Ok(())
    }
}
```

Add `pub mod admin;` to `crates/db/src/lib.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `timeout 900 cargo test -p db --test admin_queries`
Expected: PASS, 8 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/db/src/admin.rs crates/db/src/lib.rs crates/db/tests/admin_queries.rs
git commit -m "feat(db): privileged admin path for identity, grants and audit

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 6: Split `repo.rs` by domain

**Files:**
- Create: `crates/db/src/repo/mod.rs`, `crates/db/src/repo/positions.rs`, `nav.rs`, `transactions.rs`, `shareholders.rs`, `market_data.rs`, `reference.rs`, `imports.rs`
- Delete: `crates/db/src/repo.rs`
- Test: none new — the existing `crates/db/tests/*` suites are the test.

**Interfaces:**
- Consumes: nothing new.
- Produces: **no public API change.** Every symbol previously at `db::repo::X` must still resolve at `db::repo::X` via re-export. This is a pure move so that Task 8's signature change lands on files small enough to review.

Domain assignment for the move (also the mapping Task 8 uses):

| Function | Module | Domain | Action |
|---|---|---|---|
| `import_batch`, `import_workbook` | `imports.rs` | multi (see Task 9) | `import` |
| `imports_list` | `imports.rs` | `reference` | `view` |
| `position_dates`, `positions_for`, `dividends_all`, `derive_dividends` | `positions.rs` | `positions` | `view` / `view` / `view` / `import` |
| `nav_rows`, `aum_for` | `nav.rs` | `nav` | `view` |
| `operations_all` | `transactions.rs` | `transactions` | `view` |
| `shareholders_for`, `flows_for` | `shareholders.rs` | `shareholders` | `view` |
| `shareholders_replace`, `flows_upsert` | `shareholders.rs` | `shareholders` | `import` |
| `fx_all`, `ctd_for` | `market_data.rs` | `market_data` | `view` |
| `fx_upsert_many`, `adv_upsert_many`, `ctd_replace` | `market_data.rs` | `market_data` | `import` |
| `refs_all`, `contracts_all`, `emir_kpis_all`, `portfolio_codes_for`, `portfolio_by_code`, `portfolios_list`, `portfolio_get` | `reference.rs` | `reference` | `view` |
| `refs_upsert`, `contracts_upsert`, `classify_upsert_many`, `emir_kpi_upsert`, `portfolio_create`, `portfolio_update`, `portfolio_codes_replace` | `reference.rs` | `reference` | `configure` |

`fx_all`, `fx_upsert_many`, `refs_all`, `refs_upsert`, `contracts_all`, `contracts_upsert`, `classify_upsert_many`, `portfolios_list`, `portfolio_create` and `portfolio_by_code` are **instance-wide**: they take no `portfolio_id` and therefore require a wildcard grant.

- [ ] **Step 1: Record the current test baseline**

Run each existing db suite and note the pass count:

```bash
for t in derive_dividends emir_kpis futures_analytics futures_contracts futures_seeding \
         import_batch import_workbook instrument_refs liquidity_v2_repo pam_check \
         pnl_repo portfolio_codes settings_roundtrip settings_v2; do
  echo "== $t"; timeout 900 cargo test -p db --test $t 2>&1 | tail -3
done
```

Expected: all pass. Record the counts — they must be identical after the move.

- [ ] **Step 2: Create the module directory and move code**

Create `crates/db/src/repo/mod.rs`:

```rust
//! Repository queries, split by data domain. The file a query lives in is the
//! domain it belongs to, so reviewing what a domain grant exposes means reading
//! one file rather than grepping. Task 8 turns these free functions into
//! methods on `Scoped`; the split lands first so that change is reviewable.

pub mod imports;
pub mod market_data;
pub mod nav;
pub mod positions;
pub mod reference;
pub mod shareholders;
pub mod transactions;

pub use imports::*;
pub use market_data::*;
pub use nav::*;
pub use positions::*;
pub use reference::*;
pub use shareholders::*;
pub use transactions::*;
```

Move each function — and the record structs it returns — from `repo.rs` into the module named in the table above, verbatim. Shared private helpers move alongside the function that uses them; a helper used by two modules moves to `repo/mod.rs` as `pub(crate)`. Delete `crates/db/src/repo.rs` when empty.

Struct placement follows the function that produces it: `PositionRecord`, `DividendRecord` → `positions.rs`; `NavRow` → `nav.rs`; `OperationRecord` → `transactions.rs`; `Shareholder`, `FlowRecord` → `shareholders.rs`; `FxRow`, `CtdRecord` → `market_data.rs`; `InstrumentRef`, `FuturesContract`, `EmirKpi`, `Portfolio`, `PortfolioCode` → `reference.rs`; `ImportRecord`, `ImportOutcome` → `imports.rs`.

- [ ] **Step 3: Verify nothing outside the crate changed**

Run: `timeout 900 cargo build -p db -p server`
Expected: builds with no changes to any file outside `crates/db/src/repo/`. If a caller needs editing, the re-export in `mod.rs` is missing a symbol — fix the re-export, not the caller.

- [ ] **Step 4: Re-run the baseline suites**

Run the same loop from Step 1.
Expected: identical pass counts.

- [ ] **Step 5: Commit**

```bash
git add -u crates/db/src
git add crates/db/src/repo
git commit -m "refactor(db): split repo.rs into six domain modules

Pure move, no signature or behaviour change: the file a query lives in is
now the data domain it belongs to.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 7: Identity providers and the session endpoints

**Files:**
- Create: `crates/server/src/auth/mod.rs`, `crates/server/src/auth/desktop.rs`, `crates/server/src/auth/local.rs`, `crates/server/src/handlers/session.rs`
- Modify: `crates/server/Cargo.toml`, `crates/server/src/lib.rs`, `crates/server/src/state.rs`, `crates/server/src/handlers/mod.rs`, `crates/server/src/routes.rs`, `crates/server/src/error.rs`, `crates/db/src/lib.rs`
- Test: `crates/server/tests/api_session.rs`

**Interfaces:**
- Consumes: `db::admin::Admin`, `db::auth::{AuthCtx, GrantSet}`, `server::config::Mode`.
- Produces:
  - `db::Db` with `Db::from_pool(PgPool) -> Db`, `Db::connect(&str) -> anyhow::Result<Db>`, `Db::admin(&self) -> Admin<'_>`, and `pub(crate) fn pool(&self) -> &PgPool`. `Db` is `Clone` (the inner pool is an `Arc` internally).
  - `server::auth::{Principal, IdentityProvider, AuthError}`; `Principal { id: i64, display_name: String, is_administrator: bool, grants: GrantSet }`; `IdentityProvider::authenticate(&self, headers: &HeaderMap) -> Result<Principal, AuthError>`; `AuthError::{Unauthenticated, LockedOut { retry_after_secs: u64 }, Internal(anyhow::Error)}`.
  - `server::auth::desktop::DesktopSingleUser`, `server::auth::local::LocalAccounts` with `LocalAccounts::new(Arc<db::Db>)`, `login(&self, email, password, source_addr) -> Result<String, AuthError>` returning the raw session token, and `logout(&self, token) -> anyhow::Result<()>`.
  - `AppState { db: Arc<db::Db>, identity: Arc<dyn IdentityProvider>, mode: Mode }`, `AppState::desktop(PgPool) -> AppState`, `AppState::server(PgPool) -> AppState`.
  - `AppError::Unauthenticated` and `AppError::LockedOut(u64)`.
  - Routes `POST /api/login`, `POST /api/logout`, `GET /api/me`.

- [ ] **Step 1: Add dependencies**

In `crates/server/Cargo.toml` add to `[dependencies]`:

```toml
argon2 = "0.5"
async-trait = "0.1"
rand = "0.8"
axum-extra = { version = "0.10", features = ["cookie"] }
```

- [ ] **Step 2: Write the failing test**

Create `crates/server/tests/api_session.rs`:

```rust
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

async fn server_app() -> (axum::Router, sqlx::PgPool, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    std::mem::forget(dir);
    let app = server::routes::router(server::state::AppState::server(pool.clone()));
    (app, pool, edb)
}

async fn seed_user(pool: &sqlx::PgPool, email: &str, password: &str) -> i64 {
    let hash = server::auth::local::hash_password(password).unwrap();
    db::admin::Admin::new(pool).create_user(email, "Risk", &hash, false).await.unwrap()
}

fn login_req(email: &str, password: &str) -> Request<Body> {
    Request::post("/api/login")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::json!({"email": email, "password": password}).to_string()))
        .unwrap()
}

async fn status_of(app: &axum::Router, req: Request<Body>) -> StatusCode {
    app.clone().oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn login_sets_a_session_cookie_and_me_reports_the_principal() {
    let (app, pool, edb) = server_app().await;
    seed_user(&pool, "r@f.lu", "correct horse battery").await;

    let res = app.clone().oneshot(login_req("r@f.lu", "correct horse battery")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let cookie = res.headers().get("set-cookie").unwrap().to_str().unwrap().to_string();
    assert!(cookie.contains("HttpOnly"), "session cookie must be HttpOnly");
    assert!(cookie.contains("SameSite=Strict"));

    let me = app.clone().oneshot(
        Request::get("/api/me").header("cookie", cookie.split(';').next().unwrap())
            .body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(me.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&me.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["display_name"], "Risk");
    assert_eq!(body["is_administrator"], false);
    edb.stop().await;
}

#[tokio::test]
async fn me_without_a_session_is_401() {
    let (app, _pool, edb) = server_app().await;
    assert_eq!(status_of(&app, Request::get("/api/me").body(Body::empty()).unwrap()).await,
               StatusCode::UNAUTHORIZED);
    edb.stop().await;
}

#[tokio::test]
async fn a_wrong_password_is_401_and_does_not_reveal_whether_the_account_exists() {
    let (app, pool, edb) = server_app().await;
    seed_user(&pool, "r@f.lu", "correct horse battery").await;
    let known = app.clone().oneshot(login_req("r@f.lu", "wrong")).await.unwrap();
    let unknown = app.clone().oneshot(login_req("nobody@f.lu", "wrong")).await.unwrap();
    assert_eq!(known.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(unknown.status(), StatusCode::UNAUTHORIZED);
    let kb = known.into_body().collect().await.unwrap().to_bytes();
    let ub = unknown.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(kb, ub, "the two responses must be indistinguishable");
    edb.stop().await;
}

#[tokio::test]
async fn five_failures_lock_the_account_even_with_the_right_password() {
    let (app, pool, edb) = server_app().await;
    seed_user(&pool, "r@f.lu", "correct horse battery").await;
    for _ in 0..5 {
        let _ = app.clone().oneshot(login_req("r@f.lu", "wrong")).await.unwrap();
    }
    let res = app.clone().oneshot(login_req("r@f.lu", "correct horse battery")).await.unwrap();
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(res.headers().contains_key("retry-after"));
    edb.stop().await;
}

#[tokio::test]
async fn a_successful_login_clears_earlier_failures() {
    let (app, pool, edb) = server_app().await;
    seed_user(&pool, "r@f.lu", "correct horse battery").await;
    for _ in 0..3 {
        let _ = app.clone().oneshot(login_req("r@f.lu", "wrong")).await.unwrap();
    }
    assert_eq!(status_of(&app, login_req("r@f.lu", "correct horse battery")).await, StatusCode::OK);
    let st = db::admin::Admin::new(&pool).login_state("r@f.lu").await.unwrap();
    assert_eq!(st.failures, 0);
    edb.stop().await;
}

#[tokio::test]
async fn logout_revokes_the_session_immediately() {
    let (app, pool, edb) = server_app().await;
    seed_user(&pool, "r@f.lu", "correct horse battery").await;
    let res = app.clone().oneshot(login_req("r@f.lu", "correct horse battery")).await.unwrap();
    let cookie = res.headers().get("set-cookie").unwrap().to_str().unwrap()
        .split(';').next().unwrap().to_string();

    let out = app.clone().oneshot(
        Request::post("/api/logout").header("cookie", &cookie).body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(out.status(), StatusCode::NO_CONTENT);

    let me = app.clone().oneshot(
        Request::get("/api/me").header("cookie", &cookie).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(me.status(), StatusCode::UNAUTHORIZED);
    edb.stop().await;
}

#[tokio::test]
async fn desktop_mode_needs_no_login_at_all() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    std::mem::forget(dir);
    let app = server::routes::router(server::state::AppState::desktop(pool));
    let me = app.clone().oneshot(Request::get("/api/me").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(me.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&me.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["is_administrator"], true);
    edb.stop().await;
}

#[tokio::test]
async fn a_session_token_is_never_stored_in_the_clear() {
    let (app, pool, edb) = server_app().await;
    seed_user(&pool, "r@f.lu", "correct horse battery").await;
    let res = app.clone().oneshot(login_req("r@f.lu", "correct horse battery")).await.unwrap();
    let cookie = res.headers().get("set-cookie").unwrap().to_str().unwrap().to_string();
    let token = cookie.split(';').next().unwrap().splitn(2, '=').nth(1).unwrap().to_string();
    let stored: Vec<String> = sqlx::query_scalar("SELECT token_hash FROM sessions")
        .fetch_all(&pool).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_ne!(stored[0], token, "the raw token must not be in the database");
    edb.stop().await;
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `timeout 900 cargo test -p server --test api_session`
Expected: FAIL — `AppState::server` and `server::auth` do not exist.

- [ ] **Step 4: Add `Db` to the db crate**

Replace `crates/db/src/lib.rs`'s free `connect` with a `Db` wrapper, keeping `connect` for existing callers:

```rust
pub mod admin;
pub mod auth;
pub mod embedded;
pub mod repo;
pub mod scoped;   // added in Task 8; declare it then, not now
pub mod settings;

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

pub async fn connect(url: &str) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new().max_connections(5).connect(url).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}

/// Owns the connection pool. The pool is private: from Task 10 onward the only
/// routes out of this type are `scope` (which demands an `AuthCtx`) and `admin`
/// (the declared privileged path).
#[derive(Clone)]
pub struct Db {
    pool: PgPool,
}

impl Db {
    pub async fn connect(url: &str) -> anyhow::Result<Db> {
        Ok(Db { pool: connect(url).await? })
    }

    pub fn from_pool(pool: PgPool) -> Db {
        Db { pool }
    }

    pub fn admin(&self) -> crate::admin::Admin<'_> {
        crate::admin::Admin::new(&self.pool)
    }

    pub(crate) fn pool(&self) -> &PgPool {
        &self.pool
    }
}
```

Do not declare `pub mod scoped;` until Task 8 creates the file.

- [ ] **Step 5: Write the identity layer**

Create `crates/server/src/auth/mod.rs`:

```rust
pub mod desktop;
pub mod local;
pub mod middleware;   // added in Task 8

use axum::http::HeaderMap;
use db::auth::GrantSet;

/// Who is making this request, and what they may do. Everything downstream sees
/// only this — which is what lets OIDC become a third provider later with no
/// change below the seam.
#[derive(Clone, Debug)]
pub struct Principal {
    pub id: i64,
    pub display_name: String,
    pub is_administrator: bool,
    pub grants: GrantSet,
}

#[derive(Debug)]
pub enum AuthError {
    Unauthenticated,
    LockedOut { retry_after_secs: u64 },
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for AuthError {
    fn from(e: anyhow::Error) -> Self {
        AuthError::Internal(e)
    }
}

#[async_trait::async_trait]
pub trait IdentityProvider: Send + Sync + std::fmt::Debug {
    async fn authenticate(&self, headers: &HeaderMap) -> Result<Principal, AuthError>;
}
```

Do not declare `pub mod middleware;` until Task 8 creates the file.

Create `crates/server/src/auth/desktop.rs`:

```rust
use super::{AuthError, IdentityProvider, Principal};
use axum::http::HeaderMap;
use db::auth::GrantSet;

/// Desktop mode's identity. Not a bypass: it satisfies the same trait, travels
/// the same middleware, and produces the same `AuthCtx` shape as a real login.
#[derive(Debug)]
pub struct DesktopSingleUser;

#[async_trait::async_trait]
impl IdentityProvider for DesktopSingleUser {
    async fn authenticate(&self, _headers: &HeaderMap) -> Result<Principal, AuthError> {
        Ok(Principal {
            id: 0,
            display_name: "desktop".to_string(),
            is_administrator: true,
            grants: GrantSet::all_access(),
        })
    }
}
```

Create `crates/server/src/auth/local.rs`:

```rust
use super::{AuthError, IdentityProvider, Principal};
use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::http::HeaderMap;
use rand::RngCore;
use std::sync::Arc;

pub const COOKIE_NAME: &str = "borobudur_session";
pub const SESSION_TTL_HOURS: i64 = 12;
const LOCK_AFTER: i32 = 5;
const LOCK_SECS: i64 = 900;

#[derive(Debug)]
pub struct LocalAccounts {
    db: Arc<db::Db>,
}

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| anyhow::anyhow!("hashing failed: {e}"))
}

fn verify_password(password: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok(),
        Err(_) => false,
    }
}

/// 256 bits from the OS CSPRNG. Only the SHA-256 of this ever reaches the
/// database, so a stolen dump yields no usable cookie.
fn new_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn token_hash(token: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    headers.get(axum::http::header::COOKIE)?
        .to_str().ok()?
        .split(';')
        .filter_map(|p| p.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE_NAME)
        .map(|(_, v)| v.to_string())
}

impl LocalAccounts {
    pub fn new(db: Arc<db::Db>) -> Self {
        LocalAccounts { db }
    }

    /// Returns the raw session token on success. The caller sets the cookie.
    pub async fn login(&self, email: &str, password: &str) -> Result<String, AuthError> {
        let admin = self.db.admin();
        let state = admin.login_state(email).await?;
        if state.locked {
            return Err(AuthError::LockedOut { retry_after_secs: state.retry_after_secs as u64 });
        }

        let user = admin.user_by_email(email).await?;
        // An unknown account still pays the hashing cost, so response timing
        // does not distinguish "no such user" from "wrong password".
        let ok = match &user {
            Some(u) if !u.disabled => verify_password(password, &u.password_hash),
            Some(_) => false,
            None => {
                let _ = verify_password(password, "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHRzYWx0$\
                                                    5Zt7pKQKUwjHhBDUMTLDCcxT4rWmMoTPxSDLbGYwqPo");
                false
            }
        };

        if !ok {
            let st = admin.login_record_failure(email, LOCK_AFTER, LOCK_SECS).await?;
            if st.locked {
                return Err(AuthError::LockedOut { retry_after_secs: st.retry_after_secs as u64 });
            }
            return Err(AuthError::Unauthenticated);
        }

        let user = user.expect("verified above");
        admin.login_reset(email).await?;
        let token = new_token();
        admin.session_create(&token_hash(&token), user.id, SESSION_TTL_HOURS).await?;
        Ok(token)
    }

    pub async fn logout(&self, headers: &HeaderMap) -> anyhow::Result<()> {
        if let Some(t) = cookie_token(headers) {
            self.db.admin().session_delete(&token_hash(&t)).await?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl IdentityProvider for LocalAccounts {
    async fn authenticate(&self, headers: &HeaderMap) -> Result<Principal, AuthError> {
        let token = cookie_token(headers).ok_or(AuthError::Unauthenticated)?;
        let admin = self.db.admin();
        let user = admin.session_user(&token_hash(&token)).await?
            .ok_or(AuthError::Unauthenticated)?;
        let grants = admin.grants_for(user.id).await?;
        Ok(Principal {
            id: user.id,
            display_name: user.display_name,
            is_administrator: user.is_administrator,
            grants,
        })
    }
}
```

- [ ] **Step 6: Rewrite `AppState` and add the session handlers**

`crates/server/src/state.rs`:

```rust
use crate::auth::{desktop::DesktopSingleUser, local::LocalAccounts, IdentityProvider};
use crate::config::Mode;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<db::Db>,
    pub identity: Arc<dyn IdentityProvider>,
    pub mode: Mode,
}

impl AppState {
    pub fn desktop(pool: sqlx::PgPool) -> Self {
        AppState {
            db: Arc::new(db::Db::from_pool(pool)),
            identity: Arc::new(DesktopSingleUser),
            mode: Mode::Desktop,
        }
    }

    pub fn server(pool: sqlx::PgPool) -> Self {
        let db = Arc::new(db::Db::from_pool(pool));
        AppState {
            identity: Arc::new(LocalAccounts::new(db.clone())),
            db,
            mode: Mode::Server,
        }
    }
}
```

`AppState` no longer has a `pool` field, so the 16 existing `crates/server/tests/api_*.rs` files stop compiling. Fix them mechanically:

```bash
cd crates/server/tests
sed -i 's/AppState { pool: \([a-z_]*\)\.clone() }/AppState::desktop(\1.clone())/g; s/AppState { pool: \([a-z_]*\) }/AppState::desktop(\1)/g' api_*.rs
grep -n "AppState {" api_*.rs   # must print nothing
```

Create `crates/server/src/handlers/session.rs`:

```rust
use crate::auth::local::{LocalAccounts, COOKIE_NAME, SESSION_TTL_HOURS};
use crate::auth::{AuthError, Principal};
use crate::config::Mode;
use crate::error::AppError;
use crate::state::AppState;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

#[derive(serde::Deserialize)]
pub struct LoginBody {
    pub email: String,
    pub password: String,
}

pub async fn login(
    State(st): State<AppState>, Json(body): Json<LoginBody>,
) -> Result<Response, AppError> {
    let Some(local) = st.local_accounts() else {
        // Desktop mode has no accounts to log in to.
        return Err(AppError::NotFound("login is not available in desktop mode".into()));
    };
    match local.login(&body.email, &body.password).await {
        Ok(token) => {
            let secure = if st.mode == Mode::Server { "; Secure" } else { "" };
            let cookie = format!(
                "{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Strict{secure}; Max-Age={}",
                SESSION_TTL_HOURS * 3600);
            Ok(([(header::SET_COOKIE, cookie)], Json(serde_json::json!({"ok": true}))).into_response())
        }
        Err(AuthError::LockedOut { retry_after_secs }) => Err(AppError::LockedOut(retry_after_secs)),
        Err(AuthError::Unauthenticated) => Err(AppError::Unauthenticated),
        Err(AuthError::Internal(e)) => Err(AppError::Internal(e)),
    }
}

pub async fn logout(State(st): State<AppState>, headers: HeaderMap) -> Result<Response, AppError> {
    if let Some(local) = st.local_accounts() {
        local.logout(&headers).await?;
    }
    let cookie = format!("{COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0");
    Ok(([(header::SET_COOKIE, cookie)], StatusCode::NO_CONTENT).into_response())
}

#[derive(serde::Serialize)]
pub struct MeResponse {
    pub display_name: String,
    pub is_administrator: bool,
    pub capabilities: Vec<Capability>,
}

#[derive(serde::Serialize)]
pub struct Capability {
    pub domain: &'static str,
    pub action: &'static str,
    pub portfolio_id: Option<i64>,
}

pub async fn me(
    State(st): State<AppState>, headers: HeaderMap,
) -> Result<Json<MeResponse>, AppError> {
    let p: Principal = st.identity.authenticate(&headers).await.map_err(AppError::from)?;
    Ok(Json(MeResponse {
        display_name: p.display_name,
        is_administrator: p.is_administrator,
        capabilities: p.grants.iter().map(|g| Capability {
            domain: g.domain.as_str(),
            action: g.action.as_str(),
            portfolio_id: g.portfolio,
        }).collect(),
    }))
}
```

Add to `AppState`:

```rust
impl AppState {
    /// `Some` only in server mode — desktop mode has no accounts.
    pub fn local_accounts(&self) -> Option<LocalAccounts> {
        match self.mode {
            Mode::Server => Some(LocalAccounts::new(self.db.clone())),
            Mode::Desktop => None,
        }
    }
}
```

Add `pub mod session;` to `crates/server/src/handlers/mod.rs`, `pub mod auth;` to `crates/server/src/lib.rs`, and to `crates/server/src/routes.rs`:

```rust
.route("/api/login", axum::routing::post(handlers::session::login))
.route("/api/logout", axum::routing::post(handlers::session::logout))
.route("/api/me", get(handlers::session::me))
```

Extend `crates/server/src/error.rs` with two variants — and **replace the blanket
`From` impl**, which is load-bearing for correctness:

```rust
pub enum AppError {
    // ...existing variants...
    Unauthenticated,
    LockedOut(u64),
}
```

`error.rs` currently has `impl<E: Into<anyhow::Error>> From<E> for AppError`,
which routes everything to `Internal` (500). That blanket must go, for two
reasons: it makes any further `From` impl a coherence conflict, and — worse —
it would swallow a permission denial into a 500 the moment `Denied` reached a
`?`. Replace it with explicit conversions:

```rust
impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self { AppError::Internal(e) }
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self { AppError::Internal(e.into()) }
}

impl From<crate::auth::AuthError> for AppError {
    fn from(e: crate::auth::AuthError) -> Self {
        match e {
            crate::auth::AuthError::Unauthenticated => AppError::Unauthenticated,
            crate::auth::AuthError::LockedOut { retry_after_secs } => AppError::LockedOut(retry_after_secs),
            crate::auth::AuthError::Internal(e) => AppError::Internal(e),
        }
    }
}
```

Removing the blanket breaks any `?` on an error type not covered above
(`std::io::Error`, `serde_json::Error`, `calamine::Error` and similar). Fix each
by wrapping at the call site: `.map_err(anyhow::Error::from)?`. This is a
compile-driven sweep — build, fix what the compiler names, repeat. Do not
reinstate the blanket.

and in `IntoResponse`:

```rust
AppError::Unauthenticated => (
    StatusCode::UNAUTHORIZED,
    Json(serde_json::json!({"title": "Unauthorized", "status": 401, "detail": "authentication required"})),
).into_response(),
AppError::LockedOut(secs) => (
    StatusCode::TOO_MANY_REQUESTS,
    [(axum::http::header::RETRY_AFTER, secs.to_string())],
    Json(serde_json::json!({"title": "Too Many Requests", "status": 429,
                            "detail": "too many failed sign-in attempts"})),
).into_response(),
```

The blanket `impl<E: Into<anyhow::Error>> From<E> for AppError` collides with the new `From<AuthError>`. Remove `AuthError`'s `From<anyhow::Error>` conflict by *not* deriving `std::error::Error` on `AuthError` — it is converted explicitly and never through the blanket impl. If the compiler still reports a conflict, convert at the call site with a named function `AppError::from_auth(e)` instead of the trait, and drop the `From` impl.

- [ ] **Step 7: Run test to verify it passes**

Run: `timeout 900 cargo test -p server --test api_session`
Expected: PASS, 8 tests.

- [ ] **Step 8: Verify desktop parity for two existing suites**

Run: `timeout 900 cargo test -p server --test api_portfolios && timeout 900 cargo test -p server --test api_metrics`
Expected: PASS unchanged.

- [ ] **Step 9: Commit**

```bash
git add crates/server/Cargo.toml crates/server/src/auth crates/server/src/state.rs \
        crates/server/src/handlers/session.rs crates/server/src/handlers/mod.rs \
        crates/server/src/routes.rs crates/server/src/error.rs crates/server/src/lib.rs \
        crates/db/src/lib.rs crates/server/tests/api_session.rs
git add -u crates/server/tests
git commit -m "feat(server): identity seam with local accounts and desktop principal

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 8: The scoped handle, the access token, and one vertical slice

**Files:**
- Create: `crates/db/src/auth/access.rs`, `crates/db/src/scoped.rs`, `crates/server/src/auth/middleware.rs`, `crates/server/src/routes/protect.rs`
- Modify: `crates/db/src/auth/mod.rs`, `crates/db/src/lib.rs`, `crates/db/src/repo/nav.rs`, `crates/db/src/repo/positions.rs`, `crates/server/src/routes.rs`, `crates/server/src/handlers/data.rs`, `crates/server/src/error.rs`
- Test: `crates/db/tests/authorize.rs`, `crates/server/tests/api_authz_slice.rs`

**Interfaces:**
- Consumes: `AuthCtx`, `GrantSet`, `Domain`, `Action` (Task 2); `AppState`, `Principal` (Task 7).
- Produces:
  - `db::auth::access::{DomainMarker, ActionMarker, Access, GlobalAccess, Denied, DeniedKind}` and marker types `Positions, Nav, Transactions, Shareholders, MarketData, Reference` / `View, Export, Import, Configure` under `db::auth::marker`.
  - `db::scoped::Scoped<'a>` with `authorize<D, A>(&self, portfolio_id: i64) -> Result<Access<D, A>, Denied>`, `authorize_global<D, A>(&self) -> Result<GlobalAccess<D, A>, Denied>`, `may<D, A>(&self, portfolio_id: i64) -> bool`, and the migrated methods `nav_rows(&Access<Nav, View>)`, `aum_for(&Access<Nav, View>, NaiveDate)`, `position_dates(&Access<Positions, View>)`, `positions_for(&Access<Positions, View>, NaiveDate)`.
  - `db::Db::scope(&'a self, &'a AuthCtx) -> Scoped<'a>`.
  - `server::routes::protect::ProtectExt` with `.protected(path, MethodRouter<AppState>, Domain, Action)` and `.public(path, MethodRouter<AppState>)`.
  - `AppError::Forbidden(Denied)`; `From<Denied> for AppError` mapping `DeniedKind::OutOfScope` → 404 and `DeniedKind::NotGranted` → 403.
  - Request extension `AuthCtx`, inserted by `server::auth::middleware::resolve_principal`.

- [ ] **Step 1: Write the failing db test**

Create `crates/db/tests/authorize.rs`:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 900 cargo test -p db --test authorize`
Expected: FAIL — `db::auth::marker` does not exist.

- [ ] **Step 3: Write the access token and scoped handle**

Create `crates/db/src/auth/access.rs`:

```rust
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
```

Create `crates/db/src/scoped.rs`:

```rust
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
```

Add to `crates/db/src/auth/mod.rs`:

```rust
pub mod access;
pub use access::{marker, Access, ActionMarker, Denied, DeniedKind, DomainMarker, GlobalAccess};
```

Add `pub mod scoped;` to `crates/db/src/lib.rs`.

- [ ] **Step 4: Migrate the `nav` and `positions` queries onto `Scoped`**

In `crates/db/src/repo/nav.rs`, convert the two functions to inherent methods on `Scoped`, keeping the SQL byte-for-byte:

```rust
use crate::auth::marker::{Nav, View};
use crate::auth::Access;
use crate::scoped::Scoped;

impl<'a> Scoped<'a> {
    pub async fn nav_rows(&self, a: &Access<Nav, View>) -> anyhow::Result<Vec<NavRow>> {
        // body of the old nav_rows, with `self.pool` for `pool`
        // and `a.portfolio_id()` for `portfolio_id`
    }

    pub async fn aum_for(
        &self, a: &Access<Nav, View>, date: chrono::NaiveDate,
    ) -> anyhow::Result<Option<f64>> {
        // body of the old aum_for
    }
}
```

Do the same for `position_dates` and `positions_for` in `crates/db/src/repo/positions.rs` with `Access<Positions, View>`.

**Keep the old free functions** for now, delegating is not possible (they have no ctx), so leave their bodies in place — other callers still use them and Task 9 removes them. Duplication here is deliberate and short-lived; add `#[deprecated(note = "migrating to Scoped in Task 9")]` to each so the compiler lists what remains.

- [ ] **Step 5: Write the middleware and router constructors**

Create `crates/server/src/auth/middleware.rs`:

```rust
use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use db::auth::{Action, AuthCtx, Domain};

/// Resolves the principal and inserts an `AuthCtx` extension. Runs for every
/// route, protected or public — a public route may still want to know who is
/// calling, and resolving in one place keeps the two modes identical.
pub async fn resolve_principal(
    State(st): State<AppState>, mut req: Request, next: Next,
) -> Result<Response, AppError> {
    if let Ok(p) = st.identity.authenticate(req.headers()).await {
        req.extensions_mut().insert(AuthCtx {
            principal_id: p.id,
            display_name: p.display_name,
            is_administrator: p.is_administrator,
            grants: p.grants,
        });
    }
    Ok(next.run(req).await)
}

/// Enforces one route's declared primary requirement. Attached per route by
/// `.protected`.
pub async fn require(
    domain: Domain, action: Action, req: Request, next: Next,
) -> Result<Response, AppError> {
    let ctx = req.extensions().get::<AuthCtx>().cloned().ok_or(AppError::Unauthenticated)?;
    let (portfolio, req) = portfolio_id_from_path(req).await;
    let allowed = match portfolio {
        Some(id) => ctx.grants.allows(domain, action, Some(id)),
        None => ctx.grants.allows(domain, action, None),
    };
    if allowed {
        return Ok(next.run(req).await);
    }
    Err(AppError::Forbidden(db::auth::Denied {
        domain,
        action,
        portfolio,
        kind: match portfolio {
            Some(id) if !ctx.grants.any_domain_on(id) => db::auth::DeniedKind::OutOfScope,
            _ => db::auth::DeniedKind::NotGranted,
        },
    }))
}

/// Routes name the portfolio parameter `{id}` throughout.
///
/// `RawPathParams` reads a private extension through `FromRequestParts`, so it
/// cannot be fetched with `extensions().get()` — the request has to be split
/// and reassembled. Returning the request avoids a clone.
async fn portfolio_id_from_path(req: Request) -> (Option<i64>, Request) {
    use axum::extract::{FromRequestParts, RawPathParams};
    let (mut parts, body) = req.into_parts();
    let id = RawPathParams::from_request_parts(&mut parts, &()).await.ok()
        .and_then(|params| {
            params.iter()
                .find(|(k, _)| *k == "id")
                .and_then(|(_, v)| v.parse::<i64>().ok())
        });
    (id, Request::from_parts(parts, body))
}
```

Create `crates/server/src/routes/protect.rs`:

```rust
use crate::auth::middleware::require;
use crate::state::AppState;
use axum::routing::MethodRouter;
use axum::Router;
use db::auth::{Action, Domain};

/// Every route declares itself protected or public. There is no third option,
/// so an endpoint added later cannot quietly ship unguarded.
pub trait ProtectExt {
    fn protected(self, path: &str, mr: MethodRouter<AppState>, domain: Domain, action: Action) -> Self;
    fn public(self, path: &str, mr: MethodRouter<AppState>) -> Self;
}

impl ProtectExt for Router<AppState> {
    fn protected(self, path: &str, mr: MethodRouter<AppState>, domain: Domain, action: Action) -> Self {
        self.route(path, mr.layer(axum::middleware::from_fn(
            move |req, next| require(domain, action, req, next))))
    }

    fn public(self, path: &str, mr: MethodRouter<AppState>) -> Self {
        self.route(path, mr)
    }
}
```

Convert `crates/server/src/routes.rs` to use them. `/api/health`, `/api/login`, `/api/logout`, `/api/me` are `.public`; every other route is `.protected` with the pair from Task 6's table. Attach `resolve_principal` once with `.layer(axum::middleware::from_fn_with_state(state.clone(), resolve_principal))` **after** all routes, so it runs before them.

Add `AppError::Forbidden(db::auth::Denied)` to `error.rs`:

```rust
AppError::Forbidden(d) => match d.kind {
    db::auth::DeniedKind::OutOfScope => (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"title": "Not Found", "status": 404, "detail": "no such portfolio"})),
    ).into_response(),
    db::auth::DeniedKind::NotGranted => (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({"title": "Forbidden", "status": 403,
            "detail": d.reason(), "domain": d.domain.as_str(), "action": d.action.as_str(),
            "portfolio_id": d.portfolio})),
    ).into_response(),
},
```

and `impl From<db::auth::Denied> for AppError { fn from(d: db::auth::Denied) -> Self { AppError::Forbidden(d) } }`.

- [ ] **Step 6: Migrate the `data.rs` handlers**

```rust
use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::{Extension, Json};
use chrono::NaiveDate;
use db::auth::marker::{Nav, Positions, View};
use db::auth::AuthCtx;

pub async fn nav(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(pid): Path<i64>,
) -> Result<Json<Vec<db::repo::NavRow>>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Nav, View>(pid)?;
    Ok(Json(scoped.nav_rows(&a).await?))
}

pub async fn positions(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>,
    Path(pid): Path<i64>, Query(q): Query<PositionsQuery>,
) -> Result<Json<PositionsResponse>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Positions, View>(pid)?;
    let dates = scoped.position_dates(&a).await?;
    let date = match q.date {
        Some(s) => Some(s.parse::<NaiveDate>().map_err(|_| AppError::BadRequest(format!("bad date: {s}")))?),
        None => dates.first().copied(),
    };
    let rows = match date {
        Some(d) => scoped.positions_for(&a, d).await?,
        None => Vec::new(),
    };
    Ok(Json(PositionsResponse { dates, date, rows }))
}
```

The `super::portfolios::ensure` call is replaced by `authorize`: a portfolio that does not exist has no grant, so it is already a 404.

- [ ] **Step 7: Write the endpoint slice test**

Create `crates/server/tests/api_authz_slice.rs`:

```rust
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::util::ServiceExt;
use db::auth::{Action, Domain, Grant};

async fn app() -> (axum::Router, sqlx::PgPool, db::embedded::EmbeddedDb) {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    std::mem::forget(dir);
    let app = server::routes::router(server::state::AppState::server(pool.clone()));
    (app, pool, edb)
}

async fn user_with(pool: &sqlx::PgPool, grants: &[Grant]) -> String {
    let hash = server::auth::local::hash_password("pw").unwrap();
    let admin = db::admin::Admin::new(pool);
    let id = admin.create_user("u@f.lu", "U", &hash, false).await.unwrap();
    for g in grants {
        admin.grant_add(id, *g, None).await.unwrap();
    }
    let token = "t0";
    admin.session_create(&server::auth::local::token_hash(token), id, 1).await.unwrap();
    format!("borobudur_session={token}")
}

async fn portfolio(pool: &sqlx::PgPool, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO portfolios (name, kind) VALUES ($1,'ucits') RETURNING id")
        .bind(name).fetch_one(pool).await.unwrap()
}

async fn get(app: &axum::Router, uri: &str, cookie: Option<&str>) -> StatusCode {
    let mut b = Request::get(uri);
    if let Some(c) = cookie { b = b.header("cookie", c); }
    app.clone().oneshot(b.body(Body::empty()).unwrap()).await.unwrap().status()
}

#[tokio::test]
async fn unauthenticated_requests_are_401() {
    let (app, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    assert_eq!(get(&app, &format!("/api/portfolios/{pid}/nav"), None).await, StatusCode::UNAUTHORIZED);
    edb.stop().await;
}

#[tokio::test]
async fn a_granted_principal_gets_200() {
    let (app, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    let c = user_with(&pool, &[Grant { domain: Domain::Nav, action: Action::View, portfolio: Some(pid) }]).await;
    assert_eq!(get(&app, &format!("/api/portfolios/{pid}/nav"), Some(&c)).await, StatusCode::OK);
    edb.stop().await;
}

#[tokio::test]
async fn a_portfolio_outside_scope_is_404_not_403() {
    let (app, pool, edb) = app().await;
    let mine = portfolio(&pool, "Mine").await;
    let theirs = portfolio(&pool, "Theirs").await;
    let c = user_with(&pool, &[Grant { domain: Domain::Nav, action: Action::View, portfolio: Some(mine) }]).await;
    assert_eq!(get(&app, &format!("/api/portfolios/{theirs}/nav"), Some(&c)).await,
               StatusCode::NOT_FOUND,
               "403 would confirm the fund exists");
    edb.stop().await;
}

#[tokio::test]
async fn a_visible_portfolio_with_a_denied_domain_is_403() {
    let (app, pool, edb) = app().await;
    let pid = portfolio(&pool, "F").await;
    let c = user_with(&pool, &[Grant { domain: Domain::Nav, action: Action::View, portfolio: Some(pid) }]).await;
    assert_eq!(get(&app, &format!("/api/portfolios/{pid}/positions"), Some(&c)).await,
               StatusCode::FORBIDDEN);
    edb.stop().await;
}

#[tokio::test]
async fn desktop_mode_reaches_everything_without_a_cookie() {
    let dir = tempfile::tempdir().unwrap();
    let edb = db::embedded::start(dir.path(), true).await.unwrap();
    let pool = db::connect(&edb.url).await.unwrap();
    std::mem::forget(dir);
    let app = server::routes::router(server::state::AppState::desktop(pool.clone()));
    let pid = portfolio(&pool, "F").await;
    assert_eq!(get(&app, &format!("/api/portfolios/{pid}/nav"), None).await, StatusCode::OK);
    assert_eq!(get(&app, &format!("/api/portfolios/{pid}/positions"), None).await, StatusCode::OK);
    edb.stop().await;
}
```

- [ ] **Step 8: Run both tests**

Run: `timeout 900 cargo test -p db --test authorize && timeout 900 cargo test -p server --test api_authz_slice`
Expected: PASS — 6 and 5 tests.

- [ ] **Step 9: Verify desktop parity**

Run: `timeout 900 cargo test -p server --test api_metrics && timeout 900 cargo test -p server --test api_portfolio_isolation`
Expected: PASS unchanged.

- [ ] **Step 10: Commit**

```bash
git add crates/db/src/auth/access.rs crates/db/src/scoped.rs crates/db/src/auth/mod.rs \
        crates/db/src/lib.rs crates/db/src/repo crates/db/tests/authorize.rs \
        crates/server/src/auth crates/server/src/routes crates/server/src/routes.rs \
        crates/server/src/handlers/data.rs crates/server/src/error.rs \
        crates/server/tests/api_authz_slice.rs
git commit -m "feat: scoped db handle, typed access token, and protected routes

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 9: Migrate the remaining domains and handlers

**Files:**
- Modify: every module under `crates/db/src/repo/`, `crates/db/src/settings.rs`, and all 11 files under `crates/server/src/handlers/`
- Test: existing suites, plus `crates/server/tests/api_authz_matrix.rs`

**Interfaces:**
- Consumes: everything from Task 8.
- Produces: every repository function is a `Scoped` method taking `Access<D, A>` or `GlobalAccess<D, A>`; no free function in `db::repo` or `db::settings` takes a `PgPool`. `Scoped::portfolios_list(&self)` returns only portfolios the principal can see under any domain.

- [ ] **Step 1: Convert the remaining repo modules**

Apply the exact pattern from Task 8 Step 4 to every function in the Task 6 table, using the domain and action given there. Portfolio-scoped functions take `&Access<D, A>` and read the id from it; instance-wide functions take `&GlobalAccess<D, A>` and keep their existing parameters.

Two functions need more than a mechanical change:

**`portfolios_list`** must filter rather than authorize — it answers "what may I see", so a denial is not an error:

```rust
impl<'a> Scoped<'a> {
    pub async fn portfolios_list(&self) -> anyhow::Result<Vec<Portfolio>> {
        let all = /* the existing query */;
        Ok(match self.ctx.grants.visible_portfolios() {
            crate::auth::PortfolioScope::All => all,
            crate::auth::PortfolioScope::Only(ids) =>
                all.into_iter().filter(|p| ids.contains(&p.id)).collect(),
        })
    }
}
```

**`import_batch` / `import_workbook`** write positions, NAV and transactions in one transaction, so they require all three:

```rust
pub async fn import_batch(
    &self,
    positions: &Access<marker::Positions, marker::Import>,
    nav: &Access<marker::Nav, marker::Import>,
    transactions: &Access<marker::Transactions, marker::Import>,
    filename: &str, sha256: &str, b: &ingest::adapter::UniversalBatch,
) -> anyhow::Result<ImportOutcome>
```

All three tokens carry the same portfolio id; assert it in a debug assertion. When the ingest adapter later declares its own domain list, this signature is what that declaration resolves to.

- [ ] **Step 2: Convert the handlers**

For each of the 11 handler files, add `Extension(ctx): Extension<AuthCtx>`, build `let scoped = st.db.scope(&ctx);`, replace each `db::repo::f(&st.pool, pid, …)` with `scoped.f(&scoped.authorize::<D, A>(pid)?, …)`, and delete the now-redundant `super::portfolios::ensure` calls. Export endpoints (`/emir/export`, the ADV export) authorize `Action::Export`, not `Action::View`.

- [ ] **Step 3: Delete the deprecated free functions**

Remove every `#[deprecated]` free function left by Task 8 and every remaining `pub async fn …(pool: &PgPool, …)` in `db::repo` and `db::settings`.

Run: `timeout 900 cargo build -p db -p server`
Expected: builds clean. Any error is a caller that still wants the pool — convert it, do not re-export the pool.

- [ ] **Step 4: Write the endpoint matrix test**

Create `crates/server/tests/api_authz_matrix.rs`. Reuse the helpers from `api_authz_slice.rs` (copy them — this crate has no shared `tests/common` module and every `api_*.rs` file inlines its own setup) and drive a table:

```rust
struct Case { uri: &'static str, domain: Domain, action: Action }

const CASES: &[Case] = &[
    Case { uri: "/api/portfolios/{pid}/nav",                    domain: Domain::Nav,          action: Action::View },
    Case { uri: "/api/portfolios/{pid}/positions",              domain: Domain::Positions,    action: Action::View },
    Case { uri: "/api/portfolios/{pid}/metrics/summary",        domain: Domain::Nav,          action: Action::View },
    Case { uri: "/api/portfolios/{pid}/metrics/rolling",        domain: Domain::Nav,          action: Action::View },
    Case { uri: "/api/portfolios/{pid}/metrics/drawdowns",      domain: Domain::Nav,          action: Action::View },
    Case { uri: "/api/portfolios/{pid}/metrics/calendar",       domain: Domain::Nav,          action: Action::View },
    Case { uri: "/api/portfolios/{pid}/metrics/var",            domain: Domain::Nav,          action: Action::View },
    Case { uri: "/api/portfolios/{pid}/metrics/backtest",       domain: Domain::Nav,          action: Action::View },
    Case { uri: "/api/portfolios/{pid}/metrics/concentration",  domain: Domain::Positions,    action: Action::View },
    Case { uri: "/api/portfolios/{pid}/metrics/liquidity",      domain: Domain::Positions,    action: Action::View },
    Case { uri: "/api/portfolios/{pid}/metrics/rates",          domain: Domain::Positions,    action: Action::View },
    Case { uri: "/api/portfolios/{pid}/metrics/derivatives",    domain: Domain::Positions,    action: Action::View },
    Case { uri: "/api/portfolios/{pid}/pnl",                    domain: Domain::Positions,    action: Action::View },
    Case { uri: "/api/portfolios/{pid}/emir",                   domain: Domain::Positions,    action: Action::View },
    Case { uri: "/api/portfolios/{pid}/emir/export",            domain: Domain::Positions,    action: Action::Export },
    Case { uri: "/api/portfolios/{pid}/shareholders",           domain: Domain::Shareholders, action: Action::View },
    Case { uri: "/api/portfolios/{pid}/flows",                  domain: Domain::Shareholders, action: Action::View },
    Case { uri: "/api/portfolios/{pid}/settings",               domain: Domain::Reference,    action: Action::View },
    Case { uri: "/api/portfolios/{pid}/imports",                domain: Domain::Reference,    action: Action::View },
    Case { uri: "/api/portfolios/{pid}/futures-analytics",      domain: Domain::MarketData,   action: Action::View },
];
```

Four tests iterate `CASES`: no cookie → 401; the exact grant → not 401/403/404; a grant on a different portfolio → 404; a grant on a different domain (use `Domain::Reference` when the case is not `Reference`, else `Domain::Nav`) → 403.

- [ ] **Step 5: Run the matrix and the full server suite set**

```bash
timeout 900 cargo test -p server --test api_authz_matrix
for t in api_bloomberg api_bloomberg_adv api_derivatives api_emir api_futures api_imports \
         api_ingest_routing api_limits api_liquidity_v2 api_metrics api_pnl \
         api_portfolio_isolation api_portfolios api_rates_futures api_refs api_settings; do
  echo "== $t"; timeout 900 cargo test -p server --test $t 2>&1 | tail -3
done
```

Expected: the matrix passes; every existing suite passes with the counts recorded in Task 6 Step 1.

- [ ] **Step 6: Commit**

```bash
git add crates/db/src crates/server/src crates/server/tests/api_authz_matrix.rs
git commit -m "refactor: every repository query now requires an access token

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 10: Close the wall — privatise the pool and prove it

**Files:**
- Modify: `crates/db/src/lib.rs`, `crates/db/Cargo.toml`
- Create: `crates/db/tests/compile_fail.rs`, `crates/db/tests/ui/no_authctx.rs`, `crates/db/tests/ui/no_authctx.stderr`, `crates/db/tests/ui/wrong_domain.rs`, `crates/db/tests/ui/wrong_domain.stderr`, `crates/db/tests/ui/wrong_action.rs`, `crates/db/tests/ui/wrong_action.stderr`, `crates/db/tests/admin_isolation.rs`
- Test: as above

**Interfaces:**
- Consumes: everything from Tasks 8 and 9.
- Produces: `db::connect` no longer public; `db::Db::connect` is the only entry point. Test helpers use `db::Db::connect(&edb.url)`.

- [ ] **Step 1: Add trybuild**

In `crates/db/Cargo.toml` `[dev-dependencies]`: `trybuild = "1"`.

- [ ] **Step 2: Write the compile-fail cases**

`crates/db/tests/compile_fail.rs`:

```rust
/// The typed-token guarantee is only real if a regression cannot silently undo
/// it. These cases must fail to compile; if one starts compiling, someone has
/// re-exposed the pool or loosened a signature.
#[test]
fn unscoped_and_mistyped_queries_do_not_compile() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/no_authctx.rs");
    t.compile_fail("tests/ui/wrong_domain.rs");
    t.compile_fail("tests/ui/wrong_action.rs");
}
```

`crates/db/tests/ui/no_authctx.rs`:

```rust
fn main() {
    let db: db::Db = unimplemented!();
    // No AuthCtx: there is no constructor for Scoped and no accessible pool.
    let _ = db.pool();
}
```

`crates/db/tests/ui/wrong_domain.rs`:

```rust
use db::auth::marker::{Nav, Positions, View};
use db::auth::Access;
use db::scoped::Scoped;

async fn f(s: Scoped<'_>, nav_token: Access<Nav, View>) {
    // A NAV authorization must not open a positions query.
    let _ = s.positions_for(&nav_token, chrono::NaiveDate::MIN).await;
    let _: Access<Positions, View>;
}

fn main() {}
```

`crates/db/tests/ui/wrong_action.rs`:

```rust
use db::auth::marker::{Shareholders, View};
use db::auth::Access;
use db::scoped::Scoped;

async fn f(s: Scoped<'_>, view_token: Access<Shareholders, View>) {
    // Writing requires an Import token, not a View token.
    let _ = s.shareholders_replace(&view_token, &[]).await;
}

fn main() {}
```

Generate the `.stderr` files by running the test once with `TRYBUILD=overwrite`, then read each one and confirm the error is the intended one (a private method / a type mismatch), not an unrelated failure such as a missing import.

- [ ] **Step 3: Privatise `connect` and update test helpers**

In `crates/db/src/lib.rs` change `pub async fn connect` to `pub(crate) async fn connect`, and make `Db::pool` `pub(crate)` (it already is) — confirm nothing outside the crate references either.

Every test that calls `db::connect(&edb.url)` becomes `db::Db::connect(&edb.url)`. Where a test also needs a raw pool for seeding SQL, add a **test-only** accessor gated so it cannot be used in production code:

```rust
impl Db {
    /// Seeding helper for integration tests only. Not compiled into a release
    /// build, so it cannot become a production escape hatch.
    #[cfg(any(test, feature = "test-util"))]
    pub fn test_pool(&self) -> &PgPool {
        &self.pool
    }
}
```

Add `test-util = []` to `crates/db/Cargo.toml` `[features]`, and `db = { path = "../db", features = ["test-util"] }` under `crates/server/[dev-dependencies]`.

- [ ] **Step 4: Write the admin isolation test**

`crates/db/tests/admin_isolation.rs`:

```rust
/// `db::admin` is the one privileged path. It exists because loading a
/// principal's grants is what builds the AuthCtx. Nothing else may use it.
#[test]
fn only_identity_grants_and_audit_reach_the_privileged_path() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../server/src");
    let allowed = ["auth/local.rs", "auth/desktop.rs", "auth/middleware.rs",
                   "handlers/session.rs", "handlers/admin.rs", "state.rs", "startup.rs"];
    let mut offenders = Vec::new();
    for entry in walk(&src) {
        let rel = entry.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
        if allowed.contains(&rel.as_str()) { continue; }
        let text = std::fs::read_to_string(&entry).unwrap();
        if text.contains("db::admin") || text.contains(".admin()") {
            offenders.push(rel);
        }
    }
    assert!(offenders.is_empty(),
        "these files reach the privileged path and should not: {offenders:?}");
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() { out.extend(walk(&p)); }
        else if p.extension().is_some_and(|x| x == "rs") { out.push(p); }
    }
    out
}
```

- [ ] **Step 5: Run the tests**

Run: `timeout 900 cargo test -p db --test compile_fail && timeout 900 cargo test -p db --test admin_isolation`
Expected: PASS.

- [ ] **Step 6: Run every db and server suite once more**

Use the two loops from Task 6 Step 1 and Task 9 Step 5.
Expected: all pass with unchanged counts.

- [ ] **Step 7: Commit**

```bash
git add crates/db/Cargo.toml crates/db/src/lib.rs crates/db/tests crates/server/Cargo.toml
git add -u crates
git commit -m "feat(db): close the pool behind the scoped handle, with compile-fail proof

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 11: Component-level denial in composite results

**Files:**
- Modify: `crates/server/src/handlers/limits.rs`, `crates/server/src/handlers/pnl.rs`, `crates/server/src/handlers/emir.rs`
- Test: `crates/server/tests/api_partial_denial.rs`

**Interfaces:**
- Consumes: `Scoped::may<D, A>`, `Denied::reason()`.
- Produces: no new types — composite endpoints emit the existing `{"status": "unavailable", "reason": "…"}` shape for components whose domains are denied.

- [ ] **Step 1: Write the failing test**

Create `crates/server/tests/api_partial_denial.rs` (reuse the `app`, `user_with`, `portfolio` helpers from `api_authz_slice.rs`):

```rust
#[tokio::test]
async fn liquidity_computes_the_asset_side_and_marks_the_liability_side_unavailable() {
    // Grants: positions + market_data, but NOT shareholders.
    // GET /metrics/liquidity -> 200
    // body["asset_side"]["status"] != "unavailable"
    // body["redemption_scenarios"]["status"] == "unavailable"
    // body["redemption_scenarios"]["reason"] == "not permitted: shareholder register"
}

#[tokio::test]
async fn concentration_is_unavailable_rather_than_a_pass_when_positions_are_denied() {
    // Grants: nav only. GET /metrics/concentration -> 403 at the route gate.
    // Then grant positions/view on a DIFFERENT portfolio and re-check the same
    // portfolio: still 403, and no computed result is ever produced.
}

#[tokio::test]
async fn a_denied_component_reason_is_distinguishable_from_missing_data() {
    // Same portfolio, two runs:
    //  (a) shareholders granted but the register never loaded
    //      -> reason == "no shareholder register"
    //  (b) shareholders denied
    //      -> reason == "not permitted: shareholder register"
    // assert_ne! the two reasons.
}
```

Fill each body using the helper functions; the assertions above are the contract.

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 900 cargo test -p server --test api_partial_denial`
Expected: FAIL.

- [ ] **Step 3: Implement**

In `limits.rs`'s liquidity handler, replace the direct shareholder read with:

```rust
let liability = if scoped.may::<Shareholders, View>(pid) {
    let sh = scoped.authorize::<Shareholders, View>(pid)?;
    compute_redemption_scenarios(&scoped, &sh, /* … */).await?
} else {
    unavailable(&Denied {
        domain: Domain::Shareholders, action: Action::View,
        portfolio: Some(pid), kind: DeniedKind::NotGranted,
    }.reason())
};
```

where `unavailable(reason)` builds the same JSON shape the liquidity feature already uses for missing data. **Do not filter positions.** If `positions` is denied the route gate has already returned 403; there is no code path that computes a limit over a subset.

Apply the same treatment to any other composite result reading a second domain — the P&L attribution's transaction detail and the EMIR counterparty view.

- [ ] **Step 4: Run test to verify it passes**

Run: `timeout 900 cargo test -p server --test api_partial_denial`
Expected: PASS, 3 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/handlers crates/server/tests/api_partial_denial.rs
git commit -m "feat(server): denied components degrade to unavailable, never to a pass

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 12: Audit writes

**Files:**
- Create: `crates/server/src/audit.rs`
- Modify: `crates/server/src/handlers/*.rs` (export, import and configure endpoints), `crates/server/src/handlers/session.rs`, `crates/server/src/lib.rs`
- Test: `crates/server/tests/api_audit.rs`

**Interfaces:**
- Consumes: `db::admin::AuditEvent`, `AuthCtx`.
- Produces: `server::audit::record(&AppState, &AuthCtx, action: &str, domain: Option<Domain>, portfolio_id: Option<i64>, detail: serde_json::Value)` — fire-and-log; an audit failure is logged at `error!` and never fails the request that succeeded.

- [ ] **Step 1: Write the failing test**

Create `crates/server/tests/api_audit.rs`:

```rust
// Helpers as in api_authz_slice.rs.

#[tokio::test]
async fn an_export_writes_one_audit_row() { /* GET /emir/export with export grant -> 1 row, action == "export" */ }

#[tokio::test]
async fn a_view_writes_nothing() { /* GET /nav with view grant -> 0 rows */ }

#[tokio::test]
async fn a_settings_change_records_before_and_after() {
    // PUT /settings with configure grant -> 1 row, action == "configure",
    // detail["before"]["var_limit"] != detail["after"]["var_limit"]
}

#[tokio::test]
async fn an_import_writes_a_row_tied_to_the_import_ledger() {
    // POST /imports -> 1 row, action == "import", detail["import_id"] is a number
}

#[tokio::test]
async fn login_success_failure_and_lockout_are_all_recorded() {
    // 1 wrong password + 1 right password -> rows for "login_failed" and "login"
    // 5 wrong -> a row for "login_locked"
}

#[tokio::test]
async fn a_grant_change_records_who_granted_it() {
    // administrator adds a grant -> row action == "grant_added",
    // detail["domain"], detail["action"], detail["target_user_id"] present
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 900 cargo test -p server --test api_audit`
Expected: FAIL.

- [ ] **Step 3: Implement**

`crates/server/src/audit.rs`:

```rust
use crate::state::AppState;
use db::admin::AuditEvent;
use db::auth::{AuthCtx, Domain};

/// The request already succeeded when this is called. A failure to write the
/// audit row must not undo it — log loudly and carry on, because losing the
/// user's work to protect the log is the wrong trade.
pub async fn record(
    st: &AppState, ctx: &AuthCtx, action: &str,
    domain: Option<Domain>, portfolio_id: Option<i64>, detail: serde_json::Value,
) {
    let event = AuditEvent {
        user_id: (ctx.principal_id != 0).then_some(ctx.principal_id),
        actor_label: ctx.display_name.clone(),
        action: action.to_string(),
        domain,
        portfolio_id,
        detail,
        source_addr: None,
    };
    if let Err(e) = st.db.admin().audit_append(event).await {
        tracing::error!("audit write failed for {action}: {e:#}");
    }
}
```

Call it from: every export handler, every import handler (with `import_id` in the detail), every `configure` handler (with `before` and `after`), the login handler (`login`, `login_failed`, `login_locked`), and the grant administration handlers (`grant_added`, `grant_removed`, `role_assigned`, `password_reset`). Do not call it from any read path.

- [ ] **Step 4: Run test to verify it passes**

Run: `timeout 900 cargo test -p server --test api_audit`
Expected: PASS, 6 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/audit.rs crates/server/src/handlers crates/server/src/lib.rs crates/server/tests/api_audit.rs
git commit -m "feat(server): audit exports, imports, configuration and auth events

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 13: Administration endpoints and first-administrator enrolment

**Files:**
- Create: `crates/server/src/handlers/admin.rs`, `crates/server/src/startup.rs`
- Modify: `crates/server/src/routes.rs`, `crates/server/src/handlers/mod.rs`, `crates/server/src/lib.rs`, `crates/server/src/main.rs`
- Test: `crates/server/tests/api_admin.rs`, `crates/server/tests/enrolment.rs`

**Interfaces:**
- Consumes: `db::admin::Admin`, `Role`, `Grant`, `server::audit::record`.
- Produces:
  - Routes, all requiring `ctx.is_administrator`: `GET/POST /api/admin/users`, `PUT /api/admin/users/{id}/password`, `PUT /api/admin/users/{id}/disabled`, `GET/POST/DELETE /api/admin/users/{id}/grants`, `POST /api/admin/users/{id}/roles`, `GET /api/admin/audit`.
  - `POST /api/enrol` — completes first-administrator enrolment with the single-use token.
  - `server::startup::ensure_first_administrator(&db::Db, admin_email: &str) -> anyhow::Result<Option<String>>` returning the enrolment token when one was issued.

- [ ] **Step 1: Write the failing tests**

`crates/server/tests/enrolment.rs`:

```rust
#[tokio::test]
async fn an_empty_server_issues_a_single_use_enrolment_token() {
    // ensure_first_administrator(&db, "risk@firm.lu") -> Some(token)
    // The user exists, is_administrator, and cannot log in until enrolled.
}

#[tokio::test]
async fn enrolment_sets_the_password_and_consumes_the_token() {
    // POST /api/enrol {token, password} -> 204; login with that password -> 200
    // POST /api/enrol with the same token again -> 401
}

#[tokio::test]
async fn a_server_that_already_has_users_issues_nothing() {
    // create a user first; ensure_first_administrator -> None
}

#[tokio::test]
async fn desktop_mode_never_enrols() {
    // AppState::desktop -> POST /api/enrol is 404
}
```

`crates/server/tests/api_admin.rs`:

```rust
#[tokio::test]
async fn a_non_administrator_cannot_reach_any_admin_route() { /* each route -> 403 */ }

#[tokio::test]
async fn an_administrator_creates_a_user_and_grants_them_one_portfolio() {
    // POST /api/admin/users, POST .../grants
    // the new user then gets 200 on that portfolio's nav and 404 on another
}

#[tokio::test]
async fn assigning_a_role_writes_its_expanded_grants() {
    // POST .../roles {role: "auditor"} -> GET .../grants lists view on all six domains
}

#[tokio::test]
async fn disabling_a_user_revokes_their_live_session_immediately() {
    // user logs in, PUT .../disabled {true}, their next request -> 401
}

#[tokio::test]
async fn the_audit_route_returns_newest_first() { /* GET /api/admin/audit */ }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `timeout 900 cargo test -p server --test enrolment && timeout 900 cargo test -p server --test api_admin`
Expected: FAIL.

- [ ] **Step 3: Implement**

`crates/server/src/startup.rs`:

```rust
/// On first start in server mode with an empty users table, create one
/// administrator and return a single-use enrolment token. No default password
/// exists at any point, so there is nothing to forget to change.
pub async fn ensure_first_administrator(
    db: &db::Db, admin_email: &str,
) -> anyhow::Result<Option<String>> {
    let admin = db.admin();
    if admin.user_count().await? > 0 {
        return Ok(None);
    }
    let token = /* 32 random bytes, hex */;
    // The account is created with an unusable password hash so it cannot be
    // logged into, and the enrolment token is stored in `sessions` with a
    // one-hour expiry — reusing the session table means expiry and single use
    // are already implemented.
    Ok(Some(token))
}
```

Wire it into `main.rs`: in `Mode::Server`, if `cfg.admin_email` is set, call it and `tracing::info!` the token on its own line with instructions. Disabling a user must also call `sessions_delete_for`.

Every admin handler starts with an administrator check and ends with `audit::record`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `timeout 900 cargo test -p server --test enrolment && timeout 900 cargo test -p server --test api_admin`
Expected: PASS, 4 and 5 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/handlers/admin.rs crates/server/src/startup.rs crates/server/src/routes.rs \
        crates/server/src/handlers/mod.rs crates/server/src/lib.rs crates/server/src/main.rs \
        crates/server/tests/api_admin.rs crates/server/tests/enrolment.rs
git commit -m "feat(server): administration endpoints and first-administrator enrolment

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 14: Frontend — login, session handling and denial rendering

**Files:**
- Create: `frontend/src/auth.ts`, `frontend/src/pages/LoginPage.tsx`, `frontend/src/components/Unavailable.tsx`
- Modify: `frontend/src/api.ts`, `frontend/src/App.tsx`, `frontend/src/index.css`

**Interfaces:**
- Consumes: `GET /api/me`, `POST /api/login`, `POST /api/logout`.
- Produces: `Me { display_name, is_administrator, capabilities }` and `can(me, domain, action, portfolioId): boolean` in `auth.ts`; `<Unavailable reason={…} />`; `ApiError` gains `status: number`.

- [ ] **Step 1: Extend the API client**

In `frontend/src/api.ts`, give `ApiError` a `status` field and populate it in `req`. In the same place, map a 403 body into an `ApiError` carrying `detail` as the reason so callers can render it with the `unavailable` treatment. A 401 dispatches a `borobudur:unauthenticated` window event and rejects.

- [ ] **Step 2: Write the auth module**

`frontend/src/auth.ts`: `fetchMe()`, `login(email, password)`, `logout()`, and

```ts
export function can(me: Me | null, domain: string, action: string, portfolioId?: number): boolean {
  if (!me) return false;
  return me.capabilities.some(c =>
    c.domain === domain && c.action === action &&
    (c.portfolio_id === null || c.portfolio_id === portfolioId));
}
```

- [ ] **Step 3: Gate the shell**

In `App.tsx`: fetch `/api/me` once at mount. While loading, render nothing. On 401, render `<LoginPage />`. On success, provide `me` through context, hide nav links the user cannot view anywhere, and listen for `borobudur:unauthenticated` to return to the login screen **without losing the current URL**, so re-authenticating lands the user back where they were.

- [ ] **Step 4: Render denials**

`<Unavailable reason={…} />` renders the same visual treatment already used for `status: "unavailable"` components. Every page that consumes a composite result routes both a component-level `unavailable` and a caught 403 `ApiError` through it — one rendering path, two wire behaviours.

- [ ] **Step 5: Build and verify**

Run: `cd frontend && npm run build`
Expected: builds with no TypeScript errors.

Then run the app in desktop mode and confirm no login screen appears and every tab renders as before:
Run: `timeout 120 cargo run -p server` and open `http://127.0.0.1:8787`.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/auth.ts frontend/src/pages/LoginPage.tsx frontend/src/components/Unavailable.tsx \
        frontend/src/api.ts frontend/src/App.tsx frontend/src/index.css
git commit -m "feat(frontend): login, session handling and permission-denial rendering

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 15: Frontend — administration screens

**Files:**
- Create: `frontend/src/pages/AdminPage.tsx`, `frontend/src/components/GrantEditor.tsx`, `frontend/src/components/AuditLog.tsx`
- Modify: `frontend/src/App.tsx`, `frontend/src/api.ts`

**Interfaces:**
- Consumes: the `/api/admin/*` routes from Task 13; `can()` from Task 14.
- Produces: an `/admin` route rendered only when `me.is_administrator`.

- [ ] **Step 1: User list and creation**

A table of users with display name, email, administrator flag and disabled state; a form to create one; per-user actions to reset a password and to disable. Creating a user shows the generated password once and never again.

- [ ] **Step 2: Grant editor**

A matrix of six domains against four actions for a chosen scope — a named portfolio or "all portfolios" — with checkboxes that POST and DELETE individual grants. Show `view` as automatically checked and disabled whenever `export`, `import` or `configure` is checked, so the implication is visible rather than surprising.

- [ ] **Step 3: Role assignment**

A dropdown of the four roles plus a scope selector, and an "apply" action. Next to each role, text stating plainly that applying it writes grants now and that later edits to the role do not reach users already assigned it.

- [ ] **Step 4: Audit log**

A newest-first table of the most recent 200 events: time, actor, action, domain, portfolio, detail. Read-only; there is no delete control because there is no delete endpoint.

- [ ] **Step 5: Build and verify**

Run: `cd frontend && npm run build`
Expected: clean build.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/pages/AdminPage.tsx frontend/src/components/GrantEditor.tsx \
        frontend/src/components/AuditLog.tsx frontend/src/App.tsx frontend/src/api.ts
git commit -m "feat(frontend): administration screens for users, grants, roles and audit

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 16: Full verification sweep

**Files:** none — this task runs and records.

- [ ] **Step 1: Every db suite**

```bash
for t in grant_model roles auth_migration admin_queries authorize compile_fail admin_isolation \
         derive_dividends emir_kpis futures_analytics futures_contracts futures_seeding \
         import_batch import_workbook instrument_refs liquidity_v2_repo pam_check \
         pnl_repo portfolio_codes settings_roundtrip settings_v2; do
  echo "== $t"; timeout 900 cargo test -p db --test $t 2>&1 | tail -3
done
```

- [ ] **Step 2: Every server suite**

```bash
for t in config api_session api_authz_slice api_authz_matrix api_partial_denial api_audit \
         api_admin enrolment api_bloomberg api_bloomberg_adv api_derivatives api_emir \
         api_futures api_imports api_ingest_routing api_limits api_liquidity_v2 api_metrics \
         api_pnl api_portfolio_isolation api_portfolios api_rates_futures api_refs api_settings; do
  echo "== $t"; timeout 900 cargo test -p server --test $t 2>&1 | tail -3
done
```

- [ ] **Step 3: Frontend build and clippy**

```bash
cd frontend && npm run build && cd ..
timeout 900 cargo clippy -p db -p server --all-targets -- -D warnings
```

- [ ] **Step 4: Desktop smoke test**

Run `cargo run -p server` with no environment variables. Confirm: embedded PostgreSQL starts, the browser opens, no login screen appears, and Overview, Performance, P&L, Risk, VaR, Limits, Derivatives and Data all render.

- [ ] **Step 5: Server smoke test**

Start a PostgreSQL instance, then run with `BOROBUDUR_DATABASE_URL`, `BOROBUDUR_BIND=127.0.0.1:8788` and `BOROBUDUR_ADMIN_EMAIL`. Confirm: no browser opens, the enrolment token prints once, enrolment sets a password, login works, and an unauthenticated request to any `/api/portfolios/...` route returns 401.

- [ ] **Step 6: Report**

State the pass counts per suite, and name anything that did not pass. Do not claim completion without the output.

- [ ] **Step 7: Clean up leftover temp directories**

Force-killed embedded PostgreSQL runs abandon `.tmp*` directories that eventually make later test runs time out. Sweep them:

```powershell
$before = (Get-ChildItem $env:TEMP -Filter ".tmp*" -Directory -ErrorAction SilentlyContinue).Count
Get-ChildItem $env:TEMP -Filter ".tmp*" -Directory -ErrorAction SilentlyContinue |
  Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
$after = (Get-ChildItem $env:TEMP -Filter ".tmp*" -Directory -ErrorAction SilentlyContinue).Count
"before=$before after=$after"
```

---

## Self-Review

**Spec coverage.** Every section of the spec maps to a task: deployment modes and TLS exclusion → Task 1; identity seam and both providers → Task 7; the six domains, four actions, scope rule and additive-only grants → Task 2; roles as templates → Tasks 3 and 15; storage → Task 4; the privileged bootstrap path and its isolation test → Tasks 5 and 10; the `repo.rs` split → Task 6; the scoped handle, typed token and router constructors → Tasks 8 and 9; the four failure kinds → Tasks 8 and 9; the partial-denial safety rule and reason-channel separation → Task 11; login throttling → Tasks 5 and 7; the audit log's recorded and not-recorded sets → Task 12; first-administrator enrolment → Task 13; `GET /api/me` navigation → Task 14; desktop parity and per-crate bounded test execution → Tasks 6, 9 and 16.

**One correctness trap found while reviewing, now designed out.** `error.rs`'s
blanket `impl<E: Into<anyhow::Error>> From<E> for AppError` maps everything to a
500. Had `Denied` implemented `std::error::Error`, every `?` on an authorization
result would have produced an Internal Server Error instead of a 403 or 404 —
the permission model would have appeared to work in unit tests and failed
silently over HTTP. Task 8 therefore states that `Denied` must not implement
`std::error::Error`, and Task 7 replaces the blanket impl with three explicit
conversions.

**Deliberate deviations from the spec, both noted here rather than left to discovery.** The spec says a route declares one `(domain, action)` pair; Task 9's `import_batch` needs three tokens because one transaction writes three domains, which the spec anticipated in the `import` action's description but did not spell out as a signature. And `Db::test_pool` in Task 10 is a feature-gated accessor the spec does not mention; it exists because integration tests seed with raw SQL, and it is gated so it cannot be reached from a release build.

**Known thin spots.** Tasks 11, 12, 13 and 15 give test contracts and assertion lists rather than complete test bodies, because the exact JSON shapes depend on the composite responses those handlers already return and on the admin routes defined one task earlier. Their assertions are stated precisely enough to write against; an implementer who cannot satisfy one should report rather than weaken it.
