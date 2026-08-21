pub mod protect;

use crate::auth::middleware::resolve_principal;
use crate::config::Mode;
use crate::handlers;
use crate::routes::protect::ProtectExt;
use crate::state::AppState;
use axum::routing::get;
use axum::Router;
use db::auth::{Action, Domain};

pub fn router(state: AppState) -> Router {
    let mode = state.mode;
    let router = Router::new()
        .public("/api/health", get(|| async { axum::Json(serde_json::json!({"status": "ok"})) }))
        .public("/api/login", axum::routing::post(handlers::session::login))
        .public("/api/logout", axum::routing::post(handlers::session::logout))
        .public("/api/me", get(handlers::session::me))
        .admin("/api/admin/users", get(handlers::admin::users_list).post(handlers::admin::users_create))
        .admin("/api/admin/users/{id}/password", axum::routing::put(handlers::admin::password_set))
        .admin("/api/admin/users/{id}/disabled", axum::routing::put(handlers::admin::disabled_set))
        .admin("/api/admin/users/{id}/grants", get(handlers::admin::grants_list)
            .post(handlers::admin::grant_add).delete(handlers::admin::grant_remove))
        .admin("/api/admin/users/{id}/roles", axum::routing::post(handlers::admin::role_assign))
        .admin("/api/admin/audit", get(handlers::admin::audit_list))
        .protected_global("/api/refs", get(handlers::refs::list), Domain::Reference, Action::View)
        .protected_global("/api/refs/{code}", axum::routing::put(handlers::refs::put), Domain::Reference, Action::Configure)
        .protected_global("/api/futures-contracts", get(handlers::futures::contracts), Domain::Reference, Action::View)
        .protected_global("/api/futures-contracts/{root}", axum::routing::put(handlers::futures::put_contract), Domain::Reference, Action::Configure)
        // `request`/`adv-request` iterate every portfolio's positions and NAV
        // to build a fleet-wide holdings workbook — gating them on
        // `market_data` alone would let an instance-wide market-data grant
        // with zero positions grants extract every fund's holdings. Gated on
        // `positions` instead; the reference/market_data data they also read
        // is secondary — Task 11's worklist.
        .protected_global("/api/bloomberg/request", get(handlers::bloomberg::request), Domain::Positions, Action::Export)
        .protected_global("/api/bloomberg/adv-request", get(handlers::bloomberg::adv_request), Domain::Positions, Action::Export)
        // adv-due's counts are also derived from fleet-wide position data
        // even though the primary gate stays `market_data` (it discloses
        // counts only, not the holdings themselves); that extra domain isn't
        // gated here — Task 11's secondary-domain worklist.
        .protected_global("/api/bloomberg/adv-due", get(handlers::bloomberg::adv_due), Domain::MarketData, Action::View)
        // `upload`'s fx-drift cross-check also walks every portfolio's
        // positions (Positions, secondary here). A portfolio the caller
        // cannot see is now named in `fx_check_skipped` rather than simply
        // contributing nothing to `fx_check` — an empty result there used to
        // read identically to "checked the fleet, no drift found" (Task 11).
        .protected_global("/api/bloomberg/upload", axum::routing::post(handlers::bloomberg::upload), Domain::MarketData, Action::Import)
        // Filters, not authorizes: any authenticated principal may call it,
        // and `Scoped::portfolios_list` narrows the result to what their
        // grants actually cover (`PortfolioScope::All` vs `Only(ids)`).
        .authenticated("/api/portfolios", get(handlers::portfolios::list))
        // Portfolio lifecycle (create, rename, archive) stays on Reference —
        // deliberately, per the P10 decision. `Settings` took the
        // per-portfolio *configuration* half (a fund's risk parameters, its
        // depositary code mapping, its EMIR KPI records, its import ledger);
        // deciding that a fund exists at all is a different act, and it stays
        // where it was.
        .protected_global("/api/portfolios", axum::routing::post(handlers::portfolios::create), Domain::Reference, Action::Configure)
        .protected("/api/portfolios/{id}", axum::routing::put(handlers::portfolios::update), Domain::Reference, Action::Configure)
        .protected("/api/portfolios/{id}/codes", get(handlers::portfolios::codes_list), Domain::Settings, Action::View)
        .protected("/api/portfolios/{id}/codes", axum::routing::put(handlers::portfolios::codes_put), Domain::Settings, Action::Configure)
        .protected("/api/portfolios/{id}/shareholders", get(handlers::portfolios::shareholders_list), Domain::Shareholders, Action::View)
        .protected("/api/portfolios/{id}/shareholders", axum::routing::put(handlers::portfolios::shareholders_put), Domain::Shareholders, Action::Import)
        .protected("/api/portfolios/{id}/flows", get(handlers::portfolios::flows), Domain::Shareholders, Action::View)
        .protected("/api/portfolios/{id}/settings", get(handlers::settings::get), Domain::Settings, Action::View)
        .protected("/api/portfolios/{id}/settings", axum::routing::put(handlers::settings::put), Domain::Settings, Action::Configure)
        .protected("/api/portfolios/{id}/imports", get(handlers::imports::list), Domain::Settings, Action::View)
        // `import_batch`/`import_workbook` write across positions, nav and
        // transactions at once (Task 6's table: "multi"). `positions` on
        // the URL `{id}` is only a coarse route-level pre-filter — the core
        // holdings picture every ingest adapter writes. Self-identifying
        // (CACEIS) files resolve their own target portfolio via
        // `portfolio_by_code` inside the handler and can land somewhere
        // other than the URL portfolio; the handler separately authorizes
        // Positions/Nav/Transactions Import against that RESOLVED
        // portfolio — not just the URL `{id}` — before reading its row at
        // all, so a principal outside a file's resolved target learns
        // neither its name nor whether it exists (task-9 review round 1;
        // see the ordering comment in handlers/imports.rs::import_one).
        .protected("/api/portfolios/{id}/imports", axum::routing::post(handlers::imports::upload), Domain::Positions, Action::Import)
        .protected("/api/portfolios/{id}/nav", get(handlers::data::nav), Domain::Nav, Action::View)
        .protected("/api/portfolios/{id}/positions", get(handlers::data::positions), Domain::Positions, Action::View)
        .protected("/api/portfolios/{id}/metrics/summary", get(handlers::metrics::summary), Domain::Nav, Action::View)
        .protected("/api/portfolios/{id}/metrics/rolling", get(handlers::metrics::rolling), Domain::Nav, Action::View)
        .protected("/api/portfolios/{id}/metrics/drawdowns", get(handlers::metrics::drawdowns), Domain::Nav, Action::View)
        .protected("/api/portfolios/{id}/metrics/calendar", get(handlers::metrics::calendar), Domain::Nav, Action::View)
        .protected("/api/portfolios/{id}/metrics/var", get(handlers::metrics::var), Domain::Nav, Action::View)
        // Concentration's route gate is Positions only, but the 5/10/40
        // checks are enriched by Reference (issuer-group/liquidity
        // overrides that can regroup exposures across issuers). A denied
        // Reference grant is surfaced as `issuer_overrides: unavailable` in
        // the response rather than silently computing the checks without
        // the overrides — that would let a real breach hide behind a
        // checks array that still reads "ok" (Task 9 review ruling 2 / Task
        // 11's report).
        .protected("/api/portfolios/{id}/metrics/concentration", get(handlers::limits::concentration_h), Domain::Positions, Action::View)
        // Liquidity's route gate is Positions only, but the handler also
        // reads Reference (issuer-group/liquidity overrides, via the shared
        // `snapshot` helper), Nav (AUM at the snapshot date) and
        // Shareholders (the top-5 redemption register) — each a secondary
        // domain here, soft-checked rather than hard-gating the whole
        // endpoint on a grant this route doesn't declare. All three can
        // flip a scenario's status: Reference is the sharpest case — a
        // denied grant drops every position's ADV/liquidity-days override,
        // `build_positions` falls back to `liquidity_default_days` (1 day
        // for equities), and a holding that measures tens-of-days liquid
        // reports same-week liquid, which can flip a scenario from breach to
        // ok behind nothing but a `"no adv"` fallback reason (Task 9 review
        // round 1, Task 11's report). `liquidity_h` now carries an
        // `issuer_overrides` marker (mirroring concentration_h's) and a
        // `nav_status` marker distinguishing a denied Nav grant
        // ("not permitted: NAV history") from a genuinely empty one ("no NAV
        // data"). Shareholders is the one that feeds a pass/fail scenario
        // status directly: a denied grant reports the top-5 scenario
        // `unavailable` with "not permitted: shareholder register",
        // distinguishable from the pre-existing "no shareholder register"
        // (register granted but never loaded) — see `liquidity_h`.
        .protected("/api/portfolios/{id}/metrics/liquidity", get(handlers::limits::liquidity_h), Domain::Positions, Action::View)
        .protected("/api/portfolios/{id}/metrics/rates", get(handlers::limits::rates_h), Domain::Positions, Action::View)
        .protected("/api/portfolios/{id}/metrics/derivatives", get(handlers::limits::derivatives_h), Domain::Positions, Action::View)
        .protected("/api/portfolios/{id}/metrics/backtest", get(handlers::metrics::backtest), Domain::Nav, Action::View)
        // P&L also reads transaction-level trade history (`operations_all`);
        // a denied Transactions grant is surfaced as
        // `transaction_detail: unavailable` in the response (Task 11) rather
        // than silently folded into the reconciliation residual.
        .protected("/api/portfolios/{id}/pnl", get(handlers::pnl::get), Domain::Positions, Action::View)
        // EMIR also reads futures contract specs and counterparty/OTC
        // classification data (`contracts_all`, `emir_kpis_all` — reference).
        // A denied grant is surfaced as `clearing_obligation: unavailable`
        // (every verdict built on it would otherwise default to "ok"), and
        // the evidence export refuses outright rather than emit a document
        // built on it (Task 11).
        .protected("/api/portfolios/{id}/emir", get(handlers::emir::get), Domain::Positions, Action::View)
        .protected("/api/portfolios/{id}/emir/kpis/{month}", axum::routing::put(handlers::emir::put_kpi), Domain::Settings, Action::Configure)
        .protected("/api/portfolios/{id}/emir/export", get(handlers::emir::export), Domain::Positions, Action::Export)
        .protected("/api/portfolios/{id}/futures-analytics", get(handlers::futures::list_ctd), Domain::MarketData, Action::View)
        .protected("/api/portfolios/{id}/futures-analytics", axum::routing::post(handlers::futures::upload_ctd), Domain::MarketData, Action::Import)
        .protected("/api/portfolios/{id}/limit-runs", get(handlers::breaches::runs_list), Domain::Settings, Action::View)
        .protected("/api/portfolios/{id}/limit-runs", axum::routing::post(handlers::breaches::rerun), Domain::Settings, Action::Configure)
        .protected("/api/portfolios/{id}/breaches", get(handlers::breaches::register_list), Domain::Settings, Action::View)
        .protected("/api/portfolios/{id}/breaches/{bid}", get(handlers::breaches::episode_get), Domain::Settings, Action::View)
        .protected("/api/portfolios/{id}/breaches/{bid}/acknowledge", axum::routing::post(handlers::breaches::acknowledge), Domain::Settings, Action::Configure)
        .protected("/api/portfolios/{id}/breaches/{bid}/resolve", axum::routing::post(handlers::breaches::resolve), Domain::Settings, Action::Configure)
        .layer(axum::extract::DefaultBodyLimit::max(20 * 1024 * 1024))
        .fallback(crate::static_assets::static_handler);
    // Only mounted in server mode: desktop mode has no accounts, so there is
    // nothing to enrol into. Not mounting the route (rather than mounting it
    // and rejecting inside the handler) is what makes it 404 in desktop mode.
    let router = if mode == Mode::Server {
        router.public("/api/enrol", axum::routing::post(handlers::admin::enrol))
    } else {
        router
    };
    router
        .layer(axum::middleware::from_fn_with_state(state.clone(), resolve_principal))
        .with_state(state)
}
