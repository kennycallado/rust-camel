//! Parity goldens for the filesystem config loader (openspec change
//! `configunify`, Task 1.1).
//!
//! These tests lock the PRE-refactor behavior of the camel-config
//! filesystem loader over a representative resolution matrix: resolved
//! field projections for success cases and full error `Display` strings
//! for failure cases are compared byte-for-byte against committed
//! goldens under `src/config_tests/parity_goldens/`. The delegation
//! refactor (canonical `config_semantics` helpers) must not move any of
//! them; error message strings are observable behavior.
//!
//! `CamelConfig` has no `Serialize` derive and cannot get one (closure
//! bearing fields), so each case projects exactly the matrix-relevant
//! resolved fields into a test-local JSON object. Projections are
//! serialized through a std `BTreeMap` (never `serde_json::Map`): the
//! latter's backing store flips between `BTreeMap` and insertion-ordered
//! `IndexMap` depending on whether workspace feature unification enables
//! serde_json's `preserve_order` (the `siumai` dependency family does,
//! under combined multi-crate `-p` resolution), which would make the
//! golden key order flip between runs (Bd rc-io2zl, rc-k6dln).
//!
//! Regenerate after an intentional behavior change:
//! `UPDATE_GOLDENS=1 cargo test -p camel-config parity_`.

use super::*;
use std::sync::{Arc, Mutex};

// ── golden plumbing ──────────────────────────────────────────────────────

/// Regeneration switch: when `UPDATE_GOLDENS=1` is set, write the
/// golden files instead of comparing them.
fn update_goldens() -> bool {
    std::env::var("UPDATE_GOLDENS").is_ok_and(|v| v == "1")
}

fn golden_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/config_tests/parity_goldens")
        .join(name)
}

/// Byte-for-byte golden lock for text artifacts (pretty-printed JSON
/// projections and error/warn transcripts).
fn lock_text_golden(name: &str, actual: &str) {
    let path = golden_path(name);
    if update_goldens() {
        std::fs::create_dir_all(path.parent().expect("golden parent dir"))
            .expect("create goldens dir");
        std::fs::write(&path, actual).unwrap_or_else(|e| panic!("write golden {name}: {e}"));
    } else {
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read golden {name}: {e} (capture: UPDATE_GOLDENS=1)"));
        assert_eq!(expected, actual, "golden {name} drifted");
    }
}

/// Serialize one JSON projection golden. The projection is collected
/// into a std `BTreeMap` before serialization so the emitted key order
/// is always sorted, independent of serde_json's feature-dependent
/// `Map` backing (see the module docs).
fn lock_json_golden(name: &str, projection: &[(&str, serde_json::Value)]) {
    let map: std::collections::BTreeMap<String, serde_json::Value> = projection
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    let text = serde_json::to_string_pretty(&map).expect("serialize projection");
    lock_text_golden(name, &text);
}

/// Write one fixture document inside the case tempdir, creating the
/// relative directory when needed.
fn write_fixture(dir: &std::path::Path, rel: &str, contents: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create fixture dir");
    }
    std::fs::write(&path, contents).expect("write fixture file");
}

fn config_path(dir: &std::path::Path) -> String {
    dir.join("Camel.toml")
        .to_str()
        .expect("utf-8 temp path")
        .to_string()
}

// ── projection helpers ───────────────────────────────────────────────────

fn http_leaf<'a>(cfg: &'a CamelConfig, key: &str) -> Option<&'a toml::Value> {
    cfg.components.raw.get("http").and_then(|v| v.get(key))
}

fn http_int(cfg: &CamelConfig, key: &str) -> serde_json::Value {
    http_leaf(cfg, key)
        .and_then(toml::Value::as_integer)
        .map(serde_json::Value::from)
        .unwrap_or(serde_json::Value::Null)
}

fn http_str(cfg: &CamelConfig, key: &str) -> serde_json::Value {
    http_leaf(cfg, key)
        .and_then(toml::Value::as_str)
        .map(serde_json::Value::from)
        .unwrap_or(serde_json::Value::Null)
}

// ── WARN capture (case e) ────────────────────────────────────────────────

struct MessageVisitor<'a>(&'a mut Option<String>);

impl tracing::field::Visit for MessageVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            *self.0 = Some(format!("{value:?}"));
        }
    }
}

/// `tracing_subscriber` layer recording every WARN event message
/// emitted on the installing thread.
struct CaptureLayer {
    events: Arc<Mutex<Vec<String>>>,
}

