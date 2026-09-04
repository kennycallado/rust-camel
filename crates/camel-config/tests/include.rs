//! Integration tests for Camel.toml `include` feature.
use camel_config::CamelConfig;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

mod common;

fn write(dir: &Path, name: &str, content: &str) {
    let p = dir.join(name);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, content).unwrap();
}

fn path(dir: &Path) -> String {
    dir.join("Camel.toml").to_string_lossy().to_string()
}

#[test]
fn include_basic_key_visible() {
    let _guard = common::env_lock();
    let dir = TempDir::new().unwrap();
    write(dir.path(), "base.toml", r#"timeout_ms = 9999"#);
    write(dir.path(), "Camel.toml", r#"include = ["base.toml"]"#);
    let cfg = CamelConfig::from_file(&path(dir.path())).unwrap();
    assert_eq!(cfg.timeout_ms, 9999);
}

#[test]
fn include_last_wins() {
    let _guard = common::env_lock();
    let dir = TempDir::new().unwrap();
    write(dir.path(), "a.toml", r#"timeout_ms = 1000"#);
    write(dir.path(), "b.toml", r#"timeout_ms = 2000"#);
    write(
        dir.path(),
        "Camel.toml",
        r#"include = ["a.toml", "b.toml"]"#,
    );
    let cfg = CamelConfig::from_file(&path(dir.path())).unwrap();
    assert_eq!(cfg.timeout_ms, 2000);
}

#[test]
fn include_root_wins_over_includes() {
    let _guard = common::env_lock();
    let dir = TempDir::new().unwrap();
    write(dir.path(), "base.toml", r#"timeout_ms = 1000"#);
    write(
        dir.path(),
        "Camel.toml",
        "include = [\"base.toml\"]\ntimeout_ms = 5000\n",
    );
    let cfg = CamelConfig::from_file(&path(dir.path())).unwrap();
    assert_eq!(cfg.timeout_ms, 5000);
}

#[test]
fn include_profile_propagates() {
    let _guard = common::env_lock();
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "obs.toml",
        "[default]\ntimeout_ms = 100\n\n[production]\ntimeout_ms = 500\n",
    );
    write(dir.path(), "Camel.toml", r#"include = ["obs.toml"]"#);
    let cfg = CamelConfig::from_file_with_profile(&path(dir.path()), Some("production")).unwrap();
    assert_eq!(cfg.timeout_ms, 500);
}

#[test]
fn include_profile_not_leaked() {
    let _guard = common::env_lock();
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "obs.toml",
        "[default]\ntimeout_ms = 100\n\n[staging]\ntimeout_ms = 300\n",
    );
    write(dir.path(), "Camel.toml", r#"include = ["obs.toml"]"#);
    // No profile active → uses [default]
    let cfg = CamelConfig::from_file(&path(dir.path())).unwrap();
    assert_eq!(cfg.timeout_ms, 100);
}

#[test]
fn include_flat_file_with_active_profile() {
    let _guard = common::env_lock();
    // Flat include (no profile sections) must not error when a profile is active
    let dir = TempDir::new().unwrap();
    write(dir.path(), "flat.toml", r#"timeout_ms = 42"#);
    write(dir.path(), "Camel.toml", r#"include = ["flat.toml"]"#);
    let cfg = CamelConfig::from_file_with_profile(&path(dir.path()), Some("production")).unwrap();
    assert_eq!(cfg.timeout_ms, 42);
}

#[test]
fn include_path_traversal_is_error() {
    let _guard = common::env_lock();
    let dir = TempDir::new().unwrap();
    let outside = dir.path().parent().unwrap().join("secret.toml");
    fs::write(&outside, r#"timeout_ms = 999"#).unwrap();
    write(dir.path(), "Camel.toml", r#"include = ["../secret.toml"]"#);
    let err = CamelConfig::from_file(&path(dir.path())).unwrap_err();
    assert!(
        err.to_string().contains("traversal") || err.to_string().contains("escapes"),
        "expected traversal error, got: {err}"
    );
    let _ = fs::remove_file(outside);
}

#[test]
fn include_absolute_path_is_error() {
    let _guard = common::env_lock();
    let dir = TempDir::new().unwrap();
    write(dir.path(), "Camel.toml", r#"include = ["/etc/passwd"]"#);
    let err = CamelConfig::from_file(&path(dir.path())).unwrap_err();
    assert!(
        err.to_string().contains("absolute") || err.to_string().contains("relative"),
        "expected absolute path error, got: {err}"
    );
}

#[test]
fn include_not_found_is_error() {
    let _guard = common::env_lock();
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "Camel.toml",
        r#"include = ["config/missing.toml"]"#,
    );
    let err = CamelConfig::from_file(&path(dir.path())).unwrap_err();
    assert!(
        err.to_string().contains("not found") || err.to_string().contains("No such"),
        "expected not-found error, got: {err}"
    );
}

#[test]
fn include_duplicate_path_is_error() {
    let _guard = common::env_lock();
    let dir = TempDir::new().unwrap();
    write(dir.path(), "base.toml", r#"timeout_ms = 1000"#);
    write(
        dir.path(),
        "Camel.toml",
        r#"include = ["base.toml", "base.toml"]"#,
    );
    let err = CamelConfig::from_file(&path(dir.path())).unwrap_err();
    assert!(
        err.to_string().contains("duplicate"),
        "expected duplicate error, got: {err}"
    );
}

#[test]
fn include_nested_include_is_ignored() {
    let _guard = common::env_lock();
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "base.toml",
        "timeout_ms = 42\ninclude = [\"nonexistent.toml\"]\n",
    );
    write(dir.path(), "Camel.toml", r#"include = ["base.toml"]"#);
    // Should succeed — nested include is stripped, not followed
    let cfg = CamelConfig::from_file(&path(dir.path())).unwrap();
    assert_eq!(cfg.timeout_ms, 42);
}

#[test]
fn include_env_wins_over_include_and_root() {
    let _guard = common::env_lock();
    let dir = TempDir::new().unwrap();
    write(dir.path(), "base.toml", r#"timeout_ms = 1000"#);
    write(
        dir.path(),
        "Camel.toml",
        "include = [\"base.toml\"]\ntimeout_ms = 2000\n",
    );
    // SAFETY: test-only, single-threaded
    unsafe {
        std::env::set_var("CAMEL_TIMEOUT_MS", "9999");
    }
    let cfg = CamelConfig::from_file_with_env(&path(dir.path())).unwrap();
    unsafe {
        std::env::remove_var("CAMEL_TIMEOUT_MS");
    }
    assert_eq!(cfg.timeout_ms, 9999);
}

#[test]
fn include_ordering_three_files() {
    let _guard = common::env_lock();
    let dir = TempDir::new().unwrap();
    write(dir.path(), "a.toml", r#"timeout_ms = 1"#);
    write(dir.path(), "b.toml", r#"timeout_ms = 2"#);
    write(dir.path(), "c.toml", r#"timeout_ms = 3"#);
    write(
        dir.path(),
        "Camel.toml",
        r#"include = ["a.toml", "b.toml", "c.toml"]"#,
    );
    let cfg = CamelConfig::from_file(&path(dir.path())).unwrap();
    assert_eq!(cfg.timeout_ms, 3);
}

#[test]
fn include_invalid_value_type_is_error() {
    let _guard = common::env_lock();
    let dir = TempDir::new().unwrap();
    write(dir.path(), "Camel.toml", "include = 42\n");
    let err = CamelConfig::from_file(&path(dir.path())).unwrap_err();
    assert!(
        err.to_string().contains("array") || err.to_string().contains("string"),
        "expected type error, got: {err}"
    );
}

// --- include inside profile sections (bd rc-zfmj) ---

#[test]
fn include_inside_default_section_loads() {
    let _guard = common::env_lock();
    let dir = TempDir::new().unwrap();
    write(dir.path(), "sec.toml", "timeout_ms = 4242\n");
    write(
        dir.path(),
        "Camel.toml",
        "[default]\ninclude = [\"sec.toml\"]\ntimeout_ms = 100\n",
    );
    // Root [default].timeout_ms must win over the included file's value
    let cfg = CamelConfig::from_file(&path(dir.path())).unwrap();
    assert_eq!(cfg.timeout_ms, 100);
}

#[test]
fn include_inside_default_section_values_visible() {
    let _guard = common::env_lock();
    let dir = TempDir::new().unwrap();
    write(dir.path(), "sec.toml", "watch_debounce_ms = 111\n");
    write(
        dir.path(),
        "Camel.toml",
        "[default]\ninclude = [\"sec.toml\"]\ntimeout_ms = 100\n",
    );
    // Keys ONLY present in the included file must be visible
    let cfg = CamelConfig::from_file(&path(dir.path())).unwrap();
    assert_eq!(cfg.watch_debounce_ms, 111);
    assert_eq!(cfg.timeout_ms, 100);
}

#[test]
fn include_inside_active_profile_section_loads() {
    let _guard = common::env_lock();
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "dev.toml",
        "watch_debounce_ms = 111\ntimeout_ms = 100\n",
    );
    write(dir.path(), "prod.toml", "timeout_ms = 500\n");
    write(
        dir.path(),
        "Camel.toml",
        "[default]\ninclude = [\"dev.toml\"]\n\n[production]\ninclude = [\"prod.toml\"]\n",
    );
    let cfg = CamelConfig::from_file_with_profile(&path(dir.path()), Some("production")).unwrap();
    // Union: dev-only keys survive, profile include wins on conflicts
    assert_eq!(cfg.watch_debounce_ms, 111);
    assert_eq!(cfg.timeout_ms, 500);
}

#[test]
fn include_top_level_and_profile_section_union() {
    let _guard = common::env_lock();
    let dir = TempDir::new().unwrap();
    write(dir.path(), "global.toml", "timeout_ms = 7\n");
    write(dir.path(), "prod.toml", "timeout_ms = 500\n");
    write(
        dir.path(),
        "Camel.toml",
        "include = [\"global.toml\"]\n\n[default]\nwatch_debounce_ms = 100\n\n[production]\ninclude = [\"prod.toml\"]\n",
    );
    let cfg = CamelConfig::from_file_with_profile(&path(dir.path()), Some("production")).unwrap();
    // Profile-scoped include overrides top-level include on conflicts;
    // root [default] value wins over all includes.
    assert_eq!(cfg.timeout_ms, 500);
    assert_eq!(cfg.watch_debounce_ms, 100);
}

#[test]
fn include_inside_inactive_profile_section_is_not_loaded() {
    let _guard = common::env_lock();
    let dir = TempDir::new().unwrap();
    // staging.toml intentionally NOT created — it must never be read
    write(
        dir.path(),
        "Camel.toml",
        "[default]\ntimeout_ms = 100\n\n[staging]\ninclude = [\"staging.toml\"]\n",
    );
    let cfg = CamelConfig::from_file(&path(dir.path())).unwrap();
    assert_eq!(cfg.timeout_ms, 100);
}

#[test]
fn include_invalid_type_inside_section_is_error() {
    let _guard = common::env_lock();
    let dir = TempDir::new().unwrap();
    write(dir.path(), "Camel.toml", "[default]\ninclude = 42\n");
    let err = CamelConfig::from_file(&path(dir.path())).unwrap_err();
    assert!(
        err.to_string().contains("default.include"),
        "expected section-named type error, got: {err}"
    );
}

#[test]
fn include_same_file_in_top_level_and_section_is_error() {
    let _guard = common::env_lock();
    let dir = TempDir::new().unwrap();
    write(dir.path(), "base.toml", "timeout_ms = 1\n");
    write(
        dir.path(),
        "Camel.toml",
        "include = [\"base.toml\"]\n\n[default]\ninclude = [\"base.toml\"]\n",
    );
    let err = CamelConfig::from_file(&path(dir.path())).unwrap_err();
    assert!(
        err.to_string().contains("duplicate"),
        "expected duplicate error, got: {err}"
    );
}
