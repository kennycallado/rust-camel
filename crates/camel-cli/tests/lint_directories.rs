//! Integration tests for `camel lint <dir>` — recursive directory linting.
//!
//! Two observation surfaces:
//! - In-process [`run_lint_dir`] for the counts and exit code.
//! - Subprocess (`CARGO_BIN_EXE_camel`) for the exact byte streams stdout and
//!   stderr, the dispatch of `run`, and single-file byte identity against the
//!   committed goldens (`tests/golden/lint-file/`).
//!
//! Route templates are pinned by the task spec so a template change is a test
//! change, not an accidental behavioral drift.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use camel_cli::commands::lint::run_lint_dir;

// --- pinned route templates -------------------------------------------------

fn clean_route(id: &str) -> String {
    format!("id: {id}\nfrom: timer:gold?period=1000\nsteps:\n  - to: log:gold\n")
}

fn error_route(id: &str) -> String {
    format!("id: {id}\nfrom: direct:start\nsteps:\n  - to: timer:x?bogusOption=1\n")
}

fn warning_route(id: &str) -> String {
    format!("id: {id}\nfrom: direct:start\nsteps:\n  - to: mock:x\n")
}

// --- helpers ----------------------------------------------------------------

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap(); // allow-unwrap
    }
    std::fs::write(path, contents).unwrap(); // allow-unwrap
}

fn camel_lint(arg: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_camel"))
        .args(["lint", arg])
        .output()
        .unwrap() // allow-unwrap
}

