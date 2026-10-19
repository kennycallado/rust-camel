//! `camel lint <path>` — lint a route file or directory tree with the
//! production component catalog.
//!
//! Builds a `CamelContext`, registers the builtins via
//! [`crate::register_builtin_components_for_lint`], obtains the runtime
//! metadata catalog, and runs all default lint rules over the file. Diagnostics
//! are rendered with ariadne to stderr. A directory is walked recursively for
//! `.yaml`, `.yml`, and `.json` documents and every target is linted in turn.
//! Exit codes: 0 clean, 1 any error-severity diagnostic, 2 CLI misuse
//! (missing/unreadable file or directory root). Reserved documents
//! (`*.test.yaml`, `*.test.yml`, `*.job.yaml`, `*.job.yml` — `camel test` or
//! `camel job` inputs) are skipped with an info line (exit 0).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use camel_api::component_metadata::ComponentMetadataCatalog;
use camel_lint::{Diagnostic, DiagnosticCode, LintEngine, Severity};
use clap::Args;

/// CLI args for `camel lint`.
#[derive(Args, Debug)]
pub struct LintArgs {
    /// Path to the route file or directory (YAML or JSON) to lint.
    #[arg(value_name = "PATH")]
    pub file: String,
}

/// Outcome of a lint run.
#[derive(Debug)]
pub struct LintOutcome {
    /// Diagnostics emitted by the engine (empty on CLI misuse).
    pub diagnostics: Vec<Diagnostic>,
    /// Source text read from the file (empty on CLI misuse). Kept so the
    /// caller can render diagnostics with byte-exact spans.
    pub source: String,
    /// Exit code the CLI would emit: 0 clean, 1 any Error diagnostic, 2 CLI
    /// misuse (missing/unreadable file).
    pub exit_code: i32,
    /// CLI-misuse message printed to stderr when `exit_code == 2`.
    pub cli_error: Option<String>,
    /// Informational message printed to stdout (e.g. skipped test document).
    pub cli_info: Option<String>,
}

/// Outcome of a directory lint run (`camel lint <dir>`).
#[derive(Debug)]
pub struct DirLintOutcome {
    /// Exit code the CLI would emit: 2 when any target failed to read, else 1
    /// when any Error-severity diagnostic was emitted, else 0.
    pub exit_code: i32,
    /// Number of route files linted (reserved documents and read failures
    /// excluded).
    pub linted: usize,
    /// Number of reserved documents skipped.
    pub skipped: usize,
    /// Number of files that could not be read (including non-UTF-8 and
    /// walker-level I/O failures).
    pub failed: usize,
    /// Total diagnostics rendered across every linted file, all severities.
    pub diagnostics: usize,
}

/// Build the production lint engine with the full builtin component catalog.
///
/// This is the **single source of truth** for the engine construction sequence
/// that `camel lint` uses at runtime. The corpus zero-false-positives gate and
/// the production-catalog smoke test call this so their path is structurally
/// identical to the CLI path — no copy-paste drift possible.
pub async fn production_engine() -> Result<LintEngine, String> {
    let mut ctx = camel_core::CamelContext::builder()
        .build()
        .await
        .map_err(|e| format!("failed to build CamelContext: {e}"))?;
    crate::register_builtin_components_for_lint(&mut ctx);
    let catalog: Arc<dyn ComponentMetadataCatalog> = Arc::new(ctx.metadata_catalog());
    Ok(LintEngine::new(catalog).with_default_rules())
}

/// Whether a path is exempt from the Stage C `R-MOCK-IN-PRODUCTION` warning.
///
/// True when any component pair in the path is `tests` immediately followed
/// by `fixtures`. Such paths are component test fixtures (e.g.
/// `crates/components/camel-xslt/tests/fixtures/xslt-param-namespace.yaml`),
/// which legitimately send to `mock:` endpoints to assert on recorded
/// exchanges — not production routes. The exemption is scoped to the mock
/// rule only (see [`run_lint`]); other diagnostic codes are unaffected.
/// Evaluates the user-supplied path as given (not canonicalized): a bare relative filename from inside a fixtures dir will not match; repo-relative and absolute paths do (deliberate syntactic proxy per design.md).
pub fn is_stagec_exempt_path(path: &Path) -> bool {
    let comps: Vec<_> = path.components().collect();
    comps
        .windows(2)
        .any(|w| w[0].as_os_str() == "tests" && w[1].as_os_str() == "fixtures")
}

