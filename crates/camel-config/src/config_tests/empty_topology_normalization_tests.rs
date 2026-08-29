use super::*;

fn write_temp_config(contents: &str) -> tempfile::NamedTempFile {
    use std::io::Write;
    let mut f = tempfile::NamedTempFile::new().expect("temp file");
    f.write_all(contents.as_bytes()).expect("write config");
    f
}

#[test]
fn normalize_blank_string_topology_fields_to_none() {
    let mut repo = CacheRepoConfig {
        url: Some(String::new()),
        master_name: Some("   ".to_string()),
        username: Some(String::new()),
        sentinel_username: Some("\t".to_string()),
        key_prefix: Some(String::new()),
        sentinel_nodes: Some(Vec::new()),
        password: Some(String::new()),
        sentinel_password: Some("  ".to_string()),
        ..CacheRepoConfig::default()
    };
    repo.normalize_empty_topology();
    assert!(repo.url.is_none(), "empty url must normalize to None");
    assert!(
        repo.master_name.is_none(),
        "whitespace-only master_name must normalize to None"
    );
    assert!(repo.username.is_none());
    assert!(repo.sentinel_username.is_none());
    assert!(repo.key_prefix.is_none());
    assert!(
        repo.sentinel_nodes.is_none(),
        "empty sentinel array must normalize to None"
    );
    // Blank passwords are not credentials: an expanded-empty
    // `${env:PW:-}` placeholder means "unset".
    assert!(
        repo.password.is_none(),
        "blank password must normalize to None"
    );
    assert!(
        repo.sentinel_password.is_none(),
        "whitespace-only sentinel_password must normalize to None"
    );
    // Non-blank credentials are never dropped.
    repo.password = Some("real".to_string());
    repo.sentinel_password = Some("real-sentinel".to_string());
    repo.normalize_empty_topology();
    assert_eq!(repo.password.as_deref(), Some("real"));
    assert_eq!(repo.sentinel_password.as_deref(), Some("real-sentinel"));
}

#[test]
fn normalize_all_blank_sentinel_array_to_none() {
    let mut repo = CacheRepoConfig {
        sentinel_nodes: Some(vec![" ".to_string(), String::new()]),
        ..CacheRepoConfig::default()
    };
    repo.normalize_empty_topology();
    assert!(
        repo.sentinel_nodes.is_none(),
        "all-blank sentinel array must normalize to None"
    );
}

#[test]
fn mixed_blank_sentinel_array_not_normalized() {
    let mut repo = CacheRepoConfig {
        backend: "redis".to_string(),
        sentinel_nodes: Some(vec!["redis-a:26379".to_string(), " ".to_string()]),
        master_name: Some("m".to_string()),
        ..CacheRepoConfig::default()
    };
    repo.normalize_empty_topology();
    let nodes = repo
        .sentinel_nodes
        .as_ref()
        .expect("mixed blank/non-blank array must stay Some");
    assert_eq!(nodes.len(), 2);
    let config = CamelConfig {
        cache_repo: Some(repo),
        ..CamelConfig::default()
    };
    let err = config
        .validate()
        .expect_err("mixed array must keep failing validation loudly");
    let msg = err.to_string();
    assert!(
        msg.contains("sentinel node entries must be non-empty"),
        "expected the non-empty-entry sentinel message, got: {msg}"
    );
}

#[test]
fn mixed_blank_entries_fail_through_loader() {
    // Loader-level guard for design Decision 2: normalization removes
    // only ALL-blank sentinel arrays; a mixed array must still fail the
    // full from_file pipeline (loaded and validated), never be masked.
    let _guard = super::env_lock();
    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.cache_repo]
backend = "redis"
sentinel_nodes = ["redis-a:26379", " "]
master_name = "m"
"#,
    );
    let err = CamelConfig::from_file(file.path().to_str().unwrap())
        .expect_err("mixed blank/non-blank sentinel array must fail through the loader");
    let msg = err.to_string();
    assert!(
        msg.contains("sentinel node entries must be non-empty"),
        "expected the non-empty-entry sentinel message, got: {msg}"
    );
}

#[test]
fn idempotent_repo_normalize_parity() {
    // Unit: same normalization rules as cache_repo.
    let mut repo = IdempotentRepoConfig {
        backend: "redis".to_string(),
        path: None,
        durability: None,
        url: Some(String::new()),
        sentinel_nodes: Some(vec!["idem-node-a:26379".to_string()]),
        master_name: Some("mymaster".to_string()),
        sentinel_username: None,
        sentinel_password: None,
        password: None,
        username: None,
        db: None,
        key_prefix: None,
    };
    repo.normalize_empty_topology();
    assert!(repo.url.is_none());
    assert_eq!(
        repo.sentinel_nodes,
        Some(vec!["idem-node-a:26379".to_string()])
    );

    // Pipeline: an env-expanded-empty url alongside populated sentinel
    // fields loads and validates as a sentinel topology.
    let _guard = super::env_lock();
    super::unset_env("RC_TEST_IDEM_URL");
    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.idempotent_repo]
