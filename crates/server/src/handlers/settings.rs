use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::Json;
use db::settings::AppSettings;

pub async fn get(State(st): State<AppState>, Path(pid): Path<i64>) -> Result<Json<AppSettings>, AppError> {
    super::portfolios::ensure(&st.pool, pid, false).await?;
    Ok(Json(db::settings::get_settings(&st.pool, pid).await?))
}

pub async fn put(State(st): State<AppState>, Path(pid): Path<i64>, Json(s): Json<AppSettings>) -> Result<Json<AppSettings>, AppError> {
    super::portfolios::ensure(&st.pool, pid, true).await?;
    validate(&s).map_err(AppError::BadRequest)?;
    db::settings::put_settings(&st.pool, pid, &s).await?;
    Ok(Json(db::settings::get_settings(&st.pool, pid).await?))
}

fn validate(s: &AppSettings) -> Result<(), String> {
    if !(s.var_confidence > 0.5 && s.var_confidence < 1.0) { return Err("var_confidence must be in (0.5, 1)".into()); }
    if s.var_horizon_days < 1 { return Err("var_horizon_days must be >= 1".into()); }
    if s.var_window_days < 30 { return Err("var_window_days must be >= 30".into()); }
    if !(0.0..=1.0).contains(&s.var_limit) || s.var_limit == 0.0 { return Err("var_limit must be in (0, 1]".into()); }
    if s.short_dd_max_days < 1 { return Err("short_dd_max_days must be >= 1".into()); }
    if !(-0.05..=0.2).contains(&s.risk_free_rate) { return Err("risk_free_rate must be in [-5%, 20%]".into()); }
    if !(s.redemption_shock > 0.0 && s.redemption_shock < 1.0) {
        return Err("redemption_shock must be in (0, 1)".into());
    }
    if !(s.participation_rate > 0.0 && s.participation_rate <= 1.0) {
        return Err("participation_rate must be in (0, 1]".into());
    }
    if !(s.adv_stress_factor > 0.0 && s.adv_stress_factor <= 1.0) {
        return Err("adv_stress_factor must be in (0, 1]".into());
    }
    if s.liquidity_horizon_days < 1 { return Err("liquidity_horizon_days must be >= 1".into()); }
    if s.settlement_deadline_days < 1 { return Err("settlement_deadline_days must be >= 1".into()); }
    if s.adv_max_age_days < 1 { return Err("adv_max_age_days must be >= 1".into()); }
    if s.flow_lookback_days < 1 { return Err("flow_lookback_days must be >= 1".into()); }
    let Some(obj) = s.liquidity_default_days.as_object() else {
        return Err("liquidity_default_days must be a JSON object".into());
    };
    for (k, v) in obj {
        let ok = v.as_f64().map(|d| d.is_finite() && (0.0..=3650.0).contains(&d)).unwrap_or(false);
        if !ok {
            return Err(format!("liquidity_default_days[{k}] must be a number in [0, 3650]"));
        }
    }
    Ok(())
}
