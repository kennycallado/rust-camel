//! `camel test <FILE|DIR>...` — run declarative mock tests from `*.test.yaml`
//! documents.
//!
//! Each document boots a lean `CamelContext` in-process, loads its routes,
//! delivers `direct:` inputs (capturing replies for `expectReply`
//! assertions), settles traffic, and evaluates expectations against the
//! real mock component. Reply assertion rows flow through the same
//! per-endpoint `PASS`/`FAIL` path. Documents execute in CLI argument
//! order, sequentially; a document-level error is reported and execution
//! continues with the next document. Exit codes: 0 all pass, 1 any
//! expectation failure or settle timeout, 2 misuse/unreadable file/parse
//! error (precedence 2 > 1 > 0).
//!
//! Spec: openspec/changes/mock-declarative-testkit (design D2, D7).

mod beans;
pub mod document;
pub mod runner;

#[cfg(test)]
mod document_tests;

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use camel_dsl::discovery::is_test_document;
use clap::Args;

use document::parse_test_document;
use runner::run_test_doc;

/// CLI args for `camel test`.
#[derive(Args, Debug)]
pub struct TestArgs {
    /// Paths to `*.test.yaml` documents or directories to expand, in order.
    #[arg(value_name = "FILE|DIR", required = true)]
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

/// Directory names skipped during expansion, at any depth.
const EXCLUDED_DIR_NAMES: [&str; 3] = ["target", ".git", "node_modules"];

/// Expand CLI path arguments into test documents and error strings.
///
/// File arguments pass through verbatim. Directory arguments expand to the
/// test documents found recursively, skipping `target`, `.git`, and
/// `node_modules` at any depth. Within one directory argument the documents
/// are byte-sorted; across arguments, CLI order is preserved. Duplicates
/// collapse to the first occurrence via `canonicalize` (raw-path fallback
/// when canonicalization fails). A directory with no test documents yields
/// an error naming it. Symlinked directories are not followed during the walk
/// (cycle safety); non-directory entries whose name matches the test suffix are
/// collected regardless of file type.
fn expand_test_paths(args: &[PathBuf]) -> (Vec<PathBuf>, Vec<String>) {
    let mut documents = Vec::new();
    let mut errors = Vec::new();
    let mut seen = HashSet::new();

    for arg in args {
        if arg.is_dir() {
            let mut found = Vec::new();
            collect_test_documents(arg, &mut found, &mut errors);
            found.sort_by(|a, b| a.as_os_str().cmp(b.as_os_str()));
            if found.is_empty() {
                errors.push(format!("{}: no test documents found", arg.display()));
            }
            for path in found {
                push_unique(path, &mut documents, &mut seen);
            }
        } else {
            push_unique(arg.clone(), &mut documents, &mut seen);
        }
    }
    (documents, errors)
}

/// Recursively collect test documents under `dir` into `found`.
///
/// Directory entries named `target`, `.git`, or `node_modules` are skipped.
/// Symlinked directories are not followed (cycle safety); non-directory entries
/// whose name matches the test suffix are collected regardless of file type.
/// Unreadable entries push an error string naming the path.
fn collect_test_documents(dir: &Path, found: &mut Vec<PathBuf>, errors: &mut Vec<String>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            errors.push(format!("{}: {e}", dir.display()));
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                errors.push(format!("{}: {e}", dir.display()));
                continue;
            }
        };
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(e) => {
                errors.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        if file_type.is_dir() {
            let name = entry.file_name();
            if !EXCLUDED_DIR_NAMES.iter().any(|excluded| name == *excluded) {
                collect_test_documents(&path, found, errors);
            }
        } else if is_test_document(&path) {
            found.push(path);
        }
    }
}