fn camel_lint_in(dir: &Path, arg: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_camel"))
        .args(["lint", arg])
        .current_dir(dir)
        .output()
        .unwrap() // allow-unwrap
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Matches the directory summary line exactly:
/// `linted <N> file(s), <M> skipped, <F> failed, <K> diagnostic(s)`.
fn is_summary_line(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("linted ") else {
        return false;
    };
    let Some(rest) = rest.strip_suffix(" diagnostic(s)") else {
        return false;
    };
    let parts: Vec<&str> = rest.split(", ").collect();
    let [p0, p1, p2, p3] = parts.as_slice() else {
        return false;
    };
    match (
        p0.strip_suffix(" file(s)"),
        p1.strip_suffix(" skipped"),
        p2.strip_suffix(" failed"),
    ) {
        (Some(a), Some(b), Some(c)) => {
            a.parse::<usize>().is_ok()
                && b.parse::<usize>().is_ok()
                && c.parse::<usize>().is_ok()
                && p3.parse::<usize>().is_ok()
        }
        _ => false,
    }
}

// --- tests ------------------------------------------------------------------

#[tokio::test]
async fn dir_lints_exact_eligible_file_set() {
    let dir = tempfile::tempdir().unwrap(); // allow-unwrap
    let root = dir.path();
    write(root, "a.yaml", &clean_route("a"));
    write(root, "sub/b.yml", &clean_route("b"));
    write(root, "sub/c.json", &clean_route("c"));
    write(root, "notes.txt", "not a route");
    write(root, "D.YAML", &clean_route("upper")); // case-sensitive: excluded

    let outcome = run_lint_dir(root).await;
    assert_eq!(outcome.linted, 3, "outcome = {outcome:?}");
    assert_eq!(outcome.skipped, 0, "outcome = {outcome:?}");
    assert_eq!(outcome.failed, 0, "outcome = {outcome:?}");
    assert_eq!(outcome.exit_code, 0, "outcome = {outcome:?}");
}

#[tokio::test]
async fn dir_exit_one_when_one_file_errors() {
    let dir = tempfile::tempdir().unwrap(); // allow-unwrap
    let root = dir.path();
    write(root, "a.yaml", &clean_route("a"));
    write(root, "b.yaml", &clean_route("b"));
    write(root, "c.yaml", &error_route("c"));

    let outcome = run_lint_dir(root).await;
    assert_eq!(outcome.exit_code, 1, "outcome = {outcome:?}");
    assert_eq!(outcome.linted, 3, "outcome = {outcome:?}");
    assert_eq!(outcome.failed, 0, "outcome = {outcome:?}");
}

#[tokio::test]
async fn dir_exit_zero_all_clean() {
    let dir = tempfile::tempdir().unwrap(); // allow-unwrap
    let root = dir.path();
    write(root, "a.yaml", &clean_route("a"));
    write(root, "b.yaml", &clean_route("b"));
    write(root, "c.yaml", &clean_route("c"));

    let outcome = run_lint_dir(root).await;
    assert_eq!(outcome.exit_code, 0, "outcome = {outcome:?}");
    assert_eq!(outcome.diagnostics, 0, "outcome = {outcome:?}");
    assert_eq!(outcome.linted, 3, "outcome = {outcome:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn dir_exit_two_and_continues_on_read_failures() {
    let dir = tempfile::tempdir().unwrap(); // allow-unwrap
    let root = dir.path();
    write(root, "error.yaml", &error_route("e"));
    // Invalid UTF-8 is not a valid rust String: read_to_string fails.
    std::fs::write(root.join("bad.bin.yaml"), [0xFFu8]).unwrap(); // allow-unwrap
    std::os::unix::fs::symlink(root.join("missing.yaml"), root.join("broken.yaml")).unwrap(); // allow-unwrap

    let outcome = run_lint_dir(root).await;
    assert_eq!(outcome.exit_code, 2, "outcome = {outcome:?}");
    assert_eq!(outcome.failed, 2, "outcome = {outcome:?}");
    assert_eq!(outcome.linted, 1, "outcome = {outcome:?}");
    assert!(
        outcome.diagnostics >= 1,
        "the error route must still be linted; outcome = {outcome:?}"
    );

    // Byte-stream variant: exactly two read-failure lines on stderr.
    let output = camel_lint_in(root, root.to_str().unwrap()); // allow-unwrap
    assert_eq!(
        output.status.code(),
        Some(2),
        "stderr = {}",
        stderr_of(&output)
    );
    let stderr = stderr_of(&output);
    let read_failures = stderr
        .lines()
        .filter(|l| l.starts_with("error: failed to read '"))
        .count();
    assert_eq!(
        read_failures, 2,
        "expected exactly two read-failure lines; stderr:\n{stderr}"
    );
}

#[tokio::test]
async fn dir_reserved_docs_skipped_with_info() {
    let dir = tempfile::tempdir().unwrap(); // allow-unwrap
    let root = dir.path();
    write(root, "route.yaml", &clean_route("r"));
    write(root, "demo.test.yaml", "routeFiles: [x]\nexpects: {}\n");
    write(root, "demo.job.yml", "routeFiles: [x]\nexecute: {}\n");

    let outcome = run_lint_dir(root).await;
    assert_eq!(outcome.linted, 1, "outcome = {outcome:?}");
    assert_eq!(outcome.skipped, 2, "outcome = {outcome:?}");
    assert_eq!(outcome.exit_code, 0, "outcome = {outcome:?}");

    // The skip wording is exercised on stdout (one line per reserved doc).
    let output = camel_lint_in(root, root.to_str().unwrap()); // allow-unwrap
    let stdout = stdout_of(&output);
    let skip_lines = stdout
        .lines()
        .filter(|l| l.contains("is a reserved document (camel test or camel job)"))
        .count();
    assert_eq!(skip_lines, 2, "stdout:\n{stdout}");
}

#[tokio::test]
async fn dir_only_reserved_docs_exits_zero() {
    let dir = tempfile::tempdir().unwrap(); // allow-unwrap
    let root = dir.path();
    write(root, "demo.test.yaml", "routeFiles: [x]\nexpects: {}\n");
    write(root, "demo.job.yml", "routeFiles: [x]\nexecute: {}\n");

    let outcome = run_lint_dir(root).await;
    assert_eq!(outcome.linted, 0, "outcome = {outcome:?}");
    assert_eq!(outcome.skipped, 2, "outcome = {outcome:?}");
    assert_eq!(outcome.failed, 0, "outcome = {outcome:?}");
    assert_eq!(outcome.exit_code, 0, "outcome = {outcome:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn dir_summary_counts_every_outcome() {
    let dir = tempfile::tempdir().unwrap(); // allow-unwrap
    let root = dir.path();
    write(root, "a.yaml", &clean_route("a"));
    write(root, "b.yaml", &clean_route("b"));
    write(root, "warn.yaml", &warning_route("w"));
    write(root, "demo.test.yaml", "routeFiles: [x]\nexpects: {}\n");
    std::os::unix::fs::symlink(root.join("missing.yaml"), root.join("broken.yaml")).unwrap(); // allow-unwrap

    let outcome = run_lint_dir(root).await;
    assert_eq!(outcome.linted, 3, "outcome = {outcome:?}");
    assert_eq!(outcome.skipped, 1, "outcome = {outcome:?}");
    assert_eq!(outcome.failed, 1, "outcome = {outcome:?}");
    assert_eq!(outcome.diagnostics, 1, "outcome = {outcome:?}");
    assert_eq!(
        outcome.exit_code, 2,
        "failed dominates; outcome = {outcome:?}"
    );
}

#[tokio::test]
async fn dir_visit_order_sorted_depth_first() {
    let dir = tempfile::tempdir().unwrap(); // allow-unwrap
    let root = dir.path();
    write(root, "b.yaml", &error_route("b"));
    write(root, "a-b.yaml", &error_route("ab"));
    write(root, "a/z.yaml", &error_route("z"));

    let output = camel_lint_in(root, root.to_str().unwrap()); // allow-unwrap
    let stderr = stderr_of(&output);
    // Full absolute needles so `b.yaml` cannot substring-match inside
    // `a-b.yaml`.
    let z = root.join("a/z.yaml").display().to_string();
    let ab = root.join("a-b.yaml").display().to_string();
    let b = root.join("b.yaml").display().to_string();

    let pos_z = stderr.find(&z).unwrap_or_else(|| {
        panic!("stderr must contain {z}; stderr:\n{stderr}");
    }); // allow-unwrap
    let pos_ab = stderr.find(&ab).unwrap_or_else(|| {
        panic!("stderr must contain {ab}; stderr:\n{stderr}");
    }); // allow-unwrap
    let pos_b = stderr.find(&b).unwrap_or_else(|| {
        panic!("stderr must contain {b}; stderr:\n{stderr}");
    }); // allow-unwrap

    assert!(
        pos_z < pos_ab && pos_ab < pos_b,
        "expected order a/z.yaml < a-b.yaml < b.yaml; positions = ({pos_z}, {pos_ab}, {pos_b})\nstderr:\n{stderr}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn dir_unreadable_root_aborts_without_summary() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap(); // allow-unwrap
    let root = dir.path();
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o000)).unwrap(); // allow-unwrap
    if std::fs::read_dir(root).is_ok() {
        // Running as root bypasses permission bits: skip rather than assert.
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o755)).unwrap(); // allow-unwrap
        return;
    }

    // The child must not chdir into the unreadable root (chdir would itself
    // fail with EACCES before `camel` ever runs), so inherit the test cwd.
    let output = camel_lint(root.to_str().unwrap()); // allow-unwrap
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o755)).unwrap(); // allow-unwrap

    let stdout = stdout_of(&output);
    let stderr = stderr_of(&output);
    assert_eq!(output.status.code(), Some(2), "stderr:\n{stderr}");
    assert!(
        stderr
            .lines()
            .any(|l| l.starts_with("error: failed to read '")),
        "stderr must carry the root read error; stderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("linted ") && !stderr.contains("linted "),
        "abort must print no summary; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("warning: no route files found")
            && !stderr.contains("warning: no route files found"),
        "abort must print no empty-tree warning; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

#[tokio::test]
async fn dir_repeated_runs_byte_identical() {
    let dir = tempfile::tempdir().unwrap(); // allow-unwrap
    let root = dir.path();
    write(root, "a.yaml", &clean_route("a"));
    write(root, "b.yaml", &error_route("b"));
    write(root, "demo.test.yaml", "routeFiles: [x]\nexpects: {}\n");
    write(root, "demo.job.yml", "routeFiles: [x]\nexecute: {}\n");

    let arg = root.to_str().unwrap(); // allow-unwrap
    let first = camel_lint_in(root, arg);
    let second = camel_lint_in(root, arg);

    assert_eq!(first.stdout, second.stdout, "stdout must be byte-identical");
    assert_eq!(first.stderr, second.stderr, "stderr must be byte-identical");

    let stdout = stdout_of(&first);
    let expected = format!(
        "skipped: {} is a reserved document (camel test or camel job)",
        root.join("demo.test.yaml").display()
    );
    assert!(
        stdout.lines().any(|l| l == expected),
        "stdout must carry the skip line {expected:?}; stdout:\n{stdout}"
    );
}

#[tokio::test]
async fn dir_empty_dir_warns_and_exits_zero() {
    let dir = tempfile::tempdir().unwrap(); // allow-unwrap
    let root = dir.path();

    let output = camel_lint_in(root, root.to_str().unwrap()); // allow-unwrap
    let stdout = stdout_of(&output);
    let stderr = stderr_of(&output);

    assert_eq!(output.status.code(), Some(0), "stderr:\n{stderr}");
    assert!(
        stdout.contains("linted 0 file(s), 0 skipped, 0 failed, 0 diagnostic(s)"),
        "stdout:\n{stdout}"
    );
    assert!(
        stderr.contains("warning: no route files found"),
        "stderr:\n{stderr}"
    );
}

#[tokio::test]
async fn dir_stagec_exemption_inside_tree() {
    let exempt_dir = tempfile::tempdir().unwrap(); // allow-unwrap
    let exempt_root = exempt_dir.path();
    write(
        exempt_root,
        "tests/fixtures/route.yaml",
        &warning_route("fixture"),
    );

    let plain_dir = tempfile::tempdir().unwrap(); // allow-unwrap
    let plain_root = plain_dir.path();
    write(plain_root, "other/route.yaml", &warning_route("plain"));

    let exempt = run_lint_dir(exempt_root).await;
    assert_eq!(
        exempt.diagnostics, 0,
        "tests/fixtures path must drop the R-MOCK warning; outcome = {exempt:?}"
    );

    let plain = run_lint_dir(plain_root).await;
    assert!(
        plain.diagnostics >= 1,
        "a non-fixture path must keep the R-MOCK warning; outcome = {plain:?}"
    );
}

#[tokio::test]
async fn missing_root_exits_two() {
    let dir = tempfile::tempdir().unwrap(); // allow-unwrap
    let missing = dir.path().join("nonexistent-dir");

    let output = camel_lint_in(dir.path(), missing.to_str().unwrap()); // allow-unwrap
    let stderr = stderr_of(&output);
    assert_eq!(output.status.code(), Some(2), "stderr:\n{stderr}");
    assert!(
        stderr.lines().any(|l| l.starts_with("error: ")),
        "stderr:\n{stderr}"
    );
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/lint-file")
}

#[tokio::test]
async fn single_file_golden_byte_identity() {
    let golden = golden_dir();
    let cwd = golden.join("cwd");

    let cases = [
        ("g-clean", "g-clean.yaml"),
        ("g-error", "g-error.yaml"),
        ("g-missing", "g-missing.yaml"),
        ("g-reserved", "g-reserved.test.yaml"),
    ];

    for (case, arg) in cases {
        let expected_stdout = std::fs::read(golden.join(format!("{case}.stdout"))).unwrap(); // allow-unwrap
        let expected_stderr = std::fs::read(golden.join(format!("{case}.stderr"))).unwrap(); // allow-unwrap
        let expected_exit: i32 = std::fs::read_to_string(golden.join(format!("{case}.exit")))
            .unwrap() // allow-unwrap
            .trim()
            .parse()
            .unwrap(); // allow-unwrap

        let output = camel_lint_in(&cwd, arg);
        assert_eq!(
            output.stdout,
            expected_stdout,
            "{case}: stdout bytes differ\n--- expected ---\n{}\n--- actual ---\n{}",
            String::from_utf8_lossy(&expected_stdout),
            stdout_of(&output)
        );
        assert_eq!(
            output.stderr,
            expected_stderr,
            "{case}: stderr bytes differ\n--- expected ---\n{}\n--- actual ---\n{}",
            String::from_utf8_lossy(&expected_stderr),
            stderr_of(&output)
        );
        assert_eq!(
            output.status.code(),
            Some(expected_exit),
            "{case}: exit code differs"
        );
    }
}

#[tokio::test]
async fn single_file_prints_no_summary_line() {
    let golden = golden_dir();
    let cwd = golden.join("cwd");

    let output = camel_lint_in(&cwd, "g-clean.yaml");
    let stdout = stdout_of(&output);
    assert!(
        !stdout.lines().any(is_summary_line),
        "single-file mode must print no summary line; stdout:\n{stdout}"
    );
}
