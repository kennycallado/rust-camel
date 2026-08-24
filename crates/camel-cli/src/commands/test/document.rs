//! Declarative mock test document: model, parsing, and validation.
//!
//! A `*.test.yaml` sidecar declares one executable test: a route source
//! (`routeFiles` or `routeFilesFromRoot` reference OR inline `routes`,
//! exactly one), optional `direct:` inputs (each optionally carrying an
//! `expectReply` block that asserts against the producer's reply
//! exchange), non-empty `expects` keyed by `mock:` URIs (relaxed to
//! optional when at least one input declares `expectReply`), an optional
//! `settle` quiet window (`0 < settle <= 5s`), an optional `intercepts`
//! block (real endpoint URIs mapped to mock endpoints), and an optional
//! `beans` block (declarative stub beans). Unknown fields are
//! rejected (`deny_unknown_fields`); input bodies are restricted to string,
//! object, and array forms — null, boolean, and number scalars are document
//! errors.
//!
//! Spec: openspec/changes/mock-declarative-testkit (design D1).

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use camel_core::intercept::{InterceptAction, InterceptRule, InterceptRules};
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
    /// Declarative intercept map keyed by source URI.
    pub intercepts: Option<BTreeMap<String, InterceptActionDoc>>,
    /// Parsed intercept rules, populated during validation.
    #[serde(skip)]
    intercept_rules_parsed: Option<InterceptRules>,
    /// Declarative stub beans keyed by bean name.
    pub beans: Option<BTreeMap<String, BeanDeclDoc>>,
}

impl TestDocument {
    /// Parsed `settle` window, if the document declares a valid one.
    pub fn settle_duration(&self) -> Option<Duration> {
        self.settle_parsed
    }

    /// Parsed intercept rules, if the document declares any.
    pub fn intercept_rules(&self) -> Option<InterceptRules> {
        self.intercept_rules_parsed.clone()
    }

    /// Declared stub beans, if the document declares any. Validation ran
    /// eagerly in [`parse_test_document`], so the accessor is infallible.
    pub fn bean_decls(&self) -> Option<&BTreeMap<String, BeanDeclDoc>> {
        self.beans.as_ref()
    }
}

/// Declarative stub bean: a built-in in-process processor registered before
/// routes load. `methods` omitted means the stub accepts every method the
/// routes invoke on it (resolved by the runner); `config` is kind-specific.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BeanDeclDoc {
    /// Stub behavior kind.
    pub kind: BeanKindDoc,
    /// Explicit method allowlist; omitted means wildcard.
    pub methods: Option<Vec<String>>,
    /// Kind-specific configuration (`setBody` requires `body`; `fail`
    /// accepts only `message`; `echo` accepts none).
    pub config: Option<BTreeMap<String, String>>,
}

/// Supported stub bean kinds.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum BeanKindDoc {
    /// Pass the exchange through untouched.
    Echo,
    /// Replace the input body with the configured `body` string.
    SetBody,
    /// Fail with the configured `message` (or `fail bean <name>`).
    Fail,
}

/// Declarative intercept action: exactly one of `skipTo` / `divertCopyTo`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InterceptActionDoc {
    /// Skip the original send and redirect to the target `mock:` URI.
    pub skip_to: Option<String>,
    /// Copy the exchange to the target `mock:` URI while the real send continues.
    pub divert_copy_to: Option<String>,
}

