use super::*;
use crate::commands::run::tests::EnvVarGuard;
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
    write_failing_count(dir, name, 3)
}

/// Write a failing document expecting `count` exchanges (only 1 delivered).
fn write_failing_count(dir: &Path, name: &str, count: usize) -> PathBuf {
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
fn write_bad(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, "{{{ not yaml").expect("write bad doc"); // allow-unwrap
    path
}

/// Write a passing document asserting the `orders` endpoint (bare key
/// `orders` after `mock:` normalization).
fn write_orders(dir: &Path, name: &str) -> PathBuf {
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

/// Remove paths on drop (files first, then dirs) — panic-safe cleanup
/// for tests that create corpus files under the crate CWD.
struct CleanupPaths(Vec<PathBuf>);

impl Drop for CleanupPaths {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = fs::remove_file(path);
            let _ = fs::remove_dir(path);
        }
    }
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
async fn parse_error_doc_named_in_summary() {
    let dir = temp_dir("parse-named");
    let a = write_passing(&dir, "a.test.yaml");
    let bad = write_bad(&dir, "bad.test.yaml");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[a, bad], &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 2);
    assert_eq!(summary.parse_errors, 1);
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains("1 passed, 0 failed, 1 parse-error doc (skipped)"),
        "out: {out}"
    );
    let err = String::from_utf8(err).unwrap();
    assert!(err.contains("1 parse-error doc (skipped): "), "err: {err}");
    assert!(err.contains("bad.test.yaml"), "err: {err}");
    let last = err
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .unwrap_or_default();
    assert!(
        last.contains("1 parse-error doc (skipped): "),
        "last non-empty stderr line must be the naming line: {err}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn clean_run_has_no_parse_error_segment() {
    let dir = temp_dir("clean-run");
    let a = write_passing(&dir, "a.test.yaml");
    let b = write_passing(&dir, "b.test.yaml");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[a, b], &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 0);
    assert_eq!(summary.parse_errors, 0);
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("2 passed, 0 failed"), "out: {out}");
    assert!(!out.contains("parse-error"), "out: {out}");
    let err = String::from_utf8(err).unwrap();
    assert!(!err.contains("parse-error"), "err: {err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn parse_error_only_zero_ran() {
    let dir = temp_dir("parse-only");
    let bad = write_bad(&dir, "bad.test.yaml");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[bad], &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 2);
    assert_eq!(summary.passed, 0);
    assert_eq!(summary.failed, 0);
    assert_eq!(summary.parse_errors, 1);
    let out = String::from_utf8(out).unwrap();
    assert_eq!(out, "0 passed, 0 failed, 1 parse-error doc (skipped)\n");
}

/// Two parse-error documents: the plural noun and the CLI-order naming
/// line (`b1` before `b2`).
#[tokio::test(flavor = "multi_thread")]
async fn two_parse_error_docs_plural_and_order() {
    let dir = temp_dir("parse-plural-order");
    let a = write_passing(&dir, "a.test.yaml");
    let b1 = write_bad(&dir, "b1.test.yaml");
    let b2 = write_bad(&dir, "b2.test.yaml");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[a, b1, b2], &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 2);
    assert_eq!(summary.parse_errors, 2);
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains("1 passed, 0 failed, 2 parse-error docs (skipped)"),
        "out: {out}"
    );
    let err = String::from_utf8(err).unwrap();
    let last = err
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .unwrap_or_default();
    let i1 = last.find("b1.test.yaml").expect("b1 named"); // allow-unwrap
    let i2 = last.find("b2.test.yaml").expect("b2 named"); // allow-unwrap
    assert!(i1 < i2, "b1 must precede b2 in the naming line: {err}");
}

/// A verdict failure mixed with a parse error: the summary counts both
/// classes and the parse-error segment carries the singular noun.
#[tokio::test(flavor = "multi_thread")]
async fn parse_error_with_failures_mixes_counts() {
    let dir = temp_dir("parse-fail-mix");
    let f = write_failing(&dir, "f.test.yaml");
    let b = write_bad(&dir, "b.test.yaml");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[f, b], &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 2);
    assert_eq!(summary.passed, 0);
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.parse_errors, 1);
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains("0 passed, 1 failed, 1 parse-error doc (skipped)"),
        "out: {out}"
    );
}

/// An empty directory argument is an expansion (parse-class) error: it
/// counts in `parse_errors`, the naming line names the directory, and
/// stdout is exactly the zero-ran summary with the skipped segment.
#[tokio::test(flavor = "multi_thread")]
async fn expansion_error_counts_in_summary() {
    let empty = tempfile::tempdir().expect("create empty dir"); // allow-unwrap
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[empty.path().to_path_buf()], &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 2);
    assert_eq!(summary.parse_errors, 1);
    let out = String::from_utf8(out).unwrap();
    assert_eq!(out, "0 passed, 0 failed, 1 parse-error doc (skipped)\n");
    let err = String::from_utf8(err).unwrap();
    assert!(err.contains("1 parse-error doc (skipped): "), "err: {err}");
    let name = empty.path().display().to_string();
    assert!(err.contains(name.as_str()), "err must name the dir: {err}");
}

/// An expansion error alongside a passing document: the doc still runs
/// and the summary carries the skipped segment; exit stays 2.
#[tokio::test(flavor = "multi_thread")]
async fn expansion_error_alongside_passing_doc() {
    let file_dir = tempfile::tempdir().expect("create file dir"); // allow-unwrap
    let file = write_passing(file_dir.path(), "a.test.yaml");
    let empty = tempfile::tempdir().expect("create empty dir"); // allow-unwrap
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[file, empty.path().to_path_buf()], &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 2);
    assert_eq!(summary.parse_errors, 1);
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains("1 passed, 0 failed, 1 parse-error doc (skipped)"),
        "out: {out}"
    );
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
    let (docs, _explicit, errors) = expand_test_paths(&[root.to_path_buf()]);
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
    let (docs, _explicit, errors) = expand_test_paths(&[root.to_path_buf()]);
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
    let (docs, _explicit, errors) = expand_test_paths(&[root.to_path_buf()]);
    assert!(docs.is_empty(), "docs: {docs:?}");
    assert_eq!(errors.len(), 1, "errors: {errors:?}");
    assert_eq!(
        errors[0].0, root,
        "error must carry the directory path: {errors:?}"
    );
    assert_eq!(
        errors[0].1, "no test documents found",
        "error must carry the bare message: {errors:?}"
    );
}

