//! Compiled-artifact configuration parity goldens (openspec change
//! `configunify`, Task 1.3).
//!
//! The tests lock the PRE-refactor behavior of the `camel-cli` compile
//! path: a fixture project with profiles, ordered includes carrying
//! their own profile sections, and route patterns is compiled through
//! the real `camel` binary (`CARGO_BIN_EXE_camel`) with
//! `--config`/`--profile`, the artifact's embedded store is decoded,
//! and the merged configuration is recomputed through the same
//! discovery entry the runtime uses (`camel_dsl::discover_virtual_store`)
//! and compared byte-for-byte against committed goldens under
//! `tests/goldens/config-parity/`. A third golden locks the partial
//! multi-profile absence error text (stderr is observable behavior).
//! Three further goldens arrive from openspec change `profilestrict`
//! (Task 1.1): `include_only_profile_error.txt` locks the include-only
//! rejection stderr (selected profiles only in includes while the
//! document carries `[default]`), `include_only_then_absence_error.txt`
//! locks the chain-wide-absence precedence of the per-profile
//! `UnknownProfile` error, and `no_profiles_resolved.toml` locks the
//! no-profile resolved configuration. Two final goldens arrive from
//! the same change (Task 1.2): `mixed_resolved.toml` locks the
//! mixed multi-profile selection (config-declared `[prod]` plus an
//! include-only `[canary]`) and `flat_include_profile_resolved.toml`
//! locks the lenient include-profile path for a flat configuration.
//!
//! The cfgdrop2 section adds the compile-side mirror guard for
//! root-level config keys beside profile structure: behavioral
//! rejections, the root-`routes` acceptance lock, the no-`--config`
//! escape, and a six-class parity matrix asserting that
//! `sources::resolve` and the camel-config filesystem loader
//! (`CamelConfig::from_file_with_profile`) give the SAME document
//! text the SAME disposition.
//!
//! Regenerate the goldens from the current tree with:
//!
//! ```text
//! UPDATE_GOLDENS=1 cargo test -p camel-cli --test config_compile_parity
//! ```

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use camel_cli::compile::sources::{self, SourceError, SourceSelection};
use camel_cli::compile::store::VirtualDocumentStore;
use camel_cli::compile::trailer::{self, DecodedArtifact, TrailerKind};
use camel_config::CamelConfig;

/// The committed fixture project, anchored at the camel-cli crate.
fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config-parity-project")
}

/// Golden directory for this suite.
fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/goldens/config-parity")
        .join(name)
}

/// Regeneration switch: when `UPDATE_GOLDENS=1` is set, write the
/// golden files instead of comparing them.
fn update_goldens() -> bool {
    std::env::var("UPDATE_GOLDENS").is_ok_and(|v| v == "1")
}

/// Byte-for-byte golden lock for text artifacts (the resolved merged
/// configuration and the rejection stderr transcript).
fn lock_text_golden(name: &str, actual: &str) {
    let path = golden_path(name);
    if update_goldens() {
        std::fs::create_dir_all(path.parent().expect("golden parent dir"))
            .expect("create goldens dir");
        std::fs::write(&path, actual).unwrap_or_else(|e| panic!("write golden {name}: {e}"));
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read golden {name}: {e} (capture: UPDATE_GOLDENS=1)"));
    assert_eq!(
        expected, actual,
        "observed text diverges from golden {name}"
    );
}

/// Spawn `camel compile app.yaml -o <artifact> --config <config>
/// --profile <name>...` with the fixture project as the working
/// directory and a cleared environment (no inherited `CAMEL_*`, no
/// ambient values) — the same invocation convention as
/// `compile_command_test.rs`. The artifact lands in `artifact` (a
/// tempdir path) so the committed fixture tree stays untouched.
fn compile_with_profiles(config: &str, profiles: &[&str], artifact: &Path) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_camel"));
    cmd.env_clear().current_dir(fixture_dir());
    cmd.arg("compile").arg("app.yaml").arg("-o").arg(artifact);
    cmd.arg("--config").arg(config);
    for profile in profiles {
        cmd.arg("--profile").arg(profile);
    }
    cmd.output().expect("spawn `camel compile`")
}

/// stderr of a finished child, for assertion-failure context.
fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Decode a compiled v2 artifact image into its validated
/// multi-document store. Panics when the image is not a marked, valid
/// v2 artifact.
fn decoded_store(bytes: &[u8]) -> VirtualDocumentStore {
    let decoded = trailer::decode_artifact(bytes)
        .expect("trailer must be intact")
        .expect("terminal magic must mark the trailer present");
    let DecodedArtifact::V2(v2) = decoded else {
        panic!("compile must emit a v2 multi-document artifact");
    };
    VirtualDocumentStore::decode(v2.content, &v2.index).expect("store must decode")
}

