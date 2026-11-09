//! JUnit XML report writer for `camel test --junit <FILE>`.
//!
//! De-facto JUnit schema (as ingested by Jenkins/GitLab/GitHub annotations):
//! one `<testsuite>` per attempted document plus one synthetic suite per
//! expansion-level error. Row labels reuse the runner's `EndpointResult`
//! verbatim so CI annotations match the stdout PASS/FAIL lines.
//!
//! Spec: openspec/changes/test-junit-filters (design D3, D4).

use std::io::Write;
use std::path::{Path, PathBuf};

use super::declared;
use super::runner::EndpointResult;

/// One attempted test document: its path, per-endpoint rows, and optional
/// document-level error (read, parse, boot, route load, input delivery).
pub(crate) struct DocReport {
    /// Document path, rendered via `display()` in the report.
    pub path: PathBuf,
    /// Per-endpoint evaluation rows; labels are the PASS/FAIL labels verbatim.
    pub rows: Vec<EndpointResult>,
    /// Document-level error; `None` when the document ran to evaluation.
    pub doc_error: Option<String>,
    /// Derived tier annotation (`lean` / `full` / `full*` for
    /// FULL-derived unit documents); `None` when the tier could not be
    /// derived (read or parse failure).
    pub tier: Option<&'static str>,
    /// Labels a successful run would print. Empty for read/parse failures
    /// and tier-filter collisions (a collision document declares nothing).
    pub declared: Vec<String>,
}

/// One expansion-level error (unreadable directory entry, zero-document
/// directory). `name` is the path string from the error message.
pub(crate) struct ExpansionReport {
    /// Path string naming the failed expansion.
    pub name: String,
    /// Error text.
    pub error: String,
}

/// Escape `& < > " '` as XML entities and REMOVE characters XML 1.0 forbids
/// in content: control characters other than tab, LF, CR, plus the
/// non-characters U+FFFE and U+FFFF. The allowed trio passes through.
fn escape_xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            '\u{FFFE}' | '\u{FFFF}' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// First line of `text`: everything up to and excluding the first `\n`.
fn first_line(text: &str) -> &str {
    text.split('\n').next().unwrap_or(text)
}

/// Write one report line, mapping I/O errors to a path-naming `String`.
fn write_line(file: &mut std::fs::File, path: &Path, text: &str) -> Result<(), String> {
    writeln!(file, "{text}").map_err(|e| format!("{}: {e}", path.display()))
}