#[test]
fn dir_expansion_dedupes_first_occurrence() {
    let dir = tempfile::tempdir().expect("create tempdir"); // allow-unwrap
    let root = dir.path();
    let a = root.join("a.test.yaml");
    fs::write(&a, "").expect("write a"); // allow-unwrap
    let (docs, _explicit, errors) = expand_test_paths(&[root.to_path_buf(), a.clone()]);
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
    let (docs, _explicit, errors) = expand_test_paths(&args);
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

#[tokio::test(flavor = "multi_thread")]
async fn no_flags_output_is_byte_identical() {
    let dir = temp_dir("byte-identical");
    let a = write_passing(&dir, "a.test.yaml");
    let b = write_failing_count(&dir, "b.test.yaml", 2);
    let bad = write_bad(&dir, "bad.test.yaml");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(std::slice::from_ref(&dir), &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 2);
    let out = String::from_utf8(out).unwrap();
    let err = String::from_utf8(err).unwrap();
    let expected_out = format!(
        "{a} [lean]\nPASS {a}#out\n{b} [lean]\nFAIL {b}#out — MockEndpoint 'out': expected 2 exchanges, got 1\n1 passed, 1 failed, 1 parse-error doc (skipped)\n",
        a = a.display(),
        b = b.display()
    );
    assert_eq!(out, expected_out, "stdout must be byte-identical");
    let expected_err = format!(
        // noyalib 0.0.29 emits a libyaml-style parse message for flow mappings.
        "{bad}: invalid test document: expected ',' or '}}' in flow mapping at line 2 column 1\n1 parse-error doc (skipped): {bad}\n",
        bad = bad.display()
    );
    assert_eq!(err, expected_err, "stderr must be byte-identical");
}

#[tokio::test(flavor = "multi_thread")]
async fn filter_file_separator_semantics() {
    // The corpus must be reachable as plain relative paths so the
    // displayed path is exactly `a.test.yaml` / `sub/b.test.yaml`
    // (the separator-semantics scenario: `*` must not cross `/`).
    // Tests run from crates/camel-cli; create the corpus under the
    // crate CWD and remove it afterwards (Drop guard covers panics).
    let cwd = std::env::current_dir().expect("current dir"); // allow-unwrap
    let a = cwd.join("a.test.yaml");
    let sub = cwd.join("sub");
    let b = sub.join("b.test.yaml");
    fs::create_dir_all(&sub).expect("create sub"); // allow-unwrap
    write_passing(&cwd, "a.test.yaml");
    write_passing(&sub, "b.test.yaml");
    let _guard = CleanupPaths(vec![a, b, sub]);
    let mut out = Vec::new();
    let mut err = Vec::new();
    let config = TestRunConfig {
        files: vec![
            PathBuf::from("a.test.yaml"),
            PathBuf::from("sub/b.test.yaml"),
        ],
        filter_files: vec![glob::Pattern::new("*.test.yaml").expect("pattern")], // allow-unwrap
        ..Default::default()
    };
    let summary = run_tests_full(&config, &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 0);
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("a.test.yaml#out"), "out: {out}");
    assert!(
        !out.contains("sub/b.test.yaml"),
        "`*` must not cross `/`: {out}"
    );
    assert!(out.contains("1 passed, 0 failed"), "out: {out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn filter_file_applies_before_reading() {
    let dir = temp_dir("filter-before-read");
    let bad = write_bad(&dir, "bad.test.yaml");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let config = TestRunConfig {
        files: vec![bad],
        filter_files: vec![glob::Pattern::new("other*").expect("pattern")], // allow-unwrap
        ..Default::default()
    };
    let summary = run_tests_full(&config, &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 2);
    let err = String::from_utf8(err).unwrap();
    assert_eq!(
        err, "no test documents matched --filter-file other*\n",
        "stderr must hold ONLY the zero-survivors misuse error"
    );
    let out = String::from_utf8(out).unwrap();
    assert!(out.ends_with("0 passed, 0 failed\n"), "out: {out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn filter_endpoint_selects_expects_keys() {
    let dir = temp_dir("filter-endpoint");
    let orders = write_orders(&dir, "orders.test.yaml");
    let other = write_passing(&dir, "other.test.yaml");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let config = TestRunConfig {
        files: vec![orders, other],
        filter_endpoints: vec!["orders".to_string()],
        ..Default::default()
    };
    let summary = run_tests_full(&config, &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 0);
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("orders.test.yaml#orders"), "out: {out}");
    assert!(
        !out.contains("other.test.yaml"),
        "filtered-out document must be silent: {out}"
    );
    assert!(out.contains("1 passed, 0 failed"), "out: {out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn filters_compose_and() {
    // `./`-prefix semantics: a glob anchored at `./sub/` must match
    // the exact displayed-path string a `.` directory argument would
    // produce. Asserted purely (no fs) against that string.
    let options = glob::MatchOptions {
        require_literal_separator: true,
        ..glob::MatchOptions::new()
    };
    assert!(
        glob::Pattern::new("./sub/**")
            .expect("pattern") // allow-unwrap
            .matches_with("./sub/one.test.yaml", options),
        "`./sub/**` must match the `./`-prefixed displayed path"
    );
    // e2e half: a directory arg yields absolute paths, so the file
    // filter uses `**/one.test.yaml` (which crosses `/`) and the
    // endpoint filter admits only documents declaring `orders`.
    let dir = temp_dir("filters-compose");
    fs::create_dir_all(dir.join("sub")).expect("create sub"); // allow-unwrap
    write_orders(&dir.join("sub"), "one.test.yaml");
    write_orders(&dir, "two.test.yaml");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let config = TestRunConfig {
        files: vec![dir],
        filter_files: vec![glob::Pattern::new("**/one.test.yaml").expect("pattern")], // allow-unwrap
        filter_endpoints: vec!["orders".to_string()],
        ..Default::default()
    };
    let summary = run_tests_full(&config, &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 0);
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("one.test.yaml#orders"), "out: {out}");
    assert!(
        !out.contains("two.test.yaml"),
        "only the overlap document may run: {out}"
    );
    assert!(out.contains("1 passed, 0 failed"), "out: {out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn zero_survivors_is_misuse() {
    let dir = temp_dir("zero-survivors");
    let doc = write_passing(&dir, "a.test.yaml");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let config = TestRunConfig {
        files: vec![doc],
        filter_endpoints: vec!["nosuch".to_string()],
        ..Default::default()
    };
    let summary = run_tests_full(&config, &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 2);
    let err = String::from_utf8(err).unwrap();
    assert!(
        err.contains("--filter-endpoint nosuch"),
        "misuse error must name the filter: {err}"
    );
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("0 passed, 0 failed"), "out: {out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn parse_error_survives_endpoint_filter() {
    let dir = temp_dir("parse-survives-filter");
    let bad = write_bad(&dir, "bad.test.yaml");
    let ok = write_orders(&dir, "ok.test.yaml");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let config = TestRunConfig {
        files: vec![bad, ok],
        filter_endpoints: vec!["orders".to_string()],
        ..Default::default()
    };
    let summary = run_tests_full(&config, &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 2);
    let err = String::from_utf8(err).unwrap();
    assert!(
        err.contains("bad.test.yaml"),
        "parse error must surface under the endpoint filter: {err}"
    );
    assert!(
        !err.contains("no test documents matched"),
        "a parse-error survivor must suppress the zero-survivor error: {err}"
    );
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("ok.test.yaml#orders"), "out: {out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn junit_all_pass_report() {
    let dir = temp_dir("junit-all-pass");
    let path = write_passing(&dir, "a.test.yaml");
    let report = dir.join("r.xml");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let config = TestRunConfig {
        files: vec![path],
        junit: Some(report.clone()),
        ..Default::default()
    };
    let summary = run_tests_full(&config, &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 0);
    let bytes = fs::read(&report).expect("report must exist"); // allow-unwrap
    let text = String::from_utf8(bytes).unwrap();
    assert!(
        text.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"),
        "report must open with the XML declaration: {text}"
    );
    assert!(
        text.contains("tests=\"1\" failures=\"0\" errors=\"0\""),
        "root totals must count the single passing testcase: {text}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn junit_failure_detail() {
    let dir = temp_dir("junit-failure");
    let path = write_failing(&dir, "a.test.yaml");
    let report = dir.join("r.xml");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let config = TestRunConfig {
        files: vec![path],
        junit: Some(report.clone()),
        ..Default::default()
    };
    let summary = run_tests_full(&config, &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 1);
    let text = fs::read_to_string(&report).expect("report must exist"); // allow-unwrap
    assert!(
        text.contains("<failure message=\"MockEndpoint &apos;out&apos;: expected 3 exchanges, got 1\">MockEndpoint &apos;out&apos;: expected 3 exchanges, got 1</failure>"),
        "failure element must carry first-line message and full detail body: {text}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn junit_document_error_and_expansion() {
    // run 1: passing doc + unparsable doc via file args
    let dir = temp_dir("junit-doc-err");
    let ok = write_passing(&dir, "ok.test.yaml");
    let bad = write_bad(&dir, "bad.test.yaml");
    let report1 = dir.join("r1.xml");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let config = TestRunConfig {
        files: vec![ok, bad],
        junit: Some(report1.clone()),
        ..Default::default()
    };
    let summary = run_tests_full(&config, &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 2);
    let text = fs::read_to_string(&report1).expect("report must exist"); // allow-unwrap
    assert!(
        text.contains("tests=\"1\" failures=\"0\" errors=\"0\""),
        "passing suite must hold its row: {text}"
    );
    assert!(text.contains("errors=\"1\""), "doc-error suite: {text}");
    assert!(
        text.contains("<testcase name=\"&lt;document&gt;\""),
        "doc-error testcase: {text}"
    );
    assert!(text.contains("<error"), "doc-error element: {text}");
    assert!(
        text.contains("<testsuites tests=\"2\" failures=\"0\" errors=\"1\">"),
        "root totals must count the passing row plus the doc error: {text}"
    );

    // run 2: empty directory arg → expansion error
    let empty = tempfile::tempdir().expect("create empty dir"); // allow-unwrap
    let report2 = dir.join("r2.xml");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let config = TestRunConfig {
        files: vec![empty.path().to_path_buf()],
        junit: Some(report2.clone()),
        ..Default::default()
    };
    let summary = run_tests_full(&config, &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 2);
    let text = fs::read_to_string(&report2).expect("report must exist"); // allow-unwrap
    assert_eq!(
        text.matches("<testsuite ").count(),
        1,
        "exactly one synthetic suite: {text}"
    );
    assert!(
        text.contains(&format!("<testsuite name=\"{}\"", empty.path().display())),
        "synthetic suite must be named by the directory's displayed path: {text}"
    );
    assert!(
        text.contains("<testcase name=\"&lt;expansion&gt;\""),
        "expansion testcase: {text}"
    );
    assert!(text.contains("<error"), "expansion error element: {text}");
    assert!(
        text.contains("<testsuites tests=\"1\" failures=\"0\" errors=\"1\">"),
        "root totals must count the expansion error: {text}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn junit_filtered_documents_have_no_rows() {
    let dir = temp_dir("junit-filtered");
    let orders = write_orders(&dir, "orders.test.yaml");
    let other = write_passing(&dir, "other.test.yaml");
    let report = dir.join("r.xml");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let config = TestRunConfig {
        files: vec![orders, other],
        filter_endpoints: vec!["orders".to_string()],
        junit: Some(report.clone()),
        ..Default::default()
    };
    let summary = run_tests_full(&config, &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 0);
    let text = fs::read_to_string(&report).expect("report must exist"); // allow-unwrap
    assert_eq!(
        text.matches("<testsuite ").count(),
        1,
        "exactly one suite (the survivor's): {text}"
    );
    assert!(text.contains("orders.test.yaml"), "survivor suite: {text}");
    assert!(
        !text.contains("other.test.yaml"),
        "filtered-out document must produce no rows: {text}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn junit_zero_survivors_writes_empty_report() {
    let dir = temp_dir("junit-zero-survivors");
    let doc = write_passing(&dir, "a.test.yaml");
    let report = dir.join("r.xml");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let config = TestRunConfig {
        files: vec![doc],
        filter_endpoints: vec!["nosuch".to_string()],
        junit: Some(report.clone()),
        ..Default::default()
    };
    let summary = run_tests_full(&config, &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 2, "zero-survivor misuse must exit 2");
    let bytes = fs::read(&report).expect("report must exist"); // allow-unwrap
    let expected = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites tests=\"0\" failures=\"0\" errors=\"0\">\n</testsuites>\n";
    assert_eq!(
        String::from_utf8(bytes).unwrap(),
        expected,
        "empty report must be the pinned bytes (open+close root, no suites)"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn junit_write_failure_forces_exit_2() {
    let dir = temp_dir("junit-write-fail");
    let path = write_passing(&dir, "a.test.yaml");
    let report = dir.join("missing").join("r.xml");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let config = TestRunConfig {
        files: vec![path],
        junit: Some(report.clone()),
        ..Default::default()
    };
    let summary = run_tests_full(&config, &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 2, "write failure must override exit 0");
    let err = String::from_utf8(err).unwrap();
    assert!(err.contains("failed to write"), "err: {err}");
    assert!(err.contains("r.xml"), "err must name the path: {err}");
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("PASS"), "stdout must still hold PASS: {out}");
    assert!(
        out.contains("1 passed, 0 failed"),
        "summary must print before the write failure: {out}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn junit_absent_writes_nothing() {
    // The T2 byte-identity corpus (pass + fail + doc error), run twice:
    // plain `run_tests` and `run_tests_full` with `junit: None`.
    let dir = temp_dir("junit-absent");
    let a = write_passing(&dir, "a.test.yaml");
    let b = write_failing_count(&dir, "b.test.yaml", 2);
    let bad = write_bad(&dir, "bad.test.yaml");
    let would_be = dir.join("r.xml");

    let mut out1 = Vec::new();
    let mut err1 = Vec::new();
    let summary1 = run_tests(std::slice::from_ref(&dir), &mut out1, &mut err1).await;
    assert_eq!(summary1.exit_code, 2);
    assert!(
        !would_be.exists(),
        "plain run_tests must not write a report"
    );

    let mut out2 = Vec::new();
    let mut err2 = Vec::new();
    let config = TestRunConfig {
        files: vec![dir],
        junit: None,
        ..Default::default()
    };
    let summary2 = run_tests_full(&config, &mut out2, &mut err2).await;
    assert_eq!(summary2.exit_code, 2);
    assert!(!would_be.exists(), "junit None must not write a report");

    let expected_out = format!(
        "{a} [lean]\nPASS {a}#out\n{b} [lean]\nFAIL {b}#out — MockEndpoint 'out': expected 2 exchanges, got 1\n1 passed, 1 failed, 1 parse-error doc (skipped)\n",
        a = a.display(),
        b = b.display()
    );
    let expected_err = format!(
        // noyalib 0.0.29 emits a libyaml-style parse message for flow mappings.
        "{bad}: invalid test document: expected ',' or '}}' in flow mapping at line 2 column 1\n1 parse-error doc (skipped): {bad}\n",
        bad = bad.display()
    );
    assert_eq!(
        String::from_utf8(out1).unwrap(),
        expected_out,
        "run_tests stdout must match the pinned T2 string"
    );
    assert_eq!(
        String::from_utf8(err1).unwrap(),
        expected_err,
        "run_tests stderr must match the pinned T2 string"
    );
    assert_eq!(
        String::from_utf8(out2).unwrap(),
        expected_out,
        "run_tests_full stdout must match the pinned T2 string"
    );
    assert_eq!(
        String::from_utf8(err2).unwrap(),
        expected_err,
        "run_tests_full stderr must match the pinned T2 string"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn junit_escapes_settle_label() {
    let dir = temp_dir("junit-escapes");
    let path = dir.join("a.test.yaml");
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
    bodies:
      - equals: "<a&b>"
"#,
    )
    .expect("write escape doc"); // allow-unwrap
    let report = dir.join("r.xml");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let config = TestRunConfig {
        files: vec![path],
        junit: Some(report.clone()),
        ..Default::default()
    };
    let summary = run_tests_full(&config, &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 1);
    let text = fs::read_to_string(&report).expect("report must exist"); // allow-unwrap
    assert!(
        text.contains("&lt;a&amp;b&gt;"),
        "expected body must be escaped in the failure detail: {text}"
    );
    assert!(text.contains("&lt;"), "escaped angle bracket: {text}");
    assert!(text.contains("&amp;"), "escaped ampersand: {text}");
    assert!(
        !text.contains("<a&b>"),
        "raw unescaped sequence must not appear: {text}"
    );
}

#[test]
fn invalid_glob_config_is_misuse() {
    let args = TestArgs {
        files: vec![PathBuf::from(".")],
        junit: None,
        filter_files: vec!["[".to_string()],
        filter_endpoints: vec![],
        unit: false,
        integration: false,
    };
    let err = config_from_args(&args).expect_err("invalid glob must fail"); // allow-unwrap
    assert!(
        err.contains("invalid --filter-file pattern"),
        "error must name the flag: {err}"
    );
    assert!(err.contains("["), "error must echo the pattern: {err}");
}

#[test]
fn valid_flags_build_config() {
    let args = TestArgs {
        files: vec![PathBuf::from(".")],
        junit: Some(PathBuf::from("r.xml")),
        filter_files: vec!["*.test.yaml".to_string()],
        filter_endpoints: vec!["orders".to_string()],
        unit: false,
        integration: false,
    };
    let config = config_from_args(&args).expect("valid flags must build"); // allow-unwrap
    assert_eq!(config.files, args.files, "files must pass through");
    assert_eq!(config.junit, Some(PathBuf::from("r.xml")));
    assert_eq!(config.filter_files.len(), 1, "one compiled pattern");
    assert_eq!(
        config.filter_files[0].as_str(),
        "*.test.yaml",
        "pattern must compile to the source glob"
    );
    assert_eq!(
        config.filter_endpoints,
        vec!["orders".to_string()],
        "endpoint names must pass through"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn filtered_stdout_matches_direct_run() {
    let dir = temp_dir("filtered-stdout-match");
    let a = write_orders(&dir, "a.test.yaml");
    let b = write_passing(&dir, "b.test.yaml");
    let mut out_filtered = Vec::new();
    let mut err_filtered = Vec::new();
    let config = TestRunConfig {
        files: vec![a.clone(), b.clone()],
        filter_endpoints: vec!["orders".to_string()],
        ..Default::default()
    };
    let summary_filtered = run_tests_full(&config, &mut out_filtered, &mut err_filtered).await;
    let mut out_direct = Vec::new();
    let mut err_direct = Vec::new();
    let summary_direct =
        run_tests(std::slice::from_ref(&a), &mut out_direct, &mut err_direct).await;
    assert_eq!(summary_filtered.exit_code, 0);
    assert_eq!(summary_direct.exit_code, 0);
    assert_eq!(
        out_filtered, out_direct,
        "survivors' stdout must be identical to running them directly"
    );
}

/// Write a document whose route forwards each input to `mock:out`
/// `arrivals` times and expects `minCount`/`maxCount` on it.
fn write_range_doc(dir: &Path, name: &str, arrivals: usize, min: usize, max: usize) -> PathBuf {
    let path = dir.join(name);
    let mut steps = String::new();
    for _ in 0..arrivals {
        steps.push_str("      - to: \"mock:out\"\n");
    }
    fs::write(
        &path,
        format!(
            r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
{steps}inputs:
  - to: "direct:start"
    body: "x"
expects:
  mock:out:
    minCount: {min}
    maxCount: {max}
"#
        ),
    )
    .expect("write range doc"); // allow-unwrap
    path
}

/// Write a document whose route sends to `mock:out` only when the filter
/// predicate passes (`sends`); the endpoint is created at route-add either
/// way, and `maxCount: 0` is an absence claim over the settled window.
fn write_absence_doc(dir: &Path, name: &str, sends: bool) -> PathBuf {
    let path = dir.join(name);
    let needle = if sends { "x" } else { "never-sent" };
    fs::write(
        &path,
        format!(
            r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - filter:
          simple: "${{body}} == '{needle}'"
          steps:
            - to: "mock:out"
inputs:
  - to: "direct:start"
    body: "x"
expects:
  mock:out:
    maxCount: 0
settle: 200ms
"#
        ),
    )
    .expect("write absence doc"); // allow-unwrap
    path
}

#[tokio::test(flavor = "multi_thread")]
async fn range_bound_passes_inside() {
    let dir = temp_dir("range-pass");
    let path = write_range_doc(&dir, "a.test.yaml", 2, 1, 2);
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[path], &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 0);
    assert_eq!(summary.passed, 1, "in-range arrivals must pass");
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("PASS"), "out: {out}");
    assert!(out.contains("a.test.yaml#out"), "out: {out}");
    assert!(out.contains("1 passed, 0 failed"), "out: {out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn range_bound_fails_above() {
    let dir = temp_dir("range-fail");
    let path = write_range_doc(&dir, "a.test.yaml", 3, 1, 2);
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[path], &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 1);
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains("MockEndpoint 'out': expected between 1 and 2 exchanges, got 3"),
        "range failure must render the shared bound text: {out}"
    );
}

/// Write the two-mock sequence document: route `direct:start` →
/// `mock:probe-a` → `mock:probe-b` (direct sends are awaited in step
/// order, so arrival order is causal and deterministic — divert-copy
/// probes deliver detached wire-tap copies and would only assert
/// happened order), plus the declared `sequence:` list body
/// (`sequence` renders the YAML list body, e.g.
/// `"mock:probe-a", "mock:probe-b"`).
fn write_seq_doc(dir: &Path, name: &str, sequence: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(
        &path,
        format!(
            r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "mock:probe-a"
      - to: "mock:probe-b"
inputs:
  - to: "direct:start"
    body: "x"
expects:
  mock:probe-a:
    count: 1
  mock:probe-b:
    count: 1
sequence: [{sequence}]
"#
        ),
    )
    .expect("write sequence doc"); // allow-unwrap
    path
}

#[tokio::test(flavor = "multi_thread")]
async fn sequence_passes_causally_ordered_sends() {
    let dir = temp_dir("seq-ordered-pass");
    let path = write_seq_doc(&dir, "a.test.yaml", "\"mock:probe-a\", \"mock:probe-b\"");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[path], &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 0, "causally-ordered sequence must pass");
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("0 failed"), "out: {out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn sequence_reversed_fails_naming_first_divergence() {
    let dir = temp_dir("seq-reversed-fail");
    let path = write_seq_doc(&dir, "a.test.yaml", "\"mock:probe-b\", \"mock:probe-a\"");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[path], &mut out, &mut err).await;
    assert_eq!(
        summary.exit_code, 1,
        "reversed sequence must fail as a verdict"
    );
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("position 0"), "out: {out}");
    assert!(out.contains("expected probe-b"), "out: {out}");
    assert!(out.contains("got probe-a"), "out: {out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn sequence_repeats_and_narrowing() {
    let dir = temp_dir("seq-repeats-noise");
    let path = dir.join("a.test.yaml");
    fs::write(
        &path,
        r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "mock:probe-a"
      - to: "mock:probe-b"
      - to: "mock:probe-a"
      - to: "mock:noise"
inputs:
  - to: "direct:start"
    body: "x"
expects:
  mock:probe-a:
    count: 2
  mock:probe-b:
    count: 1
  mock:noise:
    count: 1
sequence: ["mock:probe-a", "mock:probe-b", "mock:probe-a"]
"#,
    )
    .expect("write repeats doc"); // allow-unwrap
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[path], &mut out, &mut err).await;
    assert_eq!(
        summary.exit_code, 0,
        "repeated entries must match consecutive arrivals; noise arrivals must be ignored"
    );
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("0 failed"), "out: {out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn sequence_arrivals_run_out_fails_naming_shortage() {
    // Declared sequence lists three arrivals but the two-probe document
    // delivers only two: the third entry must fail naming the missing
    // arrival's position, expected endpoint, and the no-further-arrival
    // marker.
    let dir = temp_dir("seq-shortage");
    let path = write_seq_doc(
        &dir,
        "a.test.yaml",
        "\"mock:probe-a\", \"mock:probe-b\", \"mock:probe-a\"",
    );
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[path], &mut out, &mut err).await;
    assert_eq!(
        summary.exit_code, 1,
        "a declared arrival with no backing arrival must fail as a verdict"
    );
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("position 2"), "out: {out}");
    assert!(out.contains("expected probe-a"), "out: {out}");
    assert!(out.contains("got <no further arrival>"), "out: {out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn sequence_extra_arrival_fails_naming_surplus() {
    // The declared sequence is satisfied, but the route sends one extra
    // arrival after the last declared entry: the surplus must fail naming
    // the position, the end-of-sequence marker, and the unexpected arrival.
    // Direct mock: sends (deterministic causal order) with a third
    // probe-a send for the surplus.
    let dir = temp_dir("seq-surplus");
    let path = dir.join("a.test.yaml");
    fs::write(
        &path,
        r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "mock:probe-a"
      - to: "mock:probe-b"
      - to: "mock:probe-a"
inputs:
  - to: "direct:start"
    body: "x"
expects:
  mock:probe-a:
    count: 2
  mock:probe-b:
    count: 1
sequence: ["mock:probe-a", "mock:probe-b"]
"#,
    )
    .expect("write sequence surplus doc"); // allow-unwrap
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[path], &mut out, &mut err).await;
    assert_eq!(
        summary.exit_code, 1,
        "an arrival past the declared sequence must fail as a verdict"
    );
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("position 2"), "out: {out}");
    assert!(out.contains("expected <end of sequence>"), "out: {out}");
    assert!(out.contains("got probe-a"), "out: {out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn divert_copy_locked_end_to_end() {
    let dir = temp_dir("divert-copy-lock");
    let path = dir.join("a.test.yaml");
    fs::write(
        &path,
        r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "seda:audit"
      - to: "mock:sink"
  - id: r2
    from: "seda:audit"
    steps:
      - to: "mock:drained"
inputs:
  - to: "direct:start"
    body: "x"
intercepts:
  seda:audit:
    divertCopyTo: "mock:audit"
expects:
  mock:audit:
    count: 1
  mock:drained:
    count: 1
"#,
    )
    .expect("write divert lock doc"); // allow-unwrap
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[path], &mut out, &mut err).await;
    assert_eq!(
        summary.exit_code, 0,
        "divert copy AND real delivery must both hold"
    );
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains("a.test.yaml#audit"),
        "divert copy must be recorded: {out}"
    );
    assert!(
        out.contains("a.test.yaml#drained"),
        "real seda consumer must receive: {out}"
    );
    assert!(out.contains("2 passed, 0 failed"), "out: {out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn max_count_zero_asserts_absence() {
    // No arrival inside the window: the absence claim holds.
    let dir = temp_dir("max-zero-pass");
    let path = write_absence_doc(&dir, "pass.test.yaml", false);
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[path], &mut out, &mut err).await;
    assert_eq!(
        summary.exit_code, 0,
        "maxCount 0 with no arrivals must pass"
    );
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("PASS"), "out: {out}");
    assert!(out.contains("pass.test.yaml#out"), "out: {out}");

    // Same-window arrival: the absence claim fails with the at-most text.
    let dir = temp_dir("max-zero-fail");
    let path = write_absence_doc(&dir, "fail.test.yaml", true);
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[path], &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 1);
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains("MockEndpoint 'out': expected at most 0 exchanges, got 1"),
        "an arrival must break the maxCount 0 claim: {out}"
    );
}

