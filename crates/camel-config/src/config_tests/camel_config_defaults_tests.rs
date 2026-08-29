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