impl<S> tracing_subscriber::Layer<S> for CaptureLayer
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if *event.metadata().level() != tracing::Level::WARN {
            return;
        }
        let mut slot = None;
        event.record(&mut MessageVisitor(&mut slot));
        if let Some(message) = slot {
            self.events
                .lock()
                .expect("capture lock")
                .push(format!("{}: {message}", event.metadata().level()));
        }
    }
}

/// Run `body` under a thread-local subscriber recording WARN messages.
/// `set_default` is thread-local, so concurrent tests neither pollute
/// this capture nor observe it (same convention as camel-core's
/// `disk_offload_tests::capture_warns`).
fn capture_warns<T>(body: impl FnOnce() -> T) -> (T, Vec<String>) {
    use tracing_subscriber::prelude::*;
    let events = Arc::new(Mutex::new(Vec::new()));
    let layer = CaptureLayer {
        events: Arc::clone(&events),
    };
    let guard = tracing_subscriber::registry().with(layer).set_default();
    let out = body();
    drop(guard);
    let captured = events.lock().expect("capture lock").clone();
    (out, captured)
}

// ── the matrix ───────────────────────────────────────────────────────────

/// Case (a): flat config with no profile structure loads as-is.
#[test]
fn parity_flat_config_matches_golden() {
    let _guard = env_lock();
    let dir = tempfile::tempdir().expect("temp dir");
    write_fixture(
        dir.path(),
        "Camel.toml",
        r#"
timeout_ms = 15000
log_level = "debug"
watch = true
routes = ["routes/a.yaml", "routes/b.yaml"]

[components.http]
max_connections = 25
"#,
    );

    let cfg = CamelConfig::from_file(&config_path(dir.path())).expect("flat config loads");
    lock_json_golden(
        "case_01.json",
        &[
            ("timeout_ms", serde_json::json!(cfg.timeout_ms)),
            ("log_level", serde_json::json!(cfg.log_level)),
            ("watch", serde_json::json!(cfg.watch)),
            ("routes", serde_json::json!(cfg.routes)),
            (
                "components_http_max_connections",
                http_int(&cfg, "max_connections"),
            ),
        ],
    );
}

/// Case (b): `[default]` + `[production]` with `CAMEL_PROFILE=production`
/// deep-merges tables recursively and REPLACES the routes array.
#[test]
fn parity_profile_deep_merge_and_array_replace() {
    let _guard = env_lock();
    let dir = tempfile::tempdir().expect("temp dir");
    write_fixture(
        dir.path(),
        "Camel.toml",
        r#"
[default]
timeout_ms = 30000
log_level = "info"
watch = false
routes = ["routes/base.yaml"]

[default.components.http]
max_connections = 10

[production]
timeout_ms = 5000
routes = ["routes/prod-main.yaml", "routes/prod-orders.yaml"]

[production.components.http]
max_connections = 99
"#,
    );

    set_env("CAMEL_PROFILE", "production");
    let loaded = CamelConfig::from_file_with_profile(&config_path(dir.path()), None);
    unset_env("CAMEL_PROFILE");
    let cfg = loaded.expect("profiled config loads");

    lock_json_golden(
        "case_02.json",
        &[
            ("timeout_ms", serde_json::json!(cfg.timeout_ms)),
            ("log_level", serde_json::json!(cfg.log_level)),
            ("watch", serde_json::json!(cfg.watch)),
            ("routes", serde_json::json!(cfg.routes)),
            (
                "components_http_max_connections",
                http_int(&cfg, "max_connections"),
            ),
        ],
    );
}

/// Case (c): unknown profile with `[default]` present fails with the
/// strict single-profile error; the full Display string is locked.
#[test]
fn parity_unknown_profile_error_string_locked() {
    let _guard = env_lock();
    let dir = tempfile::tempdir().expect("temp dir");
    write_fixture(
        dir.path(),
        "Camel.toml",
        r#"
[default]
timeout_ms = 1000
watch = false
"#,
    );

    set_env("CAMEL_PROFILE", "staging");
    let loaded = CamelConfig::from_file_with_profile(&config_path(dir.path()), None);
    unset_env("CAMEL_PROFILE");
    let err = loaded.expect_err("unknown profile must fail");

    let display = err.to_string();
    assert!(
        display.contains("Unknown profile: staging"),
        "unexpected error spelling: {display}"
    );
    lock_text_golden("case_03_error.txt", &display);
}