/// Write a route file whose circuit breaker `open_duration_ms` carries an
/// env placeholder with a 750 ms default.
fn write_cb_route_file(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(
        &path,
        r#"
routes:
  - id: cb-route
    from: "direct:start"
    circuit_breaker:
      failure_threshold: 4
      open_duration_ms: ${env:CB_MS:-750}
    steps:
      - to: "mock:out"
"#,
    )
    .expect("write cb route file"); // allow-unwrap
    path
}

/// Write a route file whose `set_header` step carries the given token in
/// its STRING-typed `value` field (the tree-walk typing canon: a
/// substituted leaf keeps string typing, so string positions interpolate).
fn write_string_header_route_file(dir: &Path, name: &str, value_token: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(
        &path,
        format!(
            r#"
routes:
  - id: lean-string-route
    from: "direct:start"
    steps:
      - set_header:
          key: lean-key
          value: {value_token}
"#
        ),
    )
    .expect("write string-header route file"); // allow-unwrap
    path
}

/// The `value` source of the route's single `set_header` step — the
/// STRING-typed field the string-position tests observe.
fn header_value(defs: &[camel_core::RouteDefinition]) -> &camel_api::declarative::ValueSourceDef {
    let def = defs.first().expect("route definition present"); // allow-unwrap
    def.steps()
        .iter()
        .find_map(|step| match step {
            camel_core::BuilderStep::DeclarativeSetHeader { value, .. } => Some(value),
            _ => None,
        })
        .expect("set_header step present") // allow-unwrap
}

