use camel_config::{CamelConfig, RhaiLimitsConfig};

#[test]
fn camel_config_with_rhai_limits_roundtrips() {
    let toml_str = r#"
[languages.rhai.limits]
max-operations = 12345
"#;
    let cfg: CamelConfig = toml::from_str(toml_str).expect("parse CamelConfig with rhai limits");
    assert_eq!(
        cfg.languages.rhai.limits.max_operations,
        Some(12_345),
        "rhai max-operations should round-trip from TOML"
    );
}

#[test]
fn camel_config_without_languages_uses_defaults() {
    let toml_str = r#"
routes = ["routes/*.yaml"]
"#;
    let cfg: CamelConfig = toml::from_str(toml_str).expect("parse CamelConfig without languages");
    assert_eq!(
        cfg.languages.rhai.limits,
        RhaiLimitsConfig::default(),
        "default rhai limits when no [languages] block"
    );
}
