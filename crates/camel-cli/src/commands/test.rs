//! `camel test <file>...` — run declarative mock tests from `*.test.yaml`
//! documents.
//!
//! Each document boots a lean `CamelContext` in-process, loads its routes,
//! delivers `direct:` inputs, settles traffic, and evaluates expectations
//! against the real mock component. Documents execute in CLI argument order,
//! sequentially; a document-level error is reported and execution continues
//! with the next document. Exit codes: 0 all pass, 1 any expectation failure
//! or settle timeout, 2 misuse/unreadable file/parse error (precedence 2 > 1
//! > 0).
//!
//! Spec: openspec/changes/mock-declarative-testkit (design D2, D7).

pub mod document;
pub mod runner;

use std::io::Write;
use std::path::PathBuf;

use clap::Args;

use document::parse_test_document;
use runner::run_test_doc;

/// CLI args for `camel test`.
#[derive(Args, Debug)]
pub struct TestArgs {
    /// Paths to `*.test.yaml` documents to run, in order.
    #[arg(value_name = "FILE", required = true)]
    pub files: Vec<PathBuf>,
}

/// Outcome of a multi-document `camel test` run.
pub struct TestRunSummary {
    /// Process exit code: 0 all pass, 1 any failure, 2 any parse error.
    pub exit_code: i32,
    /// Number of endpoints that passed.
    pub passed: usize,
    /// Number of endpoints that failed.
    pub failed: usize,
}

/// Run every test document in CLI argument order, sequentially.
///
/// A document-level error (unreadable file, parse error, boot failure) is
/// reported to `err` and execution continues with the next document. Per
/// endpoint, one `PASS`/`FAIL` line is written to `out`. Exit precedence when
/// classes mix: any parse-error class ⇒ 2, else any failed ⇒ 1, else 0.
pub async fn run_tests(
    files: &[PathBuf],
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> TestRunSummary {
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut had_parse_error = false;

    for path in files {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) => {
                had_parse_error = true;
                let _ = writeln!(err, "{}: {e}", path.display());
                continue;
            }
        };
        let doc = match parse_test_document(&text) {
            Ok(doc) => doc,
            Err(e) => {
                had_parse_error = true;
                let _ = writeln!(err, "{}: {e}", path.display());
                continue;
            }
        };
        let parent_dir = path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let result = run_test_doc(&doc, &parent_dir).await.0;

        if let Some(doc_error) = result.doc_error {
            had_parse_error = true;
            let _ = writeln!(err, "{}: {doc_error}", path.display());
            continue;
        }
        for er in result.endpoint_results {
            match er.outcome {
                Ok(()) => {
                    passed += 1;
                    let _ = writeln!(out, "PASS {}#{}", path.display(), er.endpoint);
                }
                Err(detail) => {
                    failed += 1;
                    let _ = writeln!(out, "FAIL {}#{} — {detail}", path.display(), er.endpoint);
                }
            }
        }
    }

    let _ = writeln!(out, "{passed} passed, {failed} failed");
    let exit_code = if had_parse_error {
        2
    } else if failed > 0 {
        1
    } else {
        0
    };
    TestRunSummary {
        exit_code,
        passed,
        failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    /// Create a unique temp directory for one test.
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("camel-test-cli-{tag}-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("create temp dir"); // allow-unwrap
        dir
    }

    /// Write a passing document (one `direct:` input → `mock:out`, count 1).
    fn write_passing(dir: &Path, name: &str) -> PathBuf {
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

    /// Write a failing document (expects 3 exchanges, only 1 delivered).
    fn write_failing(dir: &Path, name: &str) -> PathBuf {
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
    count: 3
"#,
        )
        .expect("write failing doc"); // allow-unwrap
        path
    }

    /// Write an invalid-YAML document.
    fn write_bad(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, "routes: [unclosed").expect("write bad doc"); // allow-unwrap
        path
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn all_pass_exits_zero() {
        let dir = temp_dir("all-pass");
        let path = write_passing(&dir, "a.test.yaml");
        let mut out = Vec::new();
        let mut err = Vec::new();
        let summary = run_tests(&[path], &mut out, &mut err).await;
        assert_eq!(summary.exit_code, 0);
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("PASS"), "out: {out}");
        assert!(out.contains("1 passed, 0 failed"), "out: {out}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn assertion_failure_exits_one() {
        let dir = temp_dir("assert-fail");
        let path = write_failing(&dir, "a.test.yaml");
        let mut out = Vec::new();
        let mut err = Vec::new();
        let summary = run_tests(&[path], &mut out, &mut err).await;
        assert_eq!(summary.exit_code, 1);
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("FAIL"), "out: {out}");
        assert!(out.contains("0 passed, 1 failed"), "out: {out}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn parse_error_continues_and_exits_two() {
        let dir = temp_dir("parse-continue");
        let a = write_passing(&dir, "a.test.yaml");
        let bad = write_bad(&dir, "bad.test.yaml");
        let mut out = Vec::new();
        let mut err = Vec::new();
        let summary = run_tests(&[a, bad.clone()], &mut out, &mut err).await;
        assert_eq!(summary.exit_code, 2);
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("PASS"), "a must be attempted: {out}");
        let err = String::from_utf8(err).unwrap();
        assert!(err.contains("bad.test.yaml"), "err: {err}");
        assert!(!err.is_empty(), "err must carry the parse error text");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn precedence_parse_beats_assertion() {
        let dir = temp_dir("precedence");
        let a = write_failing(&dir, "a.test.yaml");
        let bad = write_bad(&dir, "bad.test.yaml");
        let mut out = Vec::new();
        let mut err = Vec::new();
        let summary = run_tests(&[a, bad], &mut out, &mut err).await;
        assert_eq!(summary.exit_code, 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn multi_doc_second_failing_both_evaluated() {
        let dir = temp_dir("multi-second-fail");
        let a = write_passing(&dir, "a.test.yaml");
        let b = write_failing(&dir, "b.test.yaml");
        let mut out = Vec::new();
        let mut err = Vec::new();
        let summary = run_tests(&[a, b], &mut out, &mut err).await;
        assert_eq!(summary.exit_code, 1);
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("a.test.yaml#out"), "out: {out}");
        assert!(out.contains("b.test.yaml#out"), "out: {out}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn multi_doc_arg_order() {
        let dir = temp_dir("arg-order");
        let a = write_passing(&dir, "a.test.yaml");
        let b = write_passing(&dir, "b.test.yaml");
        let mut out = Vec::new();
        let mut err = Vec::new();
        let summary = run_tests(&[a, b], &mut out, &mut err).await;
        assert_eq!(summary.exit_code, 0);
        let out = String::from_utf8(out).unwrap();
        let ia = out.find("a.test.yaml#out").expect("a PASS line"); // allow-unwrap
        let ib = out.find("b.test.yaml#out").expect("b PASS line"); // allow-unwrap
        assert!(ia < ib, "a must precede b in out: {out}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn missing_file_exits_two() {
        let dir = temp_dir("missing");
        let path = dir.join("nope.test.yaml");
        let mut out = Vec::new();
        let mut err = Vec::new();
        let summary = run_tests(std::slice::from_ref(&path), &mut out, &mut err).await;
        assert_eq!(summary.exit_code, 2);
        let err = String::from_utf8(err).unwrap();
        assert!(err.contains("nope.test.yaml"), "err: {err}");
    }
}
