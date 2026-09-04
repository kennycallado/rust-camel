//! End-to-end tests for the `camel test` CLI flags: `--junit`,
//! `--filter-file`, and `--filter-endpoint`. These spawn the real `camel`
//! binary (bin name pinned by `camel-cli`'s `[[bin]]`), so they cover the
//! `main.rs` dispatch through `run_tests_full` — the unit tests in
//! `commands::test` cover the driver in-process.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Create a unique temp directory for one test.
fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("camel-test-flags-{tag}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("create temp dir"); // allow-unwrap
    dir
}

/// Write a passing document (one `direct:` input → `mock:out`, count 1).
fn write_passing(dir: &std::path::Path, name: &str) -> PathBuf {
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
    .expect("write passing doc"); // allow-unwrap
    path
}

/// Write a failing document expecting `count` exchanges (only 1 delivered).
fn write_failing_count(dir: &std::path::Path, name: &str, count: usize) -> PathBuf {
    let path = dir.join(name);
    fs::write(
        &path,
        format!(
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
    count: {count}
"#
        ),
    )
    .expect("write failing doc"); // allow-unwrap
    path
}

/// Write an invalid-YAML document.
fn write_bad(dir: &std::path::Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, "{{{ not yaml").expect("write bad doc"); // allow-unwrap
    path
}

/// Write a passing document asserting the `orders` endpoint (bare key
/// `orders` after `mock:` normalization).
fn write_orders(dir: &std::path::Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(
        &path,
        r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "mock:orders"
inputs:
  - to: "direct:start"
    body: "x"
expects:
  mock:orders:
    count: 1
"#,
    )
    .expect("write orders doc"); // allow-unwrap
    path
}

/// Build the T2 3-doc corpus (pass + fail + unparsable) under `dir`.
fn write_t2_corpus(dir: &std::path::Path) -> (PathBuf, PathBuf, PathBuf) {
    let a = write_passing(dir, "a.test.yaml");
    let b = write_failing_count(dir, "b.test.yaml", 2);
    let bad = write_bad(dir, "bad.test.yaml");
    (a, b, bad)
}

/// The no-flag dispatch must be byte-identical to the in-process driver:
/// the pinned T2 stdout/stderr strings (absolute displayed paths) and exit
/// code 2, proving `main.rs` → `run_tests_full` preserves flag-less CLI
/// behavior end-to-end.
#[test]
fn no_flag_dispatch_byte_identical_e2e() {
    let dir = temp_dir("no-flag");
    let (a, b, bad) = write_t2_corpus(&dir);

    let output = Command::new(env!("CARGO_BIN_EXE_camel"))
        .arg("test")
        .arg(&dir)
        .output()
        .expect("spawn camel test"); // allow-unwrap
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    let expected_out = format!(
        "{a} [lean]\nPASS {a}#out\n{b} [lean]\nFAIL {b}#out — MockEndpoint 'out': expected 2 exchanges, got 1\n1 passed, 1 failed\n",
        a = a.display(),
        b = b.display()
    );
    assert_eq!(stdout, expected_out, "stdout must be byte-identical");
    let expected_err = format!(
        // noyalib 0.0.29 emits a libyaml-style parse message for flow mappings.
        "{}: invalid test document: expected ',' or '}}' in flow mapping at line 2 column 1\n",
        bad.display()
    );
    assert_eq!(stderr, expected_err, "stderr must be byte-identical");
    assert_eq!(
        output.status.code(),
        Some(2),
        "exit code must be 2 (parse error class)"
    );
}

/// An invalid `--filter-file` glob is misuse: stderr names the flag, exit
/// is 2, and the `--junit` report path is never touched.
#[test]
fn invalid_glob_e2e_writes_no_report() {
    let dir = temp_dir("invalid-glob");
    write_t2_corpus(&dir);
    let report = dir.join("r.xml");

    let output = Command::new(env!("CARGO_BIN_EXE_camel"))
        .arg("test")
        .arg(&dir)
        .arg("--junit")
        .arg(&report)
        .arg("--filter-file")
        .arg("[")
        .output()
        .expect("spawn camel test"); // allow-unwrap
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid --filter-file pattern"),
        "stderr must name the flag: {stderr}"
    );
    assert_eq!(output.status.code(), Some(2), "invalid glob must exit 2");
    assert!(
        !report.exists(),
        "no report may be written on invalid-filter misuse"
    );
}

/// The CLI surface advertises the three flags.
#[test]
fn dispatch_help_lists_flags() {
    let output = Command::new(env!("CARGO_BIN_EXE_camel"))
        .args(["test", "--help"])
        .output()
        .expect("spawn camel test --help"); // allow-unwrap
    assert!(output.status.success(), "camel test --help must exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for flag in [
        "--junit",
        "--filter-file",
        "--filter-endpoint",
        "--unit",
        "--integration",
    ] {
        assert!(stdout.contains(flag), "help must list {flag}: {stdout}");
    }
}

/// A `.` directory argument displays `./`-prefixed paths; the glob must
/// account for the prefix. `./one*` admits only `one.test.yaml`.
#[test]
fn dot_arg_glob_e2e() {
    let dir = temp_dir("dot-glob");
    write_orders(&dir, "one.test.yaml");
    write_orders(&dir, "two.test.yaml");

    let output = Command::new(env!("CARGO_BIN_EXE_camel"))
        .args(["test", ".", "--filter-file", "./one*"])
        .current_dir(&dir)
        .output()
        .expect("spawn camel test"); // allow-unwrap
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(0),
        "exit 0 expected;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("one.test.yaml"),
        "one.test.yaml must run: {stdout}"
    );
    assert!(
        !stdout.contains("two.test.yaml"),
        "two.test.yaml must be filtered out: {stdout}"
    );
}
