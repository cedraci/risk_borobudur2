use crate::handlers;
use crate::state::AppState;
use axum::routing::get;
use axum::Router;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(|| async { axum::Json(serde_json::json!({"status": "ok"})) }))
        .route("/api/refs", get(handlers::refs::list))
        .route("/api/refs/{code}", axum::routing::put(handlers::refs::put))
        .route("/api/futures-contracts", get(handlers::futures::contracts))
        .route("/api/futures-contracts/{root}", axum::routing::put(handlers::futures::put_contract))
        .route("/api/bloomberg/request", get(handlers::bloomberg::request))
        .route("/api/bloomberg/adv-request", get(handlers::bloomberg::adv_request))
        .route("/api/bloomberg/adv-due", get(handlers::bloomberg::adv_due))
        .route("/api/bloomberg/upload", axum::routing::post(handlers::bloomberg::upload))
        .route("/api/portfolios", get(handlers::portfolios::list).post(handlers::portfolios::create))
        .route("/api/portfolios/{id}", axum::routing::put(handlers::portfolios::update))
        .route("/api/portfolios/{id}/codes", get(handlers::portfolios::codes_list).put(handlers::portfolios::codes_put))
        .route("/api/portfolios/{id}/shareholders",
            get(handlers::portfolios::shareholders_list)
                .put(handlers::portfolios::shareholders_put))
        .route("/api/portfolios/{id}/flows", get(handlers::portfolios::flows))
        .route("/api/portfolios/{id}/settings", get(handlers::settings::get).put(handlers::settings::put))
        .route("/api/portfolios/{id}/imports", get(handlers::imports::list).post(handlers::imports::upload))
        .route("/api/portfolios/{id}/nav", get(handlers::data::nav))
        .route("/api/portfolios/{id}/positions", get(handlers::data::positions))
        .route("/api/portfolios/{id}/metrics/summary", get(handlers::metrics::summary))
        .route("/api/portfolios/{id}/metrics/rolling", get(handlers::metrics::rolling))
        .route("/api/portfolios/{id}/metrics/drawdowns", get(handlers::metrics::drawdowns))
        .route("/api/portfolios/{id}/metrics/calendar", get(handlers::metrics::calendar))
        .route("/api/portfolios/{id}/metrics/var", get(handlers::metrics::var))
        .route("/api/portfolios/{id}/metrics/concentration", get(handlers::limits::concentration_h))
        .route("/api/portfolios/{id}/metrics/liquidity", get(handlers::limits::liquidity_h))
        .route("/api/portfolios/{id}/metrics/rates", get(handlers::limits::rates_h))
        .route("/api/portfolios/{id}/metrics/derivatives", get(handlers::limits::derivatives_h))
        .route("/api/portfolios/{id}/metrics/backtest", get(handlers::metrics::backtest))
        .route("/api/portfolios/{id}/pnl", get(handlers::pnl::get))
        .route("/api/portfolios/{id}/emir", get(handlers::emir::get))
        .route("/api/portfolios/{id}/emir/kpis/{month}", axum::routing::put(handlers::emir::put_kpi))
        .route("/api/portfolios/{id}/emir/export", get(handlers::emir::export))
        .route("/api/portfolios/{id}/futures-analytics",
            get(handlers::futures::list_ctd).post(handlers::futures::upload_ctd))
        .layer(axum::extract::DefaultBodyLimit::max(20 * 1024 * 1024))
        .fallback(crate::static_assets::static_handler)
        .with_state(state)
}
