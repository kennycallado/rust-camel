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
