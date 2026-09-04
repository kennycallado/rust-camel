use super::*;

fn write_temp_config(contents: &str) -> tempfile::NamedTempFile {
    use std::io::Write;
    let mut f = tempfile::NamedTempFile::new().expect("temp file");
    f.write_all(contents.as_bytes()).expect("write config");
    f
}

#[test]
fn test_merge_toml_values_merges_nested_tables() {
    let mut base: toml::Value = toml::from_str(
        r#"
[components.http]
connect_timeout_ms = 1000
pool_max_idle_per_host = 50
"#,
    )
    .unwrap();

    let overlay: toml::Value = toml::from_str(
        r#"
[components.http]
response_timeout_ms = 2000
pool_max_idle_per_host = 99
"#,
    )
    .unwrap();

    merge_toml_values(&mut base, &overlay);

    let http = base
        .get("components")
        .and_then(|v| v.get("http"))
        .expect("merged http table");
    assert_eq!(
        http.get("connect_timeout_ms").and_then(|v| v.as_integer()),
        Some(1000)
    );
    assert_eq!(
        http.get("response_timeout_ms").and_then(|v| v.as_integer()),
        Some(2000)
    );
    assert_eq!(
        http.get("pool_max_idle_per_host")
            .and_then(|v| v.as_integer()),
        Some(99)
    );
}

#[test]
fn test_from_file_with_profile_merges_default_and_profile() {
    let file = write_temp_config(
        r#"
[default]
watch = false
[default.components.http]
connect_timeout_ms = 1000
pool_max_idle_per_host = 50

[prod]
watch = true
[prod.components.http]
pool_max_idle_per_host = 200
"#,
    );

    let cfg = CamelConfig::from_file_with_profile(file.path().to_str().unwrap(), Some("prod"))
        .expect("config should load");

    assert!(cfg.watch);
    let http = cfg.components.raw.get("http").expect("http config");
    assert_eq!(
        http.get("connect_timeout_ms").and_then(|v| v.as_integer()),
        Some(1000)
    );
    assert_eq!(
        http.get("pool_max_idle_per_host")
            .and_then(|v| v.as_integer()),
        Some(200)
    );
}

#[test]
fn test_from_file_with_profile_uses_profile_when_no_default() {
    let file = write_temp_config(
        r#"
[dev]
watch = true
timeout_ms = 777
"#,
    );

    let cfg = CamelConfig::from_file_with_profile(file.path().to_str().unwrap(), Some("dev"))
        .expect("config should load");
    assert!(cfg.watch);
    assert_eq!(cfg.timeout_ms, 777);
}

#[test]
fn redb_idempotent_config_loads_via_profile_section() {
    // The real Camel.toml uses profiles: the field must live under the
    // active profile as [default.idempotent_repo] (mirrors [default.supervision]),
    // NOT top-level. This verifies the profile path that the flat-parsing
    // redb_idempotent_config_* tests above do not cover.
    let file = write_temp_config(
        r#"
[default]
[default.idempotent_repo]
path = "profile.redb"
durability = "eventual"
"#,
    );

    let cfg = CamelConfig::from_file_with_profile(file.path().to_str().unwrap(), Some("default"))
        .expect("config should load");
    let repo = cfg
        .idempotent_repo
        .expect("idempotent_repo should populate via [default.idempotent_repo]");
    assert_eq!(repo.backend, "redb");
    assert_eq!(repo.path.as_deref(), Some("profile.redb"));
    assert_eq!(repo.durability.as_deref(), Some("eventual"));
}

#[test]
fn test_from_file_with_profile_unknown_profile_returns_error() {
    let file = write_temp_config(
        r#"
[default]
watch = false
"#,
    );

    let err = CamelConfig::from_file_with_profile(file.path().to_str().unwrap(), Some("qa"))
        .expect_err("should fail");
    assert!(err.to_string().contains("Unknown profile: qa"));
}

#[test]
fn test_from_file_without_profile_uses_default_section() {
    let file = write_temp_config(
        r#"
[default]
watch = true
timeout_ms = 321
"#,
    );

    let cfg = CamelConfig::from_file(file.path().to_str().unwrap()).expect("config should load");
    assert!(cfg.watch);
    assert_eq!(cfg.timeout_ms, 321);
}

#[test]
fn test_from_file_with_env_overrides_timeout() {
    // Serialize against async env-reading tests (see ENV_OVERRIDE_LOCK).
    let _guard = super::env_lock();

    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000
"#,
    );

    // SAFETY: tests run in controlled process; we set and immediately restore env var.
    unsafe {
        std::env::set_var("CAMEL_TIMEOUT_MS", "9999");
    }

    let cfg = CamelConfig::from_file_with_env(file.path().to_str().unwrap())
        .expect("config should load with env override");
    assert_eq!(cfg.timeout_ms, 9999);

    // SAFETY: restore process env for test isolation.
    unsafe {
        std::env::remove_var("CAMEL_TIMEOUT_MS");
    }
}