/// Write a document referencing a route file via `routeFiles`, with a
/// trivial expectation (`direct:start` → `mock:out`, count 1).
fn write_route_files_doc(dir: &Path, name: &str, route_file: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(
        &path,
        format!(
            r#"
routeFiles:
  - {route_file}
inputs:
  - to: "direct:start"
    body: "x"
expects:
  mock:out:
    count: 1
"#
        ),
    )
    .expect("write routeFiles doc"); // allow-unwrap
    path
}

/// Parse a `.test.yaml` document from disk for runner-level assertions.
fn parse_doc_at(path: &Path) -> document::TestDocument {
    let text = fs::read_to_string(path).expect("read test document"); // allow-unwrap
    document::parse_test_document(&text).expect("parse test document") // allow-unwrap
}

/// File routes under the LEAN runner load through the same tree-walk-first
/// loader `camel run` uses, so a placeholder on a STRING-typed field
/// (`set_header.value`) interpolates to its `:-default` and the route loads.
#[tokio::test(flavor = "multi_thread")]
async fn lean_file_route_string_placeholder_loads() {
    let dir = temp_dir("lean-file-string");
    let route = write_string_header_route_file(&dir, "string.routes.yaml", "${env:LEAN_T:-hello}");
    let doc_path = write_route_files_doc(&dir, "a.test.yaml", "string.routes.yaml");
    let _guard = CleanupPaths(vec![route, doc_path.clone(), dir.clone()]);
    let doc = parse_doc_at(&doc_path);
    let defs = match runner::load_routes(&doc, &dir).await {
        Ok(defs) => defs,
        Err(e) => panic!("route load must succeed: {e}"),
    };
    assert_eq!(
        header_value(&defs),
        &camel_api::declarative::ValueSourceDef::Literal(serde_json::Value::String(
            "hello".to_string()
        )),
        "string-position placeholder must carry the interpolated default"
    );
}

