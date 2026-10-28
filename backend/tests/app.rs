use cpr_account_probe::{PLUGIN_ID, config::Config, manifest, plugin};

#[test]
fn manifest_and_handlers_agree() {
    let manifest = manifest().unwrap();
    assert_eq!(manifest.plugin_id().unwrap(), PLUGIN_ID);
    plugin(Config::default()).unwrap();
}

#[test]
fn config_defaults_keep_probes_off() {
    let config = Config::from_handshake(&serde_json::json!({}));
    assert!(!config.probe_enabled);
    assert_eq!(config.min_interval_minutes, 30);
    let config = Config::from_handshake(&serde_json::json!({"probeEnabled": "yes"}));
    assert!(
        !config.probe_enabled,
        "a malformed setting must not turn probes on"
    );
    let config =
        Config::from_handshake(&serde_json::json!({"probeEnabled": true, "codexVersion": "bad"}));
    assert!(config.probe_enabled);
    assert!(
        config.probe_enabled,
        "旧版本设置仍存在时应忽略 codexVersion，身份始终读取 CPR"
    );
}
