use super::*;
use camel_component_api::test_support::EnvGuard;
use std::io::Write;
use std::time::Duration;

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_from_file_async_completes_without_blocking_executor() {
    // Held across the `.await` because placeholder resolution inside
    // `from_file_async` reads env vars.
    let _guard = EnvGuard::acquire();
    let mut f = tempfile::NamedTempFile::new().expect("temp file");
    write!(
        f,
        r#"
[default]
watch = true
timeout_ms = 42
"#
    )
    .expect("write config");

    let path = f.path().to_str().unwrap().to_string();
    let result = tokio::time::timeout(
        Duration::from_millis(500),
        CamelConfig::from_file_async(&path),
    )
    .await;

    assert!(
        result.is_ok(),
        "from_file_async should not block the executor"
    );
    let config = result.unwrap().expect("config should parse");
    assert!(config.watch);
    assert_eq!(config.timeout_ms, 42);
}

#[tokio::test]
async fn test_from_file_async_with_profile_completes() {
    let mut f = tempfile::NamedTempFile::new().expect("temp file");
    write!(
        f,
        r#"
[default]
watch = false
timeout_ms = 1000

[prod]
watch = true
timeout_ms = 99
"#
    )
    .expect("write config");

    let path = f.path().to_str().unwrap().to_string();
    let result = tokio::time::timeout(
        Duration::from_millis(500),
        CamelConfig::from_file_async_with_profile(&path, Some("prod")),
    )
    .await;

    assert!(
        result.is_ok(),
        "from_file_async_with_profile should not block"
    );
    let config = result.unwrap().expect("config should parse");
    assert!(config.watch);
    assert_eq!(config.timeout_ms, 99);
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_from_file_async_with_env_completes() {
    // Held across the `.await` because `config::Environment::with_prefix(...)`
    // reads env vars deep inside `from_file_async_with_env`.
    let mut _guard = EnvGuard::acquire();
    // Clear potentially leaked env vars from other tests; this test asserts
    // the file-only value (1000).
    _guard
        .and_remove("CAMEL_TIMEOUT_MS")
        .and_remove("CAMEL_PROFILE");

    let mut f = tempfile::NamedTempFile::new().expect("temp file");
    write!(
        f,
        r#"
[default]
timeout_ms = 1000
"#
    )
    .expect("write config");

    let path = f.path().to_str().unwrap().to_string();
    let result = tokio::time::timeout(
        Duration::from_millis(500),
        CamelConfig::from_file_async_with_env(&path),
    )
    .await;

    assert!(result.is_ok(), "from_file_async_with_env should not block");
    let config = result.unwrap().expect("config should parse");
    assert_eq!(config.timeout_ms, 1000);
}
