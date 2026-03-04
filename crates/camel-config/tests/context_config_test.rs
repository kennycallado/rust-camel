use camel_config::CamelConfig;
use camel_core::RouteController;
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
    let mut ctx = camel_core::CamelContext::new();
    ctx.register_component(camel_component_timer::TimerComponent::new());
    ctx.register_component(camel_component_log::LogComponent::new());

    // Then load and add routes
    let routes =
        CamelConfig::load_routes(config_path.to_str().unwrap()).expect("Failed to load routes");

    for route in routes {
        ctx.add_route_definition(route)
            .expect("Failed to add route");
    }

    ctx.start().await.unwrap();

    // Verify route is loaded
    let status = ctx
        .route_controller()
        .lock()
        .await
        .route_status("auto-loaded-route");
    assert!(status.is_some(), "Route should be loaded from config");

    ctx.stop().await.unwrap();
}
