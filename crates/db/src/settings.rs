use crate::scoped::Scoped;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppSettings {
    pub risk_free_rate: f64,
    pub var_confidence: f64,
    pub var_horizon_days: u32,
    pub var_window_days: u32,
    pub var_limit: f64,
    pub short_dd_max_days: u32,
    #[serde(default = "default_liquidity_default_days")]
    pub liquidity_default_days: serde_json::Value,
    #[serde(default = "default_redemption_shock")]
    pub redemption_shock: f64,
    #[serde(default = "default_participation_rate")]
    pub participation_rate: f64,
    #[serde(default = "default_adv_stress_factor")]
    pub adv_stress_factor: f64,
    #[serde(default = "default_liquidity_horizon_days")]
    pub liquidity_horizon_days: u32,
    #[serde(default = "default_settlement_deadline_days")]
    pub settlement_deadline_days: u32,
    #[serde(default = "default_adv_max_age_days")]
    pub adv_max_age_days: u32,
    #[serde(default = "default_flow_lookback_days")]
    pub flow_lookback_days: u32,
}

pub fn default_liquidity_default_days() -> serde_json::Value {
    serde_json::json!({
        "Action": 1, "Fonds": 7, "Obligation": 30, "Future": 1,
        "Dividendes": 1, "Frais provisionnés": 1, "Provisions ordres": 1
    })
}

fn default_redemption_shock() -> f64 { 0.30 }
fn default_participation_rate() -> f64 { 0.25 }
fn default_adv_stress_factor() -> f64 { 0.30 }
fn default_liquidity_horizon_days() -> u32 { 60 }
fn default_settlement_deadline_days() -> u32 { 3 }
fn default_adv_max_age_days() -> u32 { 7 }
fn default_flow_lookback_days() -> u32 { 250 }

/// A pre-v2 database stores `liquidity_defaults`, a map of asset type to
/// bucket name. Map it forward at each band's upper edge rather than
/// silently reverting a portfolio to code defaults.
fn days_from_legacy_buckets(v: &serde_json::Value) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for (k, b) in v.as_object().into_iter().flatten() {
        let days = match b.as_str() {
            Some("d1") => 1, Some("d2_7") => 7, Some("d8_30") => 30, Some("d30p") => 60,
            _ => continue,
        };
        // Cash and margin accounts are capacity-infinite by engine rule, not
        // by table entry, so they are dropped rather than carried at 1 day.
        if k == "Cash Acc" || k == "Margin Acc" { continue; }
        out.insert(k.clone(), serde_json::json!(days));
    }
    serde_json::Value::Object(out)
}

impl<'a> Scoped<'a> {
    /// Settings are read as a computational input from almost every domain's
    /// handlers (metrics uses `var_confidence`/`risk_free_rate`, limits uses
    /// `liquidity_default_days`/`participation_rate`, shareholders' flows
    /// uses `flow_lookback_days`, ...), so this is deliberately NOT gated by
    /// an `Access`/`GlobalAccess` token the way a domain's own data is —
    /// requiring one would mean, say, a Nav-only grant could not compute
    /// `metrics/summary` at all. The `/settings` route itself is still
    /// gated (`Domain::Reference`) at the router and re-authorized in
    /// `handlers::settings`; this method is the shared, ungated query both
    /// that handler and every other domain's handler read through.
    pub async fn get_settings(&self, portfolio_id: i64) -> anyhow::Result<AppSettings> {
        let rows: Vec<(String, serde_json::Value)> =
            sqlx::query_as("SELECT key, value FROM settings WHERE portfolio_id = $1")
                .bind(portfolio_id)
                .fetch_all(self.pool).await?;
        let get_f = |k: &str, d: f64| rows.iter().find(|(key, _)| key == k).and_then(|(_, v)| v.as_f64()).unwrap_or(d);
        let get_u = |k: &str, d: u32| rows.iter().find(|(key, _)| key == k).and_then(|(_, v)| v.as_u64()).map(|v| v as u32).unwrap_or(d);
        let liquidity_default_days = rows.iter().find(|(key, _)| key == "liquidity_default_days")
            .map(|(_, v)| v.clone())
            .or_else(|| rows.iter().find(|(key, _)| key == "liquidity_defaults")
                .map(|(_, v)| days_from_legacy_buckets(v)))
            .unwrap_or_else(default_liquidity_default_days);
        Ok(AppSettings {
            risk_free_rate: get_f("risk_free_rate", 0.02),
            var_confidence: get_f("var_confidence", 0.99),
            var_horizon_days: get_u("var_horizon_days", 20),
            var_window_days: get_u("var_window_days", 252),
            var_limit: get_f("var_limit", 0.20),
            short_dd_max_days: get_u("short_dd_max_days", 50),
            liquidity_default_days,
            redemption_shock: get_f("redemption_shock", 0.30),
            participation_rate: get_f("participation_rate", default_participation_rate()),
            adv_stress_factor: get_f("adv_stress_factor", default_adv_stress_factor()),
            liquidity_horizon_days: get_u("liquidity_horizon_days", default_liquidity_horizon_days()),
            settlement_deadline_days: get_u("settlement_deadline_days", default_settlement_deadline_days()),
            adv_max_age_days: get_u("adv_max_age_days", default_adv_max_age_days()),
            flow_lookback_days: get_u("flow_lookback_days", default_flow_lookback_days()),
        })
    }

    pub async fn put_settings(&self, portfolio_id: i64, s: &AppSettings) -> anyhow::Result<()> {
        let pairs: Vec<(&str, serde_json::Value)> = vec![
            ("risk_free_rate", s.risk_free_rate.into()),
            ("var_confidence", s.var_confidence.into()),
            ("var_horizon_days", s.var_horizon_days.into()),
            ("var_window_days", s.var_window_days.into()),
            ("var_limit", s.var_limit.into()),
            ("short_dd_max_days", s.short_dd_max_days.into()),
            ("liquidity_default_days", s.liquidity_default_days.clone()),
            ("redemption_shock", s.redemption_shock.into()),
            ("participation_rate", s.participation_rate.into()),
            ("adv_stress_factor", s.adv_stress_factor.into()),
            ("liquidity_horizon_days", s.liquidity_horizon_days.into()),
            ("settlement_deadline_days", s.settlement_deadline_days.into()),
            ("adv_max_age_days", s.adv_max_age_days.into()),
            ("flow_lookback_days", s.flow_lookback_days.into()),
        ];
        for (k, v) in pairs {
            sqlx::query(
                "INSERT INTO settings (portfolio_id, key, value) VALUES ($1, $2, $3)
                 ON CONFLICT (portfolio_id, key) DO UPDATE SET value = EXCLUDED.value",
            )
            .bind(portfolio_id)
            .bind(k)
            .bind(v)
            .execute(self.pool)
            .await?;
        }
        Ok(())
    }
}
