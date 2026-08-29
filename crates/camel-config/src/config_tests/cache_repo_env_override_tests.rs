use super::*;
use crate::config::log_capture::capture_warns;

fn write_temp_config(contents: &str) -> tempfile::NamedTempFile {
    use std::io::Write;
    let mut f = tempfile::NamedTempFile::new().expect("temp file");
    f.write_all(contents.as_bytes()).expect("write config");
    f
}

#[test]
fn scalar_override_db_applied() {
    let _guard = super::env_lock();
    super::unset_env("CAMEL_PROFILE");
    super::unset_env("CAMEL_CACHE_REPO_DB");
    set_env("CAMEL_CACHE_REPO_DB", "3");

    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.cache_repo]
backend = "redis"
sentinel_nodes = ["n:26379"]
master_name = "m"
db = 0
"#,
    );

    let loaded = CamelConfig::from_file_with_env(file.path().to_str().unwrap());
    unset_env("CAMEL_CACHE_REPO_DB");

    let cfg = loaded.expect("config should load with the db override");
    let repo = cfg.cache_repo.as_ref().expect("cache_repo section");
    assert_eq!(repo.db, Some(3), "env override must replace the file db");
    assert!(cfg.validate().is_ok(), "overridden config must validate");
}

#[test]
fn empty_scalar_override_preserves_file_value() {
    let _guard = super::env_lock();
    super::unset_env("CAMEL_PROFILE");
    super::unset_env("CAMEL_CACHE_REPO_DB");
    set_env("CAMEL_CACHE_REPO_DB", "");

    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.cache_repo]
backend = "redis"
sentinel_nodes = ["n:26379"]
master_name = "m"
db = 5
"#,
    );

    let loaded = CamelConfig::from_file_with_env(file.path().to_str().unwrap());
    unset_env("CAMEL_CACHE_REPO_DB");

    // The empty override is skipped before type coercion, so the file
    // value stays effective and `Option<u16>` never sees "".
    let cfg = loaded.expect("empty scalar override must not fail the load");
    let repo = cfg.cache_repo.as_ref().expect("cache_repo section");
    assert_eq!(
        repo.db,
        Some(5),
        "empty scalar override must preserve the file value"
    );
}

#[test]
fn csv_override_builds_trimmed_node_list() {
    let _guard = super::env_lock();
    super::unset_env("CAMEL_PROFILE");
    super::unset_env("CAMEL_CACHE_REPO_SENTINEL_NODES");
    set_env(
        "CAMEL_CACHE_REPO_SENTINEL_NODES",
        "node-a:26379, node-b:26379",
    );

    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.cache_repo]
backend = "redis"
master_name = "m"
"#,
    );

    let loaded = CamelConfig::from_file_with_env(file.path().to_str().unwrap());
    unset_env("CAMEL_CACHE_REPO_SENTINEL_NODES");

    let cfg = loaded.expect("config should load with the CSV override");
    let repo = cfg.cache_repo.as_ref().expect("cache_repo section");
    assert_eq!(
        repo.sentinel_nodes,
        Some(vec!["node-a:26379".to_string(), "node-b:26379".to_string()]),
        "CSV override must build a trimmed node list"
    );
    assert!(cfg.validate().is_ok(), "overridden config must validate");
}

#[test]
fn csv_override_plus_master_name_validates_sentinel() {
    let _guard = super::env_lock();
    super::unset_env("CAMEL_PROFILE");
    super::unset_env("RC_TEST_MASTER");
    super::unset_env("CAMEL_CACHE_REPO_SENTINEL_NODES");
    set_env(
        "CAMEL_CACHE_REPO_SENTINEL_NODES",
        "node-a:26379,node-b:26379",
    );
    set_env("RC_TEST_MASTER", "mymaster");

    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.cache_repo]
backend = "redis"
master_name = "${env:RC_TEST_MASTER:-}"
"#,
    );

    let loaded = CamelConfig::from_file_with_env(file.path().to_str().unwrap());
    unset_env("CAMEL_CACHE_REPO_SENTINEL_NODES");
    unset_env("RC_TEST_MASTER");

    // CSV override composes with the placeholder-expanded master_name
    // into a sentinel topology that loads AND validates.
    let cfg = loaded.expect("CSV override + master_name must validate as sentinel");
    let repo = cfg.cache_repo.as_ref().expect("cache_repo section");
    assert_eq!(repo.master_name.as_deref(), Some("mymaster"));
    assert!(repo.sentinel_nodes.is_some());
    assert!(cfg.validate().is_ok());
}