/// A placeholder on the integer-typed `circuit_breaker.open_duration_ms`
/// loads through the loader's typed probe — the file forms share the
/// `camel_dsl::load_from_file_with_env` seam with `camel run` (boot
/// parity: LEAN accepts exactly what the boot path accepts), so the
/// route loads with the `:-default` coerced to `open_duration_ms == 750`.
#[tokio::test(flavor = "multi_thread")]
async fn lean_file_route_int_placeholder_loads() {
    let dir = temp_dir("lean-file-int");
    let route = write_cb_route_file(&dir, "cb.routes.yaml");
    let doc_path = write_route_files_doc(&dir, "a.test.yaml", "cb.routes.yaml");
    let _guard = CleanupPaths(vec![route, doc_path.clone(), dir.clone()]);
    let doc = parse_doc_at(&doc_path);
    let defs = match runner::load_routes(&doc, &dir).await {
        Ok(defs) => defs,
        Err(e) => panic!("route load must succeed: {e}"),
    };
    let cb = defs
        .first()
        .expect("route definition present") // allow-unwrap
        .circuit_breaker_config()
        .expect("circuit breaker config present"); // allow-unwrap
    assert_eq!(
        cb.open_duration,
        std::time::Duration::from_millis(750),
        "int-position placeholder must load via the typed probe"
    );
}

