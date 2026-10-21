//! Bridge version registry and registered Rust constant comparison.

pub mod bump;
pub mod registry;
pub mod scan;

use std::path::{Path, PathBuf};

use registry::{Bridge, Registry};

/// One bridge-version lint finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: PathBuf,
    pub line: usize,
    pub token: String,
    pub detail: String,
}

fn remedy_for(relative_path: &str) -> &'static str {
    let path = relative_path.replace('\\', "/");
    if path.contains("cxf") || path.contains("ws-soap") {
        "camel_cxf::BRIDGE_VERSION"
    } else if path.contains("jms") {
        "camel_jms::BRIDGE_VERSION"
    } else if path.contains("validator") {
        "camel_validator::BRIDGE_VERSION"
    } else if path.contains("xslt") || path.contains("xj") || path.contains("xml") {
        "camel_xslt::BRIDGE_VERSION"
    } else {
        "the bridge release page"
    }
}

/// Result of the complete bridge-version lint, including tag validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintOutcome {
    pub findings: Vec<Finding>,
    pub tag_error: Option<String>,
}

/// Run registry, constant, live-set, and optional tag checks.
pub fn lint(root: &Path, tag: Option<&str>) -> Result<LintOutcome, String> {
    if let Some(tag) = tag {
        let (bridge, version) = match parse_tag(tag) {
            Ok(parsed) => parsed,
            Err(message) => {
                return Ok(LintOutcome {
                    findings: Vec::new(),
                    tag_error: Some(message),
                });
            }
        };

        let registry = match registry::parse_registry(&root.join("bridges/versions.toml")) {
            Ok(registry) => registry,
            Err(findings) => {
                return Ok(LintOutcome {
                    findings,
                    tag_error: None,
                });
            }
        };

        let expected = registry.version(bridge);
        if version != expected {
            return Ok(LintOutcome {
                findings: Vec::new(),
                tag_error: Some(format!(
                    "tag {tag} != bridges/versions.toml {}={expected} — run 'cargo xtask bump-bridge {} {version}', commit, delete and re-push the tag",
                    bridge.key(),
                    bridge.key(),
                )),
            });
        }

        return collect_findings(root, &registry);
    }

    let registry = match registry::parse_registry(&root.join("bridges/versions.toml")) {
        Ok(registry) => registry,
        Err(findings) => {
            return Ok(LintOutcome {
                findings,
                tag_error: None,
            });
        }
    };

    collect_findings(root, &registry)
}

fn collect_findings(root: &Path, registry: &Registry) -> Result<LintOutcome, String> {
    let mut findings = lint_registry_constants(root, registry)?;
    findings.extend(scan::scan_live_set(root)?.findings);
    Ok(LintOutcome {
        findings,
        tag_error: None,
    })
}

fn parse_tag(tag: &str) -> Result<(Bridge, String), String> {
    for (prefix, bridge) in [
        ("cxf-bridge-v", Bridge::Cxf),
        ("jms-bridge-v", Bridge::Jms),
        ("xml-bridge-v", Bridge::Xml),
    ] {
        if let Some(version) = tag.strip_prefix(prefix) {
            return Ok((bridge, version.to_string()));
        }
    }

    Err(format!(
        "unknown tag '{tag}': accepted forms are cxf-bridge-v*, jms-bridge-v*, xml-bridge-v*"
    ))
}

