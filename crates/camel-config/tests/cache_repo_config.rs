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
        let stats = repo.stats().await;
        if stats.entries <= 5 {
            stable = true;
            break;
        }
    }
    assert!(
        stable,
        "entries {} did not stabilize at or below 5 within 50 polls",
        repo.stats().await.entries
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

// ── Test 8: redis backend validation ──────────────────────────────────────

#[test]
fn cache_redis_no_topology_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.url") && msg.contains("sentinel_nodes"),
        "validation error must name cache_repo.url and sentinel_nodes, got: {msg}"
    );
}

#[test]
fn cache_redis_url_and_sentinel_mutually_exclusive() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
url = "redis://127.0.0.1:6379"
sentinel_nodes = ["s-a:26379"]
master_name = "orders"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.url") && msg.contains("mutually exclusive"),
        "validation error must name cache_repo.url and mutual exclusion, got: {msg}"
    );
}

#[test]
fn cache_redis_empty_sentinel_entry_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
sentinel_nodes = ["s-a:26379", ""]
master_name = "orders"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.sentinel_nodes"),
        "validation error must name cache_repo.sentinel_nodes, got: {msg}"
    );

    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
sentinel_nodes = ["   "]
master_name = "orders"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.sentinel_nodes"),
        "whitespace-only sentinel entry must be rejected, got: {msg}"
    );

    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
sentinel_nodes = []
master_name = "orders"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.sentinel_nodes"),
        "empty sentinel_nodes list must be rejected, got: {msg}"
    );
}

#[test]
fn cache_redis_empty_master_name_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
sentinel_nodes = ["s-a:26379"]
master_name = ""
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.master_name"),
        "validation error must name cache_repo.master_name, got: {msg}"
    );
}

#[test]
fn cache_redis_orphan_master_name_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
url = "redis://127.0.0.1:6379"
master_name = "orders"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.master_name") && msg.contains("sentinel_nodes"),
        "validation error must name cache_repo.master_name and sentinel_nodes, got: {msg}"
    );
}

#[test]
fn cache_redis_orphan_sentinel_password_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
url = "redis://127.0.0.1:6379"
sentinel_password = "hunter2"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.sentinel_password") && msg.contains("sentinel_nodes"),
        "validation error must name cache_repo.sentinel_password and sentinel_nodes, got: {msg}"
    );
}

#[test]
fn cache_redis_invalid_url_scheme_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
url = "http://cache.internal:6379"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.url") && msg.contains("redis://"),
        "validation error must name cache_repo.url and the allowed schemes, got: {msg}"
    );
}

#[test]
fn cache_redis_glob_prefix_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
url = "redis://127.0.0.1:6379"
key_prefix = "camel:*"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.key_prefix"),
        "validation error must name cache_repo.key_prefix, got: {msg}"
    );
}

#[test]
fn cache_redis_redb_fields_not_required() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
url = "redis://127.0.0.1:6379"
"#,
    );

    cfg.validate()
        .expect("minimal redis config (backend + url) must pass validation");
}

#[test]
fn cache_redis_rejects_redb_fields() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
url = "redis://127.0.0.1:6379"
path = "cache.redb"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.path"),
        "validation error must name cache_repo.path, got: {msg}"
    );

    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
url = "redis://127.0.0.1:6379"
cache_size = "256MiB"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.cache_size"),
        "validation error must name cache_repo.cache_size, got: {msg}"
    );
}

#[test]
fn cache_redis_rejects_memory_fields() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
url = "redis://127.0.0.1:6379"
max_capacity = 5000
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.max_capacity"),
        "validation error must name cache_repo.max_capacity, got: {msg}"
    );
}

#[test]
fn cache_memory_rejects_redis_fields() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "memory"
url = "redis://127.0.0.1:6379"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.url"),
        "validation error must name cache_repo.url, got: {msg}"
    );

    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "memory"
key_prefix = "camel:cache"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.key_prefix"),
        "validation error must name cache_repo.key_prefix, got: {msg}"
    );
}

#[test]
fn cache_redb_rejects_redis_fields() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redb"
path = "cache.redb"
cache_size = "256MiB"
key_prefix = "camel:cache"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.key_prefix"),
        "validation error must name cache_repo.key_prefix, got: {msg}"
    );

    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redb"
