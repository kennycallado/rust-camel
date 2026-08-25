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
// JUnit report writer + report types (consumed by run_tests_full).
mod junit;

#[cfg(test)]
mod document_tests;

#[cfg(test)]
mod driver_tests;

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
    /// Write a JUnit XML report to this path after the run.
    #[arg(long, value_name = "FILE")]
    pub junit: Option<PathBuf>,
    /// Keep only documents whose displayed path matches this glob
    /// (repeatable; `*` does not cross `/`, `**` does).
    #[arg(long = "filter-file", value_name = "GLOB")]
    pub filter_files: Vec<String>,
    /// Keep only documents whose `expects` keys contain this endpoint name
    /// (repeatable; exact match).
    #[arg(long = "filter-endpoint", value_name = "NAME")]
    pub filter_endpoints: Vec<String>,
}

/// Configuration for a `camel test` run.
///
/// `Default` yields the no-flags config: all files, no JUnit report, no
/// filters. All fields are honored by `run_tests_full` (files, JUnit report
/// path, compiled file globs, endpoint names). An empty config behaves
/// exactly like the historical `run_tests`.
#[derive(Debug, Default)]
pub struct TestRunConfig {
    /// Paths to `*.test.yaml` documents or directories to expand, in order.
    pub files: Vec<PathBuf>,
    /// JUnit report path; `None` writes no report.
    pub junit: Option<PathBuf>,
    /// File glob filters; empty means no file filtering.
    pub filter_files: Vec<glob::Pattern>,
    /// Endpoint name filters; empty means no endpoint filtering.
    pub filter_endpoints: Vec<String>,
}

