use sqlx::PgPool;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppSettings {
    pub risk_free_rate: f64,
    pub var_confidence: f64,
    pub var_horizon_days: u32,
    pub var_window_days: u32,
    pub var_limit: f64,
    pub short_dd_max_days: u32,
    #[serde(default = "default_liquidity_defaults")]
    pub liquidity_defaults: serde_json::Value,
    #[serde(default = "default_redemption_shock")]
    pub redemption_shock: f64,
}

pub fn default_liquidity_defaults() -> serde_json::Value {
    serde_json::json!({
        "Action": "d1", "Fonds": "d2_7", "Future": "d1", "Obligation": "d8_30",
        "Cash Acc": "d1", "Margin Acc": "d1", "Dividendes": "d1",
        "Frais provisionnés": "d1", "Provisions ordres": "d1"
    })
}

fn default_redemption_shock() -> f64 { 0.30 }

pub async fn get_settings(pool: &PgPool) -> anyhow::Result<AppSettings> {
    let rows: Vec<(String, serde_json::Value)> =
        sqlx::query_as("SELECT key, value FROM settings").fetch_all(pool).await?;
    let get_f = |k: &str, d: f64| rows.iter().find(|(key, _)| key == k).and_then(|(_, v)| v.as_f64()).unwrap_or(d);
    let get_u = |k: &str, d: u32| rows.iter().find(|(key, _)| key == k).and_then(|(_, v)| v.as_u64()).map(|v| v as u32).unwrap_or(d);
    let liquidity_defaults = rows.iter().find(|(key, _)| key == "liquidity_defaults")
        .map(|(_, v)| v.clone())
        .unwrap_or_else(default_liquidity_defaults);
    Ok(AppSettings {
        risk_free_rate: get_f("risk_free_rate", 0.02),
        var_confidence: get_f("var_confidence", 0.99),
        var_horizon_days: get_u("var_horizon_days", 20),
        var_window_days: get_u("var_window_days", 252),
        var_limit: get_f("var_limit", 0.20),
        short_dd_max_days: get_u("short_dd_max_days", 50),
        liquidity_defaults,
        redemption_shock: get_f("redemption_shock", 0.30),
    })
}

pub async fn put_settings(pool: &PgPool, s: &AppSettings) -> anyhow::Result<()> {
    let pairs: Vec<(&str, serde_json::Value)> = vec![
        ("risk_free_rate", s.risk_free_rate.into()),
        ("var_confidence", s.var_confidence.into()),
        ("var_horizon_days", s.var_horizon_days.into()),
        ("var_window_days", s.var_window_days.into()),
        ("var_limit", s.var_limit.into()),
        ("short_dd_max_days", s.short_dd_max_days.into()),
        ("liquidity_defaults", s.liquidity_defaults.clone()),
        ("redemption_shock", s.redemption_shock.into()),
    ];
    for (k, v) in pairs {
        sqlx::query("INSERT INTO settings (key, value) VALUES ($1, $2) ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value")
            .bind(k)
            .bind(v)
            .execute(pool)
            .await?;
    }
    Ok(())
}
