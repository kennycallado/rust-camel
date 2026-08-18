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
async fn redb_registered_when_backend_redb() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("cache.redb");
    let toml = format!(
        r#"
[cache_repo]
backend = "redb"
path = "{}"
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

    let err = cfg
        .validate()
        .expect_err("validation must fail when backend=redb without path");
    let msg = err.to_string();
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

    let err = cfg
        .validate()
        .expect_err("validation must fail when backend=redb with empty path");
    let msg = err.to_string();
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

    let err = cfg
        .validate()
        .expect_err("validation must fail for unknown backend");
    let msg = err.to_string();
    assert!(
        msg.contains("postgres") || msg.contains("backend"),
        "validation error must mention the invalid backend, got: {msg}"
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