/// Inline `routes:` go through the same tree-walk-first interpolation as
/// the file forms (boot parity), so a placeholder on the STRING-typed
/// `set_header.value` field interpolates and the routes parse.
#[tokio::test(flavor = "multi_thread")]
async fn lean_inline_routes_string_placeholder_loads() {
    let dir = temp_dir("lean-inline-string");
    let doc_path = dir.join("a.test.yaml");
    fs::write(
        &doc_path,
        r#"
routes:
  - id: p-route
    from: "direct:start"
    steps:
      - set_header:
          key: k
          value: ${env:P:-one}
inputs:
  - to: "direct:start"
    body: "x"
expects:
  mock:out:
    count: 1
"#,
    )
    .expect("write inline doc"); // allow-unwrap
    let _guard = CleanupPaths(vec![doc_path.clone(), dir.clone()]);
    let doc = parse_doc_at(&doc_path);
    let defs = match runner::load_routes(&doc, &dir).await {
        Ok(defs) => defs,
        Err(e) => panic!("route load must succeed: {e}"),
    };
    assert_eq!(
        header_value(&defs),
        &camel_api::declarative::ValueSourceDef::Literal(serde_json::Value::String(
            "one".to_string()
        )),
        "inline string-position placeholder must interpolate"
    );
}

/// An inline `routes:` block carrying a placeholder on the integer-typed
/// `circuit_breaker.open_duration_ms` loads through the same typed seam
/// as the file forms (boot parity): the runner's inline branch feeds the
/// `camel_dsl::parse_routes_with_env` seam, so the loader's typed probe
/// coerces the whole-scalar leaf and the route loads with
/// `open_duration_ms == 750` (the only test that pins the inline branch
/// to the typed seam instead of the legacy text splice).
#[tokio::test(flavor = "multi_thread")]
async fn lean_inline_routes_int_placeholder_loads() {
    let dir = temp_dir("lean-inline-int");
    let doc_path = dir.join("a.test.yaml");
    fs::write(
        &doc_path,
        r#"
routes:
  - id: cb-route
    from: "direct:start"
    circuit_breaker:
      failure_threshold: 4
      open_duration_ms: ${env:CB_MS:-750}
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
    .expect("write inline cb doc"); // allow-unwrap
    let _guard = CleanupPaths(vec![doc_path.clone(), dir.clone()]);
    let doc = parse_doc_at(&doc_path);
    let defs = match runner::load_routes(&doc, &dir).await {
        Ok(defs) => defs,
        Err(e) => panic!("route load must succeed: {e}"),
    };
    let cb = defs
        .first()
        .expect("route definition present") // allow-unwrap
        .circuit_breaker_config()
        .expect("circuit breaker config present"); // allow-unwrap
    assert_eq!(
        cb.open_duration,
        std::time::Duration::from_millis(750),
        "inline int-position placeholder must load via the typed probe"
    );
}

/// An unresolved no-default placeholder on a STRING-typed field fails the
/// document with the boot-parity wording (variable name + lowercase
/// `not set`), never a serde type error.
#[tokio::test(flavor = "multi_thread")]
async fn lean_unset_no_default_doc_error_names_var() {
    let dir = temp_dir("lean-unset");
    let route = write_string_header_route_file(&dir, "unset.routes.yaml", "${env:LEAN_UNDEF_xyz}");
    let doc_path = write_route_files_doc(&dir, "a.test.yaml", "unset.routes.yaml");
    let _guard = CleanupPaths(vec![route, doc_path.clone(), dir.clone()]);
    let doc = parse_doc_at(&doc_path);
    let err = match runner::load_routes(&doc, &dir).await {
        Ok(_) => panic!("unset no-default placeholder must fail the document"),
        Err(e) => e,
    };
    assert!(err.contains("LEAN_UNDEF_xyz"), "err: {err}");
    assert!(err.contains("not set"), "err: {err}");
    assert!(
        !err.contains("invalid type"),
        "failure must not be a serde type error: {err}"
    );
}

/// The default-only lookup is hermetic: an ambient `LEAN_T` value present
/// in the process environment must not influence the resolved string
/// default.
#[tokio::test(flavor = "multi_thread")]
async fn lean_ignores_ambient_env() {
    let dir = temp_dir("lean-ambient");
    let route = write_string_header_route_file(&dir, "ambient.routes.yaml", "${env:LEAN_T:-hello}");
    let doc_path = write_route_files_doc(&dir, "a.test.yaml", "ambient.routes.yaml");
    let _guard = CleanupPaths(vec![route, doc_path.clone(), dir.clone()]);
    // The guard restores the prior value on drop, so a panicking
    // assertion cannot leak the ambient value into other tests.
    let _env = EnvVarGuard::set("LEAN_T", "ambient");
    let doc = parse_doc_at(&doc_path);
    let defs = match runner::load_routes(&doc, &dir).await {
        Ok(defs) => defs,
        Err(e) => panic!("route load must succeed: {e}"),
    };
    assert_eq!(
        header_value(&defs),
        &camel_api::declarative::ValueSourceDef::Literal(serde_json::Value::String(
            "hello".to_string()
        )),
        "default must win over the ambient value"
    );
}

/// The document `env:` map steers the file-route lookup (rc-l7m7t): the
/// `set_header.value` placeholder resolves to the fixture value `docval`
/// first, the `:-default` only as fallback. `set_header.value` is the
/// real string-typed field the spec's `title` placeholder lands on.
#[tokio::test(flavor = "multi_thread")]
async fn doc_env_steers_file_route_field() {
    let dir = temp_dir("doc-env-file");
    let route = write_string_header_route_file(&dir, "docenv.routes.yaml", "${env:LEAN_T:-hello}");
    let doc_path = dir.join("a.test.yaml");
    fs::write(
        &doc_path,
        r#"
env:
  LEAN_T: docval
routeFiles:
  - docenv.routes.yaml
inputs:
  - to: "direct:start"
    body: "x"
expects:
  mock:out:
    count: 1
"#,
    )
    .expect("write doc-env routeFiles doc"); // allow-unwrap
    let _guard = CleanupPaths(vec![route, doc_path.clone(), dir.clone()]);
    let doc = parse_doc_at(&doc_path);
    let defs = match runner::load_routes(&doc, &dir).await {
        Ok(defs) => defs,
        Err(e) => panic!("route load must succeed: {e}"),
    };
    assert_eq!(
        header_value(&defs),
        &camel_api::declarative::ValueSourceDef::Literal(serde_json::Value::String(
            "docval".to_string()
        )),
        "document env value must win over the inline default"
    );
}

/// The document `env:` map also feeds the inline-`routes:` seam: the
/// `${env:P:-one}` placeholder resolves to the fixture value `two`.
#[tokio::test(flavor = "multi_thread")]
async fn doc_env_steers_inline_routes_field() {
    let dir = temp_dir("doc-env-inline");
    let doc_path = dir.join("a.test.yaml");
    fs::write(
        &doc_path,
        r#"
env:
  P: two
routes:
  - id: p-route
    from: "direct:start"
    steps:
      - set_header:
          key: k
          value: ${env:P:-one}
inputs:
  - to: "direct:start"
    body: "x"
expects:
  mock:out:
    count: 1
"#,
    )
    .expect("write doc-env inline doc"); // allow-unwrap
    let _guard = CleanupPaths(vec![doc_path.clone(), dir.clone()]);
    let doc = parse_doc_at(&doc_path);
    let defs = match runner::load_routes(&doc, &dir).await {
        Ok(defs) => defs,
        Err(e) => panic!("route load must succeed: {e}"),
    };
    assert_eq!(
        header_value(&defs),
        &camel_api::declarative::ValueSourceDef::Literal(serde_json::Value::String(
            "two".to_string()
        )),
        "inline placeholder must resolve from the document env"
    );
}

/// A no-default placeholder resolves from the document `env:` map alone —
/// no unresolved-variable error when the fixture supplies the value.
#[tokio::test(flavor = "multi_thread")]
async fn doc_env_no_default_resolves() {
    let dir = temp_dir("doc-env-no-default");
    let route = write_string_header_route_file(&dir, "nodefault.routes.yaml", "${env:LEAN_DEF}");
    let doc_path = dir.join("a.test.yaml");
    fs::write(
        &doc_path,
        r#"
env:
  LEAN_DEF: supplied
routeFiles:
  - nodefault.routes.yaml
inputs:
  - to: "direct:start"
    body: "x"
expects:
  mock:out:
    count: 1
"#,
    )
    .expect("write doc-env no-default doc"); // allow-unwrap
    let _guard = CleanupPaths(vec![route, doc_path.clone(), dir.clone()]);
    let doc = parse_doc_at(&doc_path);
    let defs = match runner::load_routes(&doc, &dir).await {
        Ok(defs) => defs,
        Err(e) => panic!("route load must succeed: {e}"),
    };
    assert_eq!(
        header_value(&defs),
        &camel_api::declarative::ValueSourceDef::Literal(serde_json::Value::String(
            "supplied".to_string()
        )),
        "no-default placeholder must resolve from the document env"
    );
}

/// Typing semantics follow boot when the document env map supplies the
/// value: the int-typed `circuit_breaker.open_duration_ms` placeholder
/// loads through the loader's typed probe with the document env value,
/// exactly as `camel run` accepts the same file (boot parity), so the
/// route loads with `open_duration_ms == 500`.
#[tokio::test(flavor = "multi_thread")]
async fn doc_env_int_position_loads_via_probe() {
    let dir = temp_dir("doc-env-int");
    let route = write_cb_route_file(&dir, "cb.routes.yaml");
    let doc_path = dir.join("a.test.yaml");
    fs::write(
        &doc_path,
        r#"
env:
  CB_MS: "500"
routeFiles:
  - cb.routes.yaml
inputs:
  - to: "direct:start"
    body: "x"
expects:
  mock:out:
    count: 1
"#,
    )
    .expect("write doc-env int doc"); // allow-unwrap
    let _guard = CleanupPaths(vec![route, doc_path.clone(), dir.clone()]);
    let doc = parse_doc_at(&doc_path);
    let defs = match runner::load_routes(&doc, &dir).await {
        Ok(defs) => defs,
        Err(e) => panic!("route load must succeed: {e}"),
    };
    let cb = defs
        .first()
        .expect("route definition present") // allow-unwrap
        .circuit_breaker_config()
        .expect("circuit breaker config present"); // allow-unwrap
    assert_eq!(
        cb.open_duration,
        std::time::Duration::from_millis(500),
        "document env value must flow through the same probe (boot parity)"
    );
}

/// Fixture values are data, never re-scanned: the env value carrying
/// placeholder-shaped text lands in the route verbatim, with no second
/// interpolation pass over it.
#[tokio::test(flavor = "multi_thread")]
async fn env_value_never_interpolated_route() {
    let dir = temp_dir("env-value-verbatim");
    let route = write_string_header_route_file(&dir, "verbatim.routes.yaml", "${env:A:-d}");
    let doc_path = dir.join("a.test.yaml");
    fs::write(
        &doc_path,
        r#"
env:
  A: "${env:B:-x}literal"
routeFiles:
  - verbatim.routes.yaml
inputs:
  - to: "direct:start"
    body: "x"
expects:
  mock:out:
    count: 1
"#,
    )
    .expect("write verbatim doc"); // allow-unwrap
    let _guard = CleanupPaths(vec![route, doc_path.clone(), dir.clone()]);
    let doc = parse_doc_at(&doc_path);
    let defs = match runner::load_routes(&doc, &dir).await {
        Ok(defs) => defs,
        Err(e) => panic!("route load must succeed: {e}"),
    };
    assert_eq!(
        header_value(&defs),
        &camel_api::declarative::ValueSourceDef::Literal(serde_json::Value::String(
            "${env:B:-x}literal".to_string()
        )),
        "fixture value text must land verbatim, never re-interpolated"
    );
}

/// The steering scenario end to end (rc-l7m7t): the document env map
/// resolves BOTH the doc-side `repositories.cache` stub key (parse-time
/// identifier pass) and the route-side `cache.repository` reference (the
/// file-route lookup this task threads) to the same fixture value
/// `faststub`, so the stub registers and the cache step binds to it. The
/// tier derivation is asserted directly: the env-map-present document
/// provably stays unit (LEAN) tier.
#[tokio::test(flavor = "multi_thread")]
async fn steering_repository_e2e() {
    let dir = temp_dir("steering-repo");
    let route = dir.join("cache.routes.yaml");
    fs::write(
        &route,
        r#"
routes:
  - id: cache-route
    from: "direct:start"
    steps:
      - cache:
          repository: "${env:CACHE_REPO_NAME:-persistent}"
          key: k
          on_miss:
            - to: "mock:out"
"#,
    )
    .expect("write steering cache route file"); // allow-unwrap
    let doc_path = dir.join("a.test.yaml");
    fs::write(
        &doc_path,
        r#"
env:
  CACHE_REPO_NAME: faststub
routeFiles:
  - cache.routes.yaml
inputs:
  - to: "direct:start"
    body: "x"
expects:
  mock:out:
    count: 1
repositories:
  cache:
    "${env:CACHE_REPO_NAME:-persistent}": memory
"#,
    )
    .expect("write steering doc"); // allow-unwrap
    let _guard = CleanupPaths(vec![route, doc_path.clone(), dir.clone()]);
    let doc = parse_doc_at(&doc_path);
    // Doc side: the parse-time identifier pass resolved the stub key.
    let cache = doc
        .repositories
        .as_ref()
        .and_then(|repos| repos.cache.as_ref())
        .expect("cache stubs present"); // allow-unwrap
    assert_eq!(cache.len(), 1, "exactly one stub key: {cache:?}");
    assert!(
        cache.contains_key("faststub"),
        "stub key must resolve via doc env: {cache:?}"
    );
    // Route side: the file-route lookup resolved the cache step's
    // repository reference to the same fixture value.
    let defs = match runner::load_routes(&doc, &dir).await {
        Ok(defs) => defs,
        Err(e) => panic!("route load must succeed: {e}"),
    };
    let repository = defs
        .first()
        .expect("route definition present") // allow-unwrap
        .steps()
        .iter()
        .find_map(|step| match step {
            camel_core::BuilderStep::Cache { repository, .. } => repository.as_ref(),
            _ => None,
        })
        .expect("cache step present"); // allow-unwrap
    assert_eq!(
        repository, "faststub",
        "route-side reference must resolve via doc env"
    );
    // The env-map-present document provably stays unit tier.
    assert!(
        matches!(unit_tier(&doc, &defs), Tier::Lean),
        "env-map-present document must stay LEAN tier"
    );
    // End to end: the driver registers the stub under the resolved name
    // and the route's cache step resolves against it — no registration
    // error, run green.
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[doc_path], &mut out, &mut err).await;
    let out = String::from_utf8(out).expect("out is utf-8"); // allow-unwrap
    assert_eq!(
        summary.exit_code,
        0,
        "steered repository run must pass — out: {out} | err: {}",
        String::from_utf8_lossy(&err)
    );
}

