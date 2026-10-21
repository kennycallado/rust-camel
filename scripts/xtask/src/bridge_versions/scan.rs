//! Contextual bridge-version detection for the live documentation set.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use walkdir::WalkDir;

use super::Finding;

/// Findings from scanning the live documentation set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanReport {
    pub findings: Vec<Finding>,
}

/// Scan the live set under `root` for bridge-related semantic versions.
pub fn scan_live_set(root: &Path) -> Result<ScanReport, String> {
    let mut paths = BTreeSet::new();

    collect_paths(
        root,
        &root.join("crates"),
        |path| file_name(path) == Some("README.md"),
        &mut paths,
    )?;
    collect_paths(
        root,
        &root.join("docs/src"),
        |path| extension(path).as_deref() == Some("md"),
        &mut paths,
    )?;
    collect_paths(root, &root.join("examples"), has_scan_extension, &mut paths)?;
    collect_paths(
        root,
        &root.join("bridges"),
        |path| file_name(path) == Some("README.md"),
        &mut paths,
    )?;

    let mut findings = Vec::new();
    for path in paths {
        scan_path(root, &path, &mut findings)?;
    }

    Ok(ScanReport { findings })
}

fn collect_paths(
    root: &Path,
    base: &Path,
    accept: impl Fn(&Path) -> bool,
    out: &mut BTreeSet<PathBuf>,
) -> Result<(), String> {
    if !base.is_dir() {
        return Ok(());
    }

    for entry in WalkDir::new(base) {
        let entry = entry.map_err(|error| format!("walk {}: {error}", base.display()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if is_excluded(root, path) || !accept(path) {
            continue;
        }
        out.insert(path.to_path_buf());
    }

    Ok(())
}

fn is_excluded(root: &Path, path: &Path) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    if relative
        .components()
        .any(|component| component.as_os_str() == "target")
    {
        return true;
    }

    let name = file_name(path).unwrap_or_default();
    if name == "Cargo.lock" || name.starts_with("CHANGELOG") {
        return true;
    }

    if relative.starts_with("docs/adr")
        || relative.starts_with("docs/archived")
        || relative.starts_with("docs/reviews")
        || relative.starts_with("openspec/changes/archive")
    {
        return true;
    }

    if name == "Cargo.toml"
        && relative
            .components()
            .any(|component| component.as_os_str().to_string_lossy().contains("wasm"))
    {
        return true;
    }

    false
}

fn file_name(path: &Path) -> Option<&str> {
    path.file_name().and_then(|name| name.to_str())
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
}

fn has_scan_extension(path: &Path) -> bool {
    matches!(
        extension(path).as_deref(),
        Some(
            "md" | "toml"
                | "yaml"
                | "yml"
                | "json"
                | "rs"
                | "sh"
                | "properties"
                | "txt"
                | "conf"
                | "env"
        )
    )
}

fn scan_path(root: &Path, path: &Path, findings: &mut Vec<Finding>) -> Result<(), String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    let relative = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let bridge_topic = is_bridge_topic(&relative);

    let lines: Vec<&str> = contents.lines().collect();
    let sections = sections(&lines);

    let paragraphs = match extension(path).as_deref() {
        Some("rs") => rust_paragraphs(&lines),
        Some("md") => markdown_paragraphs(&lines),
        _ => line_paragraphs(&lines),
    };

    for paragraph in &paragraphs {
        scan_paragraph(
            path,
            &relative,
            paragraph,
            &sections,
            bridge_topic,
            findings,
        );
    }

    Ok(())
}

fn sections(lines: &[&str]) -> Vec<Option<String>> {
    let mut current = None;
    let mut result = Vec::with_capacity(lines.len());

    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with('[')
            && trimmed.ends_with(']')
            && !trimmed.contains(char::is_whitespace)
        {
            current = Some(trimmed.to_string());
        }
        result.push(current.clone());
    }

    result
}

