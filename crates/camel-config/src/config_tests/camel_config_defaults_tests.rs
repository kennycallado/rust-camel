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

/// Delegation pin (redact2 Task 1.2): `CacheRepoConfig` Debug delegates to
/// `camel_api::redact::redact_url` — userinfo masked through the last `@`,
/// query dropped to the `?[redacted]` sentinel.
#[test]
fn redact_url_delegation_pin_identity() {
    let cfg = CacheRepoConfig {
        url: Some("redis://admin:pw@h:6379/0?password=x".to_string()),
        ..CacheRepoConfig::default()
    };
    let dbg = format!("{cfg:?}");
    assert!(
        dbg.contains("***@h:6379/0?[redacted]"),
        "canonical redaction rendered: {dbg}"
    );
    let url_rendered = dbg
        .split_once("url: Some(\"")
        .and_then(|(_, rest)| rest.split('"').next())
        .expect("url field rendered in Debug");
    assert!(
        !url_rendered.contains("admin"),
        "userinfo masked: {url_rendered}"
    );
    assert!(
        !url_rendered.contains("pw"),
        "password masked: {url_rendered}"
    );
}

/// Delegation pin (redact2 Task 1.2): the spec cross-surface identity
/// fixture renders identically through the `CacheRepoConfig` Debug surface.
#[test]
fn redact_url_delegation_pin_shared_fixture() {
    let http = CacheRepoConfig {
        url: Some("http://h:99999/p?token=secret".to_string()),
        ..CacheRepoConfig::default()
    };
    let dbg = format!("{http:?}");
    assert!(
        dbg.contains("http://h:99999/p?[redacted]"),
        "cross-surface identity fixture: {dbg}"
    );

    let redis = CacheRepoConfig {
        url: Some("redis://h:6379/0?password=x".to_string()),
        ..CacheRepoConfig::default()
    };
    let dbg = format!("{redis:?}");
    assert!(
        dbg.contains("redis://h:6379/0?[redacted]"),
        "redis query fixture: {dbg}"
    );
}
