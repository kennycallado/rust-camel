//! `camel lint <file>` — lint a route file with the production component
//! catalog.
//!
//! Builds a `CamelContext`, registers the builtins via
//! [`crate::register_builtin_components_for_lint`], obtains the runtime
//! metadata catalog, and runs all default lint rules over the file. Diagnostics
//! are rendered with ariadne to stderr. Exit codes: 0 clean, 1 any
//! error-severity diagnostic, 2 CLI misuse (missing/unreadable file).

use std::path::Path;
use std::sync::Arc;

use camel_api::component_metadata::ComponentMetadataCatalog;
use camel_lint::{Diagnostic, LintEngine, Severity};
use clap::Args;

/// CLI args for `camel lint`.
#[derive(Args, Debug)]
pub struct LintArgs {
    /// Path to the route file (YAML or JSON) to lint.
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

/// Run the lint engine over `path` with the production component catalog.
///
/// Both the [`run`] CLI entrypoint and the in-process tests call this; the
/// returned [`LintOutcome`] carries the exit code the CLI would emit so tests
/// can assert on it without spawning a subprocess.
pub async fn run_lint(path: &Path) -> LintOutcome {
    // Test documents (`*.test.yaml`/`.test.yml`) are `camel test` inputs, not
    // route definitions — linting them as routes would emit spurious
    // diagnostics. Skip with an info line instead. The predicate is the
    // single source of truth shared with route discovery.
    if camel_dsl::discovery::is_test_document(path) {
        return LintOutcome {
            diagnostics: Vec::new(),
            source: String::new(),
            exit_code: 0,
            cli_error: None,
            cli_info: Some(format!(
                "skipped: {} is a camel test document",
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

/// CLI entrypoint for `camel lint`.
pub async fn run(args: LintArgs) {
    let outcome = run_lint(Path::new(&args.file)).await;
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

    Report::build(kind, file_id, range.start)
        .with_code(diag.code.to_string())
        .with_message(diag.message.as_str())
        .with_label(Label::new((file_id, range)))
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
        let yaml = "id: r1\nfrom: timer:foo?period=1s\nsteps:\n  - to: log:bar\n";
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
            info.contains("camel test document"),
            "info must say why the file was skipped; got = {info}"
        );
    }

    #[tokio::test]
    async fn lint_routes_normal_yaml_unchanged() {
        let yaml = "id: r1\nfrom: timer:foo?period=1s\nsteps:\n  - to: log:bar\n";
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
}
