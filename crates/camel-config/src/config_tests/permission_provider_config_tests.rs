use super::*;

#[test]
fn permission_provider_config_deserialises_with_limits() {
    let toml_str = r#"
        provider = "wasm"
        path = "plugins/authz.wasm"
        [limits]
        timeout-secs = 5
        max-memory = 10485760
        "#;
    let cfg: PermissionProviderConfig = toml::from_str(toml_str).expect("deserialize");
    assert_eq!(cfg.provider, "wasm");
    assert_eq!(cfg.path.as_deref(), Some("plugins/authz.wasm"));
    assert_eq!(cfg.limits.timeout_secs, Some(5));
    assert_eq!(cfg.limits.max_memory, Some(10_485_760));
}

#[test]
fn permission_provider_config_defaults_limits_to_none() {
    let toml_str = r#"
        provider = "keycloak"
        "#;
    let cfg: PermissionProviderConfig = toml::from_str(toml_str).expect("deserialize");
    assert_eq!(cfg.limits, crate::wasm_limits::WasmLimitsConfig::default());
}