#[test]
fn env_override_allows_supervision_nested_field() {
    // Serialize against async env-reading tests (see ENV_OVERRIDE_LOCK).
    let _guard = super::env_lock();

    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.supervision]
initial_delay_ms = 500
max_attempts = 3
"#,
    );

    // SAFETY: tests run in controlled process; we set and immediately restore env vars.
    unsafe {
        std::env::set_var("CAMEL_SUPERVISION_INITIAL_DELAY_MS", "2000");
        std::env::set_var("CAMEL_SUPERVISION_MAX_ATTEMPTS", "10");
    }

    let cfg = CamelConfig::from_file_with_env(file.path().to_str().unwrap())
        .expect("config should load with env override");
    let sup = cfg.supervision.expect("supervision should be present");
    assert_eq!(sup.initial_delay_ms, 2000);
    assert_eq!(sup.max_attempts, Some(10));

    // SAFETY: restore process env for test isolation.
    unsafe {
        std::env::remove_var("CAMEL_SUPERVISION_INITIAL_DELAY_MS");
        std::env::remove_var("CAMEL_SUPERVISION_MAX_ATTEMPTS");
    }
}

#[test]
fn env_override_allows_runtime_journal_nested_fields() {
    // Serialize against async env-reading tests (see ENV_OVERRIDE_LOCK).
    let _guard = super::env_lock();

    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.runtime_journal]
path = "journal.db"
durability = "immediate"
compaction_threshold_events = 10000
"#,
    );

    // SAFETY: tests run in controlled process; we set and immediately restore env vars.
    unsafe {
        std::env::set_var("CAMEL_RUNTIME_JOURNAL_PATH", "/override/journal.db");
        std::env::set_var("CAMEL_RUNTIME_JOURNAL_DURABILITY", "eventual");
        std::env::set_var("CAMEL_RUNTIME_JOURNAL_COMPACTION_THRESHOLD_EVENTS", "5000");
    }

    let cfg = CamelConfig::from_file_with_env(file.path().to_str().unwrap())
        .expect("config should load with env override");
    let journal = cfg
        .runtime_journal
        .expect("runtime_journal should be present");
    assert_eq!(
        journal.path,
        std::path::PathBuf::from("/override/journal.db")
    );
    assert_eq!(journal.durability, super::JournalDurability::Eventual);
    assert_eq!(journal.compaction_threshold_events, 5000);

    // SAFETY: restore process env for test isolation.
    unsafe {
        std::env::remove_var("CAMEL_RUNTIME_JOURNAL_PATH");
        std::env::remove_var("CAMEL_RUNTIME_JOURNAL_DURABILITY");
        std::env::remove_var("CAMEL_RUNTIME_JOURNAL_COMPACTION_THRESHOLD_EVENTS");
    }
}

#[test]
fn env_override_allows_idempotent_repo_nested_fields() {
    // Serialize against async env-reading tests (see ENV_OVERRIDE_LOCK).
    let _guard = super::env_lock();

    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.idempotent_repo]
path = "idempotent.db"
durability = "immediate"
"#,
    );

    // SAFETY: tests run in controlled process; we set and immediately restore env vars.
    unsafe {
        std::env::set_var("CAMEL_IDEMPOTENT_REPO_PATH", "/override/idempotent.db");
        std::env::set_var("CAMEL_IDEMPOTENT_REPO_DURABILITY", "eventual");
    }

    let cfg = CamelConfig::from_file_with_env(file.path().to_str().unwrap())
        .expect("config should load with env override");
    let repo = cfg
        .idempotent_repo
        .expect("idempotent_repo should be present");
    assert_eq!(repo.path.as_deref(), Some("/override/idempotent.db"));
    assert_eq!(repo.durability.as_deref(), Some("eventual"));

    // SAFETY: restore process env for test isolation.
    unsafe {
        std::env::remove_var("CAMEL_IDEMPOTENT_REPO_PATH");
        std::env::remove_var("CAMEL_IDEMPOTENT_REPO_DURABILITY");
    }
}