/// Recompute the merged configuration the artifact runtime would use:
/// `discover_virtual_store` over the embedded store (the same
/// assembly `build_virtual_config` performs for compiled artifacts),
/// serialized the same way the goldens were captured. `${env:}`
/// placeholders stay unresolved here (env resolution happens later,
/// against the deployment lookup), so the goldens are deterministic.
fn resolved_config_toml(store: &VirtualDocumentStore) -> String {
    let discovered =
        camel_dsl::discover_virtual_store(store, &|_: &str| -> Option<String> { None })
            .expect("runtime discovery must succeed on the compiled store");
    toml::to_string_pretty(&discovered.config).expect("serialize the resolved configuration")
}

/// Compile the fixture with `--profile production` and lock the
/// runtime-resolved merged configuration byte-for-byte.
#[test]
fn compiled_artifact_resolved_config_production_golden() {
    let dir = tempfile::tempdir().expect("tempdir");
    let artifact = dir.path().join("app.bin");

    let output = compile_with_profiles("Camel.toml", &["production"], &artifact);
    assert_eq!(
        output.status.code(),
        Some(0),
        "fixture project must compile with --profile production: {}",
        stderr_of(&output)
    );

    let bytes = std::fs::read(&artifact).expect("artifact must exist after exit 0");
    let actual = resolved_config_toml(&decoded_store(&bytes));
    lock_text_golden("production.toml", &actual);
}

/// Compile the fixture with `--profile qa` and lock the
/// runtime-resolved merged configuration byte-for-byte.
#[test]
fn compiled_artifact_resolved_config_qa_golden() {
    let dir = tempfile::tempdir().expect("tempdir");
    let artifact = dir.path().join("app.bin");

    let output = compile_with_profiles("Camel.toml", &["qa"], &artifact);
    assert_eq!(
        output.status.code(),
        Some(0),
        "fixture project must compile with --profile qa: {}",
        stderr_of(&output)
    );

    let bytes = std::fs::read(&artifact).expect("artifact must exist after exit 0");
    let actual = resolved_config_toml(&decoded_store(&bytes));
    lock_text_golden("qa.toml", &actual);
}

/// Partial multi-profile absence is a named rejection: on the
/// `[qa]`-less variant fixture, `--profile production --profile qa`
/// must exit non-zero with the exact pre-refactor stderr (the golden
/// is the authority), and no artifact may be written.
#[test]
fn compile_partial_profile_absence_error_locked() {
    let dir = tempfile::tempdir().expect("tempdir");
    let artifact = dir.path().join("app.bin");

    let output = compile_with_profiles("Camel-partial.toml", &["production", "qa"], &artifact);
    assert!(
        !output.status.success(),
        "partial profile absence must fail compilation: {}",
        stderr_of(&output)
    );
    assert!(
        !artifact.exists(),
        "a rejected compile must not write an artifact"
    );

    let stderr = stderr_of(&output);
    lock_text_golden("partial_absence_error.txt", &stderr);
}

/// Include-only profile selection is rejected at compile time (the
/// strict-at-compile mirror): on the strict fixture, `[prod]` exists
/// only in `includes/strict-prod.toml` while the configuration document
/// carries `[default]`, so `--profile prod` must exit non-zero with the
/// include-only diagnostic and no artifact may be written.
#[test]
fn compile_include_only_profile_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let artifact = dir.path().join("app.bin");

    let output = compile_with_profiles("Camel-strict.toml", &["prod"], &artifact);
    assert!(
        !output.status.success(),
        "include-only profile selection must fail compilation: {}",
        stderr_of(&output)
    );
    assert!(
        !artifact.exists(),
        "a rejected compile must not write an artifact"
    );

    let stderr = stderr_of(&output);
    lock_text_golden("include_only_profile_error.txt", &stderr);
}

/// Include-only selection defers to chain-wide absence: with
/// `--profile prod --profile qa` on the strict fixture, `[qa]` exists
/// nowhere in the configuration chain, so the per-profile
/// `UnknownProfile` rejection keeps precedence over the include-only
/// gate (the frozen per-profile error form).
#[test]
fn compile_include_only_profile_defers_to_total_absence() {
    let dir = tempfile::tempdir().expect("tempdir");
    let artifact = dir.path().join("app.bin");

    let output = compile_with_profiles("Camel-strict.toml", &["prod", "qa"], &artifact);
    assert!(
        !output.status.success(),
        "total profile absence must fail compilation: {}",
        stderr_of(&output)
    );
    assert!(
        !artifact.exists(),
        "a rejected compile must not write an artifact"
    );

    let stderr = stderr_of(&output);
    lock_text_golden("include_only_then_absence_error.txt", &stderr);
}

