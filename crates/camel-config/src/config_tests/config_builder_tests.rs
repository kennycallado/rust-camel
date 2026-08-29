use super::*;

#[test]
fn test_config_builder_sets_application_name() {
    let cfg = CamelConfigBuilder::default().log_level("debug").build();
    assert_eq!(cfg.log_level, "debug");
}

#[test]
fn test_config_builder_default() {
    let built = CamelConfigBuilder::default().build();
    let default_cfg = CamelConfig::default();
    assert_eq!(built.routes, default_cfg.routes);
    assert_eq!(built.watch, default_cfg.watch);
    assert_eq!(built.log_level, default_cfg.log_level);
    assert_eq!(built.timeout_ms, default_cfg.timeout_ms);
    assert_eq!(built.drain_timeout_ms, default_cfg.drain_timeout_ms);
    assert_eq!(built.watch_debounce_ms, default_cfg.watch_debounce_ms);
}