#[test]
fn bare_bytes_cache_size_override_deserializes() {
    let _guard = super::env_lock();
    super::unset_env("CAMEL_PROFILE");
    super::unset_env("CAMEL_CACHE_REPO_CACHE_SIZE");
    set_env("CAMEL_CACHE_REPO_CACHE_SIZE", "268435456");

    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.cache_repo]
backend = "redb"
path = "cache.redb"
"#,
    );

    let loaded = CamelConfig::from_file_with_env(file.path().to_str().unwrap());
    unset_env("CAMEL_CACHE_REPO_CACHE_SIZE");

    // The bare-bytes form is documented for cache_size; the override
    // must stay a JSON string so `Option<String>` deserialization
    // succeeds instead of failing with "invalid type: integer".
    // (redb fixture: cache_size only applies to the redb backend, so
    // this is the topology where validate() can pass with it set.)
    let cfg = loaded.expect("bare-bytes cache_size override must deserialize as a string");
    let repo = cfg.cache_repo.as_ref().expect("cache_repo section");
    assert_eq!(
        repo.cache_size.as_deref(),
        Some("268435456"),
        "numeric-like override must stay a string, not coerce to an integer"
    );
    assert!(cfg.validate().is_ok(), "overridden config must validate");
}

#[test]
fn numeric_like_key_prefix_stays_string() {
    let _guard = super::env_lock();
    super::unset_env("CAMEL_PROFILE");
    super::unset_env("CAMEL_CACHE_REPO_KEY_PREFIX");
    set_env("CAMEL_CACHE_REPO_KEY_PREFIX", "007");

    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.cache_repo]
backend = "redis"
sentinel_nodes = ["n:26379"]
master_name = "m"
"#,
    );

    let loaded = CamelConfig::from_file_with_env(file.path().to_str().unwrap());
    unset_env("CAMEL_CACHE_REPO_KEY_PREFIX");

    let cfg = loaded.expect("numeric-like key_prefix override must deserialize as a string");
    let repo = cfg.cache_repo.as_ref().expect("cache_repo section");
    assert_eq!(
        repo.key_prefix.as_deref(),
        Some("007"),
        "leading-zero prefix must stay verbatim (string, not integer 7)"
    );
    assert!(cfg.validate().is_ok(), "overridden config must validate");
}

#[test]
fn empty_csv_override_clears_populated_file_value() {
    let _guard = super::env_lock();
    super::unset_env("CAMEL_PROFILE");
    super::unset_env("CAMEL_CACHE_REPO_SENTINEL_NODES");
    set_env("CAMEL_CACHE_REPO_SENTINEL_NODES", "");

    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.cache_repo]
backend = "redis"
sentinel_nodes = ["file-node:26379"]
url = "redis://s:6379"
"#,
    );

    let loaded = CamelConfig::from_file_with_env(file.path().to_str().unwrap());
    unset_env("CAMEL_CACHE_REPO_SENTINEL_NODES");

    // The empty CSV override replaces the populated file list with [],
    // which the FR1 normalization then resolves to absent — the config
    // validates as a standalone topology.
    let cfg = loaded.expect("empty CSV override must clear the file list, not clash");
    let repo = cfg.cache_repo.as_ref().expect("cache_repo section");
    assert!(
        repo.sentinel_nodes.is_none(),
        "empty CSV override must clear the populated file value via normalization"
    );
    assert!(cfg.validate().is_ok(), "standalone topology must validate");
}

#[test]
fn credential_vars_stay_denied() {
    let _guard = super::env_lock();
    super::unset_env("CAMEL_PROFILE");
    super::unset_env("CAMEL_CACHE_REPO_URL");
    super::unset_env("CAMEL_CACHE_REPO_USERNAME");
    set_env("CAMEL_CACHE_REPO_URL", "redis://evil:6379");
    set_env("CAMEL_CACHE_REPO_USERNAME", "attacker");

    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.cache_repo]
backend = "redis"
sentinel_nodes = ["n:26379"]
master_name = "m"
"#,
    );

    let (loaded, warns) =
        capture_warns(|| CamelConfig::from_file_with_env(file.path().to_str().unwrap()));
    unset_env("CAMEL_CACHE_REPO_URL");
    unset_env("CAMEL_CACHE_REPO_USERNAME");

    let cfg = loaded.expect("config must still load (credential vars are ignored)");
    let repo = cfg.cache_repo.as_ref().expect("cache_repo section");
    assert!(repo.url.is_none(), "credential var must not inject a url");
    assert!(
        repo.username.is_none(),
        "credential var must not inject a username"
    );
    // Separate deny records, each naming its var via the structured
    // `var` field (rendered unquoted by the capture visitor) and
    // carrying the allowlist fragment.
    for var in ["CAMEL_CACHE_REPO_URL", "CAMEL_CACHE_REPO_USERNAME"] {
        let needle = format!("var={var}");
        let hits: Vec<&String> = warns.iter().filter(|w| w.contains(&needle)).collect();
        assert_eq!(
            hits.len(),
            1,
            "exactly one deny record expected for {var}, got {hits:?} in {warns:?}"
        );
        assert!(
            hits[0].contains("env var not in config override allowlist; ignored"),
            "deny record for {var} must carry the allowlist fragment: {hits:?}"
        );
    }
}