#[test]
fn env_allowlist_accepts_allowlisted_ignores_non_allowlisted() {
    // Serialize against async env-reading tests (see ENV_OVERRIDE_LOCK).
    let _guard = super::env_lock();

    let file = write_temp_config(
        r#"
[default]
drain_timeout_ms = 5000
"#,
    );

    // SAFETY: tests run in controlled process; we set and immediately restore env vars.
    unsafe {
        // Allowlisted — should override.
        std::env::set_var("CAMEL_DRAIN_TIMEOUT_MS", "999");
        // Non-allowlisted — should be silently ignored, not crash.
        std::env::set_var("CAMEL_BEANS_FOO", "bar");
    }

    let cfg = CamelConfig::from_file_with_env(file.path().to_str().unwrap())
        .expect("config should load with allowlisted env var and ignore non-allowlisted");
    // Allowlisted drain_timeout_ms should be overridden to 999.
    assert_eq!(cfg.drain_timeout_ms, 999);
    // Beans should remain at default (empty), unaffected by non-allowlisted CAMEL_BEANS_FOO.
    assert!(cfg.beans.is_empty());

    // SAFETY: restore process env for test isolation.
    unsafe {
        std::env::remove_var("CAMEL_DRAIN_TIMEOUT_MS");
        std::env::remove_var("CAMEL_BEANS_FOO");
    }
}

#[test]
fn test_from_file_resolves_placeholders_in_components_and_beans() {
    let file = write_temp_config(
        r#"
[default]
routes = ["${env:RUST_CAMEL_TEST_ROUTE:-routes/default.yaml}"]

[default.components.http]
base_url = "${env:RUST_CAMEL_TEST_BASE_URL:-http://localhost:8080}"

[default.beans.auth]
plugin = "${env:RUST_CAMEL_TEST_PLUGIN:-test-auth}"

[default.beans.auth.config]
token = "${env:RUST_CAMEL_TEST_TOKEN:-abc123}"
"#,
    );

    let cfg = CamelConfig::from_file(file.path().to_str().unwrap()).expect("config should load");

    assert_eq!(cfg.routes, vec!["routes/default.yaml"]);
    let http = cfg.components.raw.get("http").expect("http config");
    assert_eq!(
        http.get("base_url").and_then(|v| v.as_str()),
        Some("http://localhost:8080")
    );
    let bean = cfg.beans.get("auth").expect("bean auth");
    assert_eq!(bean.plugin, "test-auth");
    assert_eq!(bean.config.get("token").map(String::as_str), Some("abc123"));
}

// The successor of `test_from_file_unresolved_placeholder_keeps_original_string`
// (legacy warn-and-keep passthrough, retired) lives in
// `tests/placeholder_e2e.rs::legacy_braces_rejected_on_load_path`: the
// walk-level rejection is covered by `tests/placeholder_walk.rs`.

// ---------------------------------------------------------------------------
// Sealed scenario loader (ADR-0069 section 4: environment parity and
// hermeticity)
// ---------------------------------------------------------------------------

/// The sealed loader ignores allowlisted `CAMEL_*` ambient overrides: an
/// ambient `CAMEL_LOG_LEVEL` set in the test process must not reach the
/// scenario's loaded config (hermeticity seal, ADR-0069 section 4). The
/// unsealed `from_file_with_env` entry applies the same override as the
/// control arm.
#[test]
fn from_file_sealed_ignores_ambient_env_overrides() {
    let _guard = super::env_lock();
    let file = write_temp_config(
        r#"
[default]
log_level = "info"
"#,
    );
    let path = file.path().to_str().unwrap().to_string();

    super::set_env("CAMEL_LOG_LEVEL", "trace");
    let sealed = CamelConfig::from_file_sealed(&path, "default", &|_| None);
    let unsealed = CamelConfig::from_file_with_env(&path);
    super::unset_env("CAMEL_LOG_LEVEL");

    let sealed = sealed.expect("sealed load must succeed");
    assert_eq!(
        sealed.log_level, "info",
        "ambient CAMEL_LOG_LEVEL must not reach the sealed config"
    );
    let unsealed = unsealed.expect("unsealed control load must succeed");
    assert_eq!(
        unsealed.log_level, "trace",
        "control: the unsealed entry applies the ambient override"
    );
}

/// The sealed loader pins the profile by value: an ambient `CAMEL_PROFILE`
/// must not leak into profile selection (hermeticity seal, ADR-0069
/// section 4).
#[test]
fn from_file_sealed_ignores_ambient_profile() {
    let _guard = super::env_lock();
    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[production]
timeout_ms = 9999
"#,
    );
    let path = file.path().to_str().unwrap().to_string();

    super::set_env("CAMEL_PROFILE", "production");
    let sealed = CamelConfig::from_file_sealed(&path, "default", &|_| None);
    super::unset_env("CAMEL_PROFILE");

    let sealed = sealed.expect("sealed load must succeed");
    assert_eq!(
        sealed.timeout_ms, 1000,
        "ambient CAMEL_PROFILE must not leak into the sealed config"
    );
}