/// Expected reply assertion for one input delivery: exact-match against the
/// reply exchange the `direct:` producer returns. At least one of `body` /
/// `headers` must be present (validated in [`parse_test_document`]).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExpectReply {
    /// Reply body restricted to string (`Text`) or object/array (`Json`)
    /// forms, same rule as input bodies.
    #[serde(default, deserialize_with = "deserialize_option_input_body")]
    pub body: Option<InputBody>,
    /// Expected reply headers (exact submap of JSON values).
    pub headers: Option<HashMap<String, serde_json::Value>>,
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
    /// Optional reply assertion checked against the exchange the producer
    /// returns; absent means the reply is captured but not asserted.
    pub expect_reply: Option<ExpectReply>,
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
    /// Intercept source URI is empty.
    InterceptEmptySource,
    /// Intercept source uses the `mock:` scheme.
    InterceptMockSource { key: String },
    /// Intercept action has both or neither keys (`skipTo` / `divertCopyTo`).
    InterceptActionKeys { key: String, problem: &'static str },
    /// Intercept target is `mock:` with an empty endpoint path.
    InterceptEmptyTargetPath { key: String },
    /// Intercept target failed Stage A validation (e.g. non-`mock:` target).
    InterceptInvalid(String),
    /// A `beans:` declaration failed validation; the message carries the
    /// precise reason verbatim.
    InvalidBeans(String),
    /// An `expectReply` block failed validation; the message carries the
    /// precise reason verbatim.
    InvalidReply(String),
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
            Self::ExpectsEmpty => write!(
                f,
                "expects must declare at least one mock: endpoint unless an input declares expectReply"
            ),
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
            Self::InterceptEmptySource => write!(f, "intercept source URI must not be empty"),
            Self::InterceptMockSource { key } => {
                write!(f, "intercept source `{key}` must not start with `mock:`")
            }
            Self::InterceptActionKeys { key, problem } => write!(
                f,
                "intercept action for `{key}`: exactly one of `skipTo` or `divertCopyTo` is required (got {problem})"
            ),
            Self::InterceptEmptyTargetPath { key } => write!(
                f,
                "intercept target for `{key}` needs a mock endpoint name: `mock:` requires a non-empty endpoint path"
            ),
            Self::InterceptInvalid(msg) => write!(f, "invalid intercept: {msg}"),
            Self::InvalidBeans(msg) => write!(f, "{msg}"),
            Self::InvalidReply(msg) => write!(f, "{msg}"),
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
/// input targets `direct:`; (g) intercepts validate and build rules;
/// (h) `beans:` declarations validate (names, methods, per-kind config).
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
    // (b) `expects` is mandatory and non-empty, unless at least one input
    // declares `expectReply` (a reply-only document).
    let any_expect_reply = doc.inputs.iter().any(|i| i.expect_reply.is_some());
    if doc.expects.is_empty() && !any_expect_reply {
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
    // (f) Inputs deliver through `direct:` endpoints only; each `expectReply`
    // must declare at least one of `body` / `headers`.
    for input in &doc.inputs {
        if !input.to.starts_with(DIRECT_SCHEME_PREFIX) {
            return Err(TestDocError::UnsupportedInputScheme {
                target: input.to.clone(),
            });
        }
        if let Some(reply) = input.expect_reply.as_ref()
            && reply.body.is_none()
            && reply.headers.is_none()
        {
            return Err(TestDocError::InvalidReply(
                "expectReply must declare body or headers".to_string(),
            ));
        }
    }
    // Intercepts: validate and build InterceptRules (BTreeMap order).
    if let Some(intercepts) = doc.intercepts.as_ref() {
        let mut rules: Vec<InterceptRule> = Vec::new();
        for (source, action) in intercepts {
            if source.is_empty() {
                return Err(TestDocError::InterceptEmptySource);
            }
            if source.starts_with(MOCK_SCHEME_PREFIX) {
                return Err(TestDocError::InterceptMockSource {
                    key: source.clone(),
                });
            }
            let (target, rule_action) = match (&action.skip_to, &action.divert_copy_to) {
                (Some(t), None) => (t.as_str(), InterceptAction::SkipTo { uri: t.clone() }),
                (None, Some(t)) => (t.as_str(), InterceptAction::DivertCopyTo { uri: t.clone() }),
                (Some(_), Some(_)) => {
                    return Err(TestDocError::InterceptActionKeys {
                        key: source.clone(),
                        problem: "both",
                    });
                }
                (None, None) => {
                    return Err(TestDocError::InterceptActionKeys {
                        key: source.clone(),
                        problem: "neither",
                    });
                }
            };
            if target == "mock:" || target.starts_with("mock:?") {
                return Err(TestDocError::InterceptEmptyTargetPath {
                    key: source.clone(),
                });
            }
            rules.push(InterceptRule {
                uri: source.clone(),
                action: rule_action,
            });
        }
        match InterceptRules::new(rules) {
            Ok(parsed) => doc.intercept_rules_parsed = Some(parsed),
            Err(e) => {
                let msg = match e {
                    camel_api::CamelError::Config(inner) => inner,
                    other => other.to_string(),
                };
                return Err(TestDocError::InterceptInvalid(msg));
            }
        }
    }
    // Beans: validate declarations (BTreeMap order).
    if let Some(beans) = doc.beans.as_ref() {
        for (name, decl) in beans {
            if name.trim().is_empty() {
                return Err(TestDocError::InvalidBeans(
                    "bean names must be non-blank".to_string(),
                ));
            }
            if decl.methods == Some(vec![]) {
                return Err(TestDocError::InvalidBeans(format!(
                    "bean {name}: methods must be non-empty or omitted"
                )));
            }
            if let Some(methods) = decl.methods.as_ref() {
                for entry in methods {
                    if entry.trim().is_empty() {
                        return Err(TestDocError::InvalidBeans(format!(
                            "bean {name}: method names must be non-blank"
                        )));
                    }
                }
            }
            match decl.kind {
                BeanKindDoc::Echo => {
                    if let Some(config) = decl.config.as_ref()
                        && let Some(key) = config.keys().next()
                    {
                        return Err(TestDocError::InvalidBeans(format!(
                            "bean {name}: config key {key} is not valid for kind echo"
                        )));
                    }
                }
                BeanKindDoc::SetBody => {
                    let Some(config) = decl.config.as_ref().filter(|c| c.contains_key("body"))
                    else {
                        return Err(TestDocError::InvalidBeans(format!(
                            "bean {name}: kind setBody requires config key body"
                        )));
                    };
                    for key in config.keys() {
                        if key != "body" {
                            return Err(TestDocError::InvalidBeans(format!(
                                "bean {name}: config key {key} is not valid for kind setBody"
                            )));
                        }
                    }
                }
                BeanKindDoc::Fail => {
                    if let Some(config) = decl.config.as_ref() {
                        for key in config.keys() {
                            if key != "message" {
                                return Err(TestDocError::InvalidBeans(format!(
                                    "bean {name}: config key {key} is not valid for kind fail"
                                )));
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(doc)
}