/// Case (d): ordered includes `a.toml` then `b.toml`; the config
/// document merges above both, and the later include wins over the
/// earlier one on key conflicts.
#[test]
fn parity_ordered_includes_merge_order() {
    let _guard = env_lock();
    let dir = tempfile::tempdir().expect("temp dir");
    write_fixture(
        dir.path(),
        "Camel.toml",
        r#"
include = ["conf/a.toml", "conf/b.toml"]

[default]
timeout_ms = 1000
routes = ["routes/config.yaml"]

[default.components.http]
max_connections = 30
"#,
    );
    write_fixture(
        dir.path(),
        "conf/a.toml",
        r#"
[default]
timeout_ms = 111
routes = ["routes/a.yaml"]

[default.components.http]
max_connections = 11
base_url = "http://from-a"
"#,
    );
    write_fixture(
        dir.path(),
        "conf/b.toml",
        r#"
[default]
timeout_ms = 222
routes = ["routes/b.yaml"]

[default.components.http]
max_connections = 22
base_url = "http://from-b"
"#,
    );

    let cfg = CamelConfig::from_file(&config_path(dir.path())).expect("include config loads");
    lock_json_golden(
        "case_04.json",
        &[
            ("timeout_ms", serde_json::json!(cfg.timeout_ms)),
            ("routes", serde_json::json!(cfg.routes)),
            (
                "components_http_max_connections",
                http_int(&cfg, "max_connections"),
            ),
            ("components_http_base_url", http_str(&cfg, "base_url")),
        ],
    );
}

/// Case (e): an include fragment that itself declares `include` is
/// ignored — the include key never loads a third document — and the
/// loader's diagnostic is locked in `case_05_warn.txt`.
#[test]
fn parity_recursive_include_decl_ignored() {
    let _guard = env_lock();
    let dir = tempfile::tempdir().expect("temp dir");
    write_fixture(
        dir.path(),
        "Camel.toml",
        r#"
include = ["conf/base.toml"]

[default]
timeout_ms = 1000
log_level = "info"
"#,
    );
    write_fixture(
        dir.path(),
        "conf/base.toml",
        r#"
include = ["conf/nested.toml"]

[default]
log_level = "debug"

[default.components.http]
base_url = "http://from-base"
"#,
    );
    // Distinctive values that must NEVER surface: loading this third
    // document would flip `watch` and populate `routes`.
    write_fixture(
        dir.path(),
        "conf/nested.toml",
        r#"
[default]
watch = true
routes = ["routes/nested.yaml"]
"#,
    );

    let (loaded, warns) = capture_warns(|| CamelConfig::from_file(&config_path(dir.path())));
    let cfg = loaded.expect("load must succeed despite the recursive include declaration");

    // Only the two documents' merged values may be present: the config
    // document wins over the include, and the nested document's values
    // are absent entirely.
    assert!(
        !cfg.watch,
        "nested.toml must not load (its watch=true would surface)"
    );
    assert!(
        cfg.routes.is_empty(),
        "nested.toml must not load (its routes would surface)"
    );
    lock_json_golden(
        "case_05.json",
        &[
            ("timeout_ms", serde_json::json!(cfg.timeout_ms)),
            ("log_level", serde_json::json!(cfg.log_level)),
            ("components_http_base_url", http_str(&cfg, "base_url")),
            ("watch", serde_json::json!(cfg.watch)),
            ("routes", serde_json::json!(cfg.routes)),
        ],
    );

    assert!(
        !warns.is_empty(),
        "filesystem loader must diagnose the recursive include declaration"
    );
    lock_text_golden("case_05_warn.txt", &warns.join("\n"));
}