/// Drop `R-MOCK-IN-PRODUCTION` diagnostics when `path` is a Stage C fixture.
///
/// Component test fixtures under a `tests/fixtures/` pair legitimately send to
/// `mock:` endpoints. Only R-MOCK diagnostics are removed; all other codes pass
/// through unchanged. Shared by single-file and directory modes so both apply
/// exactly the same exemption.
fn drop_rmock_if_stagec_exempt(path: &Path, diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
    if is_stagec_exempt_path(path) {
        diagnostics
            .into_iter()
            .filter(|d| d.code != DiagnosticCode::RMock)
            .collect()
    } else {
        diagnostics
    }
}

/// Collect every lintable file under `root`, plus the per-path I/O failures
/// encountered while walking.
///
/// Walk rules (see `design.md`):
/// - Depth-first pre-order; entries in each directory are sorted by the bytes
///   of their file name before visiting.
/// - Only names ending in `.yaml`, `.yml`, or `.json` (case-sensitive) are
///   candidates.
/// - A candidate is collected only when `std::fs::metadata` (which follows
///   symlinks) reports a regular file. A metadata error on an eligible name
///   (for example a dangling symlink) is returned in the failures vector.
/// - Below the root, hidden entries and directories named `target` are pruned;
///   the root itself is exempt.
/// - Symlinked directories are never descended. Symlinked files are followed
///   by the metadata check above.
/// - A `read_dir` failure is returned in the failures vector; the walk
///   continues with the remaining entries.
///
/// The function performs no printing; the caller owns all reporting.
pub fn collect_lint_targets(root: &Path) -> (Vec<PathBuf>, Vec<(PathBuf, std::io::Error)>) {
    let mut targets = Vec::new();
    let mut failures = Vec::new();
    walk_lint_dir(root, &mut targets, &mut failures);
    (targets, failures)
}

fn walk_lint_dir(
    dir: &Path,
    targets: &mut Vec<PathBuf>,
    failures: &mut Vec<(PathBuf, std::io::Error)>,
) {
    let read = match std::fs::read_dir(dir) {
        Ok(read) => read,
        Err(err) => {
            failures.push((dir.to_path_buf(), err));
            return;
        }
    };

    let mut entries = Vec::new();
    for entry in read {
        match entry {
            Ok(entry) => entries.push(entry),
            Err(err) => failures.push((dir.to_path_buf(), err)),
        }
    }
    entries.sort_by(|a, b| {
        a.file_name()
            .as_encoded_bytes()
            .cmp(b.file_name().as_encoded_bytes())
    });

    for entry in entries {
        let name = entry.file_name();
        let path = entry.path();

        // Pruning applies to every entry below the walked root; the root
        // itself is never checked against these rules.
        if name.as_encoded_bytes().starts_with(b".") {
            continue;
        }

        let file_type = entry.file_type();
        if name == OsStr::new("target") && matches!(&file_type, Ok(ft) if ft.is_dir()) {
            continue;
        }

        // Never descend into a symlinked directory: `file_type` reports a
        // symlink (not a directory) for it, so it falls through to
        // `collect_candidate`, where the metadata-is_file check ignores it.
        // Symlinked files pass that same check and are collected.
        match file_type {
            Ok(ft) if ft.is_dir() => walk_lint_dir(&path, targets, failures),
            _ => collect_candidate(&path, &name, targets, failures),
        }
    }
}

/// Collect `path` when its name has an eligible extension and `metadata`
/// (following symlinks) reports a regular file. Metadata failures on eligible
/// names become per-path failures; non-eligible names are ignored.
fn collect_candidate(
    path: &Path,
    name: &OsStr,
    targets: &mut Vec<PathBuf>,
    failures: &mut Vec<(PathBuf, std::io::Error)>,
) {
    if !has_eligible_extension(name) {
        return;
    }
    match std::fs::metadata(path) {
        Ok(md) if md.is_file() => targets.push(path.to_path_buf()),
        Ok(_) => {}
        Err(err) => failures.push((path.to_path_buf(), err)),
    }
}