path = "cache.redb"
cache_size = "256MiB"
url = "redis://127.0.0.1:6379"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.url"),
        "validation error must name cache_repo.url, got: {msg}"
    );

    // The redb arm rejects the data-node fields with the same "does not
    // apply" posture as the other redis-only fields.
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redb"
path = "cache.redb"
cache_size = "256MiB"
password = "hunter2"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.password") && msg.contains("\"redb\""),
        "validation error must name cache_repo.password and \"redb\", got: {msg}"
    );

    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redb"
path = "cache.redb"
cache_size = "256MiB"
username = "svc"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.username") && msg.contains("\"redb\""),
        "validation error must name cache_repo.username and \"redb\", got: {msg}"
    );

    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redb"
path = "cache.redb"
cache_size = "256MiB"
db = 2
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.db") && msg.contains("\"redb\""),
        "validation error must name cache_repo.db and \"redb\", got: {msg}"
    );
}

// ── Sentinel-mode data-node fields (password / username / db) ──────────────

#[test]
fn cache_data_fields_rejected_in_url_mode() {
    // url + password → the data-node credential requires sentinel_nodes
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
url = "redis://127.0.0.1:6379"
password = "hunter2"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.password") && msg.contains("sentinel_nodes"),
        "validation error must name cache_repo.password and sentinel_nodes, got: {msg}"
    );

    // url + db → the data-node database requires sentinel_nodes
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
url = "redis://127.0.0.1:6379"
db = 2
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.db") && msg.contains("sentinel_nodes"),
        "validation error must name cache_repo.db and sentinel_nodes, got: {msg}"
    );

    // url + username → the data-node credential requires sentinel_nodes
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
url = "redis://127.0.0.1:6379"
username = "svc"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.username") && msg.contains("sentinel_nodes"),
        "validation error must name cache_repo.username and sentinel_nodes, got: {msg}"
    );
}

#[test]
fn cache_data_db_out_of_range_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
sentinel_nodes = ["s-a:26379"]
master_name = "mymaster"
db = 20000
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.db") && msg.contains("16383"),
        "validation error must name cache_repo.db and the 16383 limit, got: {msg}"
    );
}

#[test]
fn cache_memory_rejects_data_fields() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "memory"
username = "svc"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.username") && msg.contains("\"memory\""),
        "validation error must name cache_repo.username and \"memory\", got: {msg}"
    );

    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "memory"
password = "hunter2"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.password") && msg.contains("\"memory\""),
        "validation error must name cache_repo.password and \"memory\", got: {msg}"
    );

    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "memory"
db = 2
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.db") && msg.contains("\"memory\""),
        "validation error must name cache_repo.db and \"memory\", got: {msg}"
    );
}

#[test]
fn cache_data_credentials_reach_endpoint() {
    // Sentinel-mode data-node credentials must thread onto the endpoint:
    // password/username for ACL auth, db for database selection.
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
sentinel_nodes = ["s-a:26379"]
master_name = "mymaster"
password = "master-secret"
username = "svc"
db = 2
"#,
    );

    let cache = cfg.cache_repo.expect("cache_repo must load");
    let endpoint = camel_config::redis_endpoint_from_cache_repo(&cache)
        .expect("sentinel cache_repo must build an endpoint");
    assert_eq!(
        endpoint.password.as_deref(),
        Some("master-secret"),
        "data-node password must reach the endpoint"
    );
    assert_eq!(
        endpoint.username.as_deref(),
        Some("svc"),
        "data-node username must reach the endpoint"
    );
    assert_eq!(endpoint.db, 2, "data-node db must reach the endpoint");
}

// ── Test 8b: exhaustive redis-only field rejection on memory/redb ─────────
// Mirrors the idempotent_repo redb-vs-memory rejection matrix: every
// redis-only field must be rejected on the non-redis backends.

#[test]
fn url_on_memory_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "memory"
url = "redis://127.0.0.1:6379"
"#,
    );
    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.url") && msg.contains("\"memory\""),
        "validation error must name cache_repo.url and \"memory\", got: {msg}"
    );
}

