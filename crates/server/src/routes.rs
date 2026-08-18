pub mod protect;

use crate::auth::middleware::resolve_principal;
use crate::handlers;
use crate::routes::protect::ProtectExt;
use crate::state::AppState;
use axum::routing::get;
use axum::Router;
use db::auth::{Action, Domain};

pub fn router(state: AppState) -> Router {
    Router::new()
        .public("/api/health", get(|| async { axum::Json(serde_json::json!({"status": "ok"})) }))
        .public("/api/login", axum::routing::post(handlers::session::login))
        .public("/api/logout", axum::routing::post(handlers::session::logout))
        .public("/api/me", get(handlers::session::me))
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
        .protected_global("/api/bloomberg/upload", axum::routing::post(handlers::bloomberg::upload), Domain::MarketData, Action::Import)
        // Filters, not authorizes: any authenticated principal may call it,
        // and `Scoped::portfolios_list` narrows the result to what their
        // grants actually cover (`PortfolioScope::All` vs `Only(ids)`).
        .authenticated("/api/portfolios", get(handlers::portfolios::list))
        .protected_global("/api/portfolios", axum::routing::post(handlers::portfolios::create), Domain::Reference, Action::Configure)
        .protected("/api/portfolios/{id}", axum::routing::put(handlers::portfolios::update), Domain::Reference, Action::Configure)
        .protected("/api/portfolios/{id}/codes", get(handlers::portfolios::codes_list), Domain::Reference, Action::View)
        .protected("/api/portfolios/{id}/codes", axum::routing::put(handlers::portfolios::codes_put), Domain::Reference, Action::Configure)
        .protected("/api/portfolios/{id}/shareholders", get(handlers::portfolios::shareholders_list), Domain::Shareholders, Action::View)
        .protected("/api/portfolios/{id}/shareholders", axum::routing::put(handlers::portfolios::shareholders_put), Domain::Shareholders, Action::Import)
        .protected("/api/portfolios/{id}/flows", get(handlers::portfolios::flows), Domain::Shareholders, Action::View)
        .protected("/api/portfolios/{id}/settings", get(handlers::settings::get), Domain::Reference, Action::View)
        .protected("/api/portfolios/{id}/settings", axum::routing::put(handlers::settings::put), Domain::Reference, Action::Configure)
        .protected("/api/portfolios/{id}/imports", get(handlers::imports::list), Domain::Reference, Action::View)
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
        .protected("/api/portfolios/{id}/metrics/concentration", get(handlers::limits::concentration_h), Domain::Positions, Action::View)
        // Liquidity's route gate is Positions only, but the handler also
        // reads Reference (issuer-group/liquidity overrides, via the shared
        // `snapshot` helper), Nav (AUM at the snapshot date) and
        // Shareholders (the top-5 redemption register) — each a secondary
        // domain here. All three are soft-checked today and degrade to an
        // empty/no-data read (lost enrichment, the established "no data
        // yet" response shape, or "no shareholder register") rather than
        // hard-gating the whole endpoint on a grant this route doesn't
        // declare. Task 11 owns turning that silent degrade into an
        // explicit unavailable/degraded marker in the response — see the
        // VERDICT-FALSIFICATION comment on `refs_all` in handlers/limits.rs.
        .protected("/api/portfolios/{id}/metrics/liquidity", get(handlers::limits::liquidity_h), Domain::Positions, Action::View)
        .protected("/api/portfolios/{id}/metrics/rates", get(handlers::limits::rates_h), Domain::Positions, Action::View)
        .protected("/api/portfolios/{id}/metrics/derivatives", get(handlers::limits::derivatives_h), Domain::Positions, Action::View)
        .protected("/api/portfolios/{id}/metrics/backtest", get(handlers::metrics::backtest), Domain::Nav, Action::View)
        // P&L also reads transaction-level trade history (`operations_all`)
        // to decompose flows; that extra domain isn't gated here — Task 11's
        // secondary-domain worklist.
        .protected("/api/portfolios/{id}/pnl", get(handlers::pnl::get), Domain::Positions, Action::View)
        // EMIR also reads futures contract specs and counterparty/OTC
        // classification data (`contracts_all`, `emir_kpis_all` — reference);
        // that extra domain isn't gated here — Task 11's secondary-domain
        // worklist.
        .protected("/api/portfolios/{id}/emir", get(handlers::emir::get), Domain::Positions, Action::View)
        .protected("/api/portfolios/{id}/emir/kpis/{month}", axum::routing::put(handlers::emir::put_kpi), Domain::Reference, Action::Configure)
        .protected("/api/portfolios/{id}/emir/export", get(handlers::emir::export), Domain::Positions, Action::Export)
        .protected("/api/portfolios/{id}/futures-analytics", get(handlers::futures::list_ctd), Domain::MarketData, Action::View)
        .protected("/api/portfolios/{id}/futures-analytics", axum::routing::post(handlers::futures::upload_ctd), Domain::MarketData, Action::Import)
        .layer(axum::extract::DefaultBodyLimit::max(20 * 1024 * 1024))
        .fallback(crate::static_assets::static_handler)
        .layer(axum::middleware::from_fn_with_state(state.clone(), resolve_principal))
        .with_state(state)
}
