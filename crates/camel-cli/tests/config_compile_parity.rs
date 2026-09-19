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
//! Regenerate the goldens from the current tree with:
//!
//! ```text
//! UPDATE_GOLDENS=1 cargo test -p camel-cli --test config_compile_parity
//! ```

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use camel_cli::compile::store::VirtualDocumentStore;
use camel_cli::compile::trailer::{self, DecodedArtifact};

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
