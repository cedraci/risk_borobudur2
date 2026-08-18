use crate::error::AppError;
use crate::state::AppState;
use analytics::{
    annual_returns, annualized_return_from_returns, annualized_vol, daily_returns,
    drawdown_series, monthly_returns, quarterly_returns, rolling_sharpe, rolling_var,
    rolling_vol, rolling_yield_vol, sharpe_ratio, top_short_drawdowns, var_es,
    yearly_max_drawdowns, yield_vol_ratio, ytd_performance, NavPoint, VarMethod,
};
use axum::extract::{Path, Query, State};
use axum::{Extension, Json};
use db::auth::marker::{Nav, View};
use db::auth::AuthCtx;

pub const MIN_OBS: usize = 30;

#[derive(serde::Serialize)]
pub struct SummaryResponse {
    pub empty: bool,
    pub as_of: Option<chrono::NaiveDate>,
    pub nav: Option<f64>,
    pub aum: Option<f64>,
    pub ytd: Option<f64>,
    pub vol_1y: Option<f64>,
    pub vol_inception: Option<f64>,
    pub ann_return_1y: Option<f64>,
    pub yield_vol_1y: Option<f64>,
    pub sharpe_1y: Option<f64>,
    pub max_drawdown: Option<f64>,
    pub var_ucits: Option<VarBlock>,
    pub warnings: Vec<String>,
}

#[derive(serde::Serialize)]
pub struct VarBlock {
    pub confidence: f64,
    pub horizon_days: u32,
    pub window_days: u32,
    pub historical: Option<analytics::VarEs>,
    pub gaussian: Option<analytics::VarEs>,
    pub cornish_fisher: Option<analytics::VarEs>,
    pub limit: f64,
    pub utilization: Option<f64>, // historical.var / limit
    pub var_eur: Option<f64>,     // historical.var * latest AUM
}

#[derive(serde::Serialize)]
pub struct VarResponse {
    pub empty: bool,
    pub confidence: f64,
    pub horizon_days: u32,
    pub window_days: u32,
    pub methods: Option<VarBlock>,
    pub rolling: Vec<analytics::NavPoint>, // historical method, given params
    pub breaches: Vec<analytics::NavPoint>, // rolling points with value > limit
    pub limit: f64,
    pub warnings: Vec<String>,
}

fn to_points(rows: &[db::repo::NavRow]) -> Vec<NavPoint> {
    rows.iter().map(|r| NavPoint { date: r.date, value: r.nav }).collect()
}

fn var_block(rets: &[f64], confidence: f64, horizon: u32, window: u32, limit: f64, aum: Option<f64>, warnings: &mut Vec<String>) -> Option<VarBlock> {
    let window_rets: &[f64] = if rets.len() > window as usize { &rets[rets.len() - window as usize..] } else { rets };
    if window_rets.len() < MIN_OBS {
        warnings.push(format!("VaR n/a: only {} observations (< {MIN_OBS})", window_rets.len()));
        return None;
    }
    if (window_rets.len() as u32) < window {
        warnings.push(format!("VaR window shrunk to available history ({} obs < {window})", window_rets.len()));
    }
    let h = horizon as f64;
    let historical = var_es(window_rets, VarMethod::Historical, confidence, h);
    let gaussian = var_es(window_rets, VarMethod::Gaussian, confidence, h);
    let cornish_fisher = var_es(window_rets, VarMethod::CornishFisher, confidence, h);
    let utilization = historical.map(|v| v.var / limit);
    let var_eur = match (historical, aum) { (Some(v), Some(a)) => Some(v.var * a), _ => None };
    Some(VarBlock { confidence, horizon_days: horizon, window_days: window, historical, gaussian, cornish_fisher, limit, utilization, var_eur })
}

pub async fn summary(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(pid): Path<i64>,
) -> Result<Json<SummaryResponse>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Nav, View>(pid)?;
    super::portfolios::ensure(&scoped, pid, false).await?;
    let rows = scoped.nav_rows(&a).await?;
    let settings = scoped.get_settings(pid).await?;
    if rows.is_empty() {
        return Ok(Json(SummaryResponse {
            empty: true, as_of: None, nav: None, aum: None, ytd: None, vol_1y: None,
            vol_inception: None, ann_return_1y: None, yield_vol_1y: None, sharpe_1y: None,
            max_drawdown: None, var_ucits: None, warnings: vec!["No data imported yet".into()],
        }));
    }
    let nav = to_points(&rows);
    let last = rows.last().unwrap();
    let rets: Vec<f64> = daily_returns(&nav).iter().map(|p| p.value).collect();
    let mut warnings = Vec::new();

    let (ytd, vol_1y, vol_inception, ann_return_1y, yield_vol_1y, sharpe_1y, max_drawdown) =
        if rets.len() < MIN_OBS {
            warnings.push(format!("Metrics n/a: only {} observations (< {MIN_OBS})", rets.len()));
            (None, None, None, None, None, None, None)
        } else {
            if rets.len() < 252 {
                warnings.push(format!("1Y metrics use full available history ({} obs < 252)", rets.len()));
            }
            let r1y: &[f64] = if rets.len() > 252 { &rets[rets.len() - 252..] } else { &rets };
            let vol_1y = annualized_vol(r1y);
            let ann_1y = annualized_return_from_returns(r1y);
            (
                ytd_performance(&nav, last.date),
                vol_1y,
                annualized_vol(&rets),
                ann_1y,
                match (ann_1y, vol_1y) { (Some(r), Some(v)) => yield_vol_ratio(r, v), _ => None },
                match (ann_1y, vol_1y) { (Some(r), Some(v)) => sharpe_ratio(r, v, settings.risk_free_rate), _ => None },
                drawdown_series(&nav).iter().map(|p| p.value).fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.min(v)))),
            )
        };

    let var_ucits = var_block(
        &rets, settings.var_confidence, settings.var_horizon_days,
        settings.var_window_days, settings.var_limit, Some(last.aum), &mut warnings,
    );

    Ok(Json(SummaryResponse {
        empty: false, as_of: Some(last.date), nav: Some(last.nav), aum: Some(last.aum),
        ytd, vol_1y, vol_inception, ann_return_1y, yield_vol_1y, sharpe_1y, max_drawdown,
        var_ucits, warnings,
    }))
}

