use super::*;
use std::io::Write;
use std::time::Duration;

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_from_file_async_completes_without_blocking_executor() {
    // Held across the `.await` because placeholder resolution inside
    // `from_file_async` reads env vars (see ENV_OVERRIDE_LOCK).
    let _guard = super::env_lock();
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
    // Serialize against env-touching tests (see ENV_OVERRIDE_LOCK). Held
    // across the `.await` because `config::Environment::with_prefix(...)`
    // reads env vars deep inside `from_file_async_with_env`.
    let _guard = super::env_lock();

    // SAFETY: clear potentially leaked env vars from other parallel tests;
    // this test asserts the file-only value (1000).
    unsafe {
        std::env::remove_var("CAMEL_TIMEOUT_MS");
        std::env::remove_var("CAMEL_PROFILE");
    }

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