/// Case (f): an include fragment carrying its own `[default]`/
/// `[production]` sections gets per-file selection under the active
/// profile while the flat config document loads as-is.
#[test]
fn parity_include_with_own_profile_sections() {
    let _guard = env_lock();
    let dir = tempfile::tempdir().expect("temp dir");
    write_fixture(
        dir.path(),
        "Camel.toml",
        r#"
include = ["conf/frag.toml"]
timeout_ms = 1000
"#,
    );
    write_fixture(
        dir.path(),
        "conf/frag.toml",
        r#"
[default]
log_level = "info"
routes = ["routes/frag-default.yaml"]

[production]
log_level = "debug"
watch = true
routes = ["routes/frag-prod-main.yaml", "routes/frag-prod-extra.yaml"]

[production.components.http]
max_connections = 42
"#,
    );

    let cfg = CamelConfig::from_file_with_profile(&config_path(dir.path()), Some("production"))
        .expect("fragment profile selection loads");
    lock_json_golden(
        "case_06.json",
        &[
            ("timeout_ms", serde_json::json!(cfg.timeout_ms)),
            ("log_level", serde_json::json!(cfg.log_level)),
            ("watch", serde_json::json!(cfg.watch)),
            ("routes", serde_json::json!(cfg.routes)),
            (
                "components_http_max_connections",
                http_int(&cfg, "max_connections"),
            ),
        ],
    );
}

/// Case (g): `${env:PARITY_GOLDEN_VAR:-fallback}` resolves to `pinned`
/// with the variable set and `fallback` with it unset. Serialized on
/// the crate's env lock like every other env-mutating test.
#[test]
fn parity_env_override_set_and_unset() {
    let _guard = env_lock();
    let dir = tempfile::tempdir().expect("temp dir");
    write_fixture(
        dir.path(),
        "Camel.toml",
        r#"
log_level = "${env:PARITY_GOLDEN_VAR:-fallback}"
timeout_ms = 1000
"#,
    );
    let path = config_path(dir.path());

    // Arm (a): variable set.
    set_env("PARITY_GOLDEN_VAR", "pinned");
    let loaded = CamelConfig::from_file(&path);
    unset_env("PARITY_GOLDEN_VAR");
    let pinned = loaded.expect("set-arm load");
    lock_json_golden(
        "case_07a.json",
        &[
            ("log_level", serde_json::json!(pinned.log_level)),
            ("timeout_ms", serde_json::json!(pinned.timeout_ms)),
        ],
    );

    // Arm (b): variable unset.
    unset_env("PARITY_GOLDEN_VAR");
    let fallback = CamelConfig::from_file(&path).expect("unset-arm load");
    lock_json_golden(
        "case_07b.json",
        &[
            ("log_level", serde_json::json!(fallback.log_level)),
            ("timeout_ms", serde_json::json!(fallback.timeout_ms)),
        ],
    );
}

/// Case (h): section-level `routes` replacing top-level `routes`.
#[test]
fn parity_section_routes_replace_toplevel() {
    let _guard = env_lock();
    let dir = tempfile::tempdir().expect("temp dir");
    write_fixture(
        dir.path(),
        "Camel.toml",
        r#"
routes = ["routes/toplevel.yaml"]
timeout_ms = 5000
watch = true

[default]
routes = ["routes/section.yaml"]
timeout_ms = 6000
"#,
    );

    let cfg = CamelConfig::from_file(&config_path(dir.path())).expect("section overlay loads");
    lock_json_golden(
        "case_08.json",
        &[
            ("timeout_ms", serde_json::json!(cfg.timeout_ms)),
            ("routes", serde_json::json!(cfg.routes)),
            ("watch", serde_json::json!(cfg.watch)),
        ],
    );
}

/// Case (i): top-level `include = 42` (invalid type) fails with the
/// top-level error label spelling, locked byte-for-byte.
#[test]
fn parity_toplevel_include_invalid_type_error_locked() {
    let _guard = env_lock();
    let dir = tempfile::tempdir().expect("temp dir");
    write_fixture(
        dir.path(),
        "Camel.toml",
        r#"
include = 42
timeout_ms = 1000
"#,
    );

    let err = CamelConfig::from_file(&config_path(dir.path())).expect_err("must fail");
    let display = err.to_string();
    assert!(!display.is_empty(), "error Display must not be empty");
    lock_text_golden("case_09_error.txt", &display);
}

/// Case (j): `[default].include = 42` (invalid type at section level)
/// fails with the section-scoped error label spelling, locked
/// byte-for-byte.
#[test]
fn parity_section_include_invalid_type_error_locked() {
    let _guard = env_lock();
    let dir = tempfile::tempdir().expect("temp dir");
    write_fixture(
        dir.path(),
        "Camel.toml",
        r#"
[default]
include = 42
timeout_ms = 1000
"#,
    );

    let err = CamelConfig::from_file(&config_path(dir.path())).expect_err("must fail");
    let display = err.to_string();
    assert!(!display.is_empty(), "error Display must not be empty");
    lock_text_golden("case_10_error.txt", &display);
}
