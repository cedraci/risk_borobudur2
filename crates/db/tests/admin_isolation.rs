/// `db::admin` is the one privileged path. It exists because loading a
/// principal's grants (and managing users/sessions/audit) is what builds the
/// `AuthCtx` in the first place — a chicken-and-egg problem no `AuthCtx`-gated
/// `Scoped` method can solve. Nothing else may use it.
///
/// The allow-list below is not the brief's illustrative one: this codebase
/// has no `handlers/admin.rs` or `startup.rs`, and `auth/desktop.rs`,
/// `auth/middleware.rs` and `handlers/session.rs` never call `.admin()` or
/// `db::admin` — they only read the `is_administrator` flag already resolved
/// onto `Principal`/`AuthCtx`. The single real caller, checked against the
/// live tree, is `auth/local.rs` (the password/session identity provider,
/// which is exactly where user lookup, session issuance and audit logging
/// belong).
///
/// Task 12 adds `audit.rs`: the sanctioned centralizing wrapper around
/// `db::admin().audit_append(...)`. Every mutating/exporting handler calls
/// `crate::audit::record(...)` instead, and must never name `db::admin` or
/// `.admin()` itself — that is exactly what keeps handlers off this list.
#[test]
fn only_identity_grants_and_audit_reach_the_privileged_path() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../server/src");
    let allowed = ["auth/local.rs", "audit.rs"];
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