backend = "redis"
url = "${env:RC_TEST_IDEM_URL:-}"
sentinel_nodes = ["idem-node-b:26379"]
master_name = "mymaster"
"#,
    );
    let cfg = CamelConfig::from_file(file.path().to_str().unwrap()).expect("config should load");
    let repo = cfg.idempotent_repo.expect("idempotent_repo section");
    assert!(repo.url.is_none(), "expanded-empty url must be absent");
    assert_eq!(repo.master_name.as_deref(), Some("mymaster"));
}

#[test]
fn blank_key_prefix_selects_default() {
    // Pipeline: `${env:...:-}` expanding empty normalizes away so the
    // repository default prefix applies.
    let _guard = super::env_lock();
    super::unset_env("RC_TEST_PREFIX");
    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.cache_repo]
backend = "redis"
url = "redis://localhost:6379"
key_prefix = "${env:RC_TEST_PREFIX:-}"
"#,
    );
    let cfg = CamelConfig::from_file(file.path().to_str().unwrap()).expect("config should load");
    let repo = cfg.cache_repo.as_ref().expect("cache_repo section");
    assert!(
        repo.key_prefix.is_none(),
        "blank key_prefix must select the default prefix"
    );

    // Unit: a non-blank prefix value is never normalized away — even an
    // invalid one (rejection stays keyspace validation's job).
    let mut repo = CacheRepoConfig {
        key_prefix: Some("bad*prefix".to_string()),
        ..CacheRepoConfig::default()
    };
    repo.normalize_empty_topology();
    assert_eq!(repo.key_prefix.as_deref(), Some("bad*prefix"));
}

#[test]
fn invalid_key_prefix_still_rejected_by_keyspace() {
    // Pins scenario 7's second THEN clause: a non-blank prefix survives
    // normalization untouched (normalization is not a validator) and the
    // namespace-token check inside the redis topology validator rejects
    // the glob metacharacter.
    let repo = CacheRepoConfig {
        backend: "redis".to_string(),
        sentinel_nodes: Some(vec!["redis-a:26379".to_string()]),
        master_name: Some("m".to_string()),
        key_prefix: Some("bad*prefix".to_string()),
        ..CacheRepoConfig::default()
    };
    let config = CamelConfig {
        cache_repo: Some(repo),
        ..CamelConfig::default()
    };
    let err = config
        .validate()
        .expect_err("glob metacharacter in key_prefix must stay rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("cache_repo.key_prefix") && msg.contains("glob metacharacters are forbidden"),
        "expected the namespace-token rejection, got: {msg}"
    );
}

#[test]
fn sentinel_topology_selected_by_empty_expanded_url() {
    let _guard = super::env_lock();
    super::unset_env("RC_TEST_REDIS_URL");
    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.cache_repo]
backend = "redis"
url = "${env:RC_TEST_REDIS_URL:-}"
sentinel_nodes = ["node-a:26379"]
master_name = "m"
"#,
    );
    let cfg = CamelConfig::from_file(file.path().to_str().unwrap())
        .expect("sentinel topology must validate when url expands empty");
    let repo = cfg.cache_repo.as_ref().expect("cache_repo section");
    assert!(repo.url.is_none());
    assert_eq!(repo.sentinel_nodes, Some(vec!["node-a:26379".to_string()]));
    super::unset_env("RC_TEST_REDIS_URL");
}

#[test]
fn standalone_topology_selected_by_populated_url() {
    let _guard = super::env_lock();
    super::set_env("RC_TEST_REDIS_URL", "redis://host:6379");
    super::unset_env("RC_TEST_NODES_0");
    super::unset_env("RC_TEST_MASTER");
    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.cache_repo]
backend = "redis"
url = "${env:RC_TEST_REDIS_URL:-}"
sentinel_nodes = ["${env:RC_TEST_NODES_0:-}"]
master_name = "${env:RC_TEST_MASTER:-}"
"#,
    );
    let cfg = CamelConfig::from_file(file.path().to_str().unwrap())
        .expect("standalone topology must validate");
    let repo = cfg.cache_repo.as_ref().expect("cache_repo section");
    assert_eq!(repo.url.as_deref(), Some("redis://host:6379"));
    assert!(
        repo.sentinel_nodes.is_none(),
        "all-blank sentinel array from env expansion must normalize to None"
    );
    assert!(
        repo.master_name.is_none(),
        "expanded-empty master_name must normalize to None"
    );
    super::unset_env("RC_TEST_REDIS_URL");
    super::unset_env("RC_TEST_NODES_0");
    super::unset_env("RC_TEST_MASTER");
}

