//! Flat bridge version registry parser.
//!
//! The registry is the single editable source of truth for bridge versions.
//! It is intentionally flat: three quoted keys and no tables, arrays, or
//! duplicate entries.

use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

use super::Finding;

/// A bridge with a registered version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bridge {
    Cxf,
    Jms,
    Xml,
}

impl Bridge {
    /// Registry key for this bridge.
    pub fn key(self) -> &'static str {
        match self {
            Bridge::Cxf => "cxf",
            Bridge::Jms => "jms",
            Bridge::Xml => "xml",
        }
    }

    /// Parse a registry or tag bridge name.
    pub fn parse(value: &str) -> Option<Bridge> {
        match value {
            "cxf" => Some(Bridge::Cxf),
            "jms" => Some(Bridge::Jms),
            "xml" => Some(Bridge::Xml),
            _ => None,
        }
    }

    /// Rust files that must declare `BRIDGE_VERSION` for this bridge.
    pub fn const_paths(self) -> &'static [&'static str] {
        match self {
            Bridge::Cxf => &["crates/components/camel-cxf/src/lib.rs"],
            Bridge::Jms => &["crates/components/camel-jms/src/lib.rs"],
            Bridge::Xml => &[
                "crates/components/camel-xslt/src/lib.rs",
                "crates/components/camel-validator/src/lib.rs",
            ],
        }
    }
}

/// Parsed flat bridge version registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    pub cxf: String,
    pub jms: String,
    pub xml: String,
}

impl Registry {
    /// Version registered for `bridge`.
    pub fn version(&self, bridge: Bridge) -> &str {
        match bridge {
            Bridge::Cxf => &self.cxf,
            Bridge::Jms => &self.jms,
            Bridge::Xml => &self.xml,
        }
    }
}

/// Semantic version accepted by the registry and by `bump-bridge`.
pub(crate) const SEMVER_PATTERN: &str = r"^\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?$";

/// Return whether `value` is a registry-supported semantic version.
pub(crate) fn is_semver(value: &str) -> bool {
    static SEMVER: OnceLock<Regex> = OnceLock::new();
    let regex = SEMVER.get_or_init(|| Regex::new(SEMVER_PATTERN).expect("valid semver regex")); // allow-unwrap
    regex.is_match(value)
}

fn finding(path: &Path, line: usize, token: &str, shape: &str) -> Finding {
    Finding {
        path: path.to_path_buf(),
        line,
        token: token.to_string(),
        detail: format!("{}:{}: {}", path.display(), line, shape),
    }
}

