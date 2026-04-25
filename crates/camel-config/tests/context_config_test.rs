use camel_api::{CanonicalRouteSpec, RuntimeCommand};
use camel_config::config::{
    CamelConfig, JournalConfig, ObservabilityConfig, OtelCamelConfig, PlatformCamelConfig,
    StreamCachingConfig,
};
use std::fs;
use tempfile::tempdir;

#[test]
fn test_load_routes_from_config() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("Camel.toml");

    let content = r#"
[default]
log_level = "DEBUG"
"#;
    fs::write(&config_path, content).unwrap();

    let routes =
        CamelConfig::load_routes(config_path.to_str().unwrap()).expect("Failed to load routes");

    assert!(routes.is_empty()); // No routes defined in config
}

#[tokio::test]
async fn test_context_loads_routes_from_config() {
    let dir = tempdir().unwrap();

    // Create config
    let config_path = dir.path().join("Camel.toml");
    let routes_dir = dir.path().join("routes");
    fs::create_dir(&routes_dir).unwrap();

    let config_content = format!(
        r#"
[default]
routes = ["{}"]
"#,
        routes_dir.join("*.yaml").to_str().unwrap()
    );
    fs::write(&config_path, config_content).unwrap();

    // Create route file
    let route_content = r#"
routes:
  - id: "auto-loaded-route"
    from: "timer:tick?period=1000"
    steps:
      - to: "log:info"
"#;
    fs::write(routes_dir.join("test.yaml"), route_content).unwrap();

    // Create context and register components first
    let mut ctx = camel_core::CamelContext::builder().build().await.unwrap();
    ctx.register_component(camel_component_timer::TimerComponent::new());
    ctx.register_component(camel_component_log::LogComponent::new());

    // Then load and add routes
    let routes =
        CamelConfig::load_routes(config_path.to_str().unwrap()).expect("Failed to load routes");

    for route in routes {
        ctx.add_route_definition(route)
            .await
            .expect("Failed to add route");
    }

    ctx.start().await.unwrap();

    // Verify route is loaded
    let status = ctx.runtime_route_status("auto-loaded-route").await.unwrap();
    assert!(status.is_some(), "Route should be loaded from config");

    ctx.stop().await.unwrap();
}

#[tokio::test]
async fn test_configure_context_with_supervision() {
    let config = CamelConfig {
        routes: vec![],
        watch: false,
        runtime_journal: None,
        log_level: "INFO".to_string(),
        timeout_ms: 5000,
        drain_timeout_ms: 10_000,
        watch_debounce_ms: 300,
        components: Default::default(),
        observability: Default::default(),
        supervision: Some(camel_config::SupervisionCamelConfig {
            max_attempts: Some(5),
            initial_delay_ms: 1000,
            backoff_multiplier: 2.0,
            max_delay_ms: 60000,
        }),
        platform: PlatformCamelConfig::Noop,
        stream_caching: StreamCachingConfig::default(),
    };

    let result = CamelConfig::configure_context(&config).await;
    assert!(
        result.is_ok(),
        "configure_context should succeed with supervision config"
    );
}

#[tokio::test]
async fn test_configure_context_sets_shutdown_timeout() {
    let config = CamelConfig {
        routes: vec![],
        watch: false,
        runtime_journal: None,
        log_level: "INFO".to_string(),
        timeout_ms: 5000,
        drain_timeout_ms: 10_000,
        watch_debounce_ms: 300,
        components: Default::default(),
        observability: Default::default(),
        supervision: None,
        platform: PlatformCamelConfig::Noop,
        stream_caching: StreamCachingConfig::default(),
    };

    let ctx = CamelConfig::configure_context(&config)
        .await
        .expect("configure_context should succeed");

    // Verify that the shutdown timeout is set correctly from timeout_ms
    assert_eq!(
        ctx.shutdown_timeout(),
        std::time::Duration::from_millis(5000)
    );
}

#[tokio::test]
async fn test_configure_context_with_valid_log_level() {
    let config = CamelConfig {
        routes: vec![],
        watch: false,
        runtime_journal: None,
        log_level: "debug".to_string(),
        timeout_ms: 5000,
        drain_timeout_ms: 10_000,
        watch_debounce_ms: 300,
        components: Default::default(),
        observability: Default::default(),
        supervision: None,
        platform: PlatformCamelConfig::Noop,
        stream_caching: StreamCachingConfig::default(),
    };

    let result = CamelConfig::configure_context(&config).await;
    assert!(
        result.is_ok(),
        "configure_context should succeed with valid log level 'debug'"
    );
}

