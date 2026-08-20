use crate::error::AppError;
use crate::state::AppState;
use analytics::{concentration, ConPosition};
use axum::extract::{Path, Query, State};
use axum::{Extension, Json};
use chrono::NaiveDate;
use db::auth::marker::{MarketData, Nav, Positions, Reference, Shareholders, View};
use db::auth::AuthCtx;
use db::scoped::Scoped;
use std::collections::HashMap;

#[derive(serde::Deserialize)]
pub struct DateQuery { date: Option<String> }

type Snapshot = (Vec<NaiveDate>, Option<NaiveDate>, Vec<db::repo::PositionRecord>, Vec<db::repo::InstrumentRef>);

/// Reference data (`refs_all`) is a secondary domain here: this handler's
/// route is gated on `Positions`, not `Reference`, so a principal without a
/// separate `Reference` grant still gets a full positions snapshot — just
/// without the issuer-group/liquidity-override enrichment `refs_all`
/// supplies. Soft-checked (the query is skipped, not the whole request
/// failed) so the absence of that secondary grant degrades the enrichment
/// rather than failing the whole request. Callers that build a compliance
/// verdict out of the enrichment must not stop here: they re-check the same
/// grant themselves and surface an explicit `unavailable` marker (and
/// suppress any computed pass/fail value that would otherwise ride on the
/// degraded enrichment) — see `concentration_h`'s `issuer_overrides` field
/// and suppressed `checks`, and `liquidity_h`'s `issuer_overrides` field and
/// suppressed `scenarios`.
async fn snapshot(scoped: &Scoped<'_>, a: &db::auth::Access<Positions, View>, q: &DateQuery) -> Result<Snapshot, AppError> {
    let dates = scoped.position_dates(a).await?;
    let date = match &q.date {
        Some(s) => Some(s.parse::<NaiveDate>().map_err(|_| AppError::BadRequest(format!("bad date: {s}")))?),
        None => dates.first().copied(),
    };
    let rows = match date {
        Some(d) => scoped.positions_for(a, d).await?,
        None => Vec::new(),
    };
    let refs = match scoped.authorize_global::<Reference, View>() {
        Ok(rv) => scoped.refs_all(&rv).await?,
        Err(_) => Vec::new(),
    };
    Ok((dates, date, rows, refs))
}

pub(crate) fn ref_map(refs: &[db::repo::InstrumentRef]) -> HashMap<&str, &db::repo::InstrumentRef> {
    refs.iter().map(|r| (r.code.as_str(), r)).collect()
}

pub async fn concentration_h(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(pid): Path<i64>, Query(q): Query<DateQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Positions, View>(pid)?;
    super::portfolios::ensure(&scoped, pid, false).await?;
    let (dates, date, rows, refs) = snapshot(&scoped, &a, &q).await?;
    let by = ref_map(&refs);
    // The grouping rule lives in `breaches::subject_group` (via
    // `con_positions`), shared with the recorder: the grouping a check is
    // computed on must be the grouping the register records.
    let cons: Vec<ConPosition> = super::breaches::con_positions(&rows, &by);
    // Ruling 2 (Task 9 review, Task 11): a denied Reference read must not
    // silently drop the issuer-group/liquidity overrides that feed the
    // group/fund/deposit checks above — that would hide a real 5/10/40
    // breach behind a checks array that still reads "ok". Re-checked here
    // (not derived from `snapshot`'s soft-degrade) so a reader can always
    // tell "no breach" from "could not check the overrides".
    let reference_denied = scoped.authorize_global::<Reference, View>().err();
    let issuer_overrides = match &reference_denied {
        None => serde_json::json!({"status": "ok"}),
        Some(denied) => serde_json::json!({"status": "unavailable", "reason": denied.reason()}),
    };
    // Round 1 review (Important 2): a per-check "ok"/"watch"/"breach" is a
    // pass-adjacent value one field away from `issuer_overrides` — with the
    // overrides denied, under-aggregation across issuers can hide a real
    // breach, so no check (or row within it) may still present a computed
    // status. Stamped here rather than in `analytics::concentration` itself,
    // which has no notion of authorization.
    let checks: Vec<serde_json::Value> = concentration(&cons).iter().map(|c| {
        let mut v = serde_json::to_value(c).expect("Check always serializes");
        if reference_denied.is_some() {
            v["status"] = serde_json::json!("unavailable");
            if let Some(rows) = v["rows"].as_array_mut() {
                for r in rows.iter_mut() { r["status"] = serde_json::json!("unavailable"); }
            }
        }
        v
    }).collect();
    Ok(Json(serde_json::json!({
        "dates": dates,
        "date": date,
        "checks": checks,
        "issuer_overrides": issuer_overrides,
        "excluded_note": "Futures are excluded from issuer limits (not issuer exposure under 5/10/40); fee and order provisions are excluded.",
    })))
}

