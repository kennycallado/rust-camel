//! End-to-end integration test for the `camel lint` test-document skip.
//!
//! `*.test.yaml` files are `camel test` documents, not route definitions.
//! `camel lint <demo.test.yaml>` must exit 0, print exactly one info line
//! naming the file and the reason (`camel test document`) to stdout, and
//! emit no diagnostics to stderr. The skip uses
//! `camel_dsl::discovery::is_test_document`, the same predicate route
//! discovery applies (task 1.3 of `test-placement-contract`).

use std::process::Command;

/// Lint a `demo.test.yaml` in a tempdir: exit 0, one stdout info line, clean
/// stderr.
#[test]
fn cli_lint_test_doc_prints_single_info_line() {
    let dir = tempfile::tempdir().expect("tempdir");
    let doc = dir.path().join("demo.test.yaml");
    std::fs::write(&doc, "routeFiles: [x]\nexpects: {}\n").expect("write demo.test.yaml");

    let output = Command::new(env!("CARGO_BIN_EXE_camel"))
        .args(["lint", doc.to_str().expect("path is valid utf-8")])
        .output()
        .expect("failed to spawn `camel lint`");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "linting a test document must exit 0; stdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "stdout must be exactly one info line; got:\n{stdout}"
    );
    assert!(
        lines[0].contains("demo.test.yaml"),
        "info line must name the file; got: {}",
        lines[0]
    );
    assert!(
        lines[0].contains("camel test document"),
        "info line must state the skip reason; got: {}",
        lines[0]
    );

    assert!(
        stderr.is_empty(),
        "skip path must emit no diagnostics to stderr; got:\n{stderr}"
    );
}

/// Lint a route whose only finding is an inline `mock:` send: the
/// R-MOCK-IN-PRODUCTION warning is printed but must NOT affect the exit code
/// (Warning severity → exit 0). Pins the Warning/exit contract through the
/// binary.
#[test]
fn mock_rule_warning_does_not_affect_exit_code() {
    let dir = tempfile::tempdir().expect("tempdir");
    let route = dir.path().join("route.yaml");
    std::fs::write(
        &route,
        "id: r1\nfrom: direct:start\nsteps:\n  - to: mock:out\n",
    )
    .expect("write route.yaml");

    let output = Command::new(env!("CARGO_BIN_EXE_camel"))
        .args(["lint", route.to_str().expect("path is valid utf-8")])
        .output()
        .expect("failed to spawn `camel lint`");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "a Warning-only finding must exit 0; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("R-MOCK-IN-PRODUCTION"),
        "the R-MOCK-IN-PRODUCTION warning must be printed; got:\n{stderr}"
    );
}

/// Lint a fixture under a `tests/fixtures/`-shaped path that contains BOTH an
/// inline `mock:` send AND a real unknown-option error. The Stage C
/// fixture-path exemption must suppress ONLY the R-MOCK-IN-PRODUCTION
/// warning; the unknown-option Error must still be emitted and drive exit 1.
#[test]
fn fixture_path_suppresses_only_mock_rule() {
    let dir = tempfile::tempdir().expect("tempdir");
    let fixture_dir = dir.path().join("tests").join("fixtures");
    std::fs::create_dir_all(&fixture_dir).expect("create tests/fixtures");
    let route = fixture_dir.join("route.yaml");
    std::fs::write(
        &route,
        "id: r1\nfrom: direct:start\nsteps:\n  - to: mock:result\n  - to: \"log:bar?showCorrelationId=1\"\n",
    )
    .expect("write fixture route.yaml");

    let output = Command::new(env!("CARGO_BIN_EXE_camel"))
        .args(["lint", route.to_str().expect("path is valid utf-8")])
        .output()
        .expect("failed to spawn `camel lint`");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "an unknown-option Error must exit non-zero; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("R-MOCK-IN-PRODUCTION"),
        "fixture-path exemption must suppress R-MOCK-IN-PRODUCTION; got:\n{stderr}"
    );
    assert!(
        stderr.contains("R-URI-known:unknown-option"),
        "the unknown-option Error must still be emitted; got:\n{stderr}"
    );
}

/// Lint a `*.test.yaml` document whose `intercepts:` block targets `mock:`
/// endpoints. The document is skipped as a camel test document (info line,
/// exit 0) and must emit no R-MOCK-IN-PRODUCTION anywhere. This is the delta
/// scenario owner for "test documents are skipped" with mock intercepts.
#[test]
fn test_doc_with_mock_intercepts_skipped_no_rmock() {
    let dir = tempfile::tempdir().expect("tempdir");
    let doc = dir.path().join("demo.test.yaml");
    std::fs::write(
        &doc,
        "routeFiles: [x]\nintercepts: {kafka:x: {skipTo: mock:y}}\nexpects: {}\n",
    )
    .expect("write demo.test.yaml");

    let output = Command::new(env!("CARGO_BIN_EXE_camel"))
        .args(["lint", doc.to_str().expect("path is valid utf-8")])
        .output()
        .expect("failed to spawn `camel lint`");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "linting a test document must exit 0; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("camel test document"),
        "info line must state the skip reason; got:\n{stdout}"
    );
    assert!(
        !stderr.contains("R-MOCK-IN-PRODUCTION"),
        "a skipped test document must emit no R-MOCK-IN-PRODUCTION; got:\n{stderr}"
    );
    assert!(
        !stdout.contains("R-MOCK-IN-PRODUCTION"),
        "a skipped test document must print no R-MOCK-IN-PRODUCTION; got:\n{stdout}"
    );
}