fn has_eligible_extension(name: &OsStr) -> bool {
    let bytes = name.as_encoded_bytes();
    [b".yaml".as_slice(), b".yml".as_slice(), b".json".as_slice()]
        .iter()
        .any(|ext| bytes.ends_with(ext))
}

/// Run the lint engine over `path` with the production component catalog.
///
/// Both the [`run`] CLI entrypoint and the in-process tests call this; the
/// returned [`LintOutcome`] carries the exit code the CLI would emit so tests
/// can assert on it without spawning a subprocess.
pub async fn run_lint(path: &Path) -> LintOutcome {
    // Reserved documents (`*.test.yaml`/`*.test.yml` are `camel test`
    // inputs; `*.job.yaml`/`*.job.yml` are `camel job` inputs) are not
    // route definitions — linting them as routes would emit spurious
    // diagnostics. Skip with an info line instead. The predicate is the
    // single source of truth shared with route discovery.
    if camel_dsl::discovery::is_reserved_document(path) {
        return LintOutcome {
            diagnostics: Vec::new(),
            source: String::new(),
            exit_code: 0,
            cli_error: None,
            cli_info: Some(format!(
                "skipped: {} is a reserved document (camel test or camel job)",
                path.display()
            )),
        };
    }

    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            return LintOutcome {
                diagnostics: Vec::new(),
                source: String::new(),
                exit_code: 2,
                cli_error: Some(format!("failed to read '{}': {e}", path.display())),
                cli_info: None,
            };
        }
    };

    let engine = match production_engine().await {
        Ok(engine) => engine,
        Err(e) => {
            return LintOutcome {
                diagnostics: Vec::new(),
                source,
                exit_code: 2,
                cli_error: Some(e),
                cli_info: None,
            };
        }
    };
    let diagnostics = engine.lint(&source);

    let diagnostics = drop_rmock_if_stagec_exempt(path, diagnostics);

    let has_error = diagnostics.iter().any(|d| d.severity == Severity::Error);
    let exit_code = if has_error { 1 } else { 0 };

    LintOutcome {
        diagnostics,
        source,
        exit_code,
        cli_error: None,
        cli_info: None,
    }
}

/// Lint every eligible route file under `root`.
///
/// Ordering mirrors the walker: reserved documents are skipped with an info
/// line, read failures (including non-UTF-8) are reported per file, and
/// diagnostics are rendered in walker order. Walker-level failures (dangling
/// symlinks, unreadable subdirectories) are reported after the per-target loop
/// and before the single summary line. All printing happens here; the returned
/// outcome carries the counts and exit code.
pub async fn run_lint_dir(root: &Path) -> DirLintOutcome {
    // Root-abort check: an unreadable root is a hard failure. No warning, no
    // summary, no walk.
    if let Err(e) = std::fs::read_dir(root) {
        eprintln!("error: failed to read '{}': {e}", root.display());
        return DirLintOutcome {
            exit_code: 2,
            linted: 0,
            skipped: 0,
            failed: 0,
            diagnostics: 0,
        };
    }

    let engine = match production_engine().await {
        Ok(engine) => engine,
        Err(e) => {
            eprintln!("error: {e}");
            return DirLintOutcome {
                exit_code: 2,
                linted: 0,
                skipped: 0,
                failed: 0,
                diagnostics: 0,
            };
        }
    };

    let (targets, failures) = collect_lint_targets(root);
    if targets.is_empty() {
        eprintln!("warning: no route files found under {}", root.display());
    }

    let mut linted = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let mut diagnostics = 0usize;
    let mut saw_error = false;

    for path in &targets {
        if camel_dsl::discovery::is_reserved_document(path) {
            println!(
                "skipped: {} is a reserved document (camel test or camel job)",
                path.display()
            );
            skipped += 1;
            continue;
        }

        let source = match std::fs::read_to_string(path) {
            Ok(source) => source,
            Err(e) => {
                eprintln!("error: failed to read '{}': {e}", path.display());
                failed += 1;
                continue;
            }
        };

        let diags = engine.lint(&source);
        // Stage C fixture-path exemption, identical to single-file mode. The
        // walked path is used as-is (never canonicalized).
        let diags = drop_rmock_if_stagec_exempt(path, diags);

        let path_str = path.display().to_string();
        for diag in &diags {
            render_diagnostic(diag, &path_str, &source);
            diagnostics += 1;
            if diag.severity == Severity::Error {
                saw_error = true;
            }
        }
        linted += 1;
    }

    // Walker-level failures: dangling eligible symlinks, unreadable
    // subdirectories, and directory-entry errors. Reported after the
    // per-target loop so they do not interleave with rendered diagnostics.
    for (path, err) in &failures {
        eprintln!("error: failed to read '{}': {err}", path.display());
        failed += 1;
    }

    println!(
        "linted {linted} file(s), {skipped} skipped, {failed} failed, {diagnostics} diagnostic(s)"
    );

    let exit_code = if failed > 0 {
        2
    } else if saw_error {
        1
    } else {
        0
    };

    DirLintOutcome {
        exit_code,
        linted,
        skipped,
        failed,
        diagnostics,
    }
}