/// A register whose as-of date is older than this is flagged stale. It is not
/// a setting: the register is a compliance artefact with no per-portfolio
/// cadence to calibrate against, and a quarter is the interval at which one
/// would normally be refreshed.
const REGISTER_MAX_AGE_DAYS: i64 = 90;

fn build_positions(
    rows: &[db::repo::PositionRecord],
    by: &HashMap<&str, &db::repo::InstrumentRef>,
    settings: &db::settings::AppSettings,
    asof: chrono::NaiveDate,
) -> Vec<analytics::LiqPosition> {
    rows.iter().filter_map(|p| {
        let v = p.valuation_eur?;
        if v <= 0.0 { return None; }  // negatives are a cash need, not a sale
        let r = by.get(p.isin.as_str());
        Some(analytics::LiqPosition {
            code: p.isin.clone(),
            asset_type: p.asset_type.clone(),
            valuation_eur: v,
            quantity: p.quantity,
            adv_30d: r.and_then(|r| r.adv_30d),
            adv_stale: r.and_then(|r| r.adv_asof)
                .map(|d| (asof - d).num_days() > settings.adv_max_age_days as i64)
                // No as-of at all is "no adv", reported by its own reason.
                .unwrap_or(false),
            adv_eligible: r.and_then(|r| r.adv_eligible),
            market_place: r.and_then(|r| r.market_place.clone()),
            liquidity_days: r.and_then(|r| r.liquidity_days),
            default_days: super::refs::effective_days(
                &settings.liquidity_default_days, &p.asset_type, None),
        })
    }).collect()
}

/// Inputs the liquidity scenario math needs, gathered once by the caller —
/// the live handler after its own authorization/degradation decisions, or
/// the breach register's `compute` under the system context. This function
/// has no notion of authorization: denial-stamping (the `"unavailable"`
/// override when Reference is denied) stays in `liquidity_h`, which calls
/// this and then post-processes its result.
pub(crate) struct LiquidityScenarioInputs<'a> {
    pub rows: &'a [db::repo::PositionRecord],
    pub by: &'a HashMap<&'a str, &'a db::repo::InstrumentRef>,
    pub settings: &'a db::settings::AppSettings,
    pub asof: chrono::NaiveDate,
    pub nav: f64,
    pub register: &'a [db::repo::Shareholder],
    /// Reason shown when `top5`/`hybrid_top5` cannot be evaluated. Both
    /// callers agree on WHEN that happens — an empty `register` means the
    /// requirement is unknown, never 0% — and derive it here from
    /// `register.is_empty()`; they differ only on WHY the register is
    /// empty, which is all this string carries. `liquidity_h` passes the
    /// caller's denial reason when a Shareholders grant was refused; the
    /// breach register, running under the system context where no denial is
    /// possible, always passes "no shareholder register".
    pub top5_unavailable_reason: &'a str,
}

pub(crate) struct LiquidityAssembly {
    pub scenarios: Vec<serde_json::Value>,
    pub normal: Vec<analytics::Capacity>,
    pub stressed: Vec<analytics::Capacity>,
    pub coupon_gaps: Vec<analytics::CouponGap>,
    pub negative_eur: f64,
}

