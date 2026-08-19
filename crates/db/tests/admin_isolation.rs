/// `db::admin` is the one privileged path. It exists because loading a
/// principal's grants (and managing users/sessions/audit) is what builds the
/// `AuthCtx` in the first place — a chicken-and-egg problem no `AuthCtx`-gated
/// `Scoped` method can solve. Nothing else may use it.
///
/// `auth/desktop.rs`, `auth/middleware.rs` and `handlers/session.rs` never
/// call `.admin()` or `db::admin` — they only read the `is_administrator`
/// flag already resolved onto `Principal`/`AuthCtx`. `auth/local.rs` (the
/// password/session identity provider) is where user lookup, session
/// issuance and audit logging naturally belong.
///
/// Task 12 adds `audit.rs`: the sanctioned centralizing wrapper around
/// `db::admin().audit_append(...)`. Every mutating/exporting handler calls
/// `crate::audit::record(...)` instead, and must never name `db::admin` or
/// `.admin()` itself — that is exactly what keeps handlers off this list.
///
/// Task 13 adds `handlers/admin.rs` (the `/api/admin/*` administration
/// endpoints, plus `/api/enrol`) and `startup.rs`
/// (`ensure_first_administrator`) — both legitimately reach the privileged
/// path for the same reason `auth/local.rs` does: administering users,
/// grants and roles, and standing up the very first account, is precisely
/// the identity/grant machinery this module exists for.
#[test]
fn only_identity_grants_and_audit_reach_the_privileged_path() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../server/src");
    let allowed = ["auth/local.rs", "audit.rs", "handlers/admin.rs", "startup.rs"];
    let mut offenders = Vec::new();
    for entry in walk(&src) {
        let rel = entry.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
        if allowed.contains(&rel.as_str()) { continue; }
        let text = std::fs::read_to_string(&entry).unwrap();
        if text.contains("db::admin") || text.contains(".admin()") {
            offenders.push(rel);
        }
    }
    assert!(offenders.is_empty(),
        "these files reach the privileged path and should not: {offenders:?}");
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() { out.extend(walk(&p)); }
        else if p.extension().is_some_and(|x| x == "rs") { out.push(p); }
    }
    out
}