#[test]
fn unitless_numeric_stale_retention_fails_validation_with_unit_error() {
    let _guard = super::env_lock();
    super::unset_env("CAMEL_PROFILE");
    super::unset_env("CAMEL_CACHE_REPO_STALE_RETENTION");
    set_env("CAMEL_CACHE_REPO_STALE_RETENTION", "604800");

    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.cache_repo]
backend = "redis"
url = "redis://s:6379"
"#,
    );

    let loaded = CamelConfig::from_file_with_env(file.path().to_str().expect("temp path"));
    unset_env("CAMEL_CACHE_REPO_STALE_RETENTION");

    // The unitless value must deserialize verbatim as a string (no
    // integer coercion) and then fail duration validation loudly with
    // the unit-bearing hint instead of a deserialization type error.
    // Validation runs inside the load path, so the Err carries it.
    let err = loaded.expect_err("unitless stale_retention must fail the load at validation time");
    let msg = err.to_string();
    assert!(
        msg.contains("cache_repo.stale_retention: invalid duration '604800'"),
        "expected the invalid-duration error, got: {msg}"
    );
    assert!(
        msg.contains("unit-bearing"),
        "expected the unit-bearing hint, got: {msg}"
    );
    assert!(
        !msg.contains("invalid type: integer"),
        "deserialization must not coerce the value to an integer, got: {msg}"
    );
}

#[test]
fn human_readable_duration_override_applies() {
    let _guard = super::env_lock();
    super::unset_env("CAMEL_PROFILE");
    super::unset_env("CAMEL_CACHE_REPO_STALE_RETENTION");
    set_env("CAMEL_CACHE_REPO_STALE_RETENTION", "7d");

    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.cache_repo]
backend = "redis"
url = "redis://s:6379"
"#,
    );

    let loaded = CamelConfig::from_file_with_env(file.path().to_str().expect("temp path"));
    unset_env("CAMEL_CACHE_REPO_STALE_RETENTION");

    let cfg = loaded.expect("config should load with the 7d override");
    let repo = cfg.cache_repo.as_ref().expect("cache_repo section");
    assert_eq!(
        repo.stale_retention.as_deref(),
        Some("7d"),
        "unit-bearing override must apply verbatim"
    );
    assert!(
        cfg.validate().is_ok(),
        "unit-bearing duration must validate"
    );
}

#[test]
fn legacy_path_override_passes_through_verbatim() {
    let _guard = super::env_lock();
    for value in ["604800", "007"] {
        super::unset_env("CAMEL_PROFILE");
        super::unset_env("CAMEL_CACHE_REPO_PATH");
        set_env("CAMEL_CACHE_REPO_PATH", value);

        // redb topology: `path` is a legitimate field there, so the
        // load path's built-in validation accepts the overridden value.
        let file = write_temp_config(
            r#"
[default]
timeout_ms = 1000

[default.cache_repo]
backend = "redb"
path = "cache.redb"
cache_size = "256MiB"
"#,
        );

        let loaded = CamelConfig::from_file_with_env(file.path().to_str().expect("temp path"));
        unset_env("CAMEL_CACHE_REPO_PATH");

        let cfg = loaded.unwrap_or_else(|e| {
            panic!("numeric-like path override '{value}' must deserialize verbatim, got: {e}")
        });
        let repo = cfg.cache_repo.as_ref().expect("cache_repo section");
        assert_eq!(
            repo.path.as_deref(),
            Some(value),
            "path override must pass through '{value}' verbatim"
        );
    }
}

#[test]
fn numeric_backend_override_reaches_backend_validation() {
    let _guard = super::env_lock();
    super::unset_env("CAMEL_PROFILE");
    super::unset_env("CAMEL_CACHE_REPO_BACKEND");
    set_env("CAMEL_CACHE_REPO_BACKEND", "123");

    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.cache_repo]
