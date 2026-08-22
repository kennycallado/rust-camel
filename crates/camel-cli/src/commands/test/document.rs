//! Declarative mock test document: model, parsing, and validation.
//!
//! A `*.test.yaml` sidecar declares one executable test: a route source
//! (`routeFiles` or `routeFilesFromRoot` reference OR inline `routes`,
//! exactly one), optional `direct:` inputs, non-empty `expects` keyed by
//! `mock:` URIs, and an optional `settle` quiet window (`0 < settle <= 5s`). Unknown fields are
//! rejected (`deny_unknown_fields`); input bodies are restricted to string,
//! object, and array forms — null, boolean, and number scalars are document
//! errors.
//!
//! Spec: openspec/changes/mock-declarative-testkit (design D1).

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use noyalib::compat::serde_yaml;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};

/// Expects keys are `mock:` URIs; validation strips the scheme so downstream
/// code addresses mock endpoints by bare name (`mock:result` -> `result`).
const MOCK_SCHEME_PREFIX: &str = "mock:";
/// Inputs deliver synchronously through `direct:` endpoints only (v1).
const DIRECT_SCHEME_PREFIX: &str = "direct:";
/// Sentinel prefix the body deserializer embeds in serde errors; the token
/// after it is the compact JSON rendering of the rejected scalar.
const BODY_SCALAR_SENTINEL: &str = "unsupported body scalar: ";
/// Upper bound for the `settle` quiet window.
const SETTLE_MAX: Duration = Duration::from_secs(5);

/// Parsed `*.test.yaml` document. `expects` keys are normalized to bare mock
/// endpoint names during [`parse_test_document`] validation.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TestDocument {
    /// Route files to load, relative to the document's directory.
    pub route_files: Option<Vec<String>>,
    /// Route files to load, resolved against the nearest ancestor
    /// `Camel.toml` directory (the project root).
    pub route_files_from_root: Option<Vec<String>>,
    /// Inline route definitions (same schema as route files).
    pub routes: Option<serde_yaml::Value>,
    /// Optional inputs; omitted means the routes must self-start (timer).
    #[serde(default)]
    pub inputs: Vec<TestInput>,
    /// Mandatory expectations, keyed by `mock:` URI before normalization.
    #[serde(default)]
    pub expects: BTreeMap<String, ExpectSet>,
    /// Raw `settle` string (humantime format, e.g. `"500ms"`).
    pub settle: Option<String>,
    /// Parsed settle window, populated during validation.
    #[serde(skip)]
    pub settle_parsed: Option<Duration>,
}

impl TestDocument {
    /// Parsed `settle` window, if the document declares a valid one.
    pub fn settle_duration(&self) -> Option<Duration> {
        self.settle_parsed
    }
}

/// One input delivery to a `direct:` endpoint.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TestInput {
    /// Target endpoint URI; must start with `direct:`.
    pub to: String,
    /// Body restricted to string (`Text`) or object/array (`Json`) forms.
    #[serde(default, deserialize_with = "deserialize_option_input_body")]
    pub body: Option<InputBody>,
    /// Optional headers attached to the input message.
    pub headers: Option<HashMap<String, serde_json::Value>>,
}

/// Accepted input body forms. Construction only via the deserializer match —
/// null/boolean/number scalars are rejected there with the sentinel error.
#[derive(Debug)]
pub enum InputBody {
    /// YAML string body.
    Text(String),
    /// YAML object or array body, carried as JSON.
    Json(serde_json::Value),
}

/// Expectations for one mock endpoint.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExpectSet {
    /// Exact received count; mutually exclusive with `minCount`.
    pub count: Option<usize>,
    /// Minimum received count; mutually exclusive with `count`.
    pub min_count: Option<usize>,
    /// Ordered expected bodies.
    pub bodies: Option<Vec<String>>,
    /// Expected headers.
    pub headers: Option<HashMap<String, serde_json::Value>>,
}

/// Maps a deserialized JSON value to [`InputBody`], rejecting scalars with a
/// message whose `unsupported body scalar: ` prefix is the classification
/// protocol used by [`parse_test_document`].
fn input_body_from_value(value: serde_json::Value) -> Result<Option<InputBody>, String> {
    match &value {
        serde_json::Value::String(s) => Ok(Some(InputBody::Text(s.clone()))),
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
            Ok(Some(InputBody::Json(value)))
        }
        scalar => Err(format!("{BODY_SCALAR_SENTINEL}{scalar}")),
    }
}