#[test]
fn sentinel_nodes_on_memory_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "memory"
sentinel_nodes = ["s-a:26379"]
master_name = "mymaster"
"#,
    );
    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.sentinel_nodes") && msg.contains("\"memory\""),
        "validation error must name cache_repo.sentinel_nodes and \"memory\", got: {msg}"
    );
}

#[test]
fn master_name_on_memory_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "memory"
master_name = "mymaster"
"#,
    );
    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.master_name") && msg.contains("\"memory\""),
        "validation error must name cache_repo.master_name and \"memory\", got: {msg}"
    );
}

#[test]
fn sentinel_username_on_memory_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "memory"
sentinel_username = "admin"
"#,
    );
    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.sentinel_username") && msg.contains("\"memory\""),
        "validation error must name cache_repo.sentinel_username and \"memory\", got: {msg}"
    );
}

#[test]
fn sentinel_password_on_memory_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "memory"
sentinel_password = "hunter2"
"#,
    );
    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.sentinel_password") && msg.contains("\"memory\""),
        "validation error must name cache_repo.sentinel_password and \"memory\", got: {msg}"
    );
}

#[test]
fn key_prefix_on_memory_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "memory"
key_prefix = "camel:cache"
"#,
    );
    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.key_prefix") && msg.contains("\"memory\""),
        "validation error must name cache_repo.key_prefix and \"memory\", got: {msg}"
    );
}

#[test]
fn url_on_redb_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redb"
path = "cache.redb"
cache_size = "256MiB"
url = "redis://127.0.0.1:6379"
"#,
    );
    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.url") && msg.contains("\"redb\""),
        "validation error must name cache_repo.url and \"redb\", got: {msg}"
    );
}

#[test]
fn sentinel_nodes_on_redb_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redb"
path = "cache.redb"
cache_size = "256MiB"
sentinel_nodes = ["s-a:26379"]
master_name = "mymaster"
"#,
    );
    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.sentinel_nodes") && msg.contains("\"redb\""),
        "validation error must name cache_repo.sentinel_nodes and \"redb\", got: {msg}"
    );
}

#[test]
fn master_name_on_redb_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redb"
path = "cache.redb"
cache_size = "256MiB"
master_name = "mymaster"
"#,
    );
    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.master_name") && msg.contains("\"redb\""),
        "validation error must name cache_repo.master_name and \"redb\", got: {msg}"
    );
}

#[test]
fn sentinel_username_on_redb_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redb"
path = "cache.redb"
cache_size = "256MiB"
sentinel_username = "admin"
"#,
    );
    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.sentinel_username") && msg.contains("\"redb\""),
        "validation error must name cache_repo.sentinel_username and \"redb\", got: {msg}"
    );
}

#[test]
fn sentinel_password_on_redb_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redb"
path = "cache.redb"
cache_size = "256MiB"
sentinel_password = "hunter2"
"#,
    );
    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.sentinel_password") && msg.contains("\"redb\""),
        "validation error must name cache_repo.sentinel_password and \"redb\", got: {msg}"
    );
}

#[test]
fn key_prefix_on_redb_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redb"
path = "cache.redb"
cache_size = "256MiB"
key_prefix = "camel:cache"
"#,
    );
    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.key_prefix") && msg.contains("\"redb\""),
        "validation error must name cache_repo.key_prefix and \"redb\", got: {msg}"
    );
}

#[test]
fn redis_with_url_accepted() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
url = "redis://127.0.0.1:6379"
"#,
    );
    cfg.validate()
        .expect("redis backend with url must pass validation");
}

#[test]
fn redis_with_key_prefix_accepted() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
url = "redis://127.0.0.1:6379"
key_prefix = "camel:cache"
"#,
    );
    cfg.validate()
        .expect("redis backend with key_prefix must pass validation");
}

#[test]
fn redis_with_sentinel_fields_accepted() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
sentinel_nodes = ["s-a:26379"]
master_name = "mymaster"
sentinel_username = "admin"
sentinel_password = "hunter2"
password = "master-secret"
username = "svc"
db = 2
key_prefix = "camel:cache"
"#,
    );
    cfg.validate()
        .expect("redis backend with sentinel fields must pass validation");
}