backend = "memory"
"#,
    );

    let loaded = CamelConfig::from_file_with_env(file.path().to_str().expect("temp path"));
    unset_env("CAMEL_CACHE_REPO_BACKEND");

    // The numeric-looking backend name must deserialize as a string and
    // reach backend validation, not die in typed deserialization.
    // Validation runs inside the load path, so the Err carries it.
    let err = loaded.expect_err("unknown backend name must fail the load at validation time");
    let msg = err.to_string();
    assert!(
        msg.contains("123"),
        "backend validation must name the overridden value, got: {msg}"
    );
    assert!(
        !msg.contains("invalid type: integer"),
        "deserialization must not coerce the value to an integer, got: {msg}"
    );
}

#[test]
fn numeric_typed_override_fields_stay_strict() {
    let _guard = super::env_lock();
    super::unset_env("CAMEL_PROFILE");
    super::unset_env("CAMEL_CACHE_REPO_MAX_ENTRIES");
    set_env("CAMEL_CACHE_REPO_MAX_ENTRIES", "notanumber");

    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.cache_repo]
backend = "memory"
"#,
    );

    let loaded = CamelConfig::from_file_with_env(file.path().to_str().expect("temp path"));
    unset_env("CAMEL_CACHE_REPO_MAX_ENTRIES");

    // The verbatim-passthrough hygiene applies ONLY to the String-typed
    // vars: typed overrides keep today's loud deserialization failure.
    let err =
        loaded.expect_err("non-numeric override for Option<usize> must fail typed deserialization");
    let msg = err.to_string();
    assert!(
        msg.contains("max_entries"),
        "failure should name the overridden field, got: {msg}"
    );
}

#[test]
fn legacy_empty_scalar_overrides_file_value_and_fails_validation() {
    let _guard = super::env_lock();
    super::unset_env("CAMEL_PROFILE");
    super::unset_env("CAMEL_CACHE_REPO_STALE_RETENTION");
    set_env("CAMEL_CACHE_REPO_STALE_RETENTION", "");

    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000

[default.cache_repo]
backend = "redis"
url = "redis://s:6379"
stale_retention = "7d"
"#,
    );

    let loaded = CamelConfig::from_file_with_env(file.path().to_str().expect("temp path"));
    unset_env("CAMEL_CACHE_REPO_STALE_RETENTION");

    // Legacy vars are deliberately NOT empty-skip vars: the empty string
    // overrides the file value verbatim and then fails duration
    // validation loudly (no silent empty-skip semantics). Validation
    // runs inside the load path, so the Err carries it.
    let err = loaded.expect_err("empty stale_retention must fail duration validation");
    let msg = err.to_string();
    assert!(
        msg.contains("cache_repo.stale_retention: invalid duration ''"),
        "empty override must reach validation, got: {msg}"
    );
}

#[test]
fn legacy_override_lists_are_disjoint_and_allowlisted() {
    // The legacy verbatim-string vars sit OUTSIDE the kind lists by
    // design: joining STRING_ENV_OVERRIDES would break the pinned
    // STRING ⊆ EMPTY_SCALAR subset (they must not acquire empty-skip
    // semantics), and joining EMPTY_SCALAR_ENV_OVERRIDES would change
    // their established empty behavior. Every legacy var must stay a
    // member of exactly the allowlist plus this legacy list.
    assert!(
        LEGACY_STRING_ENV_OVERRIDES
            .iter()
            .all(|v| !STRING_ENV_OVERRIDES.contains(v)),
        "LEGACY_STRING_ENV_OVERRIDES and STRING_ENV_OVERRIDES must be disjoint"
    );
    assert!(
        LEGACY_STRING_ENV_OVERRIDES
            .iter()
            .all(|v| !CSV_ENV_OVERRIDES.contains(v)),
        "LEGACY_STRING_ENV_OVERRIDES and CSV_ENV_OVERRIDES must be disjoint"
    );
    assert!(
        LEGACY_STRING_ENV_OVERRIDES
            .iter()
            .all(|v| !EMPTY_SCALAR_ENV_OVERRIDES.contains(v)),
        "LEGACY_STRING_ENV_OVERRIDES and EMPTY_SCALAR_ENV_OVERRIDES must be disjoint"
    );
    assert!(
        LEGACY_STRING_ENV_OVERRIDES
            .iter()
            .all(|v| ALLOWED_ENV_OVERRIDES.contains(v)),
        "every LEGACY_STRING_ENV_OVERRIDES var must be an allowlisted override"
    );
}