#[tokio::test]
async fn test_configure_context_with_invalid_log_level() {
    let config = CamelConfig {
        routes: vec![],
        watch: false,
        runtime_journal: None,
        log_level: "invalid_level".to_string(),
        timeout_ms: 5000,
        drain_timeout_ms: 10_000,
        watch_debounce_ms: 300,
        components: Default::default(),
        observability: Default::default(),
        supervision: None,
        platform: PlatformCamelConfig::Noop,
        stream_caching: StreamCachingConfig::default(),
    };

    let result = CamelConfig::configure_context(&config).await;
    assert!(
        result.is_ok(),
        "configure_context should succeed even with invalid log level (should default to INFO)"
    );
}

#[tokio::test]
async fn test_configure_context_with_otel_enabled_registers_lifecycle() {
    let config = CamelConfig {
        routes: vec![],
        watch: false,
        runtime_journal: None,
        log_level: "INFO".to_string(),
        timeout_ms: 5000,
        drain_timeout_ms: 10_000,
        watch_debounce_ms: 300,
        components: Default::default(),
        observability: ObservabilityConfig {
            otel: Some(OtelCamelConfig {
                enabled: true,
                endpoint: "http://localhost:4317".to_string(),
                service_name: "test-service".to_string(),
                log_level: "info".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        },
        supervision: None,
        platform: PlatformCamelConfig::Noop,
        stream_caching: StreamCachingConfig::default(),
    };

    let ctx = CamelConfig::configure_context(&config)
        .await
        .expect("configure_context should succeed");

    // The health report should contain a service named "otel"
    let report = ctx.health_check();
    let otel_service = report.services.iter().find(|s| s.name == "otel");
    assert!(
        otel_service.is_some(),
        "OtelService should be registered as a lifecycle when otel.enabled=true"
    );
}

#[tokio::test]
async fn test_configure_context_without_otel_no_lifecycle() {
    let config = CamelConfig {
        routes: vec![],
        watch: false,
        runtime_journal: None,
        log_level: "INFO".to_string(),
        timeout_ms: 5000,
        drain_timeout_ms: 10_000,
        watch_debounce_ms: 300,
        components: Default::default(),
        observability: Default::default(),
        supervision: None,
        platform: PlatformCamelConfig::Noop,
        stream_caching: StreamCachingConfig::default(),
    };

    let ctx = CamelConfig::configure_context(&config)
        .await
        .expect("configure_context should succeed");

    // No OTel service should be registered
    let report = ctx.health_check();
    let otel_service = report.services.iter().find(|s| s.name == "otel");
    assert!(
        otel_service.is_none(),
        "OtelService should NOT be registered when otel is not configured"
    );
}

#[tokio::test]
async fn test_configure_context_uses_runtime_journal_from_config() {
    let dir = tempdir().unwrap();
    let journal_path = dir.path().join("config-runtime-events.db");
    let config = CamelConfig {
        routes: vec![],
        watch: false,
        runtime_journal: Some(JournalConfig {
            path: journal_path.clone(),
            durability: camel_config::JournalDurability::Immediate,
            compaction_threshold_events: 10_000,
        }),
        log_level: "INFO".to_string(),
        timeout_ms: 5000,
        drain_timeout_ms: 10_000,
        watch_debounce_ms: 300,
        components: Default::default(),
        observability: Default::default(),
        supervision: None,
        platform: PlatformCamelConfig::Noop,
        stream_caching: StreamCachingConfig::default(),
    };

    let mut ctx = CamelConfig::configure_context(&config)
        .await
        .expect("configure_context should succeed");
    ctx.register_component(camel_component_timer::TimerComponent::new());

    ctx.runtime()
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("cfg-journal-r1", "timer:tick"),
            command_id: "cfg-journal-c1".into(),
            causation_id: None,
        })
        .await
        .unwrap();

    // The redb database file should exist after context creation
    assert!(
        journal_path.exists(),
        "journal db file should exist after context creation"
    );
}
