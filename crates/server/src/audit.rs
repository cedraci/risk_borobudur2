//! The sanctioned bridge to `db::admin`'s audit log for this crate.
//!
//! Every mutating or exporting handler goes through [`record`] rather than
//! naming `db::admin`/`.admin()` itself — `crates/db/tests/admin_isolation.rs`
//! fails the build if any file in `crates/server/src` other than this one (and
//! `auth/local.rs`, which resolves identity) reaches the privileged path.

use crate::state::AppState;
use db::admin::AuditEvent;
use db::auth::{AuthCtx, Domain};

/// The request already succeeded when this is called. A failure to write the
/// audit row must not undo it — log loudly and carry on, because losing the
/// user's work to protect the log is the wrong trade.
pub async fn record(
    st: &AppState, ctx: &AuthCtx, action: &str,
    domain: Option<Domain>, portfolio_id: Option<i64>, detail: serde_json::Value,
) {
    let event = AuditEvent {
        user_id: (ctx.principal_id != 0).then_some(ctx.principal_id),
        actor_label: ctx.display_name.clone(),
        action: action.to_string(),
        domain,
        portfolio_id,
        detail,
        source_addr: None,
    };
    if let Err(e) = st.db.admin().audit_append(event).await {
        tracing::error!("audit write failed for {action}: {e:#}");
    }
}
