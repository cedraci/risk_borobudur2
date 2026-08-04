use crate::handlers;
use crate::state::AppState;
use axum::routing::get;
use axum::Router;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(|| async { axum::Json(serde_json::json!({"status": "ok"})) }))
        .route("/api/settings", get(handlers::settings::get).put(handlers::settings::put))
        .route("/api/imports", get(handlers::imports::list).post(handlers::imports::upload))
        .route("/api/nav", get(handlers::data::nav))
        .route("/api/positions", get(handlers::data::positions))
        .route("/api/metrics/summary", get(handlers::metrics::summary))
        .route("/api/metrics/rolling", get(handlers::metrics::rolling))
        .route("/api/metrics/drawdowns", get(handlers::metrics::drawdowns))
        .route("/api/metrics/calendar", get(handlers::metrics::calendar))
        .route("/api/metrics/var", get(handlers::metrics::var))
        .route("/api/metrics/concentration", get(handlers::limits::concentration_h))
        .route("/api/metrics/liquidity", get(handlers::limits::liquidity_h))
        .route("/api/metrics/rates", get(handlers::limits::rates_h))
        .route("/api/metrics/backtest", get(handlers::metrics::backtest))
        .route("/api/refs", get(handlers::refs::list))
        .route("/api/refs/{code}", axum::routing::put(handlers::refs::put))
        .layer(axum::extract::DefaultBodyLimit::max(20 * 1024 * 1024))
        .fallback(crate::static_assets::static_handler)
        .with_state(state)
}
