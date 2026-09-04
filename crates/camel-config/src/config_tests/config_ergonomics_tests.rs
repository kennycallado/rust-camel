use super::*;
use crate::config::log_capture::capture_warns;

fn parse_table(toml_str: &str) -> toml::Value {
    toml::from_str(toml_str).expect("fixture must be valid TOML")
}

// --- rc-k5bz: include base-dir for bare relative filenames ---

#[test]
fn include_base_dir_maps_bare_filename_to_current_dir() {
    assert_eq!(include_base_dir("root.toml"), std::path::PathBuf::from("."));
    assert_eq!(
        include_base_dir("nested/root.toml"),
        std::path::PathBuf::from("nested")
    );
    assert_eq!(
        include_base_dir("/root.toml"),
        std::path::PathBuf::from("/")
    );
}

/// Bare filenames historically produced an empty base dir, whose
/// canonicalize() fails with ENOENT ("failed to canonicalize base dir :").
/// Requires chdir into a temp fixture dir; serialized via CWD_LOCK.
#[test]
fn from_file_bare_relative_filename_resolves_includes() {
    let _cwd_guard = super::cwd_lock();
    let _env_guard = env_lock();
    unset_env("CAMEL_PROFILE");

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("root.toml"), "include = [\"x.toml\"]\n").unwrap();
    std::fs::write(dir.path().join("x.toml"), "watch = true\n").unwrap();

    let original = std::env::current_dir().unwrap();
    let _restore_cwd = CwdGuard(original);
    std::env::set_current_dir(dir.path()).unwrap();
    let result = CamelConfig::from_file_with_profile("root.toml", None);

    let cfg = result.expect("bare relative filename with include must load");
    assert!(cfg.watch, "included file value must merge");
}

#[test]
fn from_file_async_bare_relative_filename_resolves_includes() {
    let _cwd_guard = super::cwd_lock();
    let _env_guard = env_lock();
    unset_env("CAMEL_PROFILE");

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("root.toml"), "include = [\"x.toml\"]\n").unwrap();
    std::fs::write(dir.path().join("x.toml"), "watch = true\n").unwrap();

    let original = std::env::current_dir().unwrap();
    let _restore_cwd = CwdGuard(original);
    std::env::set_current_dir(dir.path()).unwrap();

    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let result = rt.block_on(async {
        CamelConfig::from_file_async_with_profile_and_env("root.toml", None).await
    });
    drop(rt);

    let cfg = result.expect("bare relative filename with include must load (async)");
    assert!(cfg.watch, "included file value must merge");
}

// --- rc-cflo tripwires: KNOWN_TOP_LEVEL_KEYS vs serde reality ---
//
// CamelConfig absorbs unknown top-level keys into `_extra`, so these tests
// check that every name in the warning's exclusion list is genuinely a
// serde-known field (never lands in `_extra`) and that profile-like names
// DO land there.

#[test]
fn known_top_level_keys_are_all_serde_fields() {
    let composite = r#"
routes = []
watch = false
log_level = "debug"
timeout_ms = 1000
drain_timeout_ms = 1000
watch_debounce_ms = 300
platform = { type = "noop" }
runtime_journal = { path = "tripwire.db" }
idempotent_repo = {}
cache_repo = {}
beans = {}
binds = {}
datasources = {}

[components]

[observability]

[supervision]

[stream_caching]
threshold = 64

[security]

[languages]
"#;
    let cfg: CamelConfig = toml::from_str(composite)
        .expect("every KNOWN_TOP_LEVEL_KEYS entry must deserialize as a real field");
    assert!(
        cfg._extra.is_empty(),
        "KNOWN_TOP_LEVEL_KEYS contains non-field names; landed in _extra: {:?}",
        cfg._extra
    );
}

#[test]
fn unknown_top_level_table_lands_in_extra() {
    let probe: CamelConfig = toml::from_str("[staging]\nunused = true\n").expect("parses");
    assert!(
        probe._extra.contains_key("staging"),
        "mechanism behind the rc-cflo heuristic broke: unknown top-level tables must land in _extra"
    );
}