#[derive(Debug, Clone)]
struct Line {
    number: usize,
    text: String,
}

#[derive(Debug, Clone)]
struct Paragraph {
    lines: Vec<Line>,
    table: bool,
}

fn markdown_paragraphs(lines: &[&str]) -> Vec<Paragraph> {
    let mut paragraphs = Vec::new();
    let mut current = Vec::new();

    for (index, raw) in lines.iter().enumerate() {
        let number = index + 1;
        if raw.trim().is_empty() {
            flush_paragraph(&mut current, &mut paragraphs);
        } else if raw.trim_start().starts_with('|') {
            flush_paragraph(&mut current, &mut paragraphs);
            paragraphs.push(Paragraph {
                lines: vec![Line {
                    number,
                    text: (*raw).to_string(),
                }],
                table: true,
            });
        } else {
            current.push(Line {
                number,
                text: (*raw).to_string(),
            });
        }
    }

    flush_paragraph(&mut current, &mut paragraphs);
    paragraphs
}

fn rust_paragraphs(lines: &[&str]) -> Vec<Paragraph> {
    let mut paragraphs = Vec::new();
    let mut comment = Vec::new();
    let mut previous_comment_line = None;

    for (index, raw) in lines.iter().enumerate() {
        let number = index + 1;
        if let Some(text) = strip_comment(raw) {
            let consecutive = previous_comment_line == Some(number - 1);
            if !consecutive {
                flush_paragraph(&mut comment, &mut paragraphs);
            }
            comment.push(Line {
                number,
                text: text.to_string(),
            });
            previous_comment_line = Some(number);
        } else {
            flush_paragraph(&mut comment, &mut paragraphs);
            previous_comment_line = None;
            if !raw.trim().is_empty() {
                paragraphs.push(Paragraph {
                    lines: vec![Line {
                        number,
                        text: (*raw).to_string(),
                    }],
                    table: raw.trim_start().starts_with('|'),
                });
            }
        }
    }

    flush_paragraph(&mut comment, &mut paragraphs);
    paragraphs
}

fn line_paragraphs(lines: &[&str]) -> Vec<Paragraph> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, raw)| !raw.trim().is_empty())
        .map(|(index, raw)| Paragraph {
            lines: vec![Line {
                number: index + 1,
                text: (*raw).to_string(),
            }],
            table: raw.trim_start().starts_with('|'),
        })
        .collect()
}

fn flush_paragraph(current: &mut Vec<Line>, paragraphs: &mut Vec<Paragraph>) {
    if !current.is_empty() {
        paragraphs.push(Paragraph {
            lines: std::mem::take(current),
            table: false,
        });
    }
}

fn strip_comment(raw: &str) -> Option<&str> {
    let trimmed = raw.trim_start();
    trimmed
        .strip_prefix("//!")
        .or_else(|| trimmed.strip_prefix("///"))
        .or_else(|| trimmed.strip_prefix("//"))
}

