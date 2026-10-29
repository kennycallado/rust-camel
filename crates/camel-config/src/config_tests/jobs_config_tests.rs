//! `[jobs]` table round-trip and discovery-root precedence tests.
//!
//! The table is top-level (`[jobs]`, NOT a key on `[context]`): jobs are an
//! operator tool surface, not the always-on data plane. Discovery roots are
//! normalized through `resolved_dirs()` (change `jobdiscovery`): `dirs` is
//! the ordered set whenever present — an explicit `dirs = []` means "no
//! roots" — and `["jobs"]` applies only when `dirs` is absent. Roots are
//! resolved against the Camel.toml root by callers.

use super::*;
use crate::config::log_capture::capture_warns;
use camel_component_api::test_support::EnvGuard;

#[test]
fn jobs_table_defaults_to_jobs() {
    let cfg: CamelConfig = toml::from_str("routes = []\n").expect("minimal Camel.toml parses");
    assert_eq!(
        cfg.jobs.resolved_dirs(),
        ["jobs"],
        "[jobs] absent must normalize to the default root [\"jobs\"]"
    );
}

#[test]
fn jobs_table_dirs_round_trip() {
    let cfg: CamelConfig =
        toml::from_str("[jobs]\ndirs = [\"first\", \"second\"]\n").expect("[jobs] table parses");
    assert_eq!(
        cfg.jobs.resolved_dirs(),
        ["first", "second"],
        "dirs must round-trip preserving declared order"
    );
}

#[test]
fn explicit_empty_dirs_mean_no_roots() {
    let cfg: CamelConfig = toml::from_str("[jobs]\ndirs = []\n").expect("[jobs] table parses");
    assert!(
        cfg.jobs.resolved_dirs().is_empty(),
        "explicit dirs = [] must normalize to no roots"
    );
}

#[test]
fn jobs_table_unknown_key_rejected() {
    let err = toml::from_str::<CamelConfig>("[jobs]\nbogus = true\n")
        .expect_err("unknown [jobs] keys must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("unknown field"),
        "error must name the unknown field; got: {msg}"
    );
}

#[test]
fn jobs_dir_alias_is_a_tombstone() {
    let err = toml::from_str::<CamelConfig>("[jobs]\ndir = \"legacy\"\n")
        .expect_err("removed [jobs].dir alias must fail the load");
    let msg = err.to_string();
    assert!(
        msg.contains("dir"),
        "error must name the removed key; got: {msg}"
    );
    assert!(
        msg.contains("dirs"),
        "error must name the replacement key; got: {msg}"
    );
    assert!(
        msg.contains("dirs = ["),
        "error must show the replacement spelling `dirs = [...]`; got: {msg}"
    );
}

#[test]
fn jobs_overlay_overrides_default() {
    let built = CamelConfigBuilder::default()
        .jobs(JobsCamelConfig {
            dirs: Some(vec!["ops/jobs".to_string()]),
        })
        .build();
    assert_eq!(
        built.jobs.resolved_dirs(),
        ["ops/jobs"],
        "overlay jobs() must win over the default"
    );
    let plain = CamelConfigBuilder::default().build();
    assert_eq!(
        plain.jobs.resolved_dirs(),
        ["jobs"],
        "no overlay keeps the default"
    );
}

#[test]
fn merged_profile_legacy_dir_is_a_tombstone() {
    // Cross-profile additive-merge shape (dossier jobconfig-findings): the
    // profile merge is key-by-key, so [default.jobs] dirs + [development.jobs]
    // dir leaves BOTH keys on the merged table. The load must fail loudly:
    // the removed `dir` key is a tombstone, not a silently ignored leftover.
    let _env_guard = EnvGuard::remove("CAMEL_PROFILE");

    let tree = toml::from_str(
        r#"
[default.jobs]
dirs = ["first", "second"]

[development.jobs]
dir = "legacy"
"#,
    )
    .expect("fixture must be valid TOML");

    let err = build_from_toml_value_inner(
        tree,
        Some("development"),
        false,
        Vec::new(),
        &super::ambient_lookup(),
    )
    .expect_err("merged profile carrying the removed `dir` key must fail the load");

    let msg = err.to_string();
    assert!(
        msg.contains("dir"),
        "error must name the removed key; got: {msg}"
    );
    assert!(
        msg.contains("dirs"),
        "error must name the replacement key; got: {msg}"
    );
}

#[test]
fn jobs_table_no_unselected_profile_warning() {
    let _env_guard = EnvGuard::remove("CAMEL_PROFILE");

    let tree = toml::from_str(
        r#"
[default]
log_level = "info"

[jobs]
dirs = ["ops/jobs"]
"#,
    )
    .expect("fixture must be valid TOML");

    let (_result, warns) = capture_warns(|| {
        build_from_toml_value_inner(tree, None, false, Vec::new(), &super::ambient_lookup())
    });

    let hits: Vec<&String> = warns
        .iter()
        .filter(|w| w.contains("jobs") && w.contains("CAMEL_PROFILE"))
        .collect();
    assert!(
        hits.is_empty(),
        "[jobs] is a known top-level key and must not warn as an unselected profile; got {hits:?} in {warns:?}"
    );
}