// --- rc-cflo: warn when CAMEL_PROFILE is unset but sections look like profiles ---

#[test]
fn unset_camel_profile_with_profile_like_sections_warns_once() {
    let _env_guard = env_lock();
    unset_env("CAMEL_PROFILE");

    let tree = parse_table(
        r#"
[default]
log_level = "info"

[staging]
log_level = "warn"
"#,
    );

    let (_result, warns) = capture_warns(|| {
        build_from_toml_value_inner(tree, None, false, Vec::new(), &super::ambient_lookup())
    });

    let hits: Vec<&String> = warns
        .iter()
        .filter(|w| w.contains("staging") && w.contains("CAMEL_PROFILE"))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "exactly one profile-section warn expected, got {hits:?} in {warns:?}"
    );
}

#[test]
fn no_default_section_means_no_profile_structure_no_warn() {
    let _env_guard = env_lock();
    unset_env("CAMEL_PROFILE");

    // Profile-like top-level section WITHOUT [default]: nothing can be
    // dropped (apply_profile_lenient keeps everything), so warning would
    // be a false positive.
    let tree = parse_table(
        r#"
log_level = "info"

[staging]
log_level = "warn"
"#,
    );

    let (_result, warns) = capture_warns(|| {
        build_from_toml_value_inner(tree, None, false, Vec::new(), &super::ambient_lookup())
    });

    let hits: Vec<&String> = warns
        .iter()
        .filter(|w| w.contains("staging") && w.contains("CAMEL_PROFILE"))
        .collect();
    assert_eq!(
        hits.len(),
        0,
        "no profile-section warn without [default]; got {hits:?} in {warns:?}"
    );
}

#[test]
fn active_camel_profile_suppresses_section_warn() {
    let _env_guard = env_lock();
    set_env("CAMEL_PROFILE", "staging");

    let tree = parse_table(
        r#"
[default]
log_level = "info"

[staging]
log_level = "warn"
"#,
    );

    let (result, warns) = capture_warns(|| {
        build_from_toml_value_inner(tree, None, false, Vec::new(), &super::ambient_lookup())
    });

    unset_env("CAMEL_PROFILE");
    let cfg = result.expect("staging profile must load");
    drop(cfg);
    drop(_env_guard);

    assert!(
        !warns.iter().any(|w| w.contains("CAMEL_PROFILE")),
        "no profile-section warn when CAMEL_PROFILE is active; got {warns:?}"
    );
}

// --- rc-6gqy(b): handled-elsewhere vars never reported as ignored ---

#[test]
fn camel_profile_and_config_file_never_reported_as_ignored() {
    let _env_guard = env_lock();
    set_env("CAMEL_PROFILE", "qa"); // flat tree → lenient keep-as-is
    set_env("CAMEL_CONFIG_FILE", "/nonexistent/tripwire-path.toml");
    set_env("CAMEL_ERGONOMICS_TYPO_PROBE", "1"); // positive control

    let tree = parse_table("log_level = \"debug\"");
    let (_, warns) = capture_warns(|| {
        build_from_toml_value_inner(tree, None, true, Vec::new(), &super::ambient_lookup())
    });

    // unset_env requires ENV_OVERRIDE_LOCK to be held: restore env while
    // the guard is alive, only then release it.
    unset_env("CAMEL_PROFILE");
    unset_env("CAMEL_CONFIG_FILE");
    unset_env("CAMEL_ERGONOMICS_TYPO_PROBE");
    drop(_env_guard);

    assert!(
        warns
            .iter()
            .any(|w| w.contains("CAMEL_ERGONOMICS_TYPO_PROBE")),
        "positive control failed — typo-var warn not emitted: {warns:?}"
    );
    for handled in ["CAMEL_PROFILE", "CAMEL_CONFIG_FILE"] {
        assert!(
            !warns.iter().any(|w| w.contains(handled)),
            "{handled} is honored by its own consumer and must NOT be flagged as ignored: {warns:?}"
        );
    }
}