/// Build a `TestRunConfig` from parsed CLI args.
///
/// Compiles each `--filter-file` glob once. An invalid pattern is a misuse
/// error: the dispatch prints it to stderr and exits 2 before any document
/// runs and before any report path is touched.
pub fn config_from_args(args: &TestArgs) -> Result<TestRunConfig, String> {
    let mut filter_files = Vec::with_capacity(args.filter_files.len());
    for glob in &args.filter_files {
        let pattern = glob::Pattern::new(glob)
            .map_err(|e| format!("invalid --filter-file pattern {glob}: {e}"))?;
        filter_files.push(pattern);
    }
    Ok(TestRunConfig {
        files: args.files.clone(),
        junit: args.junit.clone(),
        filter_files,
        filter_endpoints: args.filter_endpoints.clone(),
    })
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

/// Expand CLI path arguments into test documents and structured errors.
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
///
/// Errors are `(path, message)` pairs carrying the bare message (no path
/// prefix); the print site formats `{path}: {message}`.
fn expand_test_paths(args: &[PathBuf]) -> (Vec<PathBuf>, Vec<(PathBuf, String)>) {
    let mut documents = Vec::new();
    let mut errors = Vec::new();
    let mut seen = HashSet::new();

    for arg in args {
        if arg.is_dir() {
            let mut found = Vec::new();
            collect_test_documents(arg, &mut found, &mut errors);
            found.sort_by(|a, b| a.as_os_str().cmp(b.as_os_str()));
            if found.is_empty() {
                errors.push((arg.clone(), "no test documents found".to_string()));
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
/// Unreadable entries push a `(path, message)` error pair naming the path.
fn collect_test_documents(
    dir: &Path,
    found: &mut Vec<PathBuf>,
    errors: &mut Vec<(PathBuf, String)>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            errors.push((dir.to_path_buf(), e.to_string()));
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                errors.push((dir.to_path_buf(), e.to_string()));
                continue;
            }
        };
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(e) => {
                errors.push((path.clone(), e.to_string()));
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
/// classes mix: any parse-error or misuse class ⇒ 2, else any failed ⇒ 1,
/// else 0.
///
/// `config.filter_files` narrows the expanded set before any document is
/// read (glob semantics: `*` does not cross `/`); `config.filter_endpoints`
/// narrows file-admitted documents to those whose `expects` keys contain a
/// given name. A filter set with no surviving document is misuse: a stderr
/// error naming the filters, exit 2. `config.junit` writes a JUnit report
/// after the summary line; a write failure is a stderr message and exit 2.
/// An empty config behaves exactly like the historical `run_tests`.
pub async fn run_tests_full(
    config: &TestRunConfig,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> TestRunSummary {
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut had_parse_error = false;
    let mut had_misuse = false;
    let mut any_survivor = false;

    let (documents, expansion_errors) = expand_test_paths(&config.files);
    // Every expansion error becomes one `ExpansionReport` (name = displayed
    // path, error = bare message) consumed by the JUnit writer after the loop.
    let mut expansion_reports: Vec<junit::ExpansionReport> = Vec::new();
    for (path, message) in &expansion_errors {
        had_parse_error = true;
        let _ = writeln!(err, "{}: {message}", path.display());
        expansion_reports.push(junit::ExpansionReport {
            name: path.display().to_string(),
            error: message.clone(),
        });
    }

    // File filter: applied after expansion, before any document is read.
    // A document is admitted iff its ENTIRE displayed-path string matches
    // any pattern; `*` does not cross `/` (require_literal_separator).
    let any_filter = !config.filter_files.is_empty() || !config.filter_endpoints.is_empty();
    let mut admitted: Vec<&PathBuf> = Vec::new();
    if config.filter_files.is_empty() {
        admitted.extend(documents.iter());
    } else {
        let options = glob::MatchOptions {
            require_literal_separator: true,
            ..glob::MatchOptions::new()
        };
        for path in &documents {
            let displayed = path.display().to_string();
            if config
                .filter_files
                .iter()
                .any(|pattern| pattern.matches_with(&displayed, options))
            {
                admitted.push(path);
            }
        }
    }

    let mut doc_reports: Vec<junit::DocReport> = Vec::new();
    for path in admitted {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) => {
                had_parse_error = true;
                any_survivor = true;
                let _ = writeln!(err, "{}: {e}", path.display());
                doc_reports.push(junit::DocReport {
                    path: path.clone(),
                    rows: Vec::new(),
                    doc_error: Some(e.to_string()),
                });
                continue;
            }
        };
        let doc = match parse_test_document(&text) {
            Ok(doc) => doc,
            Err(e) => {
                had_parse_error = true;
                any_survivor = true;
                let _ = writeln!(err, "{}: {e}", path.display());
                doc_reports.push(junit::DocReport {
                    path: path.clone(),
                    rows: Vec::new(),
                    doc_error: Some(e.to_string()),
                });
                continue;
            }
        };
        // Endpoint filter: run iff `expects` keys contain any given
        // name; filtered-out documents produce no rows, no counts, no
        // DocReport.
        if !config.filter_endpoints.is_empty()
            && !config
                .filter_endpoints
                .iter()
                .any(|name| doc.expects.contains_key(name))
        {
            continue;
        }
        any_survivor = true;
        if let Some(stubs) = doc.repository_stubs() {
            let pairs: Vec<String> = stubs
                .stub_pairs()
                .iter()
                .map(|(kind, name)| format!("{kind}={name}"))
                .collect();
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
            doc_reports.push(junit::DocReport {
                path: path.clone(),
                rows: Vec::new(),
                doc_error: Some(doc_error),
            });
            continue;
        }
        for er in &result.endpoint_results {
            match &er.outcome {
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
        doc_reports.push(junit::DocReport {
            path: path.clone(),
            rows: result.endpoint_results,
            doc_error: None,
        });
    }
    if any_filter && !any_survivor {
        // A non-survivor is a document that was not file-admitted, or parsed
        // but excluded by the endpoint filter; read/parse failures of
        // file-admitted documents count as survivors.
        had_misuse = true;
        let _ = writeln!(err, "{}", filter_misuse_message(config));
    }

    let mut exit_code = if had_parse_error || had_misuse {
        2
    } else if failed > 0 {
        1
    } else {
        0
    };
    let _ = writeln!(out, "{passed} passed, {failed} failed");
    // JUnit report: written on exit-0/1/2 runs alike, after the human
    // summary line. A write failure is stderr + exit 2 (raise only).
    if let Some(path) = &config.junit
        && let Err(e) = junit::write_report(path, &expansion_reports, &doc_reports)
    {
        let _ = writeln!(err, "failed to write {}: {e}", path.display());
        if exit_code < 2 {
            exit_code = 2;
        }
    }
    TestRunSummary {
        exit_code,
        passed,
        failed,
    }
}

/// Render the zero-survivors misuse error naming the given filters.
///
/// Lists only the kinds that were given, values space-separated in CLI
/// order: `no test documents matched --filter-file {f1} {f2}
/// --filter-endpoint {e1}`.
fn filter_misuse_message(config: &TestRunConfig) -> String {
    let mut message = String::from("no test documents matched");
    if !config.filter_files.is_empty() {
        message.push_str(" --filter-file");
        for pattern in &config.filter_files {
            message.push(' ');
            message.push_str(pattern.as_str());
        }
    }
    if !config.filter_endpoints.is_empty() {
        message.push_str(" --filter-endpoint");
        for name in &config.filter_endpoints {
            message.push(' ');
            message.push_str(name);
        }
    }
    message
}

/// Run every test document in CLI argument order, sequentially, with no
/// filters and no JUnit report (the historical entry point).
pub async fn run_tests(
    files: &[PathBuf],
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> TestRunSummary {
    let config = TestRunConfig {
        files: files.to_vec(),
        ..Default::default()
    };
    run_tests_full(&config, out, err).await
}