/// A profile-less compile of a `[default]`-carrying configuration stays
/// green: the empty-profiles guard of the strict-at-compile mirror (and
/// of the runtime mirror in `virtual_config.rs`) keeps the
/// default-section selection valid.
#[test]
fn compile_no_profiles_default_config_accepted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let artifact = dir.path().join("app.bin");

    let output = compile_with_profiles("Camel.toml", &[], &artifact);
    assert!(
        output.status.success(),
        "profile-less compile of a [default]-carrying config must succeed: {}",
        stderr_of(&output)
    );
    assert!(
        artifact.exists(),
        "an accepted compile must write an artifact"
    );

    let bytes = std::fs::read(&artifact).expect("read the compiled artifact");
    let actual = resolved_config_toml(&decoded_store(&bytes));
    lock_text_golden("no_profiles_resolved.toml", &actual);
}

/// Mixed multi-profile selection stays accepted: on the mixed fixture,
/// `[prod]` is declared by the configuration document itself while
/// `[canary]` exists only in `includes/mixed-canary.toml`, so the
/// selection keeps at least one config-declared profile and the
/// strict-at-compile gate must not reject it (regression lock for the
/// unchanged acceptance region). Layering follows include-below-config
/// precedence (the same order the frozen `production.toml` golden
/// locks): the config document merges over includes, so the
/// config-declared `[prod]` keeps `log_level = "warn"` and canary
/// survives only with keys the document does not override
/// (`watch = true`).
#[test]
fn compile_mixed_profile_selection_accepted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let artifact = dir.path().join("app.bin");

    let output = compile_with_profiles("Camel-mixed.toml", &["prod", "canary"], &artifact);
    assert!(
        output.status.success(),
        "mixed multi-profile selection must compile: {}",
        stderr_of(&output)
    );
    assert!(
        artifact.exists(),
        "an accepted compile must write an artifact"
    );

    let bytes = std::fs::read(&artifact).expect("read the compiled artifact");
    let actual = resolved_config_toml(&decoded_store(&bytes));
    lock_text_golden("mixed_resolved.toml", &actual);
}

/// A flat configuration keeps the lenient include-profile path: the
/// flat fixture carries no `[default]` and no profile section of its
/// own, so the include-only `[prod]` selection must stay accepted at
/// compile time — the strict gate only applies to documents with
/// profile structure (regression lock for the unchanged acceptance
/// region).
#[test]
fn compile_flat_config_include_profile_accepted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let artifact = dir.path().join("app.bin");

    let output = compile_with_profiles("Camel-flat.toml", &["prod"], &artifact);
    assert!(
        output.status.success(),
        "include-profile selection on a flat config must compile: {}",
        stderr_of(&output)
    );
    assert!(
        artifact.exists(),
        "an accepted compile must write an artifact"
    );

    let bytes = std::fs::read(&artifact).expect("read the compiled artifact");
    let actual = resolved_config_toml(&decoded_store(&bytes));
    lock_text_golden("flat_include_profile_resolved.toml", &actual);
}

// ---------------------------------------------------------------------------
// cfgdrop2 Task 2.1: compile-side mirror guard + cross-path parity asserts
// ---------------------------------------------------------------------------

/// Minimal route document for the guard fixtures (same shape as the
/// committed fixture `app.yaml`).
const GUARD_ROUTE_DOC: &str =
    "routes:\n  - id: guard-entry\n    from: direct:start\n    steps:\n      - to: log:guard\n";

/// Minimal route file matched by the `routes/*.yaml` pattern in the
/// guard fixtures (same shape as the committed fixture route).
const GUARD_ROUTE_FILE: &str =
    "routes:\n  - id: parity-main\n    from: direct:start\n    steps:\n      - to: log:main\n";

/// Materialize a one-document route project with a configuration
/// document under test (`Camel.toml`). `extra` entries are
/// `(relative path, text)` pairs (e.g. a route file for a
/// `routes/*.yaml` pattern). Returns the tempdir; the caller keeps it
/// alive for the duration of the assertions.
fn guard_project(config_text: &str, extra: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("app.yaml"), GUARD_ROUTE_DOC).expect("write app.yaml");
    std::fs::write(dir.path().join("Camel.toml"), config_text).expect("write Camel.toml");
    for &(name, text) in extra {
        let path = dir.path().join(name);
        std::fs::create_dir_all(path.parent().expect("fixture path has a parent"))
            .expect("create fixture subdir");
        std::fs::write(path, text).expect("write fixture extra file");
    }
    dir
}