/// The scenario math lifted out of `liquidity_h`, shared with the breach
/// register's `compute`: the capacities and waterfall a scenario is judged
/// on must be the same whether a person is looking at the live page or the
/// register is recording it — a transcribed copy that drifts would make the
/// register disagree with the live page it is supposed to evidence.
pub(crate) fn liquidity_assembly(i: &LiquidityScenarioInputs) -> LiquidityAssembly {
    let horizon = i.settings.liquidity_horizon_days;
    let positions = build_positions(i.rows, i.by, i.settings, i.asof);
    let cap_at = |stress: f64| -> Vec<analytics::Capacity> {
        positions.iter().map(|p| analytics::capacity(p, i.settings.participation_rate, stress)).collect()
    };
    let normal = cap_at(1.0);
    let stressed = cap_at(i.settings.adv_stress_factor);

    let negative_eur: f64 = i.rows.iter().filter_map(|p| p.valuation_eur).filter(|v| *v < 0.0).sum();

    // Coupon and redemption inflows, from the depositary's own schedule. See
    // `liquidity_h`'s original comment: a missing fx rate is never defaulted
    // to parity, the bond is skipped and surfaced as a gap instead.
    let mut coupon_inputs: Vec<analytics::CouponInput> = Vec::new();
    let mut fx_gaps: Vec<analytics::CouponGap> = Vec::new();
    for p in i.rows.iter().filter(|p| p.asset_type == "Obligation") {
        let Some(r) = i.by.get(p.isin.as_str()) else { continue };
        let Some(fx_rate) = p.fx_rate else {
            fx_gaps.push(analytics::CouponGap { code: p.isin.clone(), reason: "no fx rate" });
            continue;
        };
        coupon_inputs.push(analytics::CouponInput {
            code: p.isin.clone(),
            quantity: p.quantity.unwrap_or(0.0),
            coupon_pct: r.bond_coupon_pct,
            coupon_type: r.bond_coupon_pct.map(|_| "FIX".to_string()),
            next_coupon: r.bond_next_coupon,
            maturity: r.bond_maturity,
            freq: r.bond_coupon_freq,
            accrued_eur: p.accrued_interest,
            fx_rate,
        });
    }
    let mut coupons = analytics::bond_inflows(&coupon_inputs, i.asof, horizon);
    coupons.gaps.extend(fx_gaps);

    let scenario = |key: &str, required_pct: Option<f64>, caps: &[analytics::Capacity]| -> serde_json::Value {
        let Some(pct) = required_pct else {
            return serde_json::json!({
                "key": key, "status": "unavailable", "reason": i.top5_unavailable_reason,
            });
        };
        let required = pct * i.nav;
        let w = analytics::waterfall(caps, &coupons.inflows, negative_eur, required, horizon);
        let status = match w.days {
            Some(d) if d <= i.settings.settlement_deadline_days => "ok",
            _ => "breach",
        };
        let curve: Vec<serde_json::Value> = (1..=horizon).map(|d| serde_json::json!({
            "day": d,
            "available_eur": analytics::available(caps, &coupons.inflows, negative_eur, d),
        })).collect();
        serde_json::json!({
            "key": key,
            "required_eur": required,
            "required_pct": pct,
            "register_count": i.register.len().min(5),
            "status": status,
            "waterfall": w,
            "slice_days": analytics::slice_days(caps, required, i.nav),
            "residual": analytics::residual(caps, required, i.nav, w.days.unwrap_or(horizon)),
            "curve": curve,
        })
    };

    // An empty register makes the top-5 redemption requirement UNKNOWN, not
    // zero: "if the five largest investors all redeemed at once, could the
    // fund meet it?" has no honest answer when nobody has loaded who they
    // are. Derived here rather than passed in so the rule has one home —
    // two copies of it in the callers could drift apart.
    let top5 = (!i.register.is_empty())
        .then(|| i.register.iter().take(5).map(|s| s.pct_of_nav).sum::<f64>() / 100.0);
    let fixed = Some(i.settings.redemption_shock);
    let scenarios = vec![
        scenario("top5", top5, &normal),
        scenario("fixed", fixed, &normal),
        scenario("hybrid_top5", top5, &stressed),
        scenario("hybrid_fixed", fixed, &stressed),
    ];

    LiquidityAssembly { scenarios, normal, stressed, coupon_gaps: coupons.gaps, negative_eur }
}