#[test]
fn blank_password_placeholder_selecting_standalone_validates() {
    let _guard = super::env_lock();
    super::set_env("RC_TEST_REDIS_URL", "redis://host:6379");
    super::unset_env("RC_TEST_PW");
    super::unset_env("RC_TEST_NODES_0");
    super::unset_env("RC_TEST_MASTER");
    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.cache_repo]
backend = "redis"
url = "${env:RC_TEST_REDIS_URL:-}"
password = "${env:RC_TEST_PW:-}"
sentinel_password = "${env:RC_TEST_PW:-}"
sentinel_nodes = ["${env:RC_TEST_NODES_0:-}"]
master_name = "${env:RC_TEST_MASTER:-}"
"#,
    );
    let cfg = CamelConfig::from_file(file.path().to_str().unwrap())
        .expect("blank password placeholders must not fail standalone selection");
    let repo = cfg.cache_repo.as_ref().expect("cache_repo section");
    assert!(
        repo.password.is_none(),
        "expanded-empty password must normalize to None"
    );
    assert!(
        repo.sentinel_password.is_none(),
        "expanded-empty sentinel_password must normalize to None"
    );
    assert!(cfg.validate().is_ok(), "standalone topology must validate");
    super::unset_env("RC_TEST_REDIS_URL");
    super::unset_env("RC_TEST_PW");
    super::unset_env("RC_TEST_NODES_0");
    super::unset_env("RC_TEST_MASTER");
}

#[test]
fn literal_empty_url_in_file_treated_as_unset() {
    let _guard = super::env_lock();
    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.cache_repo]
backend = "redis"
url = ""
sentinel_nodes = ["node-a:26379"]
master_name = "m"
"#,
    );
    let cfg = CamelConfig::from_file(file.path().to_str().unwrap())
        .expect("literal empty url must behave like an unset key");
    let repo = cfg.cache_repo.as_ref().expect("cache_repo section");
    assert!(repo.url.is_none());
    assert_eq!(repo.sentinel_nodes, Some(vec!["node-a:26379".to_string()]));
}

#[test]
fn both_topologies_absent_still_fails() {
    let _guard = super::env_lock();
    super::unset_env("RC_TEST_MISSING");
    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.cache_repo]
backend = "redis"
url = "${env:RC_TEST_MISSING:-}"
"#,
    );
    let err = CamelConfig::from_file(file.path().to_str().unwrap())
        .expect_err("a redis section without any topology must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("requires a topology"),
        "expected the requires-a-topology error, got: {msg}"
    );
}

#[test]
fn memory_backend_empty_url_still_rejected() {
    // Normalization is backend-gated to redis: an empty literal url on a
    // memory section keeps today's cross-backend rejection instead of
    // being silently legitimized as absent.
    let _guard = super::env_lock();
    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.cache_repo]
backend = "memory"
url = ""
"#,
    );
    let err = CamelConfig::from_file(file.path().to_str().unwrap())
        .expect_err("url must stay rejected on the memory backend");
    let msg = err.to_string();
    assert!(
        msg.contains(r#"cache_repo.url does not apply to the "memory" backend"#),
        "expected the cross-backend rejection, got: {msg}"
    );
}

#[test]
fn redb_backend_empty_url_still_rejected() {
    let _guard = super::env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let cache_path = dir.path().join("cache.redb");
    let idem_path = dir.path().join("idem.redb");

    // cache_repo: empty literal url keeps today's cross-backend rejection.
    let file = write_temp_config(&format!(
        r#"
[default]
timeout_ms = 1000

[default.cache_repo]
backend = "redb"
path = "{}"
cache_size = "64MiB"
url = ""
"#,
        cache_path.display()
    ));
    let err = CamelConfig::from_file(file.path().to_str().unwrap())
        .expect_err("url must stay rejected on the redb cache backend");
    let msg = err.to_string();
    assert!(
        msg.contains(r#"cache_repo.url does not apply to the "redb" backend"#),
        "expected the cache_repo cross-backend rejection, got: {msg}"
    );

    // idempotent_repo: identical parity for the shared topology shape.
    let file = write_temp_config(&format!(
        r#"
[default]
timeout_ms = 1000

[default.idempotent_repo]
backend = "redb"
path = "{}"
url = ""
"#,
        idem_path.display()
    ));
    let err = CamelConfig::from_file(file.path().to_str().unwrap())
        .expect_err("url must stay rejected on the redb idempotent backend");
    let msg = err.to_string();
    assert!(
        msg.contains(r#"idempotent_repo.url does not apply to the "redb" backend"#),
        "expected the idempotent_repo cross-backend rejection, got: {msg}"
    );
}