fn scan_paragraph(
    path: &Path,
    relative: &str,
    paragraph: &Paragraph,
    sections: &[Option<String>],
    bridge_topic: bool,
    findings: &mut Vec<Finding>,
) {
    let (content, offsets) = paragraph_content(&paragraph.lines);
    if content.is_empty() {
        return;
    }

    let token_matches: Vec<_> = token_re()
        .find_iter(&content)
        .filter(|matched| valid_token(&content, matched.start(), matched.end()))
        .collect();
    if token_matches.is_empty() {
        return;
    }

    let ranges: Vec<(usize, usize)> = token_matches
        .iter()
        .map(|matched| (matched.start(), matched.end()))
        .collect();
    let sentences = if paragraph.table {
        vec![(0, content.len())]
    } else {
        sentence_spans(&content, &ranges)
    };

    for matched in token_matches {
        let version = matched.as_str();
        let start = matched.start();
        let (line_index, line_number) =
            line_at(&offsets, start).unwrap_or((paragraph.lines.len().saturating_sub(1), 0));
        let Some(line) = paragraph.lines.get(line_index) else {
            continue;
        };
        let section = sections
            .get(line_number.saturating_sub(1))
            .and_then(Option::as_deref);

        let structured =
            component_pin(line, section, version) || bridge_version_literal(&line.text, version);
        let (sentence_start, sentence) =
            sentence_at(&content, &sentences, start).unwrap_or((start, ""));
        let sentence_start_relative = start - sentence_start;
        let sentence_end_relative = matched.end() - sentence_start;
        let product = product_exception(sentence, sentence_start_relative);

        let subjects = subject_ranges(sentence);
        let subject = subject_near(&subjects, sentence_start_relative, sentence_end_relative);
        let has_subject = !subjects.is_empty();
        let context = context_re().is_match(sentence);
        let contextual =
            !product && (subject || (has_subject && context) || (bridge_topic && context));

        if structured || contextual {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: line.number,
                token: version.to_string(),
                detail: format!(
                    "bridge version '{version}' must not appear in live bridge documentation; use {}",
                    super::remedy_for(relative)
                ),
            });
        }
    }
}

fn paragraph_content(lines: &[Line]) -> (String, Vec<(usize, usize, usize)>) {
    let mut content = String::new();
    let mut offsets = Vec::with_capacity(lines.len());

    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            content.push('\n');
        }
        offsets.push((content.len(), index, line.number));
        content.push_str(&line.text);
    }

    (content, offsets)
}

fn line_at(offsets: &[(usize, usize, usize)], position: usize) -> Option<(usize, usize)> {
    offsets
        .iter()
        .rev()
        .find(|(start, _, _)| *start <= position)
        .map(|(_, index, number)| (*index, *number))
}

fn sentence_spans(content: &str, tokens: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let bytes = content.as_bytes();
    let mut spans = Vec::new();
    let mut start = 0;

    for (index, byte) in bytes.iter().enumerate() {
        if !matches!(*byte, b'.' | b'!' | b'?' | b';') {
            continue;
        }
        if *byte == b'.'
            && tokens
                .iter()
                .any(|(from, to)| index >= *from && index < *to)
        {
            continue;
        }
        let boundary = bytes.get(index + 1).is_none_or(u8::is_ascii_whitespace);
        if boundary {
            spans.push((start, index + 1));
            start = index + 1;
        }
    }

    if start < bytes.len() {
        spans.push((start, bytes.len()));
    }

    spans
}

fn sentence_at<'a>(
    content: &'a str,
    spans: &[(usize, usize)],
    position: usize,
) -> Option<(usize, &'a str)> {
    spans
        .iter()
        .find(|(start, end)| position >= *start && position < *end)
        .map(|(start, end)| (*start, &content[*start..*end]))
}

fn valid_token(content: &str, start: usize, end: usize) -> bool {
    let bytes = content.as_bytes();
    if start > 0 && (bytes[start - 1].is_ascii_digit() || bytes[start - 1] == b'.') {
        return false;
    }

    if let Some(next) = bytes.get(end) {
        if next.is_ascii_digit() {
            return false;
        }
        if *next == b'.'
            && bytes
                .get(end + 1)
                .is_some_and(|after| after.is_ascii_alphanumeric() || *after == b'_')
        {
            return false;
        }
    }

    true
}

fn component_pin(line: &Line, section: Option<&str>, version: &str) -> bool {
    let Some(section) = section else {
        return false;
    };
    let component_section = matches!(section, "[components.cxf]" | "[components.jms]")
        || section.starts_with("[components.xml");
    if !component_section {
        return false;
    }

    version_assignment_re()
        .captures_iter(&line.text)
        .filter_map(|captures| captures.get(1).or_else(|| captures.get(2)))
        .any(|value| value.as_str() == version)
}

