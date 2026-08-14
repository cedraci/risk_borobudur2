use server::config::{Mode, ServerConfig};
use std::collections::HashMap;

fn cfg(pairs: &[(&str, &str)]) -> anyhow::Result<ServerConfig> {
    let map: HashMap<String, String> =
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    ServerConfig::from_vars(|k| map.get(k).cloned())
}

#[test]
fn defaults_to_desktop_mode() {
    let c = cfg(&[]).unwrap();
    assert_eq!(c.mode, Mode::Desktop);
    assert_eq!(c.database_url, None);
    assert_eq!(c.bind, "127.0.0.1:8787");
    assert!(c.open_browser);
}

#[test]
fn database_url_selects_server_mode() {
    let c = cfg(&[("BOROBUDUR_DATABASE_URL", "postgres://u@h/db")]).unwrap();
    assert_eq!(c.mode, Mode::Server);
    assert_eq!(c.database_url.as_deref(), Some("postgres://u@h/db"));
    assert_eq!(c.bind, "127.0.0.1:8787", "bind default is unchanged by mode");
    assert!(!c.open_browser, "server mode never opens a browser");
}

#[test]
fn bind_is_overridable_in_both_modes() {
    assert_eq!(cfg(&[("BOROBUDUR_BIND", "0.0.0.0:9000")]).unwrap().bind, "0.0.0.0:9000");
    let c = cfg(&[
        ("BOROBUDUR_DATABASE_URL", "postgres://u@h/db"),
        ("BOROBUDUR_BIND", "0.0.0.0:9000"),
    ]).unwrap();
    assert_eq!(c.bind, "0.0.0.0:9000");
}

#[test]
fn blank_values_are_treated_as_unset() {
    let c = cfg(&[("BOROBUDUR_DATABASE_URL", "   "), ("BOROBUDUR_BIND", "")]).unwrap();
    assert_eq!(c.mode, Mode::Desktop);
    assert_eq!(c.bind, "127.0.0.1:8787");
}

#[test]
fn admin_email_is_read_only_in_server_mode() {
    let c = cfg(&[("BOROBUDUR_ADMIN_EMAIL", "risk@firm.lu")]).unwrap();
    assert_eq!(c.admin_email, None, "desktop mode never enrols anyone");
    let c = cfg(&[
        ("BOROBUDUR_DATABASE_URL", "postgres://u@h/db"),
        ("BOROBUDUR_ADMIN_EMAIL", "risk@firm.lu"),
    ]).unwrap();
    assert_eq!(c.admin_email.as_deref(), Some("risk@firm.lu"));
}
