//! End-to-end tests for the `camel test` tier filters, tier annotation,
//! and ADR-0069 §7 taxonomy exit mapping. These spawn the real `camel`
//! binary (bin name pinned by `camel-cli`'s `[[bin]]`), so they cover
//! the `main.rs` dispatch through `run_tests_full` — the unit tests in
//! `commands::test` cover the driver in-process.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Create a unique temp directory for one test.
fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("camel-test-tier-{tag}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("create temp dir"); // allow-unwrap
    dir
}

/// Write a passing lean unit-tier document (one `direct:` input →
/// `mock:out`, count 1).
fn write_lean(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(
        &path,
        r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "mock:out"
inputs:
  - to: "direct:start"
    body: "x"
expects:
  mock:out:
    count: 1
"#,
    )
    .expect("write lean doc"); // allow-unwrap
    path
}

/// Write a passing unit-tier document with a FAILING expectation
/// (count 2, only 1 delivered) — a verdict-class, exit-1 failure.
fn write_failing_unit(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(
        &path,
        r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "mock:out"
inputs:
  - to: "direct:start"
    body: "x"
expects:
  mock:out:
    count: 2
"#,
    )
    .expect("write failing unit doc"); // allow-unwrap
    path
}

/// Write a full-tier scenario document whose actions run against the
/// in-memory fake adapter (send records, sleep settles; no receive, so
/// nothing times out).
fn write_fake_scenario(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(
        &path,
        r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "mock:out"
scenario:
  - send:
      to: "fake:partner"
      body: "hello"
  - sleep:
      duration: "10ms"
"#,
    )
    .expect("write fake scenario doc"); // allow-unwrap
    path
}

/// Write a full-tier scenario document whose `receive` action times out
/// (the fake adapter is bound but never delivers) — a verdict-class,
/// exit-1 failure.
fn write_timeout_scenario(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(
        &path,
        r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "mock:out"
scenario:
  - send:
      to: "fake:partner"
      body: "hello"
  - receive:
      from: "fake:never"
      deadline: "50ms"
"#,
    )
    .expect("write timeout scenario doc"); // allow-unwrap
    path
}

/// Write a full-tier scenario document declaring a real-transport
/// endpoint on a scheme no build provisions (`grpc`): the run reports
/// `infra-unavailable` naming the adapter — an apparatus-class, exit-2
/// failure — in featureless and `integration-http` builds alike.
fn write_grpc_scenario(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(
        &path,
        r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "mock:out"
scenario:
  - send:
      to: "grpc://127.0.0.1:1/hook"
      body: "hello"
"#,
    )
    .expect("write grpc scenario doc"); // allow-unwrap
    path
}