/// Match a TOML `version = "..."` or `version = '...'` assignment with any
/// surrounding whitespace. The captured value is compared to the discovered
/// semantic-version token, so only a quoted valid semantic version counts.
fn version_assignment_re() -> &'static Regex {
    static ASSIGNMENT: OnceLock<Regex> = OnceLock::new();
    ASSIGNMENT.get_or_init(|| {
        Regex::new(r#"\bversion\s*=\s*(?:"([^"]*)"|'([^']*)')"#)
            .expect("valid version assignment regex") // allow-unwrap
    })
}

fn bridge_version_literal(line: &str, version: &str) -> bool {
    let Some(position) = line.find("bridge_version") else {
        return false;
    };
    let rest = line[position + "bridge_version".len()..].trim_start();
    let Some(rest) = rest.strip_prefix(':').or_else(|| rest.strip_prefix('=')) else {
        return false;
    };
    let rest = rest.trim_start();
    let Some(rest) = rest.strip_prefix('"') else {
        return false;
    };
    let Some(end) = rest.find('"') else {
        return false;
    };

    &rest[..end] == version
}

fn subject_ranges(sentence: &str) -> Vec<(usize, usize)> {
    subject_re()
        .find_iter(sentence)
        .filter(|matched| {
            if !matched.as_str().eq_ignore_ascii_case("bridge") || matched.start() < 6 {
                return true;
            }
            !sentence.as_bytes()[matched.start() - 6..matched.start()]
                .eq_ignore_ascii_case(b"camel-")
        })
        .map(|matched| (matched.start(), matched.end()))
        .collect()
}

fn subject_near(subjects: &[(usize, usize)], start: usize, end: usize) -> bool {
    subjects.iter().any(|(subject_start, subject_end)| {
        if *subject_end <= start {
            start - *subject_end <= 32
        } else if *subject_start >= end {
            *subject_start - end <= 32
        } else {
            true
        }
    })
}

fn product_exception(sentence: &str, token_start: usize) -> bool {
    product_re()
        .find_iter(sentence)
        .any(|matched| matched.end() <= token_start && token_start - matched.end() <= 32)
}

fn is_bridge_topic(relative: &str) -> bool {
    matches!(
        relative,
        "crates/components/camel-cxf/README.md"
            | "crates/components/camel-jms/README.md"
            | "crates/components/camel-xslt/README.md"
            | "crates/components/camel-validator/README.md"
            | "crates/components/camel-xj/README.md"
            | "docs/src/components/ws-soap.md"
            | "docs/src/components/jms.md"
            | "docs/src/components/xml-transform.md"
            | "docs/src/components/validator.md"
            | "docs/src/services/bridge.md"
    ) || (relative.starts_with("bridges/") && relative.ends_with("/README.md"))
}

fn token_re() -> &'static Regex {
    static TOKEN: OnceLock<Regex> = OnceLock::new();
    TOKEN.get_or_init(|| {
        Regex::new(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?")
            .expect("valid token regex") // allow-unwrap
    })
}

fn subject_re() -> &'static Regex {
    static SUBJECT: OnceLock<Regex> = OnceLock::new();
    SUBJECT.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:cxf|jms|xml)-bridge(?:-v[0-9.]+)?\b|\bBRIDGE_VERSION\b|\bbridge_version\b|\bbridge\b",
        )
        .expect("valid subject regex") // allow-unwrap
    })
}

fn context_re() -> &'static Regex {
    static CONTEXT: OnceLock<Regex> = OnceLock::new();
    CONTEXT.get_or_init(|| {
        Regex::new(
            r"(?i)version|since|breaking|needs|requires|supports|or newer|or later|minimum|pinned|release pin|>=",
        )
        .expect("valid context regex") // allow-unwrap
    })
}

