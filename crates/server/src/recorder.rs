//! Computes one limit-check run for a portfolio and writes it to the register.
//!
//! Runs under `AuthCtx::system()`, not the caller's grants — see the design's
//! "The system context" section. The caller's identity is still recorded, on
//! the run row and on every timeline event, so the register says who caused
//! the run even though it does not say what they could see.

use crate::state::AppState;
use analytics::breach::{self, Proposal, SubjectHolding};
use chrono::NaiveDate;
use db::auth::marker::{Configure, Settings, View};
use db::auth::AuthCtx;
use db::repo::NewRun;
use std::collections::HashMap;

pub enum Trigger {
    Import { import_id: i64, actor_user_id: Option<i64>, actor_label: String },
    Manual { actor_user_id: Option<i64>, actor_label: String },
}

impl Trigger {
    fn kind(&self) -> &'static str {
        match self { Trigger::Import { .. } => "import", Trigger::Manual { .. } => "manual" }
    }
    fn actor(&self) -> (Option<i64>, &str) {
        match self {
            Trigger::Import { actor_user_id, actor_label, .. } => (*actor_user_id, actor_label),
            Trigger::Manual { actor_user_id, actor_label } => (*actor_user_id, actor_label),
        }
    }
    fn import_id(&self) -> Option<i64> {
        match self { Trigger::Import { import_id, .. } => Some(*import_id), _ => None }
    }
}

/// Computes and records one run. Returns the run id.
///
/// Failure here must never fail the request that triggered it — an import
/// that imported is an import, and losing the user's data to protect the
/// register is the wrong trade. Callers log and carry on; see
/// `handlers::imports`.
pub async fn record(
    st: &AppState, portfolio_id: i64, nav_date: NaiveDate, trigger: Trigger,
) -> anyhow::Result<i64> {
    let ctx = AuthCtx::system();
    let scoped = st.db.scope(&ctx);
    let view = scoped.authorize::<Settings, View>(portfolio_id)
        .map_err(|d| anyhow::anyhow!("system context refused: {d}"))?;
    let configure = scoped.authorize::<Settings, Configure>(portfolio_id)
        .map_err(|d| anyhow::anyhow!("system context refused: {d}"))?;

    let computed = crate::handlers::breaches::compute(&scoped, portfolio_id, nav_date).await?;

    let mut input_notes = computed.input_notes.clone();
    // `inputs_complete` is about data that is genuinely absent, never about
    // permissions — the system context holds every grant.
    if computed.holdings.is_empty() {
        input_notes.insert("positions".into(), "no position snapshot for this date".into());
    }
    // Computed BEFORE `record_run_and_transitions` may add its back-date note:
    // that note is about what this run did to the episode lifecycle, not about
    // an input it could not read, and a back-dated run's inputs are as complete
    // as any other's.
    let inputs_complete = input_notes.is_empty();

    // Everything the transaction needs is read first, so the only awaits held
    // under the per-portfolio lock are the writes themselves.
    //
    // A proposal is only built for episodes about to open, and only where the
    // subject maps to instruments — liquidity, VaR and EMIR subjects do not.
    let prev_date = scoped.position_dates_before(&view, portfolio_id, nav_date).await?;
    // The previous snapshot is read once, not once per episode, and only for
    // its groupings — recomputing that date's whole check set to learn two
    // numbers would be waste.
    let previous = match prev_date {
        Some(d) => Some(crate::handlers::breaches::holdings_at(&scoped, portfolio_id, d).await?),
        None => None,
    };

    let (actor_user_id, actor_label) = trigger.actor();
    let actor_label = actor_label.to_string();
    let run = NewRun {
        nav_date,
        triggered_by: trigger.kind().to_string(),
        import_id: trigger.import_id(),
        actor_user_id,
        inputs_complete,
        input_notes: serde_json::Value::Object(input_notes),
        results: computed.results,
    };

    // The run, its results and its transitions in ONE transaction, with
    // `live_episodes` read inside it under a per-portfolio advisory lock, and
    // the back-date guard applied there too — see
    // `Scoped::record_run_and_transitions` for why all three belong together.
    // `propose` is pure, so it can run while the lock is held.
    let proposals_for = |transitions: &[breach::Transition]| {
        let mut proposals: HashMap<String, Proposal> = HashMap::new();
        for t in transitions {
            if let breach::Transition::Open { check_key, subject, .. } = t {
                let now: Vec<SubjectHolding> =
                    computed.holdings.get(subject).cloned().unwrap_or_default();
                let prev: Option<Vec<SubjectHolding>> =
                    previous.as_ref().map(|(h, _)| h.get(subject).cloned().unwrap_or_default());
                let p = breach::propose(
                    subject, prev.as_deref(), &now,
                    previous.as_ref().and_then(|(_, w)| w.get(subject).copied()),
                    computed.weights.get(subject).copied(),
                );
                proposals.insert(format!("{check_key}\u{1f}{subject}"), p);
            }
        }
        proposals
    };
    let outcome = scoped.record_run_and_transitions(
        &configure, &run, &computed.findings, &actor_label, &proposals_for,
    ).await?;
    if let Some(latest) = outcome.transitions_skipped_after {
        tracing::warn!(
            "portfolio {portfolio_id}: run for {nav_date} is back-dated (newest recorded run is \
             {latest}); its results were recorded and the breach episodes left untouched");
    }
    Ok(outcome.run_id)
}
