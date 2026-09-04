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
use std::path::Path;
use std::time::Duration;

use camel_api::Body;
use camel_component_mock::BodyMatcher;
use camel_component_mock::HeaderMatcher;
use camel_core::intercept::{InterceptAction, InterceptRule, InterceptRules};
use camel_integration_test::ScenarioDocument;
use noyalib::compat::serde_yaml;
use regex::Regex;
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
/// Sentinel prefix the matcher deserializers embed in serde errors; the text
/// after it is the rendered [`TestDocError::InvalidMatcher`] message (any
/// trailing serde location suffix is stripped during classification).
const MATCHER_SENTINEL: &str = "invalid matcher: ";
/// Upper bound for the `settle` quiet window.
const SETTLE_MAX: Duration = Duration::from_secs(5);
/// Registry kinds `repositories:` accepts; displayed order in error messages.
pub(crate) const SUPPORTED_REGISTRY_KINDS: [&str; 3] = ["cache", "idempotent", "claimCheck"];

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
    /// Declarative repository stubs keyed by registry kind.
    pub repositories: Option<RepositoriesDoc>,
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

    /// Declared repository stubs, if the document declares any. Validation
    /// ran eagerly in [`parse_test_document`], so the accessor is infallible.
    pub fn repository_stubs(&self) -> Option<&RepositoriesDoc> {
        self.repositories.as_ref()
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

/// Declarative repository stubs: three optional maps — `cache`,
/// `idempotent`, `claimCheck` — each mapping a repository name to the stub
/// target. The only valid target in v1 is the literal `memory`; anything
/// else is a document error (validated in [`parse_test_document`]).
///
/// Unknown registry kinds are collected into `extra` via
/// `#[serde(flatten)]` instead of being rejected by `deny_unknown_fields`:
/// the noyalib serde_yaml shim's `unknown_field` error discards serde's
/// `_expected` list, so a `deny_unknown_fields` message cannot name the
/// supported kinds. Validation rejects a non-empty `extra` with a message
/// that lists them.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RepositoriesDoc {
    /// Cache repository stubs keyed by repository name.
    pub cache: Option<BTreeMap<String, String>>,
    /// Idempotent repository stubs keyed by repository name.
    pub idempotent: Option<BTreeMap<String, String>>,
    /// Claim-check repository stubs keyed by repository name.
    pub claim_check: Option<BTreeMap<String, String>>,
    /// Unknown registry kinds, captured rather than rejected by serde.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml::Value>,
}

