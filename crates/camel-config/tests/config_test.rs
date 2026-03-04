use camel_config::CamelConfig;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_load_basic_config() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("Camel.toml");

    let content = r#"
[default]
routes = ["routes/**/*.yaml"]
log_level = "DEBUG"
timeout_ms = 10000
"#;
    fs::write(&config_path, content).unwrap();

    let config =
        CamelConfig::from_file(config_path.to_str().unwrap()).expect("Failed to load config");

    assert_eq!(config.routes, vec!["routes/**/*.yaml"]);
    assert_eq!(config.log_level, "DEBUG");
    assert_eq!(config.timeout_ms, 10000);
}

#[test]
fn test_load_config_with_defaults() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("Camel.toml");

    let content = r#"
[default]
"#;
    fs::write(&config_path, content).unwrap();

    let config =
        CamelConfig::from_file(config_path.to_str().unwrap()).expect("Failed to load config");

    assert!(config.routes.is_empty());
    assert_eq!(config.log_level, "INFO");
    assert_eq!(config.timeout_ms, 5000);
}

#[test]
fn test_load_config_with_profile() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("Camel.toml");

    let content = r#"
[default]
log_level = "INFO"
timeout_ms = 5000

[production]
log_level = "ERROR"
timeout_ms = 30000
"#;
    fs::write(&config_path, content).unwrap();

    let config =
        CamelConfig::from_file_with_profile(config_path.to_str().unwrap(), Some("production"))
            .expect("Failed to load config");

    assert_eq!(config.log_level, "ERROR");
    assert_eq!(config.timeout_ms, 30000);
}

#[test]
fn test_env_var_override() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("Camel.toml");

    let content = r#"
[default]
log_level = "INFO"
timeout_ms = 5000
"#;
    fs::write(&config_path, content).unwrap();

    unsafe {
        std::env::set_var("CAMEL_LOG_LEVEL", "DEBUG");
        std::env::set_var("CAMEL_TIMEOUT_MS", "10000");
    }

    let config = CamelConfig::from_file_with_env(config_path.to_str().unwrap())
        .expect("Failed to load config");

    unsafe {
        std::env::remove_var("CAMEL_LOG_LEVEL");
        std::env::remove_var("CAMEL_TIMEOUT_MS");
    }

    assert_eq!(config.log_level, "DEBUG");
    assert_eq!(config.timeout_ms, 10000);
}

#[test]
fn test_nested_profile_merge() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("Camel.toml");

    let content = r#"
[default]
log_level = "INFO"
timeout_ms = 5000

[default.components.http]
connect_timeout_ms = 3000
max_connections = 100

[production]
log_level = "ERROR"

[production.components.http]
max_connections = 1000
"#;
    fs::write(&config_path, content).unwrap();

    let config =
        CamelConfig::from_file_with_profile(config_path.to_str().unwrap(), Some("production"))
            .expect("Failed to load config");

    assert_eq!(config.log_level, "ERROR");
    assert_eq!(config.timeout_ms, 5000); // From default
    assert_eq!(
        config.components.http.as_ref().unwrap().max_connections,
        1000
    );
    // This should fail with current implementation because it uses serde default (5000)
    // instead of merging from default (3000)
    assert_eq!(
        config.components.http.as_ref().unwrap().connect_timeout_ms,
        3000
    ); // From default - THIS WILL FAIL
}