#[derive(serde::Deserialize)]
pub struct RollingQuery { window: Option<usize> }

pub async fn rolling(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(pid): Path<i64>, Query(q): Query<RollingQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Nav, View>(pid)?;
    super::portfolios::ensure(&scoped, pid, false).await?;
    let rows = scoped.nav_rows(&a).await?;
    let settings = scoped.get_settings(pid).await?;
    let window = q.window.unwrap_or(60).clamp(2, 1000);
    let nav = to_points(&rows);
    Ok(Json(serde_json::json!({
        "empty": rows.is_empty(),
        "window": window,
        "vol": rolling_vol(&nav, window),
        "sharpe": rolling_sharpe(&nav, window, settings.risk_free_rate),
        "yield_vol": rolling_yield_vol(&nav, window),
    })))
}

pub async fn drawdowns(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(pid): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Nav, View>(pid)?;
    super::portfolios::ensure(&scoped, pid, false).await?;
    let rows = scoped.nav_rows(&a).await?;
    let settings = scoped.get_settings(pid).await?;
    let nav = to_points(&rows);
    let underwater = drawdown_series(&nav);
    let overall_max = underwater.iter().map(|p| p.value).fold(0.0f64, f64::min);
    Ok(Json(serde_json::json!({
        "empty": rows.is_empty(),
        "underwater": underwater,
        "yearly": yearly_max_drawdowns(&nav),
        "top_short": top_short_drawdowns(&nav, settings.short_dd_max_days as i64, 5),
        "overall_max": overall_max,
        "max_days": settings.short_dd_max_days,
    })))
}

pub async fn calendar(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(pid): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Nav, View>(pid)?;
    super::portfolios::ensure(&scoped, pid, false).await?;
    let rows = scoped.nav_rows(&a).await?;
    let nav = to_points(&rows);
    Ok(Json(serde_json::json!({
        "empty": rows.is_empty(),
        "monthly": monthly_returns(&nav),
        "quarterly": quarterly_returns(&nav),
        "annual": annual_returns(&nav),
    })))
}

#[derive(serde::Deserialize)]
pub struct VarQuery { confidence: Option<f64>, horizon: Option<u32>, window: Option<u32> }

pub async fn var(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(pid): Path<i64>, Query(q): Query<VarQuery>,
) -> Result<Json<VarResponse>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Nav, View>(pid)?;
    super::portfolios::ensure(&scoped, pid, false).await?;
    let rows = scoped.nav_rows(&a).await?;
    let settings = scoped.get_settings(pid).await?;
    let confidence = q.confidence.unwrap_or(settings.var_confidence);
    if !(confidence > 0.5 && confidence < 1.0) {
        return Err(AppError::BadRequest("confidence must be in (0.5, 1)".into()));
    }
    let horizon = q.horizon.unwrap_or(settings.var_horizon_days).max(1);
    let window = q.window.unwrap_or(settings.var_window_days).max(30);
    let nav = to_points(&rows);
    let rets: Vec<f64> = daily_returns(&nav).iter().map(|p| p.value).collect();
    let mut warnings = Vec::new();
    let aum = rows.last().map(|r| r.aum);
    let methods = var_block(&rets, confidence, horizon, window, settings.var_limit, aum, &mut warnings);
    let effective_window = (window as usize).min(rets.len().max(2));
    let rolling = if rets.len() >= MIN_OBS {
        rolling_var(&nav, effective_window, VarMethod::Historical, confidence, horizon as f64)
    } else {
        Vec::new()
    };
    let breaches: Vec<NavPoint> = rolling.iter().filter(|p| p.value > settings.var_limit).cloned().collect();
    Ok(Json(VarResponse {
        empty: rows.is_empty(), confidence, horizon_days: horizon, window_days: window,
        methods, rolling, breaches, limit: settings.var_limit, warnings,
    }))
}

pub async fn backtest(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(pid): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Nav, View>(pid)?;
    super::portfolios::ensure(&scoped, pid, false).await?;
    let rows = scoped.nav_rows(&a).await?;
    let settings = scoped.get_settings(pid).await?;
    let nav = to_points(&rows);
    let window = settings.var_window_days as usize;
    let report = analytics::backtest(&nav, window, 0.99);
    Ok(Json(serde_json::json!({
        "window": window,
        "confidence": 0.99,
        "horizon_days": 1,
        "n_points": report.points.len(),
        "insufficient": report.points.is_empty(),
        "methods": {
            "historical": report.historical,
            "gaussian": report.gaussian,
            "cornish_fisher": report.cornish_fisher,
        },
        "series": report.points,
    })))
}
