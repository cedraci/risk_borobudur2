use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::{Extension, Json};
use db::auth::marker::{Configure, Import, Reference, Shareholders, View};
use db::auth::AuthCtx;

#[derive(serde::Deserialize)]
pub struct CreateBody { pub name: String, pub kind: String }

#[derive(serde::Deserialize)]
pub struct UpdateBody { pub name: String, pub archived: bool }

fn valid_name(name: &str) -> Result<String, AppError> {
    let n = name.trim();
    if n.is_empty() {
        return Err(AppError::Unprocessable("name must not be empty".into()));
    }
    Ok(n.to_string())
}

fn valid_kind(kind: &str) -> Result<(), AppError> {
    if !matches!(kind, "ucits" | "mandate") {
        return Err(AppError::Unprocessable("kind must be 'ucits' or 'mandate'".into()));
    }
    Ok(())
}

/// Unique-violation on portfolios.name -> 422 with a helpful message; any
/// other DB error stays a 500.
fn map_name_conflict(e: anyhow::Error) -> AppError {
    let is_unique = e.downcast_ref::<sqlx::Error>()
        .and_then(|se| se.as_database_error())
        .map(|de| de.is_unique_violation())
        .unwrap_or(false);
    if is_unique {
        AppError::Unprocessable("a portfolio with that name already exists".into())
    } else {
        AppError::Internal(e)
    }
}

/// Filters rather than authorizes — the route accepts any authenticated
/// principal (`.authenticated`, not `.protected_global`); the visible set
/// narrows to what the principal's grants actually cover.
pub async fn list(State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>) -> Result<Json<Vec<db::repo::Portfolio>>, AppError> {
    let scoped = st.db.scope(&ctx);
    Ok(Json(scoped.portfolios_list().await?))
}

pub async fn create(State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Json(b): Json<CreateBody>)
    -> Result<Json<db::repo::Portfolio>, AppError>
{
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize_global::<Reference, Configure>()?;
    let name = valid_name(&b.name)?;
    valid_kind(&b.kind)?;
    let p = scoped.portfolio_create(&a, &name, &b.kind).await
        .map_err(map_name_conflict)?;
    Ok(Json(p))
}

pub async fn update(State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(id): Path<i64>, Json(b): Json<UpdateBody>)
    -> Result<Json<db::repo::Portfolio>, AppError>
{
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Reference, Configure>(id)?;
    let name = valid_name(&b.name)?;
    let p = scoped.portfolio_update(&a, &name, b.archived).await
        .map_err(map_name_conflict)?
        .ok_or_else(|| AppError::NotFound(format!("no portfolio {id}")))?;
    Ok(Json(p))
}

/// Every scoped handler's first call, AFTER authorizing the domain it
/// actually needs: a wildcard grant answers "yes" for any portfolio id,
/// including one that was never created, so authorization alone cannot 404
/// it. `mutating` requests (imports, CTD upload, KPI puts, settings puts)
/// are refused on an archived portfolio; reads stay available so history
/// remains inspectable.
pub async fn ensure(scoped: &db::scoped::Scoped<'_>, id: i64, mutating: bool)
    -> Result<db::repo::Portfolio, AppError>
{
    let p = scoped.portfolio_row(id).await?
        .ok_or_else(|| AppError::NotFound(format!("no portfolio {id}")))?;
    if mutating && p.archived {
        return Err(AppError::Conflict(format!("portfolio '{}' is archived", p.name)));
    }
    Ok(p)
}

#[derive(serde::Deserialize)]
pub struct CodeBody {
    pub source: String,
    pub code: String,
}

pub async fn codes_list(State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(pid): Path<i64>) -> Result<Json<Vec<db::repo::PortfolioCode>>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Reference, View>(pid)?;
    ensure(&scoped, pid, false).await?;
    Ok(Json(scoped.portfolio_codes_for(&a).await?))
}

/// Replace the portfolio's full code set. Codes are trimmed; empty entries
/// are 422; a code already claimed by another portfolio is 422 too.
pub async fn codes_put(State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(pid): Path<i64>, Json(body): Json<Vec<CodeBody>>) -> Result<Json<Vec<db::repo::PortfolioCode>>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Reference, Configure>(pid)?;
    ensure(&scoped, pid, true).await?;
    let mut codes: Vec<(String, String)> = Vec::with_capacity(body.len());
    for c in &body {
        let source = c.source.trim().to_lowercase();
        let code = c.code.trim().to_string();
        if source.is_empty() || code.is_empty() {
            return Err(AppError::Unprocessable("source and code must be non-empty".into()));
        }
        codes.push((source, code));
    }
    scoped.portfolio_codes_replace(&a, &codes).await.map_err(|e| {
        let is_unique = e.downcast_ref::<sqlx::Error>()
            .and_then(|se| se.as_database_error())
            .is_some_and(|de| de.is_unique_violation());
        if is_unique {
            AppError::Unprocessable("one of these codes is already mapped to another portfolio".into())
        } else {
            AppError::Internal(e)
        }
    })?;
    // `a` (Configure) already implies View in the grant set, so this
    // authorize cannot fail for a principal who reached this handler.
    let view = scoped.authorize::<Reference, View>(pid)?;
    Ok(Json(scoped.portfolio_codes_for(&view).await?))
}