/// Push `path` unless a canonicalized duplicate was seen before.
///
/// `canonicalize` failure (e.g. a nonexistent file argument) falls back to
/// the raw path for dedup; it is not an error here — the runner's read step
/// owns nonexistent-file errors.
fn push_unique(path: PathBuf, documents: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>) {
    let key = match std::fs::canonicalize(&path) {
        Ok(canonical) => canonical,
        Err(_) => path.clone(),
    };
    if seen.insert(key) {
        documents.push(path);
    }
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

    let (documents, expansion_errors) = expand_test_paths(files);
    for message in &expansion_errors {
        had_parse_error = true;
        let _ = writeln!(err, "{message}");
    }

    for path in &documents {
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
        if let Some(stubs) = doc.repository_stubs() {
            let mut pairs: Vec<String> = Vec::new();
            if let Some(cache) = &stubs.cache {
                for name in cache.keys() {
                    pairs.push(format!("cache={name}"));
                }
            }
            if let Some(idempotent) = &stubs.idempotent {
                for name in idempotent.keys() {
                    pairs.push(format!("idempotent={name}"));
                }
            }
            if let Some(claim_check) = &stubs.claim_check {
                for name in claim_check.keys() {
                    pairs.push(format!("claimCheck={name}"));
                }
            }
            if !pairs.is_empty() {
                let _ = writeln!(
                    err,
                    "R-REPOSITORY-STUB: {} stubbed as memory; backend semantics not exercised (cache: prefix purge, TTL/stale timing, disk offload, stats; idempotent/claim-check: persistence; all: backend failure) — cover them in the integration tier",
                    pairs.join(" ")
                );
            }
        }
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

    let exit_code = if had_parse_error {
        2
    } else if failed > 0 {
        1
    } else {
        0
    };
    let summary = TestRunSummary {
        exit_code,
        passed,
        failed,
    };
    let _ = writeln!(out, "{} passed, {} failed", summary.passed, summary.failed);
    summary
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
        assert_eq!(summary.passed, 1, "summary must count the passing endpoint");
        assert_eq!(summary.failed, 0, "summary must count zero failures");
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
        assert_eq!(summary.passed, 0, "summary must count zero passes");
        assert_eq!(summary.failed, 1, "summary must count the failing endpoint");
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
        assert_eq!(summary.passed, 1, "one passing endpoint across both docs");
        assert_eq!(summary.failed, 1, "one failing endpoint across both docs");
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("a.test.yaml#out"), "out: {out}");
        assert!(out.contains("b.test.yaml#out"), "out: {out}");
        assert!(out.contains("1 passed, 1 failed"), "out: {out}");
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

    #[test]
    fn dir_expansion_recursive_sorted() {
        let dir = tempfile::tempdir().expect("create tempdir"); // allow-unwrap
        let root = dir.path();
        fs::write(root.join("b.test.yaml"), "").expect("write b"); // allow-unwrap
        fs::write(root.join("a.test.yaml"), "").expect("write a"); // allow-unwrap
        fs::create_dir_all(root.join("sub")).expect("create sub"); // allow-unwrap
        fs::write(root.join("sub/c.test.yml"), "").expect("write c"); // allow-unwrap
        let (docs, errors) = expand_test_paths(&[root.to_path_buf()]);
        assert!(errors.is_empty(), "errors: {errors:?}");
        let expected = [
            root.join("a.test.yaml"),
            root.join("b.test.yaml"),
            root.join("sub/c.test.yml"),
        ];
        assert_eq!(
            docs, expected,
            "documents must be byte-sorted within the directory"
        );
    }

    #[test]
    fn dir_expansion_skips_excluded_dirs() {
        let dir = tempfile::tempdir().expect("create tempdir"); // allow-unwrap
        let root = dir.path();
        fs::write(root.join("ok.test.yaml"), "").expect("write ok"); // allow-unwrap
        fs::create_dir_all(root.join("target")).expect("create target"); // allow-unwrap
        fs::write(root.join("target/gen.test.yaml"), "").expect("write gen"); // allow-unwrap
        let (docs, errors) = expand_test_paths(&[root.to_path_buf()]);
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert_eq!(
            docs,
            vec![root.join("ok.test.yaml")],
            "target must be skipped"
        );
    }

    #[test]
    fn dir_expansion_empty_dir_is_error() {
        let dir = tempfile::tempdir().expect("create tempdir"); // allow-unwrap
        let root = dir.path();
        fs::write(root.join(".keep"), "").expect("write keep"); // allow-unwrap
        let (docs, errors) = expand_test_paths(&[root.to_path_buf()]);
        assert!(docs.is_empty(), "docs: {docs:?}");
        assert_eq!(errors.len(), 1, "errors: {errors:?}");
        assert!(
            errors[0].contains(&root.display().to_string()),
            "error must name the directory: {errors:?}"
        );
    }

    #[test]
    fn dir_expansion_dedupes_first_occurrence() {
        let dir = tempfile::tempdir().expect("create tempdir"); // allow-unwrap
        let root = dir.path();
        let a = root.join("a.test.yaml");
        fs::write(&a, "").expect("write a"); // allow-unwrap
        let (docs, errors) = expand_test_paths(&[root.to_path_buf(), a.clone()]);
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert_eq!(
            docs,
            vec![a],
            "duplicate must collapse to the first occurrence"
        );
    }

    #[test]
    fn dir_expansion_file_args_verbatim() {
        let args = vec![PathBuf::from("foo.yaml")];
        let (docs, errors) = expand_test_paths(&args);
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert_eq!(docs, args, "file args pass through unchanged");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mixed_args_dir_file_empty_order_and_exit_two() {
        let dir_a = tempfile::tempdir().expect("create dir_a"); // allow-unwrap
        let empty_dir = tempfile::tempdir().expect("create empty_dir"); // allow-unwrap
        let file_x_dir = tempfile::tempdir().expect("create file_x dir"); // allow-unwrap
        // dir_a contains one passing document
        write_passing(dir_a.path(), "a.test.yaml");
        // file_x is a standalone passing document
        let file_x = write_passing(file_x_dir.path(), "standalone.test.yaml");
        let mut out = Vec::new();
        let mut err = Vec::new();
        let summary = run_tests(
            &[
                dir_a.path().to_path_buf(),
                file_x.clone(),
                empty_dir.path().to_path_buf(),
            ],
            &mut out,
            &mut err,
        )
        .await;
        assert_eq!(summary.exit_code, 2, "empty dir must force exit 2");
        let out = String::from_utf8(out).unwrap(); // allow-unwrap
        assert!(
            out.contains("standalone.test.yaml#out"),
            "file_x must still run despite expansion error: {out}"
        );
        let ia = out.find("a.test.yaml#out").expect("dir_a PASS line"); // allow-unwrap
        let ib = out
            .find("standalone.test.yaml#out")
            .expect("file_x PASS line"); // allow-unwrap
        assert!(
            ia < ib,
            "dir_a must precede file_x across mixed args: {out}"
        );
    }
}