/// Field-level deserializer for the optional `body` field. Deserializes a
/// plain `serde_json::Value` (NOT `Option<Value>`): a missing field never
/// reaches this helper (`#[serde(default)]` supplies `None`), while an
/// explicit `body: null` arrives as `Value::Null` and is rejected — serde's
/// default `Option` handling would otherwise treat explicit null identically
/// to a missing field.
fn deserialize_option_input_body<'de, D>(deserializer: D) -> Result<Option<InputBody>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    input_body_from_value(value).map_err(D::Error::custom)
}

/// Validation and parse errors for test documents.
#[derive(Debug)]
pub enum TestDocError {
    /// Malformed YAML or a type mismatch at the serde layer.
    Yaml(String),
    /// `deny_unknown_fields` rejection.
    UnknownField(String),
    /// The declared route source keys (`routeFiles`, `routeFilesFromRoot`,
    /// `routes`) number zero or more than one; `present` lists the declared
    /// keys and is empty when none is declared.
    RouteSourceConflict { present: Vec<&'static str> },
    /// `routeFilesFromRoot` is declared but no `Camel.toml` exists in any
    /// ancestor directory of the document. Raised during runner route
    /// resolution, not during parsing.
    NoProjectRoot { doc_dir: String },
    /// `expects` is missing or empty.
    ExpectsEmpty,
    /// An `expects` key lacks the required `mock:` scheme.
    ExpectKeyMissingScheme { key: String },
    /// One `expects` entry sets both `count` and `minCount`.
    CountAndMinCount(String),
    /// `settle` failed to parse or falls outside `0 < settle <= 5s`.
    SettleOutOfRange(String),
    /// An input `to` target lacks the required `direct:` scheme.
    UnsupportedInputScheme { target: String },
    /// A body scalar (null/boolean/number) is not a supported body form.
    UnsupportedBodyScalar(String),
}

impl fmt::Display for TestDocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Yaml(raw) => write!(f, "invalid test document: {raw}"),
            Self::UnknownField(raw) => write!(f, "unknown field in test document: {raw}"),
            Self::RouteSourceConflict { present } => {
                if present.is_empty() {
                    write!(
                        f,
                        "exactly one route source (`routeFiles`, `routeFilesFromRoot`, \
                         or `routes`) is required"
                    )
                } else {
                    write!(
                        f,
                        "route sources {} are mutually exclusive; exactly one route \
                         source is required",
                        present
                            .iter()
                            .map(|key| format!("`{key}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            }
            Self::NoProjectRoot { doc_dir } => write!(
                f,
                "NoProjectRoot: routeFilesFromRoot requires a Camel.toml in an ancestor \
                 directory of {doc_dir}; none was found."
            ),
            Self::ExpectsEmpty => write!(f, "expects must declare at least one mock: endpoint"),
            Self::ExpectKeyMissingScheme { key } => {
                write!(f, "expects key `{key}` must start with `mock:`")
            }
            Self::CountAndMinCount(endpoint) => write!(
                f,
                "expects entry `{endpoint}` must not set both count and minCount"
            ),
            Self::SettleOutOfRange(raw) => {
                write!(
                    f,
                    "settle `{raw}` out of range: must satisfy 0 < settle <= 5s"
                )
            }
            Self::UnsupportedInputScheme { target } => {
                write!(f, "input target `{target}` must start with `direct:`")
            }
            Self::UnsupportedBodyScalar(raw) => write!(
                f,
                "unsupported body scalar `{raw}`: only string, object, and array bodies are supported"
            ),
        }
    }
}

impl std::error::Error for TestDocError {}

/// Classifies a noyalib (serde_yaml compat) error text. The body-scalar
/// sentinel is extracted first — it originates inside the field deserializer
/// and must not be swallowed by the generic branches. The scalar token is the
/// first whitespace-delimited word after the sentinel, which strips any
/// location suffix the compat layer appends.
fn classify_yaml_error(raw: &str) -> TestDocError {
    if let Some((_, after)) = raw.split_once(BODY_SCALAR_SENTINEL) {
        let scalar = after.split_whitespace().next().unwrap_or_default();
        return TestDocError::UnsupportedBodyScalar(scalar.to_string());
    }
    if raw.contains("unknown field") {
        return TestDocError::UnknownField(raw.to_string());
    }
    TestDocError::Yaml(raw.to_string())
}

/// Parses and validates a `*.test.yaml` document. Validation order:
/// (a) exactly one route source (`routeFiles`, `routeFilesFromRoot`, or
/// `routes`); (b) non-empty `expects`; (c) every `expects` key uses the
/// `mock:` scheme (then the map is rebuilt with bare endpoint names);
/// (d) `count`/`minCount` exclusivity; (e) `settle` range; (f) every
/// input targets `direct:`.
pub fn parse_test_document(text: &str) -> Result<TestDocument, TestDocError> {
    let mut doc = serde_yaml::from_str::<TestDocument>(text)
        .map_err(|e| classify_yaml_error(&e.to_string()))?;

    // (a) Exactly one route source: collect the declared keys and require
    // the count to be one — zero or several is a conflict.
    let mut present: Vec<&'static str> = Vec::new();
    if doc.route_files.is_some() {
        present.push("routeFiles");
    }
    if doc.route_files_from_root.is_some() {
        present.push("routeFilesFromRoot");
    }
    if doc.routes.is_some() {
        present.push("routes");
    }
    if present.len() != 1 {
        return Err(TestDocError::RouteSourceConflict { present });
    }
    // (b) `expects` is mandatory and non-empty.
    if doc.expects.is_empty() {
        return Err(TestDocError::ExpectsEmpty);
    }
    // (c) Every key must be a `mock:` URI.
    for key in doc.expects.keys() {
        if !key.starts_with(MOCK_SCHEME_PREFIX) {
            return Err(TestDocError::ExpectKeyMissingScheme { key: key.clone() });
        }
    }
    // Normalize to bare endpoint names, rejecting count+minCount combos (d).
    let raw_expects = std::mem::take(&mut doc.expects);
    for (key, set) in raw_expects {
        let bare = key[MOCK_SCHEME_PREFIX.len()..].to_string();
        if set.count.is_some() && set.min_count.is_some() {
            return Err(TestDocError::CountAndMinCount(bare));
        }
        doc.expects.insert(bare, set);
    }
    // (e) `settle`: humantime string with 0 < settle <= 5s.
    if let Some(raw) = doc.settle.clone() {
        let parsed = humantime::parse_duration(&raw)
            .map_err(|_| TestDocError::SettleOutOfRange(raw.clone()))?;
        if parsed.is_zero() || parsed > SETTLE_MAX {
            return Err(TestDocError::SettleOutOfRange(raw));
        }
        doc.settle_parsed = Some(parsed);
    }
    // (f) Inputs deliver through `direct:` endpoints only.
    for input in &doc.inputs {
        if !input.to.starts_with(DIRECT_SCHEME_PREFIX) {
            return Err(TestDocError::UnsupportedInputScheme {
                target: input.to.clone(),
            });
        }
    }
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err_of(yaml: &str) -> TestDocError {
        match parse_test_document(yaml) {
            Ok(doc) => panic!("document should fail to parse, got: {doc:?}"),
            Err(e) => e,
        }
    }

    #[test]
    fn valid_reference_doc_parses() {
        let yaml = r#"
routeFiles: [config/routes.yaml]
expects:
  mock:result:
    count: 3
"#;
        let doc = parse_test_document(yaml).expect("valid document should parse"); // allow-unwrap
        assert_eq!(
            doc.route_files,
            Some(vec!["config/routes.yaml".to_string()])
        );
        assert!(doc.routes.is_none());
        // Key normalized: `mock:` prefix stripped to bare endpoint name.
        let set = doc
            .expects
            .get("result")
            .expect("normalized key `result` present"); // allow-unwrap
        assert_eq!(set.count, Some(3));
        assert!(!doc.expects.contains_key("mock:result"));
    }

    #[test]
    fn valid_inline_routes_parses() {
        let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
    to: "mock:result"
expects:
  mock:result:
    minCount: 2
"#;
        let doc = parse_test_document(yaml).expect("valid document should parse"); // allow-unwrap
        assert!(doc.route_files.is_none());
        assert!(doc.routes.is_some());
        let set = doc
            .expects
            .get("result")
            .expect("normalized key `result` present"); // allow-unwrap
        assert_eq!(set.min_count, Some(2));
    }

    #[test]
    fn unknown_field_rejected() {
        let yaml = r#"
bogus: 1
routes:
  - id: r1
    from: "direct:start"
expects:
  mock:result:
    count: 1
"#;
        let err = err_of(yaml);
        assert!(
            matches!(err, TestDocError::UnknownField(ref msg) if msg.contains("bogus")),
            "expected UnknownField naming `bogus`, got: {err:?}"
        );
    }

    #[test]
    fn empty_expects_rejected() {
        let explicit = r#"
routes:
  - id: r1
    from: "timer:tick"
expects: {}
"#;
        assert!(
            matches!(err_of(explicit), TestDocError::ExpectsEmpty),
            "explicit empty expects must be rejected"
        );
        // No `expects` key at all: serde default yields an empty map that
        // reaches validation.
        let missing = r#"
routeFiles: [config/routes.yaml]
"#;
        assert!(
            matches!(err_of(missing), TestDocError::ExpectsEmpty),
            "missing expects must be rejected"
        );
    }

    #[test]
    fn bare_expect_key_rejected() {
        let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
expects:
  result:
    count: 1
"#;
        assert!(matches!(
            err_of(yaml),
            TestDocError::ExpectKeyMissingScheme { ref key } if key == "result"
        ));
    }

    #[test]
    fn route_source_conflict_rejected() {
        let both = r#"
routeFiles: [config/routes.yaml]
routes:
  - id: r1
    from: "direct:start"
expects:
  mock:result:
    count: 1
"#;
        assert!(matches!(
            err_of(both),
            TestDocError::RouteSourceConflict { .. }
        ));
        // Neither source present is the same conflict.
        let neither = r#"
expects:
  mock:result:
    count: 1
"#;
        assert!(matches!(
            err_of(neither),
            TestDocError::RouteSourceConflict { .. }
        ));
    }

    #[test]
    fn route_files_from_root_parses() {
        let yaml = r#"
routeFilesFromRoot: [config/routes.yaml]
expects:
  mock:result:
    count: 1
"#;
        let doc = parse_test_document(yaml).expect("valid document should parse"); // allow-unwrap
        assert_eq!(
            doc.route_files_from_root,
            Some(vec!["config/routes.yaml".to_string()])
        );
        assert!(doc.route_files.is_none());
        assert!(doc.routes.is_none());
    }

    #[test]
    fn route_files_from_root_plus_route_files_rejected() {
        let yaml = r#"
routeFiles: [config/routes.yaml]
routeFilesFromRoot: [config/routes.yaml]
expects:
  mock:result:
    count: 1
"#;
        let err = err_of(yaml);
        let msg = err.to_string();
        assert!(
            matches!(
                err,
                TestDocError::RouteSourceConflict { ref present }
                    if *present == ["routeFiles", "routeFilesFromRoot"]
            ),
            "expected RouteSourceConflict listing routeFiles + routeFilesFromRoot, got: {err:?}"
        );
        assert!(
            msg.contains("routeFiles") && msg.contains("routeFilesFromRoot"),
            "display must name both keys, got: {msg}"
        );
    }

    #[test]
    fn route_files_from_root_plus_routes_rejected() {
        let yaml = r#"
routeFilesFromRoot: [config/routes.yaml]
routes:
  - id: r1
    from: "direct:start"
expects:
  mock:result:
    count: 1
"#;
        let err = err_of(yaml);
        let msg = err.to_string();
        assert!(
            matches!(
                err,
                TestDocError::RouteSourceConflict { ref present }
                    if *present == ["routeFilesFromRoot", "routes"]
            ),
            "expected RouteSourceConflict listing routeFilesFromRoot + routes, got: {err:?}"
        );
        assert!(
            msg.contains("routeFilesFromRoot") && msg.contains("routes"),
            "display must name both keys, got: {msg}"
        );
    }

    #[test]
    fn no_route_source_rejected() {
        let yaml = r#"
expects:
  mock:result:
    count: 1
"#;
        let err = err_of(yaml);
        let msg = err.to_string();
        assert!(
            matches!(
                err,
                TestDocError::RouteSourceConflict { ref present } if present.is_empty()
            ),
            "expected RouteSourceConflict with empty present, got: {err:?}"
        );
        assert!(
            msg.contains("exactly one") && msg.contains("required"),
            "display must state exactly one route source is required, got: {msg}"
        );
    }

    #[test]
    fn all_three_route_sources_rejected() {
        let yaml = r#"
routeFiles: [config/routes.yaml]
routeFilesFromRoot: [config/routes.yaml]
routes:
  - id: r1
    from: "direct:start"
expects:
  mock:result:
    count: 1
"#;
        let err = err_of(yaml);
        assert!(
            matches!(
                err,
                TestDocError::RouteSourceConflict { ref present }
                    if *present == ["routeFiles", "routeFilesFromRoot", "routes"]
            ),
            "expected RouteSourceConflict listing all three keys, got: {err:?}"
        );
    }

    #[test]
    fn route_files_and_routes_still_rejected() {
        let yaml = r#"
routeFiles: [config/routes.yaml]
routes:
  - id: r1
    from: "direct:start"
expects:
  mock:result:
    count: 1
"#;
        let err = err_of(yaml);
        let msg = err.to_string();
        assert!(
            matches!(
                err,
                TestDocError::RouteSourceConflict { ref present }
                    if *present == ["routeFiles", "routes"]
            ),
            "expected RouteSourceConflict listing routeFiles + routes, got: {err:?}"
        );
        assert!(
            msg.contains("routeFiles") && msg.contains("routes"),
            "display must name both keys, got: {msg}"
        );
    }

    #[test]
    fn count_and_min_count_rejected() {
        let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
expects:
  mock:result:
    count: 1
    minCount: 2
"#;
        assert!(matches!(
            err_of(yaml),
            TestDocError::CountAndMinCount(ref endpoint) if endpoint == "result"
        ));
    }

    #[test]
    fn settle_zero_rejected() {
        let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
expects:
  mock:result:
    count: 1
settle: "0ms"
"#;
        assert!(
            matches!(err_of(yaml), TestDocError::SettleOutOfRange(ref raw) if raw == "0ms"),
            "settle 0ms must be out of range"
        );
    }

    #[test]
    fn settle_over_5s_rejected() {
        let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
expects:
  mock:result:
    count: 1
settle: "10s"
"#;
        assert!(
            matches!(err_of(yaml), TestDocError::SettleOutOfRange(ref raw) if raw == "10s"),
            "settle 10s must be out of range"
        );
    }

    #[test]
    fn settle_boundaries_accepted() {
        fn doc_with_settle(raw: &str) -> String {
            format!(
                r#"
routes:
  - id: r1
    from: "direct:start"
expects:
  mock:result:
    count: 1
settle: "{raw}"
"#
            )
        }
        let doc = parse_test_document(&doc_with_settle("1ms")).expect("1ms is inside the range"); // allow-unwrap
        assert_eq!(doc.settle_duration(), Some(Duration::from_millis(1)));
        let doc =
            parse_test_document(&doc_with_settle("5s")).expect("5s is the inclusive upper bound"); // allow-unwrap
        assert_eq!(doc.settle_duration(), Some(Duration::from_secs(5)));
    }

    #[test]
    fn non_direct_input_rejected() {
        let yaml = r#"
routes:
  - id: r1
    from: "direct:start"
inputs:
  - to: "seda:q"
    body: x
expects:
  mock:result:
    count: 1
"#;
        assert!(matches!(
            err_of(yaml),
            TestDocError::UnsupportedInputScheme { ref target } if target == "seda:q"
        ));
    }

    #[test]
    fn body_scalars_rejected() {
        fn doc_with_body(body: &str) -> String {
            format!(
                r#"
routes:
  - id: r1
    from: "direct:start"
inputs:
  - to: "direct:start"
    body: {body}
expects:
  mock:result:
    count: 1
"#
            )
        }
        for (raw, expected) in [("null", "null"), ("true", "true"), ("7", "7")] {
            let err = err_of(&doc_with_body(raw));
            assert!(
                matches!(err, TestDocError::UnsupportedBodyScalar(ref s) if s == expected),
                "body `{raw}` must surface UnsupportedBodyScalar(\"{expected}\"), got: {err:?}"
            );
        }
    }

    #[test]
    fn body_forms_accepted() {
        fn doc_with_body(body: &str) -> String {
            format!(
                r#"
routes:
  - id: r1
    from: "direct:start"
inputs:
  - to: "direct:start"
    body: {body}
expects:
  mock:result:
    count: 1
"#
            )
        }
        let doc = parse_test_document(&doc_with_body("x")).expect("string body parses"); // allow-unwrap
        assert!(matches!(
            doc.inputs[0].body,
            Some(InputBody::Text(ref s)) if s == "x"
        ));
        let doc = parse_test_document(&doc_with_body("{a: 1}")).expect("object body parses"); // allow-unwrap
        assert!(matches!(
            doc.inputs[0].body,
            Some(InputBody::Json(ref v)) if v.get("a") == Some(&serde_json::json!(1))
        ));
        let doc = parse_test_document(&doc_with_body("[1, 2]")).expect("array body parses"); // allow-unwrap
        assert!(matches!(
            doc.inputs[0].body,
            Some(InputBody::Json(ref v)) if *v == serde_json::json!([1, 2])
        ));
        // A missing body key is legal (None), unlike an explicit null.
        let no_body = r#"
routes:
  - id: r1
    from: "direct:start"
inputs:
  - to: "direct:start"
expects:
  mock:result:
    count: 1
"#;
        let doc = parse_test_document(no_body).expect("input without body parses"); // allow-unwrap
        assert!(doc.inputs[0].body.is_none());
    }
}