pub async fn liquidity_h(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(pid): Path<i64>, Query(q): Query<DateQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Positions, View>(pid)?;
    super::portfolios::ensure(&scoped, pid, false).await?;
    let (dates, date, rows, refs) = snapshot(&scoped, &a, &q).await?;
    let settings = scoped.get_settings(pid).await?;
    let by = ref_map(&refs);
    let horizon = settings.liquidity_horizon_days;

    // CRITICAL (Task 9 review round 1): `by` above silently drops every
    // position's ADV/liquidity-days override when Reference is denied.
    // `build_positions` then falls back to `liquidity_default_days` (1 day
    // for equities — db/src/settings.rs), so a holding that measures
    // tens-of-days liquid reports same-week liquid instead, and a scenario
    // can flip from breach to ok with only a `"no adv"` fallback reason —
    // which reads as missing data, not denial. Mirrors concentration_h's
    // `issuer_overrides` marker: re-checked directly (not derived from
    // `snapshot`'s soft-degrade) so a reader can tell "checked" from
    // "computed on a denied read".
    let reference_denied = scoped.authorize_global::<Reference, View>().err();
    let issuer_overrides = match &reference_denied {
        None => serde_json::json!({"status": "ok"}),
        Some(denied) => serde_json::json!({"status": "unavailable", "reason": denied.reason()}),
    };

    let params = serde_json::json!({
        "participation_rate": settings.participation_rate,
        "adv_stress_factor": settings.adv_stress_factor,
        "liquidity_horizon_days": horizon,
        "settlement_deadline_days": settings.settlement_deadline_days,
        "adv_max_age_days": settings.adv_max_age_days,
        "redemption_shock": settings.redemption_shock,
        "day_unit": "business days (Mon-Fri, no holiday calendar)",
    });

    // Nav is a secondary domain here (this route is gated on Positions): a
    // principal without a separate Nav grant degrades to the same "no data
    // yet" empty shape a portfolio with no NAV history at all would
    // produce. `nav_unavailable_reason` (Critical item 3, round 1 review)
    // keeps those two cases distinguishable the same way
    // `shareholders_denied_reason` does below: a denial reads "not
    // permitted: NAV history", a genuinely absent row reads "no NAV data".
    // Authorization is checked FIRST, before looking at `date` at all
    // (round 2 review): checking it only inside the `Some(d)` arm meant a
    // denied Nav grant on a portfolio with no position snapshot at all
    // (`date: None`) silently read as "no NAV data" — a denial disguised as
    // missing data, exactly what this marker exists to prevent.
    let (nav_opt, nav_unavailable_reason): (Option<f64>, Option<String>) = match scoped.authorize::<Nav, View>(pid) {
        Ok(nv) => match date {
            Some(d) => {
                let v = scoped.aum_for(&nv, d).await?;
                let reason = if v.is_none() { Some("no NAV data".to_string()) } else { None };
                (v, reason)
            }
            None => (None, Some("no NAV data".to_string())),
        },
        Err(denied) => (None, Some(denied.reason())),
    };
    let nav_status = match &nav_unavailable_reason {
        None => serde_json::json!({"status": "ok"}),
        Some(reason) => serde_json::json!({"status": "unavailable", "reason": reason}),
    };

    // An absent snapshot or NAV returns the established empty shape rather
    // than an error, matching every other metrics endpoint.
    let (Some(asof), Some(nav)) = (date, nav_opt) else {
        return Ok(Json(serde_json::json!({
            "dates": dates, "date": date, "nav": null, "params": params,
            "coverage": serde_json::Value::Null, "asset": serde_json::Value::Null,
            "scenarios": [], "negative_memo": 0.0, "negative_memo_eur": 0.0,
            "issuer_overrides": issuer_overrides,
            "nav_status": nav_status,
        })));
    };

    let negative_memo: f64 = rows.iter().filter_map(|p| p.weight).filter(|w| *w < 0.0).sum();

    // Shareholders is a secondary domain here too — see the routes.rs
    // comment on this route: it degrades the top-5 scenario to
    // "unavailable" rather than gating the whole endpoint. A denied grant
    // and a granted-but-empty register both leave `register` empty, but they
    // are not the same fact and must not share a reason string: a denial
    // says the principal cannot see the register at all, an empty register
    // says nobody has loaded one yet. `shareholders_denied_reason` carries
    // the distinction through to the scenario below.
    let (register, shareholders_denied_reason): (Vec<db::repo::Shareholder>, Option<String>) =
        match scoped.authorize::<Shareholders, View>(pid) {
            Ok(sv) => (scoped.shareholders_for(&sv).await?, None),
            Err(denied) => (Vec::new(), Some(denied.reason())),
        };
    let top5_unavailable_reason = shareholders_denied_reason
        .clone()
        .unwrap_or_else(|| "no shareholder register".to_string());
    // The capacities/waterfall/scenario math lives in `liquidity_assembly`,
    // shared with the breach register's `compute` — see its doc comment.
    let assembly = liquidity_assembly(&LiquidityScenarioInputs {
        rows: &rows, by: &by, settings: &settings, asof, nav,
        register: &register, top5_unavailable_reason: &top5_unavailable_reason,
    });
    let mut scenarios = assembly.scenarios;
    let normal = assembly.normal;
    let stressed = assembly.stressed;

    // CRITICAL residual (Task 9 review round 2): `caps` (`normal`/`stressed`)
    // are built from `positions`, which is built from `by` — the same
    // Reference-degraded map `issuer_overrides` warns about. Every scenario
    // above reads `caps`, so with Reference denied EVERY scenario's
    // "ok"/"breach" verdict, `waterfall`, `slice_days` and `residual` are
    // computed on the default-days fallback, not the fund's real liquidity —
    // exactly the breach-reads-as-ok risk `issuer_overrides` exists to flag.
    // A sibling marker is not a suppression: the computed fields must
    // themselves read `unavailable`/`null` (mirrors what `concentration_h`
    // now does to `checks`), stamped here, uniformly, for every key that
    // `liquidity_assembly` did not already mark `unavailable` on its own
    // account (an empty register) — including `top5`/`hybrid_top5` when the
    // register happens to be loaded even though Reference is denied.
    if let Some(denied) = &reference_denied {
        for v in scenarios.iter_mut() {
            if v.get("status").and_then(|s| s.as_str()) != Some("unavailable") {
                *v = serde_json::json!({
                    "key": v["key"],
                    "required_eur": v["required_eur"],
                    "required_pct": v["required_pct"],
                    "register_count": v["register_count"],
                    "status": "unavailable",
                    "reason": denied.reason(),
                    "waterfall": serde_json::Value::Null,
                    "slice_days": serde_json::Value::Null,
                    "residual": serde_json::Value::Null,
                    "curve": serde_json::Value::Null,
                });
            }
        }
    }

    let measured_eur: f64 = normal.iter().filter(|c| c.measured).map(|c| c.valuation_eur).sum();
    let fallbacks: Vec<serde_json::Value> = normal.iter()
        .filter_map(|c| c.reason.map(|r| serde_json::json!({"code": c.code, "reason": r})))
        .collect();

    Ok(Json(serde_json::json!({
        "dates": dates,
        "date": date,
        "nav": nav,
        "params": params,
        "coverage": {
            "adv_pct_of_nav": if nav > 0.0 { measured_eur / nav } else { 0.0 },
            "fallbacks": fallbacks,
            "coupon_gaps": assembly.coupon_gaps,
            "register": {
                "count": register.len(),
                "as_of": register.iter().map(|s| s.as_of).min(),
                "stale": register.iter().any(|s| (asof - s.as_of).num_days() > REGISTER_MAX_AGE_DAYS),
                "status": if shareholders_denied_reason.is_some() { "unavailable" } else { "ok" },
                "reason": shareholders_denied_reason,
            },
        },
        "asset": {
            "normal": analytics::asset_profile(&normal, nav),
            "stressed": analytics::asset_profile(&stressed, nav),
        },
        "scenarios": scenarios,
        "negative_memo": negative_memo,
        "negative_memo_eur": assembly.negative_eur,
        "issuer_overrides": issuer_overrides,
        "nav_status": nav_status,
    })))
}