/// CLI entrypoint for `camel lint`.
pub async fn run(args: LintArgs) {
    let path = Path::new(&args.file);
    // A directory dispatches to the recursive walker; a file (or a missing or
    // dangling path) keeps the existing single-file behavior.
    if let Ok(metadata) = std::fs::metadata(path)
        && metadata.is_dir()
    {
        let outcome = run_lint_dir(path).await;
        std::process::exit(outcome.exit_code);
    }

    let outcome = run_lint(path).await;
    if let Some(err) = &outcome.cli_error {
        eprintln!("error: {err}");
    } else if let Some(info) = &outcome.cli_info {
        println!("{info}");
    } else {
        let file_id = args.file.as_str();
        for diag in &outcome.diagnostics {
            render_diagnostic(diag, file_id, &outcome.source);
        }
    }
    std::process::exit(outcome.exit_code);
}

/// Render a single diagnostic to stderr with ariadne.
fn render_diagnostic(diag: &Diagnostic, file_id: &str, source: &str) {
    use ariadne::{Label, Report, ReportKind, Source};

    let kind = match diag.severity {
        Severity::Error => ReportKind::Error,
        Severity::Warning => ReportKind::Warning,
        Severity::Info => ReportKind::Advice,
    };
    let range = clamp_range(diag.span.start, diag.span.end, source.len());

    // ariadne 0.6: the span carries the source id — (id, range) tuples
    // replace the old build(kind, id, offset) + Label::new((id, range)) split.
    let span = (file_id, range);
    Report::build(kind, span.clone())
        .with_code(diag.code.to_string())
        .with_message(diag.message.as_str())
        .with_label(Label::new(span))
        .finish()
        .eprint((file_id, Source::from(source)))
        .ok();
    // Render failure is non-fatal: the diagnostic still counts toward the
    // exit code. ariadne rarely fails post-clamp; logging at warn would
    // require a tracing subscriber that lint (short-lived) may not install.
}