/// Parse `bridges/versions.toml`.
///
/// Returns every finding when the file is absent or its shape is not the flat
/// three-key registry.
pub fn parse_registry(path: &Path) -> Result<Registry, Vec<Finding>> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(_) => {
            return Err(vec![Finding {
                path: path.to_path_buf(),
                line: 0,
                token: String::new(),
                detail: format!("missing registry file: {}", path.display()),
            }]);
        }
    };

    let mut findings = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    let mut cxf = None;
    let mut jms = None;
    let mut xml = None;

    for (index, raw) in contents.lines().enumerate() {
        let line_no = index + 1;
        let line = raw.trim();

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') {
            findings.push(finding(path, line_no, line, "table header"));
            continue;
        }

        if line.contains('[') || line.contains(']') || line.contains('{') || line.contains('}') {
            findings.push(finding(path, line_no, line, "non-flat value"));
            continue;
        }

        let Some((key_raw, value_raw)) = line.split_once('=') else {
            findings.push(finding(path, line_no, line, "missing '='"));
            continue;
        };

        let key = key_raw.trim();
        let value = value_raw.trim();

        if !matches!(key, "cxf" | "jms" | "xml") {
            findings.push(finding(path, line_no, key, "unknown key"));
            continue;
        }

        let duplicate = seen.contains(&key);
        if !duplicate {
            seen.push(key);
        }

        let Some(rest) = value.strip_prefix('"') else {
            findings.push(finding(path, line_no, value, "unquoted value"));
            continue;
        };
        let Some(close) = rest.find('"') else {
            findings.push(finding(path, line_no, value, "unquoted value"));
            continue;
        };

        let inner = &rest[..close];
        let trailing = &rest[close + 1..];
        if trailing.chars().any(|c| !c.is_whitespace()) {
            findings.push(finding(path, line_no, value, "trailing content"));
            continue;
        }

        if !is_semver(inner) {
            findings.push(finding(path, line_no, inner, "invalid version"));
            continue;
        }

        if duplicate {
            findings.push(finding(path, line_no, key, "duplicate key"));
            continue;
        }

        match key {
            "cxf" => cxf = Some(inner.to_string()),
            "jms" => jms = Some(inner.to_string()),
            "xml" => xml = Some(inner.to_string()),
            _ => unreachable!("key was validated above"),
        }
    }

    for key in ["cxf", "jms", "xml"] {
        if !seen.contains(&key) {
            findings.push(finding(path, 0, key, "missing key"));
        }
    }

    match (cxf, jms, xml) {
        (Some(cxf), Some(jms), Some(xml)) if findings.is_empty() => Ok(Registry { cxf, jms, xml }),
        _ => Err(findings),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn write_registry(contents: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("versions.toml");
        std::fs::write(&path, contents).expect("write registry fixture");
        (dir, path)
    }

    #[test]
    fn registry_parses_flat_three_keys() {
        let (_dir, path) = write_registry("cxf = \"0.8.4\"\njms = \"0.8.4\"\nxml = \"0.8.1\"\n");

        let registry = parse_registry(&path).expect("flat registry parses");

        assert_eq!(registry.cxf, "0.8.4");
        assert_eq!(registry.jms, "0.8.4");
        assert_eq!(registry.xml, "0.8.1");
    }

    #[test]
    fn registry_allows_blank_and_comment_lines() {
        let (_dir, path) = write_registry(
            "\n# bridge versions\ncxf = \"0.8.4\"\njms = \"0.8.4\"\nxml = \"0.8.1\"\n",
        );

        let registry = parse_registry(&path).expect("comments and blank lines are allowed");

        assert_eq!(registry.cxf, "0.8.4");
        assert_eq!(registry.jms, "0.8.4");
        assert_eq!(registry.xml, "0.8.1");
    }

    #[test]
    fn registry_rejects_table_header() {
        let (_dir, path) =
            write_registry("cxf = \"0.8.4\"\njms = \"0.8.4\"\nxml = \"0.8.1\"\n[bridges]\n");

        let findings = parse_registry(&path).expect_err("table headers are rejected");

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, 4);
        assert!(findings[0].detail.contains("table header"));
        assert!(findings[0].detail.contains(&path.display().to_string()));
    }

    #[test]
    fn registry_rejects_unknown_key() {
        let (_dir, path) =
            write_registry("cxf = \"0.8.4\"\njms = \"0.8.4\"\nxml = \"0.8.1\"\nhttp = \"0.8.4\"\n");

        let findings = parse_registry(&path).expect_err("unknown keys are rejected");

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].token, "http");
        assert!(findings[0].detail.contains("unknown key"));
    }

    #[test]
    fn registry_rejects_duplicate_key() {
        let (_dir, path) =
            write_registry("cxf = \"0.8.4\"\njms = \"0.8.4\"\nxml = \"0.8.1\"\nxml = \"0.8.1\"\n");

        let findings = parse_registry(&path).expect_err("duplicate keys are rejected");

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].token, "xml");
        assert_eq!(findings[0].line, 4);
        assert!(findings[0].detail.contains("duplicate key"));
    }

    #[test]
    fn registry_rejects_unquoted_value() {
        let (_dir, path) = write_registry("cxf = 0.8.4\njms = \"0.8.4\"\nxml = \"0.8.1\"\n");

        let findings = parse_registry(&path).expect_err("unquoted values are rejected");

        assert_eq!(findings.len(), 1);
        assert!(findings[0].detail.contains("unquoted value"));
    }

    #[test]
    fn registry_rejects_trailing_content() {
        let (_dir, path) =
            write_registry("cxf = \"0.8.4\" # pin\njms = \"0.8.4\"\nxml = \"0.8.1\"\n");

        let findings = parse_registry(&path).expect_err("trailing content is rejected");

        assert_eq!(findings.len(), 1);
        assert!(findings[0].detail.contains("trailing content"));
    }

    #[test]
    fn registry_rejects_missing_key() {
        let (_dir, path) = write_registry("cxf = \"0.8.4\"\njms = \"0.8.4\"\n");

        let findings = parse_registry(&path).expect_err("missing keys are rejected");

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].token, "xml");
        assert!(findings[0].detail.contains("missing key"));
    }
}