/// The path as the CLI displays it: relative to the run directory when
/// the document lives under it (the binary runs with `current_dir` set
/// to that directory), absolute otherwise.
fn displayed(dir: &Path, path: &Path) -> String {
    path.strip_prefix(dir)
        .map(|rel| rel.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

/// Run `camel test` with the given arguments in `dir` and return
/// (stdout, stderr, exit code).
fn run_camel(dir: &Path, args: &[&str]) -> (String, String, Option<i32>) {
    let output = Command::new(env!("CARGO_BIN_EXE_camel"))
        .arg("test")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spawn camel test"); // allow-unwrap
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code(),
    )
}

/// `--unit` over a directory holding one lean and one full (scenario)
/// document runs only the lean document; the full document produces no
/// stdout lines and no error, and the exit code is 0.
#[test]
fn unit_filter_excludes_full_silently() {
    let dir = temp_dir("unit-excludes");
    let lean = write_lean(&dir, "lean.test.yaml");
    write_fake_scenario(&dir, "full.test.yaml");

    let (stdout, stderr, code) = run_camel(&dir, &["--unit", "."]);
    let lean_line = format!("{} [lean]", displayed(&dir, &lean));
    assert!(stdout.contains(&lean_line), "stdout: {stdout}");
    assert!(
        !stdout.contains("full.test.yaml"),
        "full doc must be silent: {stdout}"
    );
    assert!(stdout.contains("1 passed, 0 failed"), "stdout: {stdout}");
    assert_eq!(code, Some(0), "stderr: {stderr}");
}

/// A full document named explicitly under `--unit` fails with the
/// `tier-filter-collision` class and exit code 2.
#[test]
fn explicit_full_collides_under_unit() {
    let dir = temp_dir("explicit-collides");
    let full = write_fake_scenario(&dir, "full.test.yaml");

    let (stdout, stderr, code) = run_camel(&dir, &["--unit", "full.test.yaml"]);
    assert!(
        stderr.contains("tier-filter-collision"),
        "stderr must carry the class: {stderr}"
    );
    assert!(
        stderr.contains(&displayed(&dir, &full)),
        "stderr must name the document: {stderr}"
    );
    assert!(!stdout.contains("PASS"), "no row may run: {stdout}");
    assert_eq!(code, Some(2));
}

/// `--unit --integration` together is misuse: exit 2 with the error
/// printed before any document is read (no stdout at all).
#[test]
fn both_flags_misuse() {
    let dir = temp_dir("both-flags");
    write_lean(&dir, "lean.test.yaml");

    let (stdout, stderr, code) = run_camel(&dir, &["--unit", "--integration", "."]);
    assert_eq!(code, Some(2), "stderr: {stderr}");
    assert!(!stderr.is_empty(), "the misuse must be reported");
    assert!(
        stdout.is_empty(),
        "no document may be read or summarized: {stdout}"
    );
}

/// An unfiltered run of one lean document annotates its stdout line
/// with `[lean]`.
#[test]
fn tier_annotation_in_output() {
    let dir = temp_dir("annotation");
    let lean = write_lean(&dir, "lean.test.yaml");

    let (stdout, _, code) = run_camel(&dir, &["lean.test.yaml"]);
    assert!(
        stdout.contains(&format!("{} [lean]", displayed(&dir, &lean))),
        "stdout line must carry [lean]: {stdout}"
    );
    assert_eq!(code, Some(0));
}

/// Tier and file filters compose AND: `--unit --filter-file './sub/**'`
/// over a directory with lean docs at the root and in `sub/` runs only
/// the lean, glob-matching document.
#[test]
fn tier_filter_composes_with_file_filter() {
    let dir = temp_dir("compose");
    let sub = dir.join("sub");
    fs::create_dir_all(&sub).expect("create sub dir"); // allow-unwrap
    let root_lean = write_lean(&dir, "root.test.yaml");
    let sub_lean = write_lean(&sub, "only.test.yaml");
    write_fake_scenario(&sub, "full.test.yaml");

    let (stdout, stderr, code) = run_camel(&dir, &[".", "--unit", "--filter-file", "./sub/**"]);
    assert!(
        stdout.contains(&format!("{} [lean]", displayed(&dir, &sub_lean))),
        "stdout: {stdout}"
    );
    assert!(
        !stdout.contains(&displayed(&dir, &root_lean)),
        "stdout: {stdout}"
    );
    assert!(!stdout.contains("full.test.yaml"), "stdout: {stdout}");
    assert_eq!(code, Some(0), "stderr: {stderr}");
}

/// With no filter flags, one lean and one FakeAdapter scenario document
/// both execute and report their derived tiers.
#[test]
fn no_filter_runs_everything_at_derived_tier() {
    let dir = temp_dir("no-filter");
    let lean = write_lean(&dir, "lean.test.yaml");
    let full = write_fake_scenario(&dir, "full.test.yaml");

    let (stdout, stderr, code) = run_camel(&dir, &["."]);
    assert!(
        stdout.contains(&format!("{} [lean]", displayed(&dir, &lean))),
        "lean tier must be reported: {stdout}"
    );
    assert!(
        stdout.contains(&format!("{} [full]", displayed(&dir, &full))),
        "full tier must be reported: {stdout}"
    );
    // The scenario's two actions each carry a verdict line; a `.`
    // directory argument displays `./`-prefixed paths.
    assert!(
        stdout.contains(&format!(
            "PASS ./{}#scenario[0] send",
            displayed(&dir, &full)
        )),
        "send action line: {stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "PASS ./{}#scenario[1] sleep",
            displayed(&dir, &full)
        )),
        "sleep action line: {stdout}"
    );
    assert!(stdout.contains("3 passed, 0 failed"), "stdout: {stdout}");
    assert_eq!(code, Some(0), "stderr: {stderr}");
}

/// A scenario `receive` whose partner never delivers times out and is a
/// verdict-class failure: the action line reports `receive-timeout` and
/// the exit code is 1.
#[test]
fn scenario_receive_timeout_exits_1() {
    let dir = temp_dir("receive-timeout");
    let full = write_timeout_scenario(&dir, "timeout.test.yaml");

    let (stdout, stderr, code) = run_camel(&dir, &["timeout.test.yaml"]);
    assert!(
        stdout.contains(&format!(
            "FAIL {}#scenario[1] receive",
            displayed(&dir, &full)
        )),
        "the failing action line must name the action: {stdout}"
    );
    assert!(
        stdout.contains("receive-timeout"),
        "the action line must report receive-timeout: {stdout}"
    );
    assert!(stdout.contains("1 passed, 1 failed"), "stdout: {stdout}");
    assert_eq!(code, Some(1), "verdict class exits 1; stderr: {stderr}");
}

/// An apparatus failure keeps precedence over a verdict failure: a
/// document with a failing expectation plus a document failing
/// `infra-unavailable` (a `grpc:` endpoint — a scheme no build
/// provisions) are both reported, and the exit code is 2.
#[test]
fn apparatus_failure_keeps_precedence() {
    let dir = temp_dir("apparatus-precedence");
    let failing = write_failing_unit(&dir, "verdict.test.yaml");
    let grpc = write_grpc_scenario(&dir, "infra.test.yaml");

    let (stdout, stderr, code) = run_camel(&dir, &["verdict.test.yaml", "infra.test.yaml"]);
    assert!(
        stdout.contains(&format!("FAIL {}#out", displayed(&dir, &failing))),
        "the verdict failure must be reported: {stdout}"
    );
    assert!(
        stderr.contains("infra-unavailable"),
        "the apparatus failure must be reported: {stderr}"
    );
    assert!(
        stderr.contains("grpc"),
        "the apparatus failure must name the adapter: {stderr}"
    );
    assert!(
        stderr.contains(&displayed(&dir, &grpc)),
        "stderr must name the document: {stderr}"
    );
    assert_eq!(code, Some(2), "apparatus class keeps precedence");
}