impl RepositoriesDoc {
    /// Label/name pairs for every declared repository stub, in registry-kind
    /// order (`cache` → `idempotent` → `claimCheck`), then map iteration
    /// order. Single source of truth for registry label strings and warning
    /// formatting; validation order in [`validate_repositories`] follows the
    /// same [`SUPPORTED_REGISTRY_KINDS`] sequence.
    pub(crate) fn stub_pairs(&self) -> Vec<(&'static str, &str)> {
        let mut out = Vec::new();
        if let Some(cache) = &self.cache {
            for name in cache.keys() {
                out.push((SUPPORTED_REGISTRY_KINDS[0], name.as_str()));
            }
        }
        if let Some(idempotent) = &self.idempotent {
            for name in idempotent.keys() {
                out.push((SUPPORTED_REGISTRY_KINDS[1], name.as_str()));
            }
        }
        if let Some(claim_check) = &self.claim_check {
            for name in claim_check.keys() {
                out.push((SUPPORTED_REGISTRY_KINDS[2], name.as_str()));
            }
        }
        out
    }
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

impl InterceptActionDoc {
    /// Converts to the core intercept action by key presence.
    ///
    /// [`parse_test_document`] admits exactly one of `skipTo` /
    /// `divertCopyTo` and rejects `both`/`neither` with
    /// [`TestDocError::InterceptActionKeys`], so parsed documents only
    /// exercise the first two arms; the mapping stays total (skipTo
    /// wins; neither degrades to an empty target) with no panic and no
    /// log.
    pub(crate) fn to_core_action(&self) -> InterceptAction {
        match (&self.skip_to, &self.divert_copy_to) {
            (Some(uri), _) => InterceptAction::SkipTo { uri: uri.clone() },
            (None, Some(uri)) => InterceptAction::DivertCopyTo { uri: uri.clone() },
            (None, None) => InterceptAction::DivertCopyTo { uri: String::new() },
        }
    }
}

/// Expected reply assertion for one input delivery: matcher-based against the
/// reply exchange the `direct:` producer returns. At least one of `body` /
/// `headers` must be present (validated in [`parse_test_document`]).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExpectReply {
    /// Reply body under the dual grammar: bare scalars, arrays, and
    /// non-matcher objects are literal `equals`; a single-recognized-key
    /// object is that matcher.
    #[serde(default, deserialize_with = "deserialize_reply_body")]
    pub body: Option<BodyMatcher>,
    /// Expected reply headers (dual grammar: literal JSON values stay
    /// `equals`; a sole-key `equals`/`regex`/`exists` map is that matcher).
    #[serde(default, deserialize_with = "deserialize_reply_headers")]
    pub headers: Option<HashMap<String, HeaderMatcher>>,
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
    /// Ordered expected bodies under the strict matcher grammar (bare
    /// strings are exact `equals`).
    #[serde(default, deserialize_with = "deserialize_bodies")]
    pub bodies: Option<Vec<BodyMatcher>>,
    /// Expected headers under the dual grammar (literal values stay
    /// `equals`; a sole-key `equals`/`regex`/`exists` map is that matcher).
    #[serde(default, deserialize_with = "deserialize_expect_headers")]
    pub headers: Option<HashMap<String, HeaderMatcher>>,
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

// ---------------------------------------------------------------------------
// Matcher grammar (mock-matchers Task 2.1)
// ---------------------------------------------------------------------------

/// Recognized body matcher keys.
fn is_body_matcher_key(key: &str) -> bool {
    matches!(
        key,
        "equals" | "regex" | "contains" | "startsWith" | "endsWith" | "exists" | "jsonSubset"
    )
}

/// Reserved key rejected with the same message in every grammar position.
fn predicate_error(field: &str) -> TestDocError {
    TestDocError::InvalidMatcher(format!("{field}: predicate matchers are not supported"))
}

/// Map a JSON value to a [`Body`] using the input-body rule: strings are
/// text, every other JSON form (object, array, number, boolean, null) is
/// carried as JSON.
fn body_from_json(value: &serde_json::Value) -> Body {
    match value {
        serde_json::Value::String(s) => Body::Text(s.clone()),
        other => Body::Json(other.clone()),
    }
}

/// Build the body matcher for one recognized matcher key and its payload.
/// `equals` maps its payload through the input-body value-to-Body mapping;
/// `regex`/`contains`/`startsWith`/`endsWith` require a string payload
/// (regex patterns compile-verified at parse time); `exists` requires a null
/// payload; `jsonSubset` requires an object.
fn body_matcher_from_map(
    key: &str,
    payload: &serde_json::Value,
    field: &str,
) -> Result<BodyMatcher, TestDocError> {
    match key {
        "equals" => Ok(BodyMatcher::Equals(body_from_json(payload))),
        "regex" | "contains" | "startsWith" | "endsWith" => {
            let Some(pattern) = payload.as_str() else {
                return Err(TestDocError::InvalidMatcher(format!(
                    "{field}: `{key}` requires a string payload"
                )));
            };
            if key == "regex"
                && let Err(e) = Regex::new(pattern)
            {
                return Err(TestDocError::InvalidMatcher(format!(
                    "{field}: invalid regex in `{key}` `{pattern}`: {e}"
                )));
            }
            Ok(match key {
                "regex" => BodyMatcher::Regex(pattern.to_string()),
                "contains" => BodyMatcher::Contains(pattern.to_string()),
                "startsWith" => BodyMatcher::StartsWith(pattern.to_string()),
                _ => BodyMatcher::EndsWith(pattern.to_string()),
            })
        }
        "exists" => {
            if payload.is_null() {
                Ok(BodyMatcher::Exists)
            } else {
                Err(TestDocError::InvalidMatcher(format!(
                    "{field}: `exists` takes no argument"
                )))
            }
        }
        "jsonSubset" => match payload {
            serde_json::Value::Object(map) => Ok(BodyMatcher::JsonSubset(
                serde_json::Value::Object(map.clone()),
            )),
            _ => Err(TestDocError::InvalidMatcher(format!(
                "{field}: `jsonSubset` must be an object"
            ))),
        },
        _ => Err(TestDocError::InvalidMatcher(format!(
            "{field}: unknown matcher key `{key}`"
        ))),
    }
}

/// Shape error for a strict body entry that is neither a bare string nor a
/// single-recognized-key matcher map; names the offending keys when present.
fn body_entry_shape_error(
    map: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> TestDocError {
    let mut keys = map.iter();
    let detail = match (map.len(), keys.next()) {
        (0, None) => "a matcher map must have exactly one key (empty map)".to_string(),
        (1, Some((key, _))) => format!("unknown matcher key `{key}`"),
        _ => format!(
            "a matcher map must have exactly one key (got {})",
            map.keys()
                .map(|key| format!("`{key}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    TestDocError::InvalidMatcher(format!(
        "{field} entries must be strings or matcher maps: {detail}"
    ))
}

/// Strict grammar for `expects.bodies` entries: a bare string is an exact
/// `equals`; a map with exactly one recognized body matcher key is that
/// matcher; any other scalar, a bare array, or a map with zero, multiple, or
/// unrecognized keys is a document error.
fn parse_body_entry(value: &serde_json::Value, field: &str) -> Result<BodyMatcher, TestDocError> {
    match value {
        serde_json::Value::String(s) => Ok(BodyMatcher::Equals(Body::Text(s.clone()))),
        serde_json::Value::Object(map) => {
            let mut keys = map.iter();
            let sole = if map.len() == 1 { keys.next() } else { None };
            if let Some((key, payload)) = sole {
                if key == "predicate" {
                    return Err(predicate_error(field));
                }
                if is_body_matcher_key(key) {
                    return body_matcher_from_map(key, payload, field);
                }
            }
            Err(body_entry_shape_error(map, field))
        }
        _ => Err(TestDocError::InvalidMatcher(format!(
            "{field} entries must be strings or matcher maps: bare scalars and \
             arrays are not body expectations"
        ))),
    }
}

/// Dual grammar for header values (`expects.headers` and
/// `expectReply.headers`): a map whose sole key is a recognized header
/// matcher key (`equals`, `regex`, `exists`) is that matcher; any other
/// value — scalar, array, multi-key or non-matcher object — is a literal
/// `equals` compared structurally. A sole `jsonSubset` or `predicate` key is
/// a document error.
fn parse_header_value(
    value: &serde_json::Value,
    field: &str,
) -> Result<HeaderMatcher, TestDocError> {
    let serde_json::Value::Object(map) = value else {
        return Ok(HeaderMatcher::Equals(value.clone()));
    };
    let mut keys = map.iter();
    let sole = if map.len() == 1 { keys.next() } else { None };
    if let Some((key, payload)) = sole {
        match key.as_str() {
            "equals" => Ok(HeaderMatcher::Equals(payload.clone())),
            "regex" => {
                let Some(pattern) = payload.as_str() else {
                    return Err(TestDocError::InvalidMatcher(format!(
                        "{field}: `regex` requires a string payload"
                    )));
                };
                Regex::new(pattern).map_err(|e| {
                    TestDocError::InvalidMatcher(format!(
                        "{field}: invalid regex in `regex` `{pattern}`: {e}"
                    ))
                })?;
                Ok(HeaderMatcher::Regex(pattern.to_string()))
            }
            "exists" => {
                if payload.is_null() {
                    Ok(HeaderMatcher::Exists)
                } else {
                    Err(TestDocError::InvalidMatcher(format!(
                        "{field}: `exists` takes no argument"
                    )))
                }
            }
            "jsonSubset" => Err(TestDocError::InvalidMatcher(format!(
                "{field}: `jsonSubset` applies to bodies only"
            ))),
            "predicate" => Err(predicate_error(field)),
            _ => Ok(HeaderMatcher::Equals(value.clone())),
        }
    } else {
        Ok(HeaderMatcher::Equals(value.clone()))
    }
}

/// Dual grammar for `expectReply.body`: every bare scalar (string, number,
/// boolean, null) and every array is a literal `equals` value; a JSON object
/// with exactly one recognized body matcher key is that matcher; a sole
/// `predicate` key is a document error; any other JSON object is a literal
/// `equals` value (structural equality).
fn parse_reply_body(value: &serde_json::Value, field: &str) -> Result<BodyMatcher, TestDocError> {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys = map.iter();
            let sole = if map.len() == 1 { keys.next() } else { None };
            if let Some((key, payload)) = sole {
                if key == "predicate" {
                    return Err(predicate_error(field));
                }
                if is_body_matcher_key(key) {
                    return body_matcher_from_map(key, payload, field);
                }
            }
            Ok(BodyMatcher::Equals(Body::Json(value.clone())))
        }
        serde_json::Value::String(s) => Ok(BodyMatcher::Equals(Body::Text(s.clone()))),
        scalar_or_array => Ok(BodyMatcher::Equals(Body::Json(scalar_or_array.clone()))),
    }
}

/// Field-level deserializer for `expects.bodies` (strict grammar). Errors
/// carry the matcher sentinel so [`classify_yaml_error`] reconstructs
/// [`TestDocError::InvalidMatcher`].
fn deserialize_bodies<'de, D>(deserializer: D) -> Result<Option<Vec<BodyMatcher>>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<Vec<serde_json::Value>>::deserialize(deserializer)?;
    let Some(entries) = raw else {
        return Ok(None);
    };
    let mut matchers = Vec::with_capacity(entries.len());
    for entry in &entries {
        let matcher = parse_body_entry(entry, "bodies")
            .map_err(|e| D::Error::custom(format!("{MATCHER_SENTINEL}{e}")))?;
        matchers.push(matcher);
    }
    Ok(Some(matchers))
}

/// Shared map-value deserializer for the two header positions; `field_prefix`
/// distinguishes `headers` from `expectReply.headers` in error messages.
fn deserialize_header_map<'de, D>(
    deserializer: D,
    field_prefix: &str,
) -> Result<Option<HashMap<String, HeaderMatcher>>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<HashMap<String, serde_json::Value>>::deserialize(deserializer)?;
    let Some(headers) = raw else {
        return Ok(None);
    };
    let mut matchers = HashMap::with_capacity(headers.len());
    for (key, value) in &headers {
        let field = format!("{field_prefix}[{key}]");
        let matcher = parse_header_value(value, &field)
            .map_err(|e| D::Error::custom(format!("{MATCHER_SENTINEL}{e}")))?;
        matchers.insert(key.clone(), matcher);
    }
    Ok(Some(matchers))
}

