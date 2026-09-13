//! `[jobs]` table round-trip and discovery-root precedence tests.
//!
//! The table is top-level (`[jobs]`, NOT a key on `[context]`): jobs are an
//! operator tool surface, not the always-on data plane. Discovery roots are
//! normalized through `resolved_dirs()` (change `jobdiscovery`): `dirs` is
//! the ordered set whenever present — an explicit `dirs = []` means "no
//! roots" and wins over legacy `dir` — while the legacy `dir` alias folds
//! into a single root, and `["jobs"]` applies only when both keys are
//! absent. Roots are resolved against the Camel.toml root by callers.

use super::*;
use crate::config::log_capture::capture_warns;

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
fn explicit_job_dirs_override_legacy_dir() {
    let cfg: CamelConfig =
        toml::from_str("[jobs]\ndir = \"legacy\"\ndirs = [\"first\", \"second\"]\n")
            .expect("dual-key [jobs] table parses");
    assert_eq!(
        cfg.jobs.resolved_dirs(),
        ["first", "second"],
        "dirs must take precedence and legacy dir must not add a duplicate root"
    );
}

#[test]
fn empty_dirs_override_legacy_dir() {
    let cfg: CamelConfig =
        toml::from_str("[jobs]\ndir = \"legacy\"\ndirs = []\n").expect("[jobs] table parses");
    assert!(
        cfg.jobs.resolved_dirs().is_empty(),
        "explicit dirs = [] must win over legacy dir and normalize to no roots"
    );
}

#[test]
fn jobs_table_unknown_key_rejected() {
    let err = toml::from_str::<CamelConfig>("[jobs]\nbogus = true\n")
        .expect_err("deny_unknown_fields must reject unknown [jobs] keys");
    let msg = err.to_string();
    assert!(
        msg.contains("unknown field"),
        "error must name the unknown field; got: {msg}"
    );
}

#[test]
fn legacy_jobs_dir_alias_is_supported() {
    let cfg: CamelConfig =
        toml::from_str("[jobs]\ndir = \"legacy-jobs\"\n").expect("[jobs] table parses");
    assert_eq!(
        cfg.jobs.resolved_dirs(),
        ["legacy-jobs"],
        "legacy dir must fold into exactly one root"
    );
}

#[test]
fn jobs_table_dir_round_trip() {
    let cfg: CamelConfig =
        toml::from_str("[jobs]\ndir = \"ops/jobs\"\n").expect("[jobs] table parses");
    assert_eq!(
        cfg.jobs.resolved_dirs(),
        ["ops/jobs"],
        "explicit legacy dir must survive the merge as the sole root"
    );
}

#[test]
fn jobs_overlay_overrides_default() {
    let built = CamelConfigBuilder::default()
        .jobs(JobsCamelConfig {
            dir: Some("ops/jobs".to_string()),
            dirs: None,
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
fn jobs_table_no_unselected_profile_warning() {
    let _env_guard = env_lock();
    unset_env("CAMEL_PROFILE");

    let tree = toml::from_str(
        r#"
[default]
log_level = "info"

[jobs]
dir = "ops/jobs"
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
