use camel_api::cache::{CacheEntry, CacheRepository, ContentType};
use camel_config::CamelConfig;
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;

fn entry() -> CacheEntry {
    CacheEntry {
        bytes: vec![1, 2, 3],
        content_type: ContentType::Bytes,
        expires_at: None,
    }
}

fn make_cfg(toml: &str) -> CamelConfig {
    config::Config::builder()
        .add_source(config::File::from_str(toml, config::FileFormat::Toml))
        .build()
        .unwrap()
        .try_deserialize::<CamelConfig>()
        .unwrap()
}

// ── Test 1: redb backed persistent repo registered ────────────────────────

#[tokio::test]
async fn redb_with_cache_size_builds() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("cache.redb");
    let toml = format!(
        r#"
[cache_repo]
backend = "redb"
path = "{}"
cache_size = "256MiB"
"#,
        db_path.to_str().unwrap()
    );

    let cfg = make_cfg(&toml);
    let ctx = CamelConfig::configure_context(&cfg).await.unwrap();

    assert!(
        ctx.cache_repository("persistent").is_some(),
        "persistent cache repository must be registered when backend=redb"
    );
    assert!(
        ctx.cache_repository("memory").is_some(),
        "default memory cache repository must still be present"
    );
}

#[tokio::test]
async fn redb_builds_with_cache_size_and_sweep_interval() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("cache.redb");
    let toml = format!(
        r#"
[cache_repo]
backend = "redb"
path = "{}"
cache_size = "512MiB"
sweep_interval = "30m"
"#,
        db_path.to_str().unwrap()
    );

    let cfg = make_cfg(&toml);
    let ctx = CamelConfig::configure_context(&cfg).await.unwrap();

    assert!(
        ctx.cache_repository("persistent").is_some(),
        "persistent cache repository must be registered when backend=redb"
    );
}

#[tokio::test]
async fn redb_builds_without_sweep_interval_using_default() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("cache.redb");
    let toml = format!(
        r#"
[cache_repo]
backend = "redb"
path = "{}"
cache_size = "512MiB"
"#,
        db_path.to_str().unwrap()
    );

    let cfg = make_cfg(&toml);
    let ctx = CamelConfig::configure_context(&cfg).await.unwrap();

    assert!(
        ctx.cache_repository("persistent").is_some(),
        "persistent cache repository must be registered when backend=redb"
    );
}

#[tokio::test]
async fn redb_builds_without_stale_retention_using_default() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("cache.redb");
    let toml = format!(
        r#"
[cache_repo]
backend = "redb"
path = "{}"
cache_size = "512MiB"
"#,
        db_path.to_str().unwrap()
    );

    let cfg = make_cfg(&toml);
    let ctx = CamelConfig::configure_context(&cfg).await.unwrap();

    assert!(
        ctx.cache_repository("persistent").is_some(),
        "persistent cache repository must be registered when backend=redb"
    );
}

// ── Test 2: no persistent when cache_repo absent ──────────────────────────

#[tokio::test]
async fn redb_absent_when_backend_memory_or_unset() {
    let cfg = make_cfg("");
    let ctx = CamelConfig::configure_context(&cfg).await.unwrap();

    assert!(
        ctx.cache_repository("persistent").is_none(),
        "persistent cache repository must not be registered by default"
    );
    assert!(
        ctx.cache_repository("memory").is_some(),
        "default memory cache repository must be present"
    );
}

// ── Test 3: validation rejects empty path for redb ────────────────────────

#[test]
fn empty_redb_path_rejected_at_validation() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redb"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("path"),
        "validation error must mention path, got: {msg}"
    );
}

#[test]
fn empty_redb_path_string_rejected_at_validation() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redb"
path = ""
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("path"),
        "validation error must mention path, got: {msg}"
    );
}

#[test]
fn unknown_backend_rejected_at_validation() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "postgres"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("postgres") || msg.contains("backend"),
        "validation error must mention the invalid backend, got: {msg}"
    );
}

// ── Test 3b: redb cache_size and interval validation ──────────────────────

#[test]
fn missing_cache_size_on_redb_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redb"
path = "cache.redb"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.cache_size"),
        "validation error must mention cache_repo.cache_size, got: {msg}"
    );
}

#[test]
fn malformed_cache_size_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redb"
path = "cache.redb"
cache_size = "thirty"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.cache_size"),
        "validation error must mention cache_repo.cache_size, got: {msg}"
    );
}

#[test]
fn overflowing_cache_size_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redb"
path = "cache.redb"
cache_size = "18446744073709551616B"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.cache_size"),
        "validation error must mention cache_repo.cache_size, got: {msg}"
    );
}

#[test]
fn malformed_sweep_interval_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redb"
path = "cache.redb"
cache_size = "256MiB"
sweep_interval = "1x"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.sweep_interval"),
        "validation error must mention cache_repo.sweep_interval, got: {msg}"
    );
}

#[test]
fn zero_sweep_interval_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redb"
path = "cache.redb"
cache_size = "256MiB"
sweep_interval = "0s"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.sweep_interval") && msg.contains("positive"),
        "validation error must mention cache_repo.sweep_interval and require positive, got: {msg}"
    );
}