fn product_re() -> &'static Regex {
    static PRODUCT: OnceLock<Regex> = OnceLock::new();
    PRODUCT.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:Quarkus|GraalVM|ActiveMQ|Artemis|MQTT|OpenAPI|cargo|rustc|JDK|Saxon|Xerces|camel-bridge)\b",
        )
        .expect("valid product regex") // allow-unwrap
    })
}

#[cfg(test)]
mod tests {
    use super::scan_live_set;
    use tempfile::TempDir;

    fn write_file(root: &std::path::Path, relative_path: &str, contents: &str) {
        let path = root.join(relative_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create fixture directory");
        }
        std::fs::write(path, contents).expect("write fixture file");
    }

    fn fixture(files: &[(&str, &str)]) -> TempDir {
        let dir = TempDir::new().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("bridges")).expect("create bridges directory");
        std::fs::create_dir_all(dir.path().join("crates")).expect("create crates directory");
        for (relative_path, contents) in files {
            write_file(dir.path(), relative_path, contents);
        }
        dir
    }

    fn scan(files: &[(&str, &str)]) -> (TempDir, super::ScanReport) {
        let dir = fixture(files);
        let report = scan_live_set(dir.path()).expect("fixture scan succeeds");
        (dir, report)
    }

    fn tokens(report: &super::ScanReport) -> Vec<&str> {
        report
            .findings
            .iter()
            .map(|finding| finding.token.as_str())
            .collect()
    }

    #[test]
    fn component_block_pin_is_rejected() {
        let (_dir, report) = scan(&[(
            "examples/cxf-example/Camel.toml",
            "[components.cxf]\nversion = \"0.8.4\"\n",
        )]);

        assert_eq!(tokens(&report), vec!["0.8.4"]);
        assert_eq!(report.findings[0].line, 2);
    }

    #[test]
    fn component_pins_accept_toml_spacing_and_quotes() {
        let (_dir, report) = scan(&[
            (
                "examples/cxf-example/Camel.toml",
                "[components.cxf]\nversion=\"0.8.4\"\n",
            ),
            (
                "examples/cxf-tabs-example/Camel.toml",
                "[components.cxf]\nversion\t=\t\"0.8.4\"\n",
            ),
            (
                "examples/cxf-single-example/Camel.toml",
                "[components.cxf]\nversion = '0.8.4'\n",
            ),
            (
                "examples/xml-example/Camel.toml",
                "[components.xml]\nversion=\"0.8.4\"\n",
            ),
            (
                "examples/xml-validator-example/Camel.toml",
                "[components.xml_validator]\nversion = '0.8.4'\n",
            ),
            (
                "examples/unrelated-example/Camel.toml",
                "[components.unrelated]\nversion = \"0.8.4\"\n",
            ),
        ]);

        let expected = [
            "examples/cxf-example/Camel.toml",
            "examples/cxf-tabs-example/Camel.toml",
            "examples/cxf-single-example/Camel.toml",
            "examples/xml-example/Camel.toml",
            "examples/xml-validator-example/Camel.toml",
        ];
        assert_eq!(
            report.findings.len(),
            expected.len(),
            "{:?}",
            report.findings
        );

        for relative in expected {
            let finding = report
                .findings
                .iter()
                .find(|finding| finding.path.ends_with(relative))
                .unwrap_or_else(|| panic!("missing finding for {relative}: {:?}", report.findings));
            assert_eq!(finding.token, "0.8.4", "{relative}");
            assert_eq!(finding.line, 2, "{relative}");
        }

        assert!(
            report.findings.iter().all(|finding| !finding
                .path
                .ends_with("examples/unrelated-example/Camel.toml")),
            "unrelated component section produced a finding: {:?}",
            report.findings
        );
    }

    #[test]
    fn history_and_minimum_prose_are_rejected() {
        let (_dir, report) = scan(&[(
            "crates/components/camel-jms/README.md",
            "since bridge 0.8.0\nBreaking in 0.8.0\nneeds 0.8.4 or newer\n",
        )]);

        assert_eq!(tokens(&report), vec!["0.8.0", "0.8.0", "0.8.4"]);
    }

    #[test]
    fn sentence_final_version_and_ip_boundaries() {
        let (_dir, report) = scan(&[(
            "crates/components/camel-xj/README.md",
            "Since bridge 0.8.0.\n127.0.0.1:8080\n0.0.0.0:9090\n",
        )]);

        assert_eq!(tokens(&report), vec!["0.8.0"]);
    }

    #[test]
    fn paragraph_join_reports_cross_line_version() {
        let (_dir, report) = scan(&[(
            "crates/components/camel-jms/README.md",
            "The bridge\nrequires 0.8.4.\n",
        )]);

        assert_eq!(tokens(&report), vec!["0.8.4"]);
        assert_eq!(report.findings[0].line, 2);
    }

    #[test]
    fn sentence_punctuation_keeps_tokens_intact() {
        let (_dir, report) = scan(&[(
            "crates/components/camel-jms/README.md",
            "Since bridge 0.8.0. A later bridge needs 0.8.4;\n",
        )]);

        assert_eq!(tokens(&report), vec!["0.8.0", "0.8.4"]);
    }

    #[test]
    fn product_exception_is_position_bound() {
        let (_dir, report) = scan(&[(
            "docs/src/components/jms.md",
            "| ActiveMQ Classic 5.18.3 | bridge cap |\n| ActiveMQ Artemis 2.36.0 | bridge cap |\nthe bridge 0.8.0 acks ActiveMQ deliveries\n",
        )]);

        assert_eq!(tokens(&report), vec!["0.8.0"]);
    }

    #[test]
    fn product_never_overrides_component_block_or_bridge_version() {
        let (_dir, report) = scan(&[
            (
                "examples/cxf-example/Camel.toml",
                "[components.cxf]\n# Saxon version = \"0.8.4\"\n",
            ),
            (
                "crates/components/camel-xj/README.md",
                "// Saxon bridge_version: \"1.0.0\"\n",
            ),
        ]);

        let mut found = tokens(&report);
        found.sort_unstable();
        assert_eq!(found, vec!["0.8.4", "1.0.0"]);
    }

    #[test]
    fn explicit_bridge_binary_and_tag_terms() {
        let (_dir, report) = scan(&[(
            "bridges/probe/README.md",
            "cxf-bridge-v0.8.4\njms-bridge 0.8.0\n",
        )]);

        assert_eq!(tokens(&report), vec!["0.8.4", "0.8.0"]);
    }

    #[test]
    fn bridge_subjects_are_case_insensitive_whole_words() {
        let (_dir, report) = scan(&[(
            "crates/components/camel-jms/README.md",
            "Since bridge 0.8.0\n**Breaking in 0.8.0**\ncamel-bridge v0.59.0\n",
        )]);

        assert_eq!(report.findings.len(), 2);
        assert!(
            report
                .findings
                .iter()
                .all(|finding| !finding.token.contains("0.59.0"))
        );
    }

    #[test]
    fn bridge_version_field_is_rejected() {
        let (_dir, report) = scan(&[(
            "crates/components/camel-xj/README.md",
            "bridge_version: \"1.0.0\"\n",
        )]);

        assert_eq!(tokens(&report), vec!["1.0.0"]);
        assert!(
            report.findings[0]
                .detail
                .contains("camel_xslt::BRIDGE_VERSION")
        );
    }

    #[test]
    fn rust_executable_bridge_version_literal_is_reported() {
        let (_dir, report) = scan(&[(
            "examples/probe/src/lib.rs",
            "let cfg = XjComponentConfig { bridge_version: \"1.0.0\".to_string(), bridge_binary_path: None };\n",
        )]);

        assert_eq!(tokens(&report), vec!["1.0.0"]);
        assert_eq!(report.findings[0].line, 1);
    }

    #[test]
    fn rust_bridge_version_constant_expression_is_legal() {
        let (_dir, report) = scan(&[(
            "examples/probe/src/lib.rs",
            "let cfg = XjComponentConfig { bridge_version: camel_xslt::BRIDGE_VERSION.to_string(), bridge_binary_path: None };\n",
        )]);

        assert!(report.findings.is_empty(), "{:?}", report.findings);
    }

    #[test]
    fn bridge_readme_numeric_versions_are_rejected() {
        let (_dir, report) = scan(&[(
            "bridges/cxf/README.md",
            "pinned 0.8.1\nrelease pin is 0.8.1\n",
        )]);

        assert_eq!(tokens(&report), vec!["0.8.1", "0.8.1"]);
    }

    #[test]
    fn rust_comment_bridge_versions_are_reported() {
        let (_dir, report) = scan(&[(
            "examples/jms-container-example/src/main.rs",
            "//! Since bridge 0.8.0\n\n// it bridge 0.8.0 drops headers\n",
        )]);

        assert_eq!(tokens(&report), vec!["0.8.0", "0.8.0"]);
        assert_eq!(report.findings[0].line, 1);
        assert_eq!(report.findings[1].line, 3);
    }

    #[test]
    fn rust_comment_paragraph_joins_lines() {
        let (_dir, report) = scan(&[(
            "examples/jms-container-example/src/main.rs",
            "//! The JMS bridge\n//! requires 0.8.4.\n",
        )]);

        assert_eq!(tokens(&report), vec!["0.8.4"]);
        assert_eq!(report.findings[0].line, 2);
    }

    #[test]
    fn bridge_topic_context_word_reports_without_subject() {
        let (_dir, report) = scan(&[(
            "crates/components/camel-jms/README.md",
            "**Breaking in 0.8.0:**\n",
        )]);

        assert_eq!(tokens(&report), vec!["0.8.0"]);
    }

    #[test]
    fn unrelated_versions_and_ips_stay_legal() {
        let (_dir, report) = scan(&[
            ("crates/foo/README.md", "since 0.16.0\n"),
            ("examples/foo/Cargo.toml", "version = \"0.1.0\"\n"),
            ("examples/wasm-guest/Cargo.toml", "version = \"0.8.1\"\n"),
            (
                "examples/foo/README.md",
                "minijinja/2.21.0\nMQTT 3.1.1\nOpenAPI 3.0.3\n127.0.0.1:8080\n",
            ),
        ]);

        assert!(report.findings.is_empty(), "{:?}", report.findings);
    }

    #[test]
    fn history_directories_stay_exempt() {
        let (_dir, report) = scan(&[
            ("docs/adr/0001.md", "bridge 0.8.0\n"),
            ("docs/archived/old.md", "bridge 0.8.0\n"),
            ("docs/reviews/old.md", "bridge 0.8.0\n"),
            ("openspec/changes/archive/old/spec.md", "bridge 0.8.0\n"),
            ("CHANGELOG.md", "bridge 0.8.0\n"),
            ("examples/probe/README.md", "bridge 0.8.0\n"),
        ]);

        assert_eq!(report.findings.len(), 1);
        assert!(
            report.findings[0]
                .path
                .ends_with("examples/probe/README.md")
        );
    }

    #[test]
    fn later_sentence_bridge_subject_proximity() {
        let (_dir, report) = scan(&[(
            "crates/demo/README.md",
            "Introduction lasting at least forty characters. The bridge 0.8.0 drops headers.\n",
        )]);

        assert_eq!(tokens(&report), vec!["0.8.0"]);
        assert_eq!(report.findings[0].line, 1);
    }

    #[test]
    fn later_sentence_product_exception() {
        let (_dir, report) = scan(&[(
            "bridges/cxf/README.md",
            "Introduction lasting at least forty characters. ActiveMQ version 5.18.3.\n",
        )]);

        assert!(report.findings.is_empty(), "{:?}", report.findings);
    }
}