/// Write the JUnit report for one `camel test` run.
///
/// Root totals count every testcase across suites. Per suite, `tests` is the
/// row count plus the doc-error testcase (0 or 1) plus the skipped testcase
/// count; `failures` counts rows with an `Err` outcome; `errors` counts
/// doc-error and expansion testcases; `skipped` counts declared labels with no
/// verdict row. The `skipped` attribute renders only when the level holds at
/// least one skipped testcase, so fully evaluated suites stay byte-identical.
/// All text and attribute values pass through [`escape_xml`]. A trailing
/// newline follows `</testsuites>`.
pub(crate) fn write_report(
    path: &Path,
    expansion: &[ExpansionReport],
    reports: &[DocReport],
) -> Result<(), String> {
    let mut file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;

    struct SuiteStats {
        tests: usize,
        failures: usize,
        errors: usize,
        skipped: usize,
    }
    let suite_stats: Vec<SuiteStats> = reports
        .iter()
        .map(|report| {
            let skipped =
                declared::skipped_cases(&report.declared, &report.rows, report.doc_error.is_some())
                    .len();
            SuiteStats {
                tests: report.rows.len() + usize::from(report.doc_error.is_some()) + skipped,
                failures: report.rows.iter().filter(|r| r.outcome.is_err()).count(),
                errors: usize::from(report.doc_error.is_some()),
                skipped,
            }
        })
        .collect();
    let mut tests: usize = suite_stats.iter().map(|s| s.tests).sum();
    let failures: usize = suite_stats.iter().map(|s| s.failures).sum();
    let mut errors: usize = suite_stats.iter().map(|s| s.errors).sum();
    let skipped: usize = suite_stats.iter().map(|s| s.skipped).sum();
    tests += expansion.len();
    errors += expansion.len();

    let root_skipped_attr = if skipped > 0 {
        format!(r#" skipped="{skipped}""#)
    } else {
        String::new()
    };

    write_line(&mut file, path, r#"<?xml version="1.0" encoding="UTF-8"?>"#)?;
    write_line(
        &mut file,
        path,
        &format!(
            r#"<testsuites tests="{tests}" failures="{failures}" errors="{errors}"{root_skipped_attr}>"#
        ),
    )?;

    for (report, stats) in reports.iter().zip(&suite_stats) {
        let path_str = escape_xml(&report.path.display().to_string());
        let suite_tests = stats.tests;
        let suite_failures = stats.failures;
        let suite_errors = stats.errors;
        let suite_skipped_attr = if stats.skipped > 0 {
            format!(r#" skipped="{}""#, stats.skipped)
        } else {
            String::new()
        };
        write_line(
            &mut file,
            path,
            &format!(
                r#"<testsuite name="{path_str}" tests="{suite_tests}" failures="{suite_failures}" errors="{suite_errors}"{suite_skipped_attr}>"#
            ),
        )?;
        if let Some(tier) = report.tier {
            write_line(
                &mut file,
                path,
                &format!(r#"<properties><property name="tier" value="{tier}"/></properties>"#),
            )?;
        }
        for row in &report.rows {
            let name = escape_xml(&row.endpoint);
            match &row.outcome {
                Ok(()) => write_line(
                    &mut file,
                    path,
                    &format!(r#"<testcase name="{name}" classname="{path_str}" />"#),
                )?,
                Err(detail) => {
                    let message = escape_xml(first_line(detail));
                    let body = escape_xml(detail);
                    write_line(
                        &mut file,
                        path,
                        &format!(
                            r#"<testcase name="{name}" classname="{path_str}"><failure message="{message}">{body}</failure></testcase>"#
                        ),
                    )?;
                }
            }
        }
        if let Some(doc_error) = &report.doc_error {
            let message = escape_xml(first_line(doc_error));
            let body = escape_xml(doc_error);
            write_line(
                &mut file,
                path,
                &format!(
                    r#"<testcase name="&lt;document&gt;" classname="{path_str}"><error message="{message}">{body}</error></testcase>"#
                ),
            )?;
        }
        for (label, reason) in
            declared::skipped_cases(&report.declared, &report.rows, report.doc_error.is_some())
        {
            let name = escape_xml(label);
            let message = escape_xml(&format!("not run: {reason}"));
            write_line(
                &mut file,
                path,
                &format!(
                    r#"<testcase name="{name}" classname="{path_str}"><skipped message="{message}"/></testcase>"#
                ),
            )?;
        }
        write_line(&mut file, path, "</testsuite>")?;
    }

    for exp in expansion {
        let name = escape_xml(&exp.name);
        let message = escape_xml(first_line(&exp.error));
        let body = escape_xml(&exp.error);
        write_line(
            &mut file,
            path,
            &format!(r#"<testsuite name="{name}" tests="1" failures="0" errors="1">"#),
        )?;
        write_line(
            &mut file,
            path,
            &format!(
                r#"<testcase name="&lt;expansion&gt;"><error message="{message}">{body}</error></testcase>"#
            ),
        )?;
        write_line(&mut file, path, "</testsuite>")?;
    }

    write_line(&mut file, path, "</testsuites>")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Render one `DocReport` in its own unique tempdir and return the XML.
    fn write_report_to(report: &DocReport) -> String {
        let dir = tempfile::tempdir().expect("create tempdir"); // allow-unwrap
        let path = dir.path().join("report.xml");
        write_report(&path, &[], std::slice::from_ref(report)).expect("write report"); // allow-unwrap
        std::fs::read_to_string(&path).expect("read report") // allow-unwrap
    }

    #[test]
    fn escape_xml_escapes_five_and_strips_controls() {
        let input = "a<b>&\"'c\u{0001}\u{0007}d\te\nf\u{FFFE}\u{FFFF}";
        assert_eq!(
            escape_xml(input),
            "a&lt;b&gt;&amp;&quot;&apos;cd\te\nf",
            "five entities escaped, controls and non-characters removed"
        );
    }

    #[test]
    fn write_report_all_pass_golden() {
        let dir = tempfile::tempdir().expect("create tempdir"); // allow-unwrap
        let path = dir.path().join("report.xml");
        let report = DocReport {
            path: PathBuf::from("a.test.yaml"),
            rows: vec![EndpointResult {
                endpoint: "out".to_string(),
                outcome: Ok(()),
            }],
            doc_error: None,
            tier: Some("lean"),
            declared: Vec::new(),
        };
        write_report(&path, &[], &[report]).expect("write report"); // allow-unwrap
        let bytes = std::fs::read(&path).expect("read report"); // allow-unwrap
        let expected = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites tests=\"1\" failures=\"0\" errors=\"0\">\n<testsuite name=\"a.test.yaml\" tests=\"1\" failures=\"0\" errors=\"0\">\n<properties><property name=\"tier\" value=\"lean\"/></properties>\n<testcase name=\"out\" classname=\"a.test.yaml\" />\n</testsuite>\n</testsuites>\n";
        assert_eq!(
            String::from_utf8(bytes).expect("utf8"), // allow-unwrap
            expected,
            "all-pass report must match the golden bytes exactly"
        );
    }

    #[test]
    fn write_report_failure_doc_error_expansion_golden() {
        let dir = tempfile::tempdir().expect("create tempdir"); // allow-unwrap
        let path = dir.path().join("report.xml");
        let report_a = DocReport {
            path: PathBuf::from("a.test.yaml"),
            rows: vec![EndpointResult {
                endpoint: "<settle>".to_string(),
                outcome: Err("mismatch: line1\nline2".to_string()),
            }],
            doc_error: None,
            tier: Some("lean"),
            declared: Vec::new(),
        };
        let report_b = DocReport {
            path: PathBuf::from("b.test.yaml"),
            rows: vec![],
            doc_error: Some("boot failed\ncause".to_string()),
            tier: None,
            declared: Vec::new(),
        };
        let expansion = vec![ExpansionReport {
            name: "./empty".to_string(),
            error: "no test documents found".to_string(),
        }];
        write_report(&path, &expansion, &[report_a, report_b]).expect("write report"); // allow-unwrap
        let text = std::fs::read_to_string(&path).expect("read report"); // allow-unwrap
        assert!(
            text.contains("<testcase name=\"&lt;settle&gt;\" classname=\"a.test.yaml\"><failure message=\"mismatch: line1\">mismatch: line1\nline2</failure></testcase>"),
            "failure row must carry first-line message and full detail: {text}"
        );
        assert!(
            text.contains("<testcase name=\"&lt;document&gt;\" classname=\"b.test.yaml\"><error message=\"boot failed\">boot failed\ncause</error></testcase>"),
            "doc-error row must render as an error testcase: {text}"
        );
        assert!(
            text.contains("<testcase name=\"&lt;expansion&gt;\"><error message=\"no test documents found\">no test documents found</error></testcase>"),
            "expansion suite must hold a single error testcase: {text}"
        );
        assert!(
            text.contains("<testsuites tests=\"3\" failures=\"1\" errors=\"2\">"),
            "root totals must count every testcase: {text}"
        );
    }

    #[test]
    fn junit_settle_skips_declared_cases() {
        let report = DocReport {
            path: PathBuf::from("settle.test.yaml"),
            rows: vec![EndpointResult {
                endpoint: "<settle>".to_string(),
                outcome: Err("deadline exceeded".to_string()),
            }],
            doc_error: None,
            tier: Some("lean"),
            declared: vec!["a".to_string(), "b".to_string()],
        };
        let text = write_report_to(&report);
        assert!(
            text.contains(
                r#"<testsuite name="settle.test.yaml" tests="3" failures="1" errors="0" skipped="2">"#
            ),
            "suite must count the settle row plus both skipped cases: {text}"
        );
        assert!(
            text.contains(r#"<testsuites tests="3" failures="1" errors="0" skipped="2">"#),
            "root totals must sum the suite's skipped cases: {text}"
        );
        assert_eq!(
            text.matches(r#"<skipped message="not run: settle timeout"/>"#)
                .count(),
            2,
            "both declared labels must render as settle-timeout skips: {text}"
        );
        for label in ["a", "b"] {
            assert!(
                text.contains(&format!(
                    r#"<testcase name="{label}" classname="settle.test.yaml"><skipped message="not run: settle timeout"/></testcase>"#
                )),
                "skipped testcase must carry the document classname: {text}"
            );
        }
    }

    #[test]
    fn junit_document_error_skips_declared_cases() {
        let report = DocReport {
            path: PathBuf::from("b.test.yaml"),
            rows: vec![],
            doc_error: Some("boot failed".to_string()),
            tier: None,
            declared: vec!["a".to_string(), "b".to_string(), "c".to_string()],
        };
        let text = write_report_to(&report);
        assert!(
            text.contains(
                r#"<testsuite name="b.test.yaml" tests="4" failures="0" errors="1" skipped="3">"#
            ),
            "suite must count the doc error plus three skipped cases: {text}"
        );
        assert_eq!(
            text.matches(r#"<testcase name="&lt;document&gt;" classname="b.test.yaml">"#)
                .count(),
            1,
            "exactly one document error testcase: {text}"
        );
        assert_eq!(
            text.matches(r#"<skipped message="not run: document error"/>"#)
                .count(),
            3,
            "every declared label must render as a document-error skip: {text}"
        );
    }

    #[test]
    fn junit_early_stop_skips_unreached_actions() {
        let report = DocReport {
            path: PathBuf::from("s.test.yaml"),
            rows: vec![
                EndpointResult {
                    endpoint: "scenario[0] send".to_string(),
                    outcome: Ok(()),
                },
                EndpointResult {
                    endpoint: "scenario[1] receive".to_string(),
                    outcome: Ok(()),
                },
                EndpointResult {
                    endpoint: "scenario[2] validate".to_string(),
                    outcome: Err("boom".to_string()),
                },
            ],
            doc_error: None,
            tier: Some("full"),
            declared: vec![
                "scenario[0] send".to_string(),
                "scenario[1] receive".to_string(),
                "scenario[2] validate".to_string(),
                "scenario[3] send".to_string(),
            ],
        };
        let text = write_report_to(&report);
        assert!(
            text.contains(
                r#"<testsuite name="s.test.yaml" tests="4" failures="1" errors="0" skipped="1">"#
            ),
            "suite must count three verdict rows plus one skipped case: {text}"
        );
        assert_eq!(
            text.matches("<testcase ").count(),
            4,
            "three verdict rows plus one skipped testcase: {text}"
        );
        assert_eq!(
            text.matches(r#"<skipped message="not run: stopped after scenario[2] failure"/>"#)
                .count(),
            1,
            "only the unreached action must be skipped: {text}"
        );
        assert!(
            text.contains(
                r#"<testcase name="scenario[3] send" classname="s.test.yaml"><skipped message="not run: stopped after scenario[2] failure"/></testcase>"#
            ),
            "the skipped testcase must name the unreached action: {text}"
        );
    }

    #[test]
    fn write_report_write_failure_is_err() {
        let dir = tempfile::tempdir().expect("create tempdir"); // allow-unwrap
        let path = dir.path().join("missing").join("report.xml");
        let err = write_report(&path, &[], &[]).expect_err("write must fail"); // allow-unwrap
        assert!(err.contains("report.xml"), "err must name the path: {err}");
    }

    #[test]
    fn write_report_escapes_doc_path() {
        let dir = tempfile::tempdir().expect("create tempdir"); // allow-unwrap
        let path = dir.path().join("report.xml");
        let report = DocReport {
            path: PathBuf::from("tmp/a&b<c>.test.yaml"),
            rows: vec![EndpointResult {
                endpoint: "out".to_string(),
                outcome: Ok(()),
            }],
            doc_error: None,
            tier: Some("full"),
            declared: Vec::new(),
        };
        write_report(&path, &[], &[report]).expect("write report"); // allow-unwrap
        let text = std::fs::read_to_string(&path).expect("read report"); // allow-unwrap
        assert!(
            text.contains(r#"<property name="tier" value="full"/>"#),
            "tier property must render: {text}"
        );
        assert!(
            text.contains(r#"name="tmp/a&amp;b&lt;c&gt;.test.yaml""#),
            "suite name attribute must escape &: {text}"
        );
        assert!(
            text.contains(r#"classname="tmp/a&amp;b&lt;c&gt;.test.yaml""#),
            "classname attribute must escape path: {text}"
        );
        assert!(
            !text.contains("a&b<c>"),
            "raw path must not appear in XML: {text}"
        );
    }
}