/// Run `sources::resolve` with the selection `camel compile --config
/// <name> --profile <name>...` would build (CLI-default payload cap).
/// `config = None` reproduces the no-`--config` invocation.
fn resolve_config(
    dir: &Path,
    config: Option<&str>,
    profiles: &[&str],
) -> Result<sources::ResolvedSources, SourceError> {
    let selection = SourceSelection {
        config_path: config.map(|name| dir.join(name)),
        profiles: profiles.iter().map(|p| (*p).to_string()).collect(),
        max_payload_bytes: trailer::MAX_PAYLOAD_BYTES as u64,
        embed_secrets: false,
    };
    sources::resolve(&dir.join("app.yaml"), TrailerKind::Route, &selection)
}

/// A root-level known scalar beside `[default]` is mirror-rejected at
/// compile time: resolution fails naming the key and the accepted
/// shapes — the loader's disposition for the same document.
#[test]
fn compile_rejects_root_known_scalar_beside_default() {
    let dir = guard_project("timeout_ms = 5\n\n[default]\nx = 1\n", &[]);
    let err = resolve_config(dir.path(), Some("Camel.toml"), &[])
        .expect_err("root known scalar beside [default] must fail compilation");
    let msg = err.to_string();
    assert!(
        msg.contains("timeout_ms"),
        "error must name the discarded key: {msg}"
    );
    assert!(
        msg.contains("flat document"),
        "error must name the flat-document accepted shape: {msg}"
    );
}

/// A root-level known table beside `[default]` is mirror-rejected at
/// compile time: resolution fails naming the key (the silent-discard
/// class, table form).
#[test]
fn compile_rejects_root_known_table_beside_default() {
    let dir = guard_project(
        "[runtime_journal]\npath = \"j.db\"\n\n[default]\nlog_level = \"info\"\n",
        &[],
    );
    let err = resolve_config(dir.path(), Some("Camel.toml"), &[])
        .expect_err("root known table beside [default] must fail compilation");
    let msg = err.to_string();
    assert!(
        msg.contains("runtime_journal"),
        "error must name the discarded key: {msg}"
    );
}

/// Root `routes` beside `[default]` stays accepted (the documented
/// exception): resolution succeeds and the plan's route patterns
/// start from the root list — the pattern-accumulator overlay walk is
/// untouched by the guard.
#[test]
fn compile_accepts_root_routes_beside_default_unchanged() {
    let dir = guard_project(
        "routes = [\"routes/*.yaml\"]\n\n[default]\nlog_level = \"info\"\n",
        &[("routes/main.yaml", GUARD_ROUTE_FILE)],
    );
    let resolved = resolve_config(dir.path(), Some("Camel.toml"), &[])
        .expect("root routes beside [default] must keep compiling");
    assert_eq!(
        resolved.source_plan,
        vec!["app.yaml".to_string(), "routes/main.yaml".to_string()],
        "route patterns must start from the root routes list"
    );
}

/// A near-miss root table beside a selected profile section is
/// mirror-rejected: resolution fails naming both the misspelled table
/// and the probable intended key.
#[test]
fn compile_rejects_near_miss_table_beside_selected_profile() {
    let dir = guard_project(
        "[obsevrability]\ny = 2\n\n[prod]\nlog_level = \"warn\"\n",
        &[],
    );
    let err = resolve_config(dir.path(), Some("Camel.toml"), &["prod"])
        .expect_err("near-miss table beside selected profile must fail compilation");
    let msg = err.to_string();
    assert!(
        msg.contains("obsevrability"),
        "error must name the misspelled table: {msg}"
    );
    assert!(
        msg.contains("observability"),
        "error must name the probable intended key: {msg}"
    );
}

/// A far-name root table beside a selected profile stays accepted
/// (negative lock): unselected-profile semantics are untouched by the
/// near-miss discriminator.
#[test]
fn compile_accepts_far_table_beside_selected_profile() {
    let dir = guard_project("[staging]\ny = 2\n\n[prod]\nlog_level = \"info\"\n", &[]);
    resolve_config(dir.path(), Some("Camel.toml"), &["prod"])
        .expect("far-name table beside selected profile must stay accepted");
}

