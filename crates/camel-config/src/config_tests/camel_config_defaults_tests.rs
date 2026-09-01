use super::*;

#[test]
fn watch_debounce_ms_default_is_300() {
    let config: CamelConfig = toml::from_str("").unwrap();
    assert_eq!(config.watch_debounce_ms, 300);
}

#[test]
fn watch_debounce_ms_custom_value() {
    let config: CamelConfig = toml::from_str("watch_debounce_ms = 50").unwrap();
    assert_eq!(config.watch_debounce_ms, 50);
}

#[test]
fn stream_caching_default_threshold_is_set() {
    let config: CamelConfig = toml::from_str("").unwrap();
    assert_eq!(
        config.stream_caching.threshold,
        camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD
    );
}

#[test]
fn stream_caching_custom_threshold_value() {
    let config: CamelConfig = toml::from_str("[stream_caching]\nthreshold = 1234").unwrap();
    assert_eq!(config.stream_caching.threshold, 1234);
}

#[test]
fn camel_config_debug_redacts_extra() {
    // Audit 2026-08-31, F5-5: unknown top-level keys may carry credentials;
    // Debug must not render them.
    let mut cfg = CamelConfig::default();
    cfg._extra.insert(
        "db_password".to_string(),
        toml::Value::String("supersecret".to_string()),
    );
    let dbg = format!("{cfg:?}");
    assert!(!dbg.contains("supersecret"), "extra values redacted: {dbg}");
}

#[test]
fn bean_config_debug_redacts_config_map() {
    let mut bean = BeanConfig::default();
    bean.config
        .insert("password".to_string(), "supersecret".to_string());
    let dbg = format!("{bean:?}");
    assert!(!dbg.contains("supersecret"), "bean config redacted: {dbg}");
}
