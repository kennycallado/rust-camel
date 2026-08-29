use super::*;

fn parse(toml: &str) -> CamelConfig {
    let cfg = config::Config::builder()
        .add_source(config::File::from_str(toml, config::FileFormat::Toml))
        .build()
        .unwrap();
    cfg.try_deserialize().unwrap()
}

#[test]
fn languages_defaults_when_absent() {
    let cfg = parse("");
    assert_eq!(cfg.languages, crate::LanguagesConfig::default());
}

#[test]
fn languages_parses_rhai_limits() {
    let cfg = parse(
        r#"
            [languages.rhai.limits]
            max-operations = 500000
            execution-timeout-ms = 5000
            "#,
    );
    assert_eq!(cfg.languages.rhai.limits.max_operations, Some(500_000));
    assert_eq!(cfg.languages.rhai.limits.execution_timeout_ms, Some(5000));
    assert_eq!(cfg.languages.rhai.limits.max_string_size, None);
}

#[test]
fn config_parses_binds_section() {
    let cfg = parse(
        r#"
            [binds."0.0.0.0:8080"]
            allow_public_exposure = true
            "#,
    );
    assert_eq!(
        cfg.binds
            .get("0.0.0.0:8080")
            .map(|b| b.allow_public_exposure),
        Some(true)
    );
    // Absent section → empty map → gate unacknowledged everywhere.
    let bare = parse("");
    assert!(bare.binds.is_empty());
}

#[test]
fn languages_parses_js_limits() {
    let cfg = parse(
        r#"
            [languages.js.limits]
            execution-timeout-ms = 3000
            max-loop-iterations = 1000000
            "#,
    );
    assert_eq!(cfg.languages.js.limits.execution_timeout_ms, Some(3000));
    assert_eq!(cfg.languages.js.limits.max_loop_iterations, Some(1_000_000));
    assert_eq!(cfg.languages.js.limits.max_recursion_depth, None);
}

#[test]
fn languages_parses_both_engines() {
    let cfg = parse(
        r#"
            [languages.rhai.limits]
            max-operations = 100000
            [languages.js.limits]
            execution-timeout-ms = 5000
            "#,
    );
    assert_eq!(cfg.languages.rhai.limits.max_operations, Some(100_000));
    assert_eq!(cfg.languages.js.limits.execution_timeout_ms, Some(5000));
}

#[test]
fn languages_through_camel_config_default() {
    let cfg: CamelConfig = toml::from_str("").unwrap();
    assert_eq!(cfg.languages, crate::LanguagesConfig::default());
}

#[test]
fn languages_through_camel_config_from_str() {
    let toml_str = r#"
            timeout_ms = 5000

            [languages.rhai.limits]
            max-operations = 500000

            [languages.js.limits]
            execution-timeout-ms = 3000
        "#;
    let cfg: CamelConfig = toml::from_str(toml_str).expect("deserialize");
    assert_eq!(cfg.timeout_ms, 5000);
    assert_eq!(cfg.languages.rhai.limits.max_operations, Some(500_000));
    assert_eq!(cfg.languages.js.limits.execution_timeout_ms, Some(3000));
}