/// Compare every positive-registry `BRIDGE_VERSION` declaration with
/// `registry`.
///
/// A missing file or missing declaration, a duplicate declaration, and a value
/// mismatch each produce a finding. Read errors on a target file are reported
/// as a missing declaration for that path.
pub fn lint_registry_constants(root: &Path, registry: &Registry) -> Result<Vec<Finding>, String> {
    let const_re = regex::Regex::new(r#"^pub const BRIDGE_VERSION: &str = "([^"]+)";$"#)
        .map_err(|e| format!("invalid BRIDGE_VERSION regex: {e}"))?;

    let mut findings = Vec::new();

    for bridge in [Bridge::Cxf, Bridge::Jms, Bridge::Xml] {
        let expected = registry.version(bridge);

        for relative_path in bridge.const_paths() {
            let path = root.join(relative_path);
            let contents = match std::fs::read_to_string(&path) {
                Ok(contents) => contents,
                Err(_) => {
                    findings.push(Finding {
                        path,
                        line: 0,
                        token: "BRIDGE_VERSION".to_string(),
                        detail: format!("missing BRIDGE_VERSION in {relative_path}"),
                    });
                    continue;
                }
            };

            let declarations: Vec<(usize, String)> = contents
                .lines()
                .enumerate()
                .filter_map(|(index, line)| {
                    const_re
                        .captures(line.trim())
                        .map(|captures| (index + 1, captures[1].to_string()))
                })
                .collect();

            match declarations.as_slice() {
                [] => findings.push(Finding {
                    path,
                    line: 0,
                    token: "BRIDGE_VERSION".to_string(),
                    detail: format!("missing BRIDGE_VERSION in {relative_path}"),
                }),
                [(line, actual)] => {
                    if actual != expected {
                        findings.push(Finding {
                            path,
                            line: *line,
                            token: actual.clone(),
                            detail: format!(
                                "bridge version mismatch: registry key '{}' expected '{}' actual '{}'; replace with {} or the bridge release page",
                                bridge.key(),
                                expected,
                                actual,
                                remedy_for(relative_path),
                            ),
                        });
                    }
                }
                [(_, _), (second_line, _), ..] => findings.push(Finding {
                    path,
                    line: *second_line,
                    token: "BRIDGE_VERSION".to_string(),
                    detail: format!("duplicate BRIDGE_VERSION in {relative_path}"),
                }),
            }
        }
    }

    Ok(findings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const CXF: &str = "crates/components/camel-cxf/src/lib.rs";
    const JMS: &str = "crates/components/camel-jms/src/lib.rs";
    const XSLT: &str = "crates/components/camel-xslt/src/lib.rs";
    const VALIDATOR: &str = "crates/components/camel-validator/src/lib.rs";

    fn registry() -> Registry {
        Registry {
            cxf: "0.8.4".to_string(),
            jms: "0.8.4".to_string(),
            xml: "0.8.1".to_string(),
        }
    }

    fn write_file(root: &Path, relative_path: &str, contents: &str) {
        let path = root.join(relative_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create fixture directory");
        }
        std::fs::write(path, contents).expect("write fixture file");
    }

    fn constant(version: &str) -> String {
        format!("pub const BRIDGE_VERSION: &str = \"{version}\";\n")
    }

    fn four_matching_constants(root: &Path) {
        write_file(root, CXF, &constant("0.8.4"));
        write_file(root, JMS, &constant("0.8.4"));
        write_file(root, XSLT, &constant("0.8.1"));
        write_file(root, VALIDATOR, &constant("0.8.1"));
    }

    #[test]
    fn constants_match_registry_in_fixture() {
        let dir = TempDir::new().expect("tempdir");
        four_matching_constants(dir.path());

        let findings =
            lint_registry_constants(dir.path(), &registry()).expect("fixture scan succeeds");

        assert!(findings.is_empty(), "unexpected findings: {findings:?}");
    }

    #[test]
    fn constant_drift_reports_exact_evidence() {
        let dir = TempDir::new().expect("tempdir");
        write_file(dir.path(), CXF, &constant("0.8.3"));
        write_file(dir.path(), JMS, &constant("0.8.4"));
        write_file(dir.path(), XSLT, &constant("0.8.1"));
        write_file(dir.path(), VALIDATOR, &constant("0.8.1"));

        let findings =
            lint_registry_constants(dir.path(), &registry()).expect("fixture scan succeeds");

        assert_eq!(findings.len(), 1);
        let finding = &findings[0];
        assert_eq!(finding.path, dir.path().join(CXF));
        assert_eq!(finding.line, 1);
        assert!(finding.detail.contains("cxf"));
        assert!(finding.detail.contains("0.8.4"));
        assert!(finding.detail.contains("0.8.3"));
    }

    #[test]
    fn both_xml_constants_are_checked() {
        let dir = TempDir::new().expect("tempdir");
        write_file(dir.path(), CXF, &constant("0.8.4"));
        write_file(dir.path(), JMS, &constant("0.8.4"));
        write_file(dir.path(), XSLT, &constant("0.8.1"));
        write_file(dir.path(), VALIDATOR, &constant("0.8.0"));

        let findings =
            lint_registry_constants(dir.path(), &registry()).expect("fixture scan succeeds");

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].path, dir.path().join(VALIDATOR));
        assert!(findings[0].detail.contains("xml"));
    }

    #[test]
    fn missing_constant_is_reported() {
        let dir = TempDir::new().expect("tempdir");
        write_file(dir.path(), CXF, &constant("0.8.4"));
        write_file(dir.path(), JMS, &constant("0.8.4"));
        write_file(dir.path(), XSLT, &constant("0.8.1"));
        write_file(dir.path(), VALIDATOR, "// no bridge version here\n");

        let findings =
            lint_registry_constants(dir.path(), &registry()).expect("fixture scan succeeds");

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].path, dir.path().join(VALIDATOR));
        assert!(findings[0].detail.contains("missing BRIDGE_VERSION"));
    }

    #[test]
    fn duplicate_constant_is_reported() {
        let dir = TempDir::new().expect("tempdir");
        write_file(
            dir.path(),
            CXF,
            &format!("{}{}", constant("0.8.4"), constant("0.8.4")),
        );
        write_file(dir.path(), JMS, &constant("0.8.4"));
        write_file(dir.path(), XSLT, &constant("0.8.1"));
        write_file(dir.path(), VALIDATOR, &constant("0.8.1"));

        let findings =
            lint_registry_constants(dir.path(), &registry()).expect("fixture scan succeeds");

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].path, dir.path().join(CXF));
        assert!(findings[0].detail.contains("duplicate BRIDGE_VERSION"));
    }

    fn lint_fixture() -> TempDir {
        let dir = TempDir::new().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("bridges")).expect("create bridges directory");
        std::fs::create_dir_all(dir.path().join("crates")).expect("create crates directory");
        dir
    }

    fn write_registry(root: &Path, cxf: &str, jms: &str, xml: &str) {
        write_file(
            root,
            "bridges/versions.toml",
            &format!("cxf = \"{cxf}\"\njms = \"{jms}\"\nxml = \"{xml}\"\n"),
        );
    }

    #[test]
    fn matching_tag_passes() {
        let dir = lint_fixture();
        write_registry(dir.path(), "0.8.4", "0.8.4", "0.8.1");
        four_matching_constants(dir.path());

        let outcome = lint(dir.path(), Some("cxf-bridge-v0.8.4")).expect("lint succeeds");

        assert!(outcome.tag_error.is_none(), "{:?}", outcome.tag_error);
        assert!(outcome.findings.is_empty(), "{:?}", outcome.findings);
    }

    #[test]
    fn mismatched_tag_prints_exact_diagnostic() {
        let dir = lint_fixture();
        write_registry(dir.path(), "0.8.4", "0.8.4", "0.8.1");
        four_matching_constants(dir.path());

        let outcome = lint(dir.path(), Some("xml-bridge-v0.8.2")).expect("lint succeeds");

        assert_eq!(
            outcome.tag_error.as_deref(),
            Some(
                "tag xml-bridge-v0.8.2 != bridges/versions.toml xml=0.8.1 — run 'cargo xtask bump-bridge xml 0.8.2', commit, delete and re-push the tag"
            )
        );
        assert!(outcome.findings.is_empty(), "{:?}", outcome.findings);
    }

    #[test]
    fn unknown_tag_prefix_lists_accepted_forms() {
        let dir = lint_fixture();
        write_registry(dir.path(), "0.8.4", "0.8.4", "0.8.1");

        let outcome = lint(dir.path(), Some("foo-bridge-v1.2.3")).expect("lint succeeds");

        let error = outcome.tag_error.expect("unknown prefix is rejected");
        assert!(error.contains("cxf-bridge-v*"));
        assert!(error.contains("jms-bridge-v*"));
        assert!(error.contains("xml-bridge-v*"));
    }

    #[test]
    fn matching_tag_does_not_bypass_constant_drift() {
        let dir = lint_fixture();
        write_registry(dir.path(), "0.8.4", "0.8.4", "0.8.1");
        write_file(dir.path(), CXF, &constant("0.8.3"));
        write_file(dir.path(), JMS, &constant("0.8.4"));
        write_file(dir.path(), XSLT, &constant("0.8.1"));
        write_file(dir.path(), VALIDATOR, &constant("0.8.1"));

        let outcome = lint(dir.path(), Some("cxf-bridge-v0.8.4")).expect("lint succeeds");

        assert!(outcome.tag_error.is_none(), "{:?}", outcome.tag_error);
        assert_eq!(outcome.findings.len(), 1);
        assert_eq!(outcome.findings[0].path, dir.path().join(CXF));
    }

    #[test]
    fn matching_tag_does_not_bypass_live_finding() {
        let dir = lint_fixture();
        write_registry(dir.path(), "0.8.4", "0.8.4", "0.8.1");
        four_matching_constants(dir.path());
        write_file(
            dir.path(),
            "examples/cxf-example/Camel.toml",
            "[components.cxf]\nversion = \"0.8.4\"\n",
        );

        let outcome = lint(dir.path(), Some("cxf-bridge-v0.8.4")).expect("lint succeeds");

        assert!(outcome.tag_error.is_none(), "{:?}", outcome.tag_error);
        assert_eq!(outcome.findings.len(), 1);
        assert_eq!(outcome.findings[0].token, "0.8.4");
    }
}
