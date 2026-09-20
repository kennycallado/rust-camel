//! Regression tests for the fail-closed `camel run` route-add policy
//! (bd rc-n6jre): a discovered route that cannot be registered — unknown
//! component scheme, duplicate id, ... — must abort startup with a
//! non-zero exit instead of booting an empty context that runs forever
//! and exits 0 on the first signal.
//!
//! Feature-independent by construction: the route references a scheme no
//! flavor ever ships, so the abort fires identically under the default
//! flavor and under `--features exec`. The initial load shares one code
//! path for non-watch and watch modes (the reload watcher starts only
//! after route registration), so the single `watch = false` fixture here
//! covers both; ExecBundle-specific startup-guard behavior has its own
//! exec-gated suite in `run_exec_guard_test.rs`.
//!
//! These tests spawn the actual `camel` binary (via `CARGO_BIN_EXE_camel`)
//! against a fresh `tempfile::TempDir` fixture, reusing the shared
//! subprocess plumbing in `tests/common`.

mod common;

use std::time::Duration;

use common::{spawn_camel_run, spawn_drained, wait_exit_code_bounded};

/// Route-add failure aborts startup: unknown scheme, non-zero exit, cause
/// and route id surfaced, fast. The 5 s ceiling is the assertion — the
/// pre-fix behavior was the rc-n6jre hang (empty context running forever),
/// which must fail here in 5 s, not after the generous 30 s test deadline.
#[test]
fn unknown_scheme_route_add_aborts_startup() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("Camel.toml"),
        r#"[default]
routes = ["routes/*.yaml"]
log_level = "INFO"
watch = false
"#,
    )
    .expect("write Camel.toml");
    std::fs::create_dir_all(dir.path().join("routes")).expect("create routes/");
    std::fs::write(
        dir.path().join("routes").join("bad.yaml"),
        r#"routes:
  - id: "badscheme"
    from: "timer:tick?period=60000"
    steps:
      - to: "nosuchcomponent:x"
"#,
    )
    .expect("write route yaml");

    let mut child = spawn_camel_run(dir.path());
    let drained = spawn_drained(&mut child.0);

    let exit_code = wait_exit_code_bounded(&mut child.0, Duration::from_secs(5));

    let output = drained.finish();

    assert!(
        exit_code > 0,
        "route-add failure must abort with a non-zero exit code within 5 s \
         (got {exit_code}; -1 means still running at the deadline — the \
         rc-n6jre empty-context hang)\n{output}"
    );
    assert!(
        output.contains("Component not found"),
        "abort output must name the cause; got:\n{output}"
    );
    assert!(
        output.contains("badscheme"),
        "abort output must surface the route id; got:\n{output}"
    );
}