#[test]
fn cache_redis_malformed_stale_retention_rejected() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
url = "redis://127.0.0.1:6379"
stale_retention = "forever-ish"
"#,
    );

    let err = cfg
        .validate()
        .expect_err("malformed stale_retention must fail at validate() for the redis backend");
    assert!(
        err.to_string().contains("cache_repo.stale_retention"),
        "error must name the field, got: {err}"
    );
}

// ── Test 9: Debug redaction of redis credentials ──────────────────────────

#[test]
fn cache_debug_redacts_credentials() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
url = "redis://user:secret@h:6379"
sentinel_password = "hunter2"
"#,
    );

    let rendered = format!("{:?}", cfg.cache_repo.expect("cache_repo must load"));
    assert!(
        !rendered.contains("secret") && !rendered.contains("hunter2"),
        "Debug output must not leak credentials, got: {rendered}"
    );
    assert!(
        rendered.contains("redis://***@h:6379"),
        "Debug output must redact URL userinfo as ***, got: {rendered}"
    );
}

#[test]
fn cache_debug_redacts_data_credentials() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
sentinel_nodes = ["s-a:26379"]
master_name = "mymaster"
password = "master-secret"
username = "svc-user"
"#,
    );

    let rendered = format!("{:?}", cfg.cache_repo.expect("cache_repo must load"));
    assert!(
        !rendered.contains("master-secret") && !rendered.contains("svc-user"),
        "Debug output must not leak data-node credentials, got: {rendered}"
    );
    assert!(
        rendered.contains("password: Some(\"***\")")
            && rendered.contains("username: Some(\"***\")"),
        "Debug output must render the data-node credentials redacted, got: {rendered}"
    );
}

// ── Test 10: redis cache repository wiring ────────────────────────────────

#[tokio::test]
async fn redis_cache_arm_unreachable_url_fails_build_with_named_error() {
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
url = "redis://127.0.0.1:1/0"
"#,
    );

    let err = match CamelConfig::configure_context(&cfg).await {
        Ok(_) => panic!("an unreachable redis url must fail the context build"),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("cache_repo"),
        "build error must name cache_repo, got: {err}"
    );
}

// ── db-in-path url grammar (Phase-3 review finding 1) ─────────────────────

#[test]
fn redis_url_db_in_path_rejected_at_validate() {
    // The component URI dialect selects the database with the `?db=N` query
    // parameter; a `/N` path suffix fails the grammar (`invalid port
    // '6379/0'`). validate() deep-parses the url with the same parser used
    // at registration, so the rejection surfaces here, not at build time.
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
url = "redis://127.0.0.1:6379/0"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("cache_repo.url") && msg.contains("invalid port '6379/0'"),
        "validation error must name cache_repo.url and the invalid port, got: {msg}"
    );

    let cfg = make_cfg(
        r#"
[idempotent_repo]
backend = "redis"
url = "redis://127.0.0.1:6379/0"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("idempotent_repo.url") && msg.contains("invalid port '6379/0'"),
        "validation error must name idempotent_repo.url and the invalid port, got: {msg}"
    );
}

#[test]
fn example_camel_toml_round_trips_validate_and_endpoints() {
    // The checked-in example config must load, validate, and map to both
    // redis endpoints — the exact path `cargo run` exercises at startup.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/redis-repositories/Camel.toml");
    let cfg = CamelConfig::from_file(path.to_str().expect("path must be utf-8"))
        .unwrap_or_else(|e| panic!("example Camel.toml must load: {e}"));

    cfg.validate()
        .expect("checked-in example must pass validate()");

    let cache = cfg.cache_repo.as_ref().expect("example sets cache_repo");
    let idem = cfg
        .idempotent_repo
        .as_ref()
        .expect("example sets idempotent_repo");
    camel_config::redis_endpoint_from_cache_repo(cache)
        .expect("example cache_repo.url must build a redis endpoint");
    camel_config::redis_endpoint_from_idempotent_repo(idem)
        .expect("example idempotent_repo.url must build a redis endpoint");
}