/// Regression pin for the added env layer: a variable present ONLY in the
/// process environment stays unresolved — ambient is never a resolution
/// source, with or without a document env map.
#[tokio::test(flavor = "multi_thread")]
async fn ambient_only_stays_unresolved() {
    let dir = temp_dir("ambient-only");
    let route =
        write_string_header_route_file(&dir, "ambient-only.routes.yaml", "${env:AMBIENT_ONLY}");
    let doc_path = write_route_files_doc(&dir, "a.test.yaml", "ambient-only.routes.yaml");
    let _guard = CleanupPaths(vec![route, doc_path.clone(), dir.clone()]);
    // The guard restores the prior value on drop, so a panicking
    // assertion cannot leak the ambient value into other tests.
    let _env = EnvVarGuard::set("AMBIENT_ONLY", "ambient");
    let doc = parse_doc_at(&doc_path);
    let err = match runner::load_routes(&doc, &dir).await {
        Ok(_) => panic!("ambient-only variable must stay unresolved"),
        Err(e) => e,
    };
    assert!(err.contains("AMBIENT_ONLY"), "err: {err}");
    assert!(err.contains("not set"), "err: {err}");
}

/// The pilot pattern (validate-partner-self-declare task 2): a proxy
/// route dials an upstream with a varying query, and the document's
/// ONLY partner reference is the validate action's object-form
/// self-declaration (`provisioning: harness` + `bindVar: UPSTREAM`) —
/// no sacrificial `receive` anywhere. The driver wires the validate
/// ref like a send/receive ref, the scripted harness partner binds,
/// the env tier folds `UPSTREAM -> http://<bound>` in (harness tier
/// wins over the document env), and the route's `${env:UPSTREAM}`
/// interpolates the bound listener at boot — the route file precedent
/// is the flagship retry fixture (`${env:PARTNER_URL}/order`), and the
/// loopback dial needs the `[components.http] allow_internal` opt-in
/// the same fixture declares.
#[tokio::test(flavor = "multi_thread")]
async fn validate_only_partner_proxies_varying_query() {
    let dir = temp_dir("validate-only-partner");
    let camel_toml = dir.join("Camel.toml");
    fs::write(
        &camel_toml,
        "log_level = \"info\"\n\n[components.http]\nallow_internal = true\n",
    )
    .expect("write Camel.toml"); // allow-unwrap
    let routes = dir.join("routes.yaml");
    fs::write(
        &routes,
        r#"routes:
  - id: tile-proxy
    from: direct:start
    steps:
      - to: ${env:UPSTREAM}/tiles?bbox=1.2
"#,
    )
    .expect("write routes.yaml"); // allow-unwrap
    let path = dir.join("a.test.yaml");
    fs::write(
        &path,
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:start
- validate:
    target:
      partner:
        endpoint: http://upstream/tiles
        provisioning: harness
        bindVar: UPSTREAM
    expectation: {count: 1}
    deadline: 5s
partners:
  http://upstream/tiles:
  # The script matches the path-with-query as received
  # (`HttpWireRequest::path` keeps query bytes), so the pass proves the
  # varying query dialed the upstream verbatim.
  - path: /tiles?bbox=1.2
    response:
      status: 200
      body: tile
"#,
    )
    .expect("write scenario doc"); // allow-unwrap
    let _guard = CleanupPaths(vec![camel_toml, routes, path.clone(), dir.clone()]);
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[path], &mut out, &mut err).await;
    let out = String::from_utf8(out).expect("out is utf-8"); // allow-unwrap
    assert_eq!(
        summary.exit_code,
        0,
        "the validate-only partner run must pass — out: {out} | err: {}",
        String::from_utf8_lossy(&err)
    );
    assert!(out.contains("PASS"), "out: {out}");
    assert!(
        out.contains("scenario[1] validate"),
        "the validate action must run and pass: {out}"
    );
}

