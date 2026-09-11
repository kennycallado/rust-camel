//! `[jobs]` table round-trip tests (bd rc-10d50, change `job-ux-reshape`).
//!
//! The table is top-level (`[jobs]`, NOT a key on `[context]`): jobs are an
//! operator tool surface, not the always-on data plane. One key, `dir`,
//! defaulting to `"jobs"`, resolved against the Camel.toml root by callers.

use super::*;
use crate::config::log_capture::capture_warns;

#[test]
fn jobs_table_defaults_when_absent() {
    let cfg: CamelConfig = toml::from_str("routes = []\n").expect("minimal Camel.toml parses");
    assert_eq!(
        cfg.jobs.dir, "jobs",
        "[jobs] absent must default dir to \"jobs\""
    );
}

#[test]
fn jobs_table_dir_round_trip() {
    let cfg: CamelConfig =
        toml::from_str("[jobs]\ndir = \"ops/jobs\"\n").expect("[jobs] table parses");
    assert_eq!(
        cfg.jobs.dir, "ops/jobs",
        "explicit dir must survive the merge"
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
fn jobs_overlay_overrides_default() {
    let built = CamelConfigBuilder::default()
        .jobs(JobsCamelConfig {
            dir: "ops/jobs".to_string(),
        })
        .build();
    assert_eq!(
        built.jobs.dir, "ops/jobs",
        "overlay jobs() must win over the default"
    );
    let plain = CamelConfigBuilder::default().build();
    assert_eq!(plain.jobs.dir, "jobs", "no overlay keeps the default");
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