/// Without `--config` the guard never runs: a route document beside a
/// stray `Camel.toml` carrying root keys + `[default]` compiles
/// untouched (no configuration is read at all).
#[test]
fn compile_guard_needs_config_selection() {
    let dir = guard_project("timeout_ms = 5\n\n[default]\nx = 1\n", &[]);
    let resolved =
        resolve_config(dir.path(), None, &[]).expect("no --config selection must bypass the guard");
    assert!(
        resolved.config_references.is_empty(),
        "no-config compile must embed no configuration: {:?}",
        resolved.config_references
    );
}

/// Cross-path parity matrix (cfgdrop2): the SAME document text drives
/// (a) the compile guard path (`sources::resolve` with an explicit
/// `--config` selection) and (b) the camel-config filesystem loader
/// (`CamelConfig::from_file_with_profile` over the same temp file).
/// For every class, `(compile.is_err(), error-names-key)` must equal
/// `(loader.is_err(), error-names-key)` — one document, one
/// disposition on both front doors. Loader profiles are pinned
/// explicitly (`Some("default")` / `Some("prod")`) so ambient
/// `CAMEL_PROFILE` never skews a class; the flat class passes `None`
/// (lenient path, ambient-profile immune).
#[test]
fn parity_matrix_loader_vs_compile_same_disposition() {
    struct MatrixClass {
        name: &'static str,
        config_text: &'static str,
        compile_profiles: &'static [&'static str],
        loader_profile: Option<&'static str>,
        /// Substrings every error on BOTH paths must contain (empty =
        /// the class is accepted on both paths).
        expect_named: &'static [&'static str],
        extra: &'static [(&'static str, &'static str)],
    }
    let classes = [
        MatrixClass {
            name: "root known scalar beside [default]",
            config_text: "timeout_ms = 5\n\n[default]\nlog_level = \"info\"\n",
            compile_profiles: &[],
            loader_profile: Some("default"),
            expect_named: &["timeout_ms"],
            extra: &[],
        },
        MatrixClass {
            name: "root known table beside [default]",
            config_text: "[runtime_journal]\npath = \"j.db\"\n\n[default]\nlog_level = \"info\"\n",
            compile_profiles: &[],
            loader_profile: Some("default"),
            expect_named: &["runtime_journal"],
            extra: &[],
        },
        MatrixClass {
            name: "root routes beside [default]",
            config_text: "routes = [\"routes/*.yaml\"]\n\n[default]\nlog_level = \"info\"\n",
            compile_profiles: &[],
            loader_profile: Some("default"),
            expect_named: &[],
            extra: &[("routes/main.yaml", GUARD_ROUTE_FILE)],
        },
        MatrixClass {
            name: "near-miss table beside selected profile",
            config_text: "[obsevrability]\ny = 2\n\n[prod]\nlog_level = \"warn\"\n",
            compile_profiles: &["prod"],
            loader_profile: Some("prod"),
            expect_named: &["obsevrability", "observability"],
            extra: &[],
        },
        MatrixClass {
            name: "far table beside selected profile",
            config_text: "[staging]\ny = 2\n\n[prod]\nlog_level = \"info\"\n",
            compile_profiles: &["prod"],
            loader_profile: Some("prod"),
            expect_named: &[],
            extra: &[],
        },
        MatrixClass {
            name: "flat document",
            config_text: "log_level = \"info\"\ntimeout_ms = 30000\n",
            compile_profiles: &[],
            loader_profile: None,
            expect_named: &[],
            extra: &[],
        },
    ];
    for class in classes {
        let dir = guard_project(class.config_text, class.extra);
        let compile = resolve_config(dir.path(), Some("Camel.toml"), class.compile_profiles);
        let loader = CamelConfig::from_file_with_profile(
            dir.path()
                .join("Camel.toml")
                .to_str()
                .expect("utf-8 temp path"),
            class.loader_profile,
        );
        assert_eq!(
            compile.is_err(),
            loader.is_err(),
            "class {:?}: loader and compile must agree on the disposition \
             (compile {:?}, loader {:?})",
            class.name,
            compile.as_ref().err().map(ToString::to_string),
            loader.as_ref().err().map(ToString::to_string),
        );
        for expected in class.expect_named {
            if let Err(err) = &compile {
                let msg = err.to_string();
                assert!(
                    msg.contains(expected),
                    "class {:?}: compile error must name {expected:?}: {msg}",
                    class.name
                );
            }
            if let Err(err) = &loader {
                let msg = err.to_string();
                assert!(
                    msg.contains(expected),
                    "class {:?}: loader error must name {expected:?}: {msg}",
                    class.name
                );
            }
        }
    }
}