#[test]
fn allowlist_completeness_pinned() {
    // The 8 non-credential overrides must be allowlisted...
    for var in [
        "CAMEL_CACHE_REPO_PAYLOAD",
        "CAMEL_CACHE_REPO_PAYLOAD_DIR",
        "CAMEL_CACHE_REPO_CACHE_SIZE",
        "CAMEL_CACHE_REPO_SWEEP_INTERVAL",
        "CAMEL_CACHE_REPO_MASTER_NAME",
        "CAMEL_CACHE_REPO_KEY_PREFIX",
        "CAMEL_CACHE_REPO_DB",
        "CAMEL_CACHE_REPO_SENTINEL_NODES",
    ] {
        assert!(
            ALLOWED_ENV_OVERRIDES.contains(&var),
            "{var} must be an allowlisted override"
        );
    }
    // ...and the 5 credential vars must NEVER appear in any override list.
    for var in [
        "CAMEL_CACHE_REPO_URL",
        "CAMEL_CACHE_REPO_USERNAME",
        "CAMEL_CACHE_REPO_PASSWORD",
        "CAMEL_CACHE_REPO_SENTINEL_USERNAME",
        "CAMEL_CACHE_REPO_SENTINEL_PASSWORD",
    ] {
        assert!(
            !ALLOWED_ENV_OVERRIDES.contains(&var),
            "credential var {var} must never be allowlisted"
        );
        assert!(
            !CSV_ENV_OVERRIDES.contains(&var),
            "credential var {var} must never be a CSV override"
        );
        assert!(
            !EMPTY_SCALAR_ENV_OVERRIDES.contains(&var),
            "credential var {var} must never be an empty-scalar override"
        );
        assert!(
            !STRING_ENV_OVERRIDES.contains(&var),
            "credential var {var} must never be a string-kind override"
        );
    }
    // Kind-const invariants. STRING ∩ CSV and EMPTY ∩ CSV must be
    // disjoint: a var listed in two dispatch arms would make the merge
    // order silently pick one kind's semantics, breaking the
    // empty-scalar-preserves vs empty-CSV-clears asymmetry (and verbatim
    // string passthrough). STRING ∩ EMPTY is deliberately NOT disjoint —
    // every string-kind var MUST also be an empty-scalar var (an empty
    // "" must never reach typed deserialization; the skip arm runs
    // first) — so that relationship is pinned as a subset instead.
    assert!(
        CSV_ENV_OVERRIDES
            .iter()
            .all(|v| !EMPTY_SCALAR_ENV_OVERRIDES.contains(v)),
        "CSV_ENV_OVERRIDES and EMPTY_SCALAR_ENV_OVERRIDES must be disjoint"
    );
    assert!(
        STRING_ENV_OVERRIDES
            .iter()
            .all(|v| !CSV_ENV_OVERRIDES.contains(v)),
        "STRING_ENV_OVERRIDES and CSV_ENV_OVERRIDES must be disjoint"
    );
    assert!(
        STRING_ENV_OVERRIDES
            .iter()
            .all(|v| EMPTY_SCALAR_ENV_OVERRIDES.contains(v)),
        "every STRING_ENV_OVERRIDES var must also be an EMPTY_SCALAR_ENV_OVERRIDES var"
    );
}

#[test]
fn empty_preexisting_typed_override_still_fails() {
    // Regression pin: the empty-scalar skip is scoped to the NEW vars.
    // CAMEL_TIMEOUT_MS is a pre-existing allowlisted var, so an empty
    // value keeps today's loud typed-deserialization failure instead of
    // being silently skipped.
    let _guard = super::env_lock();
    super::unset_env("CAMEL_PROFILE");
    set_env("CAMEL_TIMEOUT_MS", "");

    let file = write_temp_config(
        r#"
[default]
timeout_ms = 1000
"#,
    );

    let loaded = CamelConfig::from_file_with_env(file.path().to_str().unwrap());
    unset_env("CAMEL_TIMEOUT_MS");

    let err = loaded.expect_err("empty pre-existing typed override must still fail");
    let msg = err.to_string();
    assert!(
        msg.contains("timeout_ms"),
        "failure should name the overridden field, got: {msg}"
    );
}

#[test]
fn env_lock_recovers_after_poison() {
    // Poison the mutex by panicking while holding the guard.
    let _ = std::panic::catch_unwind(|| {
        let _g = env_lock();
        panic!("poison source");
    });
    // The second acquisition must recover from the poison instead of
    // panicking; binding the guard proves it is held.
    let _guard = env_lock();
    assert!(
        ENV_OVERRIDE_LOCK.try_lock().is_err(),
        "env_lock guard must be held after poison recovery"
    );
}