#[test]
fn malformed_stale_retention_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redb"
path = "cache.redb"
cache_size = "256MiB"
stale_retention = "forever-ish"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.stale_retention"),
        "validation error must mention cache_repo.stale_retention, got: {msg}"
    );
}

// ── Test 4: memory backend with custom max_capacity ───────────────────────

#[tokio::test]
async fn memory_max_capacity_supplied_via_config() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "memory"
max_capacity = 5
"#,
    );

    let ctx = CamelConfig::configure_context(&cfg).await.unwrap();
    let repo = ctx
        .cache_repository("memory")
        .expect("memory cache repository must be present");
    let repo: Arc<dyn CacheRepository> = repo;

    for i in 0..10 {
        repo.set(&format!("k{i}"), entry(), None).await.unwrap();
    }

    // Poll until stable (bounded; moka eviction is async).
    let mut stable = false;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        let stats = repo.stats();
        if stats.entries <= 5 {
            stable = true;
            break;
        }
    }
    assert!(
        stable,
        "entries {} did not stabilize at or below 5 within 50 polls",
        repo.stats().entries
    );
}

// ── Test 5: memory backend defaults to 10_000 when max_capacity omitted ───

#[tokio::test]
async fn memory_max_capacity_defaults_when_omitted() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "memory"
"#,
    );

    let ctx = CamelConfig::configure_context(&cfg).await.unwrap();
    let repo = ctx
        .cache_repository("memory")
        .expect("memory cache repository must be present");
    let repo: Arc<dyn CacheRepository> = repo;

    // Insert several entries; the default 10_000 capacity means none should
    // evict with these small insertions.
    for i in 0..20 {
        repo.set(&format!("k{i}"), entry(), None).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Verify entries are present (moka entry_count() is approximate).
    let val = repo.get("k0").await.unwrap();
    assert!(val.is_some(), "k0 must be in cache after set");
    let val = repo.get("k19").await.unwrap();
    assert!(val.is_some(), "k19 must be in cache after set");
}

// ── Test 6: profile section loads fields ──────────────────────────────────

#[test]
fn profile_section_loads() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("Camel.toml");

    let content = r#"
[default.cache_repo]
backend = "redb"
path = "cache.redb"
cache_size = "256MiB"
max_entries = 500
stale_retention = "24h"
"#;
    std::fs::write(&config_path, content).unwrap();

    let config = CamelConfig::from_file(config_path.to_str().unwrap())
        .expect("Failed to load config with cache_repo profile section");

    let cache = config.cache_repo.expect("cache_repo should be loaded");
    assert_eq!(cache.backend, "redb");
    assert_eq!(cache.path.as_deref(), Some("cache.redb"));
    assert_eq!(cache.max_entries, Some(500));
    assert!(
        cache.stale_retention.is_some(),
        "stale_retention must be set"
    );
}

#[test]
fn profile_section_loads_defaults_when_minimal() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("Camel.toml");

    let content = r#"
[default.cache_repo]
backend = "memory"
"#;
    std::fs::write(&config_path, content).unwrap();

    let config = CamelConfig::from_file(config_path.to_str().unwrap())
        .expect("Failed to load minimal config");

    let cache = config.cache_repo.expect("cache_repo should be loaded");
    assert_eq!(cache.backend, "memory");
    assert!(cache.path.is_none());
    assert!(cache.max_capacity.is_none());
    assert!(cache.max_entries.is_none());
}

// ── Test 7: cross-backend field rejection ─────────────────────────────────

#[test]
fn cache_size_on_memory_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "memory"
cache_size = "512MiB"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.cache_size"),
        "validation error must name cache_repo.cache_size as not applicable, got: {msg}"
    );
}

#[test]
fn path_on_memory_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "memory"
path = "data/cache.redb"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.path"),
        "validation error must name cache_repo.path as not applicable, got: {msg}"
    );
}

#[test]
fn stale_retention_on_memory_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "memory"
stale_retention = "168h"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.stale_retention"),
        "validation error must name cache_repo.stale_retention as not applicable, got: {msg}"
    );
}

#[test]
fn max_entries_on_memory_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "memory"
max_entries = 100
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.max_entries"),
        "validation error must name cache_repo.max_entries as not applicable, got: {msg}"
    );
}

#[test]
fn sweep_interval_on_memory_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "memory"
sweep_interval = "30m"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.sweep_interval"),
        "validation error must name cache_repo.sweep_interval as not applicable, got: {msg}"
    );
}

#[test]
fn max_capacity_on_redb_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redb"
path = "cache.redb"
cache_size = "256MiB"
max_capacity = 5000
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.max_capacity"),
        "validation error must name cache_repo.max_capacity as not applicable, got: {msg}"
    );
}

#[tokio::test]
async fn omitted_stale_retention_stays_none_on_memory() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "memory"
max_capacity = 5000
"#,
    );

    let cache = cfg
        .cache_repo
        .as_ref()
        .expect("cache_repo should be loaded");
    assert!(
        cache.stale_retention.is_none(),
        "omitted stale_retention must deserialize as None on the memory backend"
    );

    let ctx = CamelConfig::configure_context(&cfg).await.unwrap();
    assert!(
        ctx.cache_repository("memory").is_some(),
        "memory cache repository must be present"
    );
}