/// Map a diagnostic's byte span onto a valid ariadne range, clamping to the
/// source length and ensuring `end >= start`.
fn clamp_range(start: usize, end: usize, len: usize) -> std::ops::Range<usize> {
    let s = start.min(len);
    let e = end.min(len).max(s);
    s..e
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[tokio::test]
    async fn lint_clean_route_exits_zero() {
        let yaml = "id: r1\nfrom: timer:foo?period=1000\nsteps:\n  - to: log:bar\n";
        let mut tmp = tempfile::NamedTempFile::new().unwrap(); // allow-unwrap
        write!(tmp, "{yaml}").unwrap(); // allow-unwrap

        let outcome = run_lint(tmp.path()).await;
        assert_eq!(
            outcome.exit_code, 0,
            "clean route must exit 0; diags = {:?}",
            outcome.diagnostics
        );
        assert!(
            outcome.diagnostics.is_empty(),
            "clean route must emit no diagnostics; got = {:?}",
            outcome.diagnostics
        );
    }

    #[tokio::test]
    async fn lint_route_with_error_exits_one() {
        let yaml = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?bogusOption=1\n";
        let mut tmp = tempfile::NamedTempFile::new().unwrap(); // allow-unwrap
        write!(tmp, "{yaml}").unwrap(); // allow-unwrap

        let outcome = run_lint(tmp.path()).await;
        assert_eq!(outcome.exit_code, 1, "route with an error must exit 1");
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|d| d.severity == Severity::Error),
            "expected at least one Error diagnostic; got = {:?}",
            outcome.diagnostics
        );
    }

    #[tokio::test]
    async fn lint_missing_file_exits_two() {
        let outcome = run_lint(std::path::Path::new("/nonexistent/route-lint-missing.yaml")).await;
        assert_eq!(outcome.exit_code, 2);
        assert!(outcome.cli_error.is_some());
        assert!(outcome.diagnostics.is_empty());
    }

    #[tokio::test]
    async fn lint_skips_test_document_with_info() {
        let dir = tempfile::tempdir().unwrap(); // allow-unwrap
        let path = dir.path().join("demo.test.yaml");
        std::fs::write(&path, "routeFiles: [x]\nexpects: {}\n").unwrap(); // allow-unwrap

        let outcome = run_lint(&path).await;
        assert_eq!(
            outcome.exit_code, 0,
            "test document must be skipped with exit 0"
        );
        assert!(
            outcome.diagnostics.is_empty(),
            "test document must emit no diagnostics; got = {:?}",
            outcome.diagnostics
        );
        let info = outcome.cli_info.expect("skip must set cli_info"); // allow-unwrap
        assert!(
            info.contains("reserved document"),
            "info must say why the file was skipped; got = {info}"
        );
    }

    #[tokio::test]
    async fn lint_routes_normal_yaml_unchanged() {
        let yaml = "id: r1\nfrom: timer:foo?period=1000\nsteps:\n  - to: log:bar\n";
        let mut tmp = tempfile::NamedTempFile::new().unwrap(); // allow-unwrap
        write!(tmp, "{yaml}").unwrap(); // allow-unwrap

        let outcome = run_lint(tmp.path()).await;
        assert_eq!(outcome.exit_code, 0);
        assert!(
            outcome.cli_info.is_none(),
            "normal route must not be skipped"
        );
        assert!(outcome.diagnostics.is_empty());
    }

    #[test]
    fn register_for_lint_does_not_capture_handles() {
        // Compile-time signature check: the function takes &mut CamelContext
        // and returns () — no bridge/pool/datasource/path handle is captured
        // or returned.
        let _: fn(&mut camel_core::CamelContext) = crate::register_builtin_components_for_lint;
    }

    fn write_lint_file(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap(); // allow-unwrap
        }
        std::fs::write(path, "").unwrap(); // allow-unwrap
    }

    #[test]
    fn walker_collects_exact_eligible_set() {
        let dir = tempfile::tempdir().unwrap(); // allow-unwrap
        let root = dir.path();
        write_lint_file(&root.join("a.yaml"));
        write_lint_file(&root.join("notes.txt"));
        write_lint_file(&root.join("D.YAML"));
        write_lint_file(&root.join("sub/b.yml"));
        write_lint_file(&root.join("sub/c.json"));

        let (targets, failures) = collect_lint_targets(root);
        assert!(failures.is_empty(), "unexpected failures: {failures:?}");
        assert_eq!(
            targets,
            vec![
                root.join("a.yaml"),
                root.join("sub/b.yml"),
                root.join("sub/c.json"),
            ]
        );
    }

    #[test]
    fn walker_prunes_hidden_and_target_below_root() {
        let dir = tempfile::tempdir().unwrap(); // allow-unwrap
        let root = dir.path();
        write_lint_file(&root.join(".hidden/r.yaml"));
        write_lint_file(&root.join("target/f.json"));
        write_lint_file(&root.join("routes/g.yaml"));

        let (targets, failures) = collect_lint_targets(root);
        assert!(failures.is_empty(), "unexpected failures: {failures:?}");
        assert_eq!(targets, vec![root.join("routes/g.yaml")]);
    }

    #[test]
    fn walker_hidden_root_and_target_root_are_linted() {
        let dir = tempfile::tempdir().unwrap(); // allow-unwrap
        let hidden = dir.path().join(".hidden");
        let target = dir.path().join("target");
        write_lint_file(&hidden.join("r.yaml"));
        write_lint_file(&target.join("f.json"));

        let (hidden_targets, hidden_failures) = collect_lint_targets(&hidden);
        assert!(
            hidden_failures.is_empty(),
            "unexpected failures: {hidden_failures:?}"
        );
        assert_eq!(hidden_targets, vec![hidden.join("r.yaml")]);

        let (target_targets, target_failures) = collect_lint_targets(&target);
        assert!(
            target_failures.is_empty(),
            "unexpected failures: {target_failures:?}"
        );
        assert_eq!(target_targets, vec![target.join("f.json")]);
    }

    #[cfg(unix)]
    #[test]
    fn walker_symlink_dir_not_descended_symlink_file_collected() {
        let dir = tempfile::tempdir().unwrap(); // allow-unwrap
        let root = dir.path();
        write_lint_file(&root.join("real.yaml"));
        std::os::unix::fs::symlink(root, root.join("loop")).unwrap(); // allow-unwrap
        std::os::unix::fs::symlink(root.join("real.yaml"), root.join("link.yaml")).unwrap(); // allow-unwrap

        let (targets, failures) = collect_lint_targets(root);
        assert!(failures.is_empty(), "unexpected failures: {failures:?}");
        assert_eq!(
            targets,
            vec![root.join("link.yaml"), root.join("real.yaml")]
        );
    }

    #[cfg(unix)]
    #[test]
    fn walker_fifo_ignored() {
        let dir = tempfile::tempdir().unwrap(); // allow-unwrap
        let pipe = dir.path().join("pipe.yaml");
        let status = std::process::Command::new("mkfifo")
            .arg(&pipe)
            .status()
            .unwrap(); // allow-unwrap
        assert!(status.success(), "mkfifo failed: {status}");

        let (targets, failures) = collect_lint_targets(dir.path());
        assert_eq!(targets, Vec::<PathBuf>::new());
        assert!(failures.is_empty(), "unexpected failures: {failures:?}");
    }

    #[cfg(unix)]
    #[test]
    fn walker_dangling_symlink_is_failure() {
        let dir = tempfile::tempdir().unwrap(); // allow-unwrap
        let root = dir.path();
        let broken = root.join("broken.yaml");
        std::os::unix::fs::symlink(root.join("missing.yaml"), &broken).unwrap(); // allow-unwrap

        let (targets, failures) = collect_lint_targets(root);
        assert_eq!(targets, Vec::<PathBuf>::new());
        assert_eq!(failures.len(), 1, "failures = {failures:?}");
        assert_eq!(failures[0].0, broken);
    }

    #[cfg(unix)]
    #[test]
    fn walker_unreadable_subdir_reported_with_root_gate() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap(); // allow-unwrap
        let root = dir.path();
        let locked = root.join("locked");
        std::fs::create_dir(&locked).unwrap(); // allow-unwrap
        write_lint_file(&root.join("ok.yaml"));
        write_lint_file(&locked.join("hidden.yaml"));

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap(); // allow-unwrap
        // Running as root bypasses permission bits: skip rather than assert a
        // failure that cannot occur.
        let readable = std::fs::read_dir(&locked).is_ok();
        if readable {
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap(); // allow-unwrap
            return;
        }

        let (targets, failures) = collect_lint_targets(root);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap(); // allow-unwrap

        assert_eq!(targets, vec![root.join("ok.yaml")]);
        assert_eq!(failures.len(), 1, "failures = {failures:?}");
        assert_eq!(failures[0].0, locked);
    }

    #[test]
    fn walker_sorted_dfs_order_trap() {
        let dir = tempfile::tempdir().unwrap(); // allow-unwrap
        let root = dir.path();
        write_lint_file(&root.join("b.yaml"));
        write_lint_file(&root.join("a-b.yaml"));
        write_lint_file(&root.join("a/z.yaml"));

        let (targets, failures) = collect_lint_targets(root);
        assert!(failures.is_empty(), "unexpected failures: {failures:?}");
        assert_eq!(
            targets,
            vec![
                root.join("a/z.yaml"),
                root.join("a-b.yaml"),
                root.join("b.yaml"),
            ]
        );
    }
}
