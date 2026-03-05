use camel_config::SupervisionCamelConfig;
use std::time::Duration;

#[test]
fn test_supervision_config_default_values() {
    let config = SupervisionCamelConfig::default();
    assert_eq!(config.max_attempts, Some(5));
    assert_eq!(config.initial_delay_ms, 1000);
    assert_eq!(config.backoff_multiplier, 2.0);
    assert_eq!(config.max_delay_ms, 60000);
}

#[test]
fn test_supervision_config_to_supervision_config() {
    let camel_config = SupervisionCamelConfig {
        max_attempts: Some(3),
        initial_delay_ms: 500,
        backoff_multiplier: 1.5,
        max_delay_ms: 30000,
    };

    let api_config = camel_config.into_supervision_config();

    assert_eq!(api_config.max_attempts, Some(3));
    assert_eq!(api_config.initial_delay, Duration::from_millis(500));
    assert_eq!(api_config.backoff_multiplier, 1.5);
    assert_eq!(api_config.max_delay, Duration::from_millis(30000));
}