pub async fn rates_h(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(pid): Path<i64>, Query(q): Query<DateQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Positions, View>(pid)?;
    super::portfolios::ensure(&scoped, pid, false).await?;
    let (dates, date, rows, refs) = snapshot(&scoped, &a, &q).await?;
    let by = ref_map(&refs);
    // Ruling 2 pattern (concentration_h/liquidity_h): a denied Reference read
    // must not silently collapse every bond to `missing: true` — that reads
    // identically to "these bonds genuinely lack coupon/maturity data" when
    // it actually means "could not check at all". Re-checked directly (not
    // derived from `snapshot`'s soft-degrade) so the two stay distinguishable.
    let reference_denied = scoped.authorize_global::<Reference, View>().err();
    let reference_status = match &reference_denied {
        None => serde_json::json!({"status": "ok"}),
        Some(denied) => serde_json::json!({"status": "unavailable", "reason": denied.reason()}),
    };
    let mut bonds = Vec::new();
    let mut total_dv01 = 0.0f64;
    let mut missing_any = false;
    for p in rows.iter().filter(|p| p.asset_type == "Obligation") {
        if let Some(denied) = &reference_denied {
            missing_any = true;
            bonds.push(serde_json::json!({
                "code": p.isin, "name": p.name, "missing": true,
                "status": "unavailable", "reason": denied.reason(),
            }));
            continue;
        }
        let r = by.get(p.isin.as_str());
        let complete = r.map(|r| r.bond_coupon_pct.is_some() && r.bond_maturity.is_some() && r.bond_coupon_freq.is_some()).unwrap_or(false);
        let metrics = match (complete, p.price, p.valuation_eur, p.weight, date) {
            (true, Some(price), Some(mv), Some(w), Some(d)) => {
                let r = r.unwrap();
                analytics::bond_metrics(price, r.bond_coupon_pct.unwrap(), r.bond_coupon_freq.unwrap() as u32, d, r.bond_maturity.unwrap())
                    .map(|m| (m, price, mv, w, r))
            }
            _ => None,
        };
        match metrics {
            Some((m, price, mv, w, r)) => {
                let dv01 = m.modified * mv * 1e-4;
                total_dv01 += dv01;
                bonds.push(serde_json::json!({
                    "code": p.isin, "name": p.name, "missing": false,
                    "coupon_pct": r.bond_coupon_pct, "maturity": r.bond_maturity, "freq": r.bond_coupon_freq,
                    "price": price, "ytm": m.ytm, "mod_duration": m.modified, "dv01_eur": dv01, "weight": w,
                }));
            }
            None => {
                missing_any = true;
                bonds.push(serde_json::json!({ "code": p.isin, "name": p.name, "missing": true }));
            }
        }
    }
    // Bond futures: only contracts classified interest_rate, and only where
    // CTD analytics exist for this exact NAV date. No carry-forward. Both
    // `contracts_all` (Reference) and `ctd_for` (MarketData) are secondary
    // domains here — see `snapshot`'s comment on `refs_all` above; the same
    // soft-degrade applies.
    let specs = match scoped.authorize_global::<Reference, View>() {
        Ok(rv) => scoped.contracts_all(&rv).await?,
        Err(_) => Vec::new(),
    };
    let snap = future_positions(&rows, &specs);
    let unconfirmed: std::collections::HashSet<&str> =
        snap.unconfirmed.iter().map(String::as_str).collect();
    let ctd = match date {
        Some(d) => match scoped.authorize::<MarketData, View>(pid) {
            Ok(mv) => scoped.ctd_for(&mv, d).await?,
            Err(_) => Vec::new(),
        },
        None => Vec::new(),
    };
    let mut futures = Vec::new();
    // A `Future` row with no resolvable spec cannot pass the candidate filter
    // below, so it never gets a chance to set this flag from inside the loop -
    // yet its DV01 is just as absent from the total. `futures_missing_any:
    // false` is a positive assertion of completeness and must not be made while
    // a futures row in the snapshot could not be evaluated at all.
    let mut futures_missing_any = !snap.no_spec.is_empty();
    // Candidate bond futures: contracts confirmed `interest_rate`, plus
    // contracts still sitting at the import-time seed's default `other` -
    // for any `Comdty`-suffixed ticker, since a bare ticker can't distinguish
    // a bond future from a commodity future (see db::repo::import_workbook)
    // - AND whose root the user has not yet confirmed. `other` is only a
    // placeholder while unconfirmed; once the user confirms a spec, that
    // confirmation is authoritative. A root confirmed `interest_rate` stays
    // (or joins) here; a root confirmed to any other category (including a
    // deliberate, terminal `other`) drops out for good, instead of sitting
    // forever with `missing: true` and pinning `futures_missing_any`.
    for f in snap.positions.iter().filter(|f| {
        f.category == analytics::Category::InterestRate
            || (f.category == analytics::Category::Other
                && analytics::contract_root(&f.ticker).is_some_and(|r| unconfirmed.contains(r.as_str())))
    }) {
        let a = ctd.iter().find(|c| c.ticker == f.ticker);
        let dv01 = match (a, f.point_value, f.fx_rate, f.qty) {
            (Some(a), Some(pv), Some(fx), Some(qty)) => analytics::dv01_position(
                &analytics::CtdAnalytics {
                    mod_duration: a.ctd_mod_duration,
                    clean_price: a.ctd_clean_price,
                    accrued: a.ctd_accrued,
                    conversion_factor: a.conversion_factor,
                },
                pv,
                qty,
                fx,
            ),
            _ => None,
        };
        match dv01 {
            Some(d) => {
                total_dv01 += d;
                let a = a.unwrap();
                futures.push(serde_json::json!({
                    "ticker": f.ticker, "name": f.name, "missing": false,
                    "qty": f.qty, "price": f.price, "point_value": f.point_value,
                    "ctd_isin": a.ctd_isin, "ctd_mod_duration": a.ctd_mod_duration,
                    "conversion_factor": a.conversion_factor, "dv01_eur": d,
                    "curve": specs.iter()
                        .find(|s| Some(&s.contract_root) == analytics::contract_root(&f.ticker).as_ref())
                        .and_then(|s| s.curve.clone()),
                }));
            }
            None => {
                futures_missing_any = true;
                futures.push(serde_json::json!({
                    "ticker": f.ticker, "name": f.name, "missing": true,
                    "qty": f.qty, "price": f.price, "point_value": f.point_value,
                }));
            }
        }
    }
    // Nav is a secondary domain here too — see `liquidity_h`.
    let aum = match date {
        Some(d) => match scoped.authorize::<Nav, View>(pid) {
            Ok(nv) => scoped.aum_for(&nv, d).await?.unwrap_or(0.0),
            Err(_) => 0.0,
        },
        None => 0.0,
    };
    // Signed P&L, not a magnitude. `dv01 = modified x mv x 1e-4` is positive
    // for a long bond, and the price relation is `dP = -D_mod x P x dy`, so a
    // +100bp move changes net assets by `-100 x total_dv01`. A long bond book
    // therefore reports a NEGATIVE sensitivity (it loses on a rate rise) and a
    // book that is net short rates once futures are counted reports a positive
    // one. Without the minus sign the figure read as the exact opposite of its
    // own label the moment futures could push the total negative.
    //
    // The denominator is AUM at the same NAV date. An unknown AUM yields null,
    // not 0.00% - a confident zero next to a large non-zero DV01 is a lie, and
    // the derivatives handler already routes the same missing input to null.
    //
    // CRITICAL (mirrors liquidity_h's scenario suppression): with Reference
    // denied, every bond above skipped its own DV01 (no coupon/maturity/freq
    // to compute from) and every future below fails the same candidate
    // filter for want of a spec — `total_dv01` therefore lands on exactly
    // 0.0, and `nav_sensitivity_100bp` on a confident `-0.0`/`0.0`. That
    // reads as "verified flat", not "could not check" — the same
    // breach-reads-as-ok risk `issuer_overrides` exists to flag elsewhere.
    // Both are nulled here rather than left to report a computed pass.
    let nav_sensitivity_100bp = match (&reference_denied, aum) {
        (Some(_), _) => serde_json::Value::Null,
        (None, a) if a > 0.0 => serde_json::json!(-100.0 * total_dv01 / a),
        (None, _) => serde_json::Value::Null,
    };
    let total_dv01_eur = match &reference_denied {
        Some(_) => serde_json::Value::Null,
        None => serde_json::json!(total_dv01),
    };
    Ok(Json(serde_json::json!({
        "dates": dates,
        "date": date,
        "bonds": bonds,
        "futures": futures,
        "total_dv01_eur": total_dv01_eur,
        // 100bp in EUR as a fraction of net assets, i.e. total DV01 scaled
        // from 1bp to 100bp and divided by AUM. This replaces the old
        // sum(modified x weight) x 0.01, which inherited a unit mismatch
        // from the source workbook's own `Poids` column (it adds unconverted
        // accrued interest to a EUR valuation); the DV01-based figure is the
        // more defensible of the two, and it extends naturally to futures,
        // which carry no market-value weight at all.
        "nav_sensitivity_100bp": nav_sensitivity_100bp,
        "missing_any": missing_any,
        "futures_missing_any": futures_missing_any,
        // Futures held in this snapshot that carry no contract spec at all.
        // They are absent from the DV01 above for a different reason than a
        // missing CTD row, and the fix is a different one, so they are named
        // separately rather than folded into the CTD prompt.
        "futures_no_spec": snap.no_spec,
        "reference_status": reference_status,
    })))
}

