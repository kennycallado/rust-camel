use std::path::Path;

use camel_integration_test::ScenarioDocument;
use noyalib::compat::serde_yaml;

use super::parse_test_document;
use super::{BODY_SCALAR_SENTINEL, MATCHER_SENTINEL, TestDocError, TestDocument};

/// One parsed test document in whichever vocabulary it declares: the
/// unit-tier mock vocabulary or the integration-tier `scenario:`
/// vocabulary. Dispatch sniffs the `scenario:` key, so a document can
/// never be parsed by both parsers.
pub(crate) enum ParsedDocument {
    /// A unit-tier document (`inputs` / `expects` / `intercepts`).
    Unit(Box<TestDocument>),
    /// A full-tier scenario document (`scenario:` section).
    Scenario(Box<ScenarioDocument>),
}

/// Whether the text declares a top-level `execute:` section (the
/// `camel job` vocabulary). A third classified section, mutually
/// exclusive with both test vocabularies.
fn declares_execute(text: &str) -> bool {
    serde_yaml::from_str::<serde_yaml::Value>(text)
        .ok()
        .and_then(|value| value.get("execute").map(|_| true))
        .unwrap_or(false)
}

/// Whether the text declares a top-level `scenario:` section. Text that
/// does not deserialize at all carries no scenario section; the
/// unit-tier parser then produces the document error.
fn declares_scenario(text: &str) -> bool {
    serde_yaml::from_str::<serde_yaml::Value>(text)
        .ok()
        .and_then(|value| value.get("scenario").map(|_| true))
        .unwrap_or(false)
}

/// Parses one test document in whichever vocabulary it declares: a
/// `scenario:` section routes to the scenario parser (which re-reads
/// the file from `path` and enforces the suffix and mixing rules);
/// anything else routes to the unit-tier parser. An `execute:` section
/// is the third classified vocabulary — a job document, rejected with
/// a pointer to `camel job`. Errors are rendered `Display` strings —
/// every parse failure of all parsers is a load-time, exit-2 class.
pub(crate) fn parse_document(path: &Path, text: &str) -> Result<ParsedDocument, String> {
    if declares_execute(text) {
        Err(format!(
            "{}: document declares an execute: section (the `camel job` vocabulary); \
              camel test does not run job documents",
            path.display()
        ))
    } else if declares_scenario(text) {
        camel_integration_test::parse_scenario_document(path)
            .map(|scenario| ParsedDocument::Scenario(Box::new(scenario)))
            .map_err(|e| e.to_string())
    } else {
        parse_test_document(text)
            .map(|doc| ParsedDocument::Unit(Box::new(doc)))
            .map_err(|e| e.to_string())
    }
}

/// Classifies a noyalib (serde_yaml compat) error text. The body-scalar and
/// matcher sentinels are extracted first — they originate inside field
/// deserializers and must not be swallowed by the generic branches. The
/// scalar token is the first whitespace-delimited word after the sentinel,
/// which strips any location suffix the compat layer appends; the matcher
/// message spans multiple words, so a trailing ` at line ...` suffix is cut
/// explicitly.
pub(super) fn classify_yaml_error(raw: &str) -> TestDocError {
    if let Some((_, after)) = raw.split_once(BODY_SCALAR_SENTINEL) {
        let scalar = after.split_whitespace().next().unwrap_or_default();
        return TestDocError::UnsupportedBodyScalar(scalar.to_string());
    }
    if let Some((_, after)) = raw.split_once(MATCHER_SENTINEL) {
        let msg = after.split_once(" at line ").map_or(after, |(msg, _)| msg);
        return TestDocError::InvalidMatcher(msg.to_string());
    }
    if raw.contains("unknown field") {
        return TestDocError::UnknownField(raw.to_string());
    }
    TestDocError::Yaml(raw.to_string())
}