#[derive(serde::Deserialize)]
pub struct ShareholderBody {
    pub label: String,
    pub pct_of_nav: f64,
    pub as_of: chrono::NaiveDate,
}

pub async fn shareholders_list(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(pid): Path<i64>,
) -> Result<Json<Vec<db::repo::Shareholder>>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Shareholders, View>(pid)?;
    ensure(&scoped, pid, false).await?;
    Ok(Json(scoped.shareholders_for(&a).await?))
}

/// Replace the portfolio's whole register. Every check runs before any
/// write, so a rejected payload leaves the stored register untouched.
pub async fn shareholders_put(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(pid): Path<i64>, Json(body): Json<Vec<ShareholderBody>>,
) -> Result<Json<Vec<db::repo::Shareholder>>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Shareholders, Import>(pid)?;
    ensure(&scoped, pid, true).await?;
    let mut total = 0.0;
    let mut rows = Vec::with_capacity(body.len());
    for b in &body {
        let label = b.label.trim();
        if label.is_empty() {
            return Err(AppError::Unprocessable("label must not be blank".into()));
        }
        if !(b.pct_of_nav.is_finite() && b.pct_of_nav > 0.0 && b.pct_of_nav <= 100.0) {
            return Err(AppError::Unprocessable(format!(
                "{label}: pct_of_nav must be in (0, 100]")));
        }
        total += b.pct_of_nav;
        rows.push((label.to_string(), b.pct_of_nav, b.as_of));
    }
    // A register summing past the whole fund is a typo, not a portfolio.
    if total > 100.0 {
        return Err(AppError::Unprocessable(format!(
            "register totals {total:.2}% of NAV, which exceeds 100%")));
    }
    scoped.shareholders_replace(&a, &rows).await?;
    // `a` (Import) already implies View in the grant set, so this authorize
    // cannot fail for a principal who reached this handler.
    let view = scoped.authorize::<Shareholders, View>(pid)?;
    Ok(Json(scoped.shareholders_for(&view).await?))
}

/// The observed subscription/redemption history, for comparison against the
/// *configured* redemption shock used in the liquidity scenarios. This is a
/// read: history must stay inspectable even on an archived portfolio.
pub async fn flows(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(pid): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Shareholders, View>(pid)?;
    ensure(&scoped, pid, false).await?;
    let settings = scoped.get_settings(pid).await?;
    let records = scoped.flows_for(&a, settings.flow_lookback_days).await?;

    // Aggregate the share classes into one fund-level series. Each class
    // contributes its own net amount and its own net assets, so no
    // NAV-per-share ambiguity arises and multi-class portfolios need no
    // special case here.
    //
    // A blank *amount* cell is parsed upstream as 0.0 ("no flow that day"),
    // but a blank outstanding_shares/nav_per_share is parsed as None ("NAV
    // not yet struck" — an ordinary depositary-lag state, not a zero). If we
    // let a None NAV contribute 0.0 to the denominator while still counting
    // that class's full subscription/redemption in the numerator, a date
    // with an unstruck NAV would fabricate an inflated outflow percentage.
    // So the whole date is excluded whenever any contributing class is
    // missing either NAV input — never zero-filled — and the exclusion is
    // reported rather than swallowed.
    let mut by_date: std::collections::BTreeMap<chrono::NaiveDate, (f64, f64, bool)> = Default::default();
    for r in &records {
        let e = by_date.entry(r.flow_date).or_insert((0.0, 0.0, true));
        e.0 += r.subscription_amount - r.redemption_amount;
        match (r.outstanding_shares, r.nav_per_share) {
            (Some(shares), Some(nav)) => e.1 += shares * nav,
            _ => e.2 = false,
        }
    }
    let mut dates_excluded_no_nav: usize = 0;
    let obs: Vec<analytics::FlowObs> = by_date.into_iter()
        .filter_map(|(date, (net_eur, nav_eur, complete))| {
            if complete {
                Some(analytics::FlowObs { date, net_eur, nav_eur })
            } else {
                dates_excluded_no_nav += 1;
                None
            }
        })
        .collect();

    Ok(Json(match analytics::flow_stats(&obs) {
        Some(s) => {
            let mut v = serde_json::json!(s);
            v["dates_excluded_no_nav"] = serde_json::json!(dates_excluded_no_nav);
            v
        }
        None => serde_json::json!({
            "status": "unavailable",
            "n_observations": obs.len(),
            "dates_excluded_no_nav": dates_excluded_no_nav,
            "reason": format!(
                "{} observation(s) loaded; {} are needed before an observed outflow means anything",
                obs.len(), analytics::MIN_FLOW_OBSERVATIONS),
        }),
    }))
}