/// Futures positions for a snapshot, joined to their contract specs, with
/// prices decoded.
pub(crate) struct FuturesSnapshot {
    pub(crate) positions: Vec<analytics::FuturePosition>,
    /// Roots whose spec exists but the user has not confirmed yet.
    pub(crate) unconfirmed: Vec<String>,
    /// Tickers of `Future` rows with no resolvable spec at all: the root is
    /// absent from `futures_contracts`, or the ticker would not parse into one.
    /// Such a row is neither `interest_rate` nor `unconfirmed`, so the rates
    /// candidate filter below drops it; any completeness claim made by the
    /// rates section has to consult this list first.
    no_spec: Vec<String>,
}

pub(crate) fn future_positions(
    rows: &[db::repo::PositionRecord],
    specs: &[db::repo::FuturesContract],
) -> FuturesSnapshot {
    let by_root: HashMap<&str, &db::repo::FuturesContract> =
        specs.iter().map(|c| (c.contract_root.as_str(), c)).collect();
    let mut out = Vec::new();
    let mut unconfirmed = Vec::new();
    let mut no_spec = Vec::new();
    for p in rows.iter().filter(|p| p.asset_type == "Future") {
        let ticker = p.ticker.clone().unwrap_or_else(|| p.isin.clone());
        let spec = analytics::contract_root(&ticker).and_then(|r| by_root.get(r.as_str()).copied());
        match spec {
            Some(s) if !s.confirmed => unconfirmed.push(s.contract_root.clone()),
            Some(_) => {}
            None => no_spec.push(ticker.clone()),
        }
        let conv = spec
            .and_then(|s| analytics::PriceConvention::parse(&s.price_convention))
            .unwrap_or(analytics::PriceConvention::Decimal);
        out.push(analytics::FuturePosition {
            ticker,
            name: p.name.clone().unwrap_or_default(),
            currency: p.currency.clone().unwrap_or_default(),
            category: spec
                .and_then(|s| analytics::Category::parse(&s.category))
                .unwrap_or(analytics::Category::Other),
            // Absent, not zero: a blank or `#VALUE!` price cell is a state the
            // ingest layer deliberately tolerates without erroring, and
            // `unwrap_or(0.0)` here turned it into a fully populated row
            // reporting exactly zero exposure and zero DV01.
            qty: p.quantity,
            price: p.price.map(|x| analytics::decode_price(x, conv)),
            point_value: spec.and_then(|s| s.point_value),
            fx_rate: p.fx_rate,
            unconfirmed: spec.is_some_and(|s| !s.confirmed),
        });
    }
    unconfirmed.sort();
    unconfirmed.dedup();
    FuturesSnapshot { positions: out, unconfirmed, no_spec }
}