/// Spec scenario "no-default identifier fails naming the variable at exit
/// 2", driver level: a document whose `repositories.cache` key carries a
/// no-default `${env:NO_SUCH_VAR}` placeholder fails document validation —
/// the driver reports the parse-error class (exit 2) on stderr, naming the
/// variable.
#[tokio::test(flavor = "multi_thread")]
async fn env_unresolved_identifier_exits_2() {
    let dir = temp_dir("env-unresolved");
    let path = dir.join("a.test.yaml");
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
repositories:
  cache:
    "${env:NO_SUCH_VAR}": memory
"#,
    )
    .expect("write env-unresolved doc"); // allow-unwrap
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests(&[path], &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 2);
    let err = String::from_utf8(err).unwrap();
    assert!(
        err.contains("NO_SUCH_VAR"),
        "err must name the unresolved variable: {err}"
    );
}

/// Write a route file whose single route steps through `wasm:` — a
/// component outside the lean registry (ADR-0064) — then `mock:result`.
/// No wasm guest file is needed: the lean registry misses before any
/// file resolution.
fn write_wasm_route_file(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(
        &path,
        r#"
routes:
  - id: r1
    from: "direct:start"
    steps:
      - to: "wasm:echo.wasm"
      - to: "mock:result"
"#,
    )
    .expect("write wasm route file"); // allow-unwrap
    path
}

/// Write a unit document referencing the `wasm:` route file via
/// `routeFiles`, with one `direct:start` input and `mock:result` count 1.
fn write_wasm_doc(dir: &Path, name: &str, route_file: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(
        &path,
        format!(
            r#"
routeFiles:
  - {route_file}
inputs:
  - to: "direct:start"
    body: "hi"
expects:
  mock:result:
    count: 1
"#
        ),
    )
    .expect("write wasm unit doc"); // allow-unwrap
    path
}

/// Spec scenario "full-derived unit document annotates full* with
/// advisory": the annotation tells the truth (`full*` — derived FULL,
/// executed on the lean boot) and one stderr advisory names the lean
/// registry before the run.
#[tokio::test(flavor = "multi_thread")]
async fn unit_wasm_doc_annotates_full_star_with_advisory() {
    let dir = temp_dir("unit-wasm-star");
    let route = write_wasm_route_file(&dir, "route.yaml");
    let doc_path = write_wasm_doc(&dir, "doc.test.yaml", "route.yaml");
    let _guard = CleanupPaths(vec![route, doc_path.clone(), dir.clone()]);
    let config = TestRunConfig {
        files: vec![doc_path],
        ..Default::default()
    };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests_full(&config, &mut out, &mut err).await;
    let out = String::from_utf8(out).unwrap();
    let err = String::from_utf8(err).unwrap();
    assert!(out.contains("doc.test.yaml [full*]\n"), "out: {out}");
    assert!(err.contains("R-UNIT-FULL:"), "err: {err}");
    assert!(
        err.contains("direct, log, mock, seda, timer"),
        "advisory must name the lean registry: {err}"
    );
    assert_eq!(
        summary.exit_code, 2,
        "the lean-registry miss is a document-level failure: out: {out} err: {err}"
    );
}

/// Spec scenario "lean registry miss is actionable": the reported
/// doc-error names the missing component AND the lean registry AND the
/// scenario alternative in one string — never a bare
/// `Component not found: wasm`.
#[tokio::test(flavor = "multi_thread")]
async fn unit_wasm_failure_names_lean_registry_and_alternative() {
    let dir = temp_dir("unit-wasm-hint");
    let route = write_wasm_route_file(&dir, "route.yaml");
    let doc_path = write_wasm_doc(&dir, "doc.test.yaml", "route.yaml");
    let _guard = CleanupPaths(vec![route, doc_path.clone(), dir.clone()]);
    let config = TestRunConfig {
        files: vec![doc_path],
        ..Default::default()
    };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let _summary = run_tests_full(&config, &mut out, &mut err).await;
    let err = String::from_utf8(err).unwrap();
    let line = err
        .lines()
        .find(|line| line.contains("Component not found: wasm"))
        .expect("doc-error line naming the missing component"); // allow-unwrap
    assert!(
        line.contains("lean registry") && line.contains("scenario document"),
        "doc-error must carry the appended hint in one string: {line}"
    );
}

/// Regression guard: a lean-derived document keeps the `[lean]`
/// annotation exactly and writes no `R-UNIT-FULL` advisory.
#[tokio::test(flavor = "multi_thread")]
async fn lean_doc_annotation_stays_lean() {
    let dir = temp_dir("lean-star-regression");
    let doc_path = write_passing(&dir, "doc.test.yaml");
    let _guard = CleanupPaths(vec![doc_path.clone(), dir.clone()]);
    let config = TestRunConfig {
        files: vec![doc_path],
        ..Default::default()
    };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = run_tests_full(&config, &mut out, &mut err).await;
    assert_eq!(summary.exit_code, 0);
    let out = String::from_utf8(out).unwrap();
    let err = String::from_utf8(err).unwrap();
    assert!(out.contains("doc.test.yaml [lean]\n"), "out: {out}");
    assert!(!err.contains("R-UNIT-FULL"), "err: {err}");
}