/// Field-level deserializer for `expects.headers` values (dual grammar).
fn deserialize_expect_headers<'de, D>(
    deserializer: D,
) -> Result<Option<HashMap<String, HeaderMatcher>>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_header_map(deserializer, "headers")
}

/// Field-level deserializer for `expectReply.headers` values (dual grammar).
fn deserialize_reply_headers<'de, D>(
    deserializer: D,
) -> Result<Option<HashMap<String, HeaderMatcher>>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_header_map(deserializer, "expectReply.headers")
}

/// Field-level deserializer for `expectReply.body` (dual grammar).
/// Deserializes a plain `serde_json::Value` (NOT `Option<Value>`): a missing
/// field never reaches this helper, while an explicit `body: null` is the
/// literal `equals null` matcher.
fn deserialize_reply_body<'de, D>(deserializer: D) -> Result<Option<BodyMatcher>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    parse_reply_body(&value, "expectReply.body")
        .map(Some)
        .map_err(|e| D::Error::custom(format!("{MATCHER_SENTINEL}{e}")))
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
    /// A `repositories:` declaration failed validation; the message carries
    /// the precise reason verbatim.
    InvalidRepositories(String),
    /// An `expectReply` block failed validation; the message carries the
    /// precise reason verbatim.
    InvalidReply(String),
    /// A matcher entry failed grammar validation; the message carries the
    /// precise reason verbatim.
    InvalidMatcher(String),
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
            Self::InvalidRepositories(msg) => write!(f, "{msg}"),
            Self::InvalidReply(msg) => write!(f, "{msg}"),
            Self::InvalidMatcher(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for TestDocError {}

/// One parsed test document in whichever vocabulary it declares: the
/// unit-tier mock vocabulary or the integration-tier `scenario:`
/// vocabulary. Dispatch sniffs the `scenario:` key, so a document can
/// never be parsed by both parsers.
pub(crate) enum ParsedDocument {
    /// A unit-tier document (`inputs` / `expects` / `intercepts`).
    Unit(Box<TestDocument>),
    /// A full-tier scenario document (`scenario:` section).
    Scenario(ScenarioDocument),
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
/// anything else routes to the unit-tier parser. Errors are rendered
/// `Display` strings — every parse failure of both parsers is a
/// load-time, exit-2 class.
pub(crate) fn parse_document(path: &Path, text: &str) -> Result<ParsedDocument, String> {
    if declares_scenario(text) {
        camel_integration_test::parse_scenario_document(path)
            .map(ParsedDocument::Scenario)
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
fn classify_yaml_error(raw: &str) -> TestDocError {
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

/// Parses and validates a `*.test.yaml` document. Validation order:
/// (a) exactly one route source (`routeFiles`, `routeFilesFromRoot`, or
/// `routes`); (b) non-empty `expects`; (c) every `expects` key uses the
/// `mock:` scheme (then the map is rebuilt with bare endpoint names);
/// (d) `count`/`minCount` exclusivity; (e) `settle` range; (f) every
/// input targets `direct:`; (g) intercepts validate and build rules;
/// (h) `beans:` declarations validate (names, methods, per-kind config);
/// (i) `repositories:` declarations validate (registry kinds, stub targets,
/// blank names, built-in `memory` name).
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
            let both = action.skip_to.is_some() && action.divert_copy_to.is_some();
            let neither = action.skip_to.is_none() && action.divert_copy_to.is_none();
            if both || neither {
                return Err(TestDocError::InterceptActionKeys {
                    key: source.clone(),
                    problem: if both { "both" } else { "neither" },
                });
            }
            let rule_action = action.to_core_action();
            let target = match &rule_action {
                InterceptAction::SkipTo { uri } | InterceptAction::DivertCopyTo { uri } => {
                    uri.as_str()
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
    // Repositories: validate stub declarations (BTreeMap order).
    validate_repositories(&doc)?;
    Ok(doc)
}

/// Validates the `repositories:` stub declarations. For each declared
/// registry map and each (`name`, `target`) pair: the target must be the
/// literal `memory`; the name must be non-blank after trimming; and the
/// name must not be the built-in `memory` (registration would collide with
/// the built-in repository). Unknown registry kinds land in `extra` and are
/// rejected first with a message listing the supported kinds.
fn validate_repositories(doc: &TestDocument) -> Result<(), TestDocError> {
    let Some(repos) = doc.repositories.as_ref() else {
        return Ok(());
    };
    // Unknown registry kinds arrive in `extra` (see `RepositoriesDoc`). The
    // noyalib serde_yaml shim cannot produce the supported-kinds list from a
    // `deny_unknown_fields` error (it discards serde's `_expected`), so the
    // rejection is a validation error with an explicit message.
    if !repos.extra.is_empty() {
        let kinds = repos
            .extra
            .keys()
            .map(|kind| format!("`{kind}`"))
            .collect::<Vec<_>>()
            .join(", ");
        let noun = if repos.extra.len() == 1 {
            "unknown registry kind"
        } else {
            "unknown registry kinds"
        };
        return Err(TestDocError::InvalidRepositories(format!(
            "{noun} {kinds}; supported kinds: {}",
            SUPPORTED_REGISTRY_KINDS.join(", ")
        )));
    }
    for (registry, map) in [
        (SUPPORTED_REGISTRY_KINDS[0], &repos.cache),
        (SUPPORTED_REGISTRY_KINDS[1], &repos.idempotent),
        (SUPPORTED_REGISTRY_KINDS[2], &repos.claim_check),
    ] {
        let Some(map) = map.as_ref() else {
            continue;
        };
        for (name, target) in map {
            if name.trim().is_empty() {
                return Err(TestDocError::InvalidRepositories(
                    "repository names must be non-blank".to_string(),
                ));
            }
            if name == "memory" {
                return Err(TestDocError::InvalidRepositories(format!(
                    "repository {name}: `memory` is a built-in repository name and cannot be stubbed"
                )));
            }
            if target != "memory" {
                return Err(TestDocError::InvalidRepositories(format!(
                    "repository {registry} `{name}`: unsupported stub target `{target}`; \
                     only `memory` is supported"
                )));
            }
        }
    }
    Ok(())
}