pub async fn derivatives_h(
    State(st): State<AppState>, Extension(ctx): Extension<AuthCtx>, Path(pid): Path<i64>, Query(q): Query<DateQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let scoped = st.db.scope(&ctx);
    let a = scoped.authorize::<Positions, View>(pid)?;
    super::portfolios::ensure(&scoped, pid, false).await?;
    let (dates, date, rows, _refs) = snapshot(&scoped, &a, &q).await?;
    // Reference is a secondary domain here — see `snapshot`'s comment above.
    // Re-checked directly (mirrors concentration_h/liquidity_h/rates_h): with
    // it denied, `specs` below is empty and every future's `category`
    // silently falls back to `Other` (see `future_positions`) — the
    // `categories` breakdown would then read as a real "all other" verdict
    // instead of "could not classify". The marker names the reason;
    // `categories` itself is nulled below rather than left to report the
    // misclassified aggregate as a normal result.
    let reference_denied = scoped.authorize_global::<Reference, View>().err();
    let reference_status = match &reference_denied {
        None => serde_json::json!({"status": "ok"}),
        Some(denied) => serde_json::json!({"status": "unavailable", "reason": denied.reason()}),
    };
    let specs = match scoped.authorize_global::<Reference, View>() {
        Ok(rv) => scoped.contracts_all(&rv).await?,
        Err(_) => Vec::new(),
    };
    // Nav is a secondary domain here too — see `liquidity_h`. A denied grant
    // must not silently read as `aum: 0` (indistinguishable from a fund that
    // genuinely has no NAV row yet): `aum` stays `null` and `nav_status`
    // names the reason, while `0.0` is still passed into `exposure` so its
    // own `aum > 0.0` guard safely excludes every row from `pct_nav` rather
    // than dividing by an unknown denominator.
    let (aum, nav_status): (Option<f64>, serde_json::Value) = match date {
        Some(d) => match scoped.authorize::<Nav, View>(pid) {
            Ok(nv) => (Some(scoped.aum_for(&nv, d).await?.unwrap_or(0.0)), serde_json::json!({"status": "ok"})),
            Err(denied) => (None, serde_json::json!({"status": "unavailable", "reason": denied.reason()})),
        },
        None => (Some(0.0), serde_json::json!({"status": "ok"})),
    };
    let snap = future_positions(&rows, &specs);
    let rep = analytics::exposure(&snap.positions, aum.unwrap_or(0.0));
    let categories = match &reference_denied {
        Some(_) => serde_json::Value::Null,
        None => serde_json::to_value(&rep.categories).expect("CategoryTotals always serializes"),
    };
    Ok(Json(serde_json::json!({
        "dates": dates,
        "date": date,
        "aum": aum,
        "categories": categories,
        "total": rep.total,
        "rows": rep.rows,
        "excluded": rep.excluded,
        "unconfirmed": snap.unconfirmed,
        "reference_status": reference_status,
        "nav_status": nav_status,
        "note": "Notional by reference to the underlying; long and short each in absolute value as a percentage of net assets. No netting.",
    })))
}
