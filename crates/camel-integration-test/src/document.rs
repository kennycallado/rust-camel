//! Scenario document model, parsing, and validation (ADR-0069 sections
//! 1-2).
//!
//! A scenario document is a `.test.yaml` (or `.test.yml`) sidecar that
//! declares one integration-tier test: exactly one route source
//! (`routeFiles`, `routeFilesFromRoot`, or inline `routes`), an ordered
//! `scenario:` action list, an optional `env:` map with fixed fixture
//! values, an optional `envPassthrough:` allowlist, and an optional
//! pinned `profile`. Unknown fields are rejected.
//!
//! The scenario vocabulary and the unit-tier vocabulary (`inputs`,
//! `expects`, `intercepts`) never mix in one document. A document with
//! `scenario:` that also declares a unit-tier section is rejected at
//! load time.
//!
//! Durations (`deadline`, `duration`) are humantime strings, for
//! example `"5s"` or `"250ms"`, parsed during validation so errors can
//! name the action index.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use camel_api::Value;
use camel_core::RouteDefinition;
use noyalib::compat::serde_yaml;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};

// ---------------------------------------------------------------------------
// Public model
// ---------------------------------------------------------------------------

/// A parsed scenario document. Route file paths stay as declared;
/// resolving them against the document directory or the project root is
/// the runner's job, the same split the unit-tier parser keeps.
#[derive(Debug)]
pub struct ScenarioDocument {
    /// The single declared route source.
    pub route_source: RouteSource,
    /// Ordered scenario actions.
    pub scenario: Vec<ScenarioAction>,
    /// Fixed fixture values for the scenario; the layered environment
    /// source reads these before any ambient value.
    pub env: Option<BTreeMap<String, String>>,
    /// Ambient variable names allowed to pass through to the scenario.
    pub env_passthrough: Option<Vec<String>>,
    /// Profile pinned per document; an ambient profile would break
    /// hermeticity.
    pub profile: Option<String>,
}

/// The route source of a scenario document. Exactly one form is
/// declared; the parser rejects zero or multiple declarations.
///
/// Not `Clone`: the inline form carries `RouteDefinition`s, which are
/// not `Clone`.
#[non_exhaustive]
pub enum RouteSource {
    /// Route files to load, relative to the document's directory.
    RouteFiles(Vec<PathBuf>),
    /// Route files to load, resolved against the nearest ancestor
    /// `Camel.toml` directory (the project root).
    RouteFilesFromRoot(Vec<PathBuf>),
    /// Inline route definitions, parsed at load time.
    Inline(Vec<RouteDefinition>),
}

impl std::fmt::Debug for RouteSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // `RouteDefinition` implements neither `Debug` nor `Clone`;
            // the inline form reports its route count only.
            Self::RouteFiles(files) => f.debug_tuple("RouteFiles").field(files).finish(),
            Self::RouteFilesFromRoot(files) => {
                f.debug_tuple("RouteFilesFromRoot").field(files).finish()
            }
            Self::Inline(routes) => f
                .debug_tuple("Inline")
                .field(&format_args!("{} route definitions", routes.len()))
                .finish(),
        }
    }
}

/// One ordered scenario action (ADR-0069 section 11, adopted from
/// Citrus: `send`, `receive` with a mandatory deadline, `sleep`,
/// `validate`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ScenarioAction {
    /// Send a message to an endpoint.
    Send {
        /// Target endpoint reference.
        to: EndpointRef,
        /// Message body; omitted means an empty body.
        body: Option<Value>,
        /// Message headers.
        headers: Option<BTreeMap<String, Value>>,
        /// Resolved method: explicit or inferred (`POST` with a body,
        /// `GET` without), uppercase.
        method: String,
    },
    /// Receive a message from an endpoint before the deadline passes.
    Receive {
        /// Source endpoint reference.
        from: EndpointRef,
        /// Mandatory deadline, real monotonic time.
        deadline: Duration,
        /// Extractions into scenario variables, keyed by variable name.
        extract: Option<BTreeMap<String, String>>,
    },
    /// Pause the scenario for the given duration.
    Sleep {
        /// Sleep length.
        duration: Duration,
    },
    /// Assert an expectation against a scenario target.
    Validate {
        /// What to validate: the last message received on an endpoint,
        /// or a scenario variable.
        target: ScenarioTarget,
        /// Matcher expectation.
        expectation: Expectation,
    },
}

impl ScenarioAction {
    /// The `(bind variable, endpoint)` bindings this action's endpoint
    /// references declare.
    fn bindings(&self) -> Vec<(&str, &str)> {
        fn endpoint_bindings(endpoint: &EndpointRef) -> Vec<(&str, &str)> {
            endpoint.binding().into_iter().collect()
        }
        match self {
            Self::Send { to, .. } => endpoint_bindings(to),
            Self::Receive { from, .. } => endpoint_bindings(from),
            Self::Validate { target, .. } => match target {
                ScenarioTarget::LastReceived(endpoint) => endpoint_bindings(endpoint),
                ScenarioTarget::Variable(_) => Vec::new(),
            },
            Self::Sleep { .. } => Vec::new(),
        }
    }
}

/// What a `validate` action asserts against.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ScenarioTarget {
    /// The last message received on the endpoint.
    LastReceived(EndpointRef),
    /// A scenario variable set by an earlier `extract`. Variable
    /// existence is validated at run time.
    Variable(String),
}

/// An endpoint reference: a bare endpoint string or a map with
/// `endpoint`, `provisioning`, and `bindVar` keys.
#[derive(Debug, Clone, PartialEq)]
pub struct EndpointRef {
    /// Endpoint URI, for example `http://127.0.0.1:9999/hook`.
    pub endpoint: String,
    /// Who owns the partner lifecycle; only `harness` is implemented in
    /// v1.
    pub provisioning: Option<Provisioning>,
    /// Scenario variable name the harness fills with this endpoint's
    /// bound address when provisioning is `harness`.
    pub bind_var: Option<String>,
}

impl EndpointRef {
    /// The `(bind variable, endpoint)` binding this reference declares,
    /// if any. The reserved env-key rule collects these pairs.
    fn binding(&self) -> Option<(&str, &str)> {
        self.bind_var
            .as_deref()
            .map(|bind_var| (bind_var, self.endpoint.as_str()))
    }
}

/// Partner provisioning source (ADR-0069 section 9). The axis is who
/// owns the lifecycle. `testcontainer` and `user-provided` are reserved
/// grammar values; the parser rejects them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Provisioning {
    /// The harness binds an in-process listener on `127.0.0.1:0`. The
    /// only source implemented in v1.
    Harness,
}

/// A validation expectation. The grammar keys mirror the mock-testkit
/// matcher rules: `equals`, `regex`, `contains`, `startsWith`,
/// `endsWith`, `exists`, `jsonSubset`.
///
/// Grammar (dual, `expectReply.body` style): a bare value is a literal
/// `equals`; an object with exactly one recognized matcher key is that
/// matcher (this reading takes precedence over the literal one); any
/// other object — zero, multiple, or unrecognized keys — is a literal
/// `equals` compared structurally. `regex` patterns are
/// compile-verified at load time, matching the unit-tier matcher
/// rules.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Expectation {
    /// Exact equality against the value.
    Equals(Value),
    /// Regular expression match, compile-verified at load time.
    Regex(String),
    /// Substring containment.
    Contains(String),
    /// Prefix match.
    StartsWith(String),
    /// Suffix match.
    EndsWith(String),
    /// The value under validation is present.
    Exists,
    /// Recursive-subset match against an object.
    JsonSubset(Value),
}

// ---------------------------------------------------------------------------
// Raw serde stage
// ---------------------------------------------------------------------------

/// Raw document form. Unit-tier sections are captured, not rejected at
/// the serde layer, so the mixing ban can name them. Scenario items
/// stay raw values: the single-key action dispatch runs during
/// validation so errors can name the action index.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawDocument {
    route_files: Option<Vec<String>>,
    route_files_from_root: Option<Vec<String>>,
    routes: Option<serde_yaml::Value>,
    scenario: Option<Vec<serde_yaml::Value>>,
    env: Option<BTreeMap<String, String>>,
    env_passthrough: Option<Vec<String>>,
    profile: Option<String>,
    // Unit-tier vocabulary, present only to detect and name the mixing
    // ban violation.
    inputs: Option<serde_yaml::Value>,
    expects: Option<serde_yaml::Value>,
    intercepts: Option<serde_yaml::Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawSend {
    to: RawEndpointRef,
    body: Option<Value>,
    headers: Option<BTreeMap<String, Value>>,
    /// Raw `method` string; optional. Validation resolves it (explicit
    /// or inferred from body presence) so errors can name the action
    /// index.
    method: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawReceive {
    from: RawEndpointRef,
    /// Raw humantime string; required by validation, not by serde, so
    /// the error can name the action index.
    deadline: Option<String>,
    extract: Option<BTreeMap<String, String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawSleep {
    /// Raw humantime string.
    duration: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawValidate {
    /// Raw `target` node; the single-key form (`lastReceived` /
    /// `variable`) converts during validation.
    target: serde_yaml::Value,
    expectation: Value,
}

/// Raw endpoint reference: bare string or map with `endpoint`,
/// `provisioning`, and `bindVar`.
#[derive(Debug, Clone)]
struct RawEndpointRef {
    endpoint: String,
    provisioning: Option<String>,
    bind_var: Option<String>,
}

impl RawEndpointRef {
    /// Deserializes from a bare string (shorthand) or a map.
    fn from_yaml_value(value: serde_yaml::Value) -> Result<Self, String> {
        match value {
            serde_yaml::Value::String(endpoint) => Ok(Self {
                endpoint,
                provisioning: None,
                bind_var: None,
            }),
            serde_yaml::Value::Mapping(ref map) => {
                // Field-by-field extraction: a hand-rolled map walk gives
                // errors that name the offending key, which the
                // deny_unknown_fields machinery of the compat shim
                // cannot.
                let mut endpoint: Option<String> = None;
                let mut provisioning: Option<String> = None;
                let mut bind_var: Option<String> = None;
                for (key, value) in map {
                    match key.as_str() {
                        "endpoint" | "provisioning" | "bindVar" => {
                            let text = value.as_str().ok_or_else(|| {
                                format!(
                                    "endpoint reference `{key}` must be a string, got {value:?}"
                                )
                            })?;
                            match key.as_str() {
                                "endpoint" => endpoint = Some(text.to_string()),
                                "provisioning" => provisioning = Some(text.to_string()),
                                _ => bind_var = Some(text.to_string()),
                            }
                        }
                        other => {
                            return Err(format!("unknown field `{other}` in endpoint reference"));
                        }
                    }
                }
                let endpoint = endpoint
                    .ok_or_else(|| "endpoint reference requires the `endpoint` key".to_string())?;
                Ok(Self {
                    endpoint,
                    provisioning,
                    bind_var,
                })
            }
            other => Err(format!(
                "endpoint reference must be a string or a map, got {other:?}"
            )),
        }
    }
}

impl<'de> Deserialize<'de> for RawEndpointRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_yaml::Value::deserialize(deserializer)?;
        RawEndpointRef::from_yaml_value(value).map_err(D::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Parse and validation errors for scenario documents.
///
/// Exit-code mapping for the CLI adapter (ADR-0069 section 7):
/// classification is by variant, never by message text. Every variant
/// is a load-time failure and maps to exit 2.
///
/// - `doc-validation` class — Display carries the `doc-validation:`
///   token: `NotTestDocument`, `MissingScenario`, `MixedVocabulary`,
///   `Validation`, `ReservedEnvKey`, `InlineRoutes`.
/// - `infra-unavailable` class — `UnsupportedProvisioning` (reserved
///   provisioning grammar; Display names the class).
/// - Unit-tier message parity — `RouteSourceMissing` and
///   `RouteSourceConflict` render the unit-tier parser's messages
///   verbatim, without the token, so both parsers report identical
///   text; the CLI maps them to exit 2 as doc parse errors, the same
///   as the unit tier does today.
/// - Read and serde failures — `Io`, `Yaml`, `UnknownField` map to
///   exit 2 as doc parse errors (unreadable file, broken grammar).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DocError {
    /// The document file could not be read.
    #[error("failed to read test document {path}: {source}")]
    Io {
        /// Path of the unreadable document.
        path: PathBuf,
        /// Underlying read failure.
        source: std::io::Error,
    },
    /// Malformed YAML or a type mismatch at the serde layer.
    #[error("invalid test document: {0}")]
    Yaml(String),
    /// A `deny_unknown_fields` rejection.
    #[error("unknown field in test document: {0}")]
    UnknownField(String),
    /// The path lacks the reserved `.test.yaml` / `.test.yml` suffix.
    #[error(
        "doc-validation: not a test document: {path} (reserved suffixes are `.test.yaml` and `.test.yml`)"
    )]
    NotTestDocument {
        /// The rejected path.
        path: PathBuf,
    },
    /// The document declares no `scenario:` section.
    #[error("doc-validation: scenario document must declare a `scenario:` section")]
    MissingScenario,
    /// The document mixes the scenario vocabulary with unit-tier
    /// sections.
    #[error(
        "doc-validation: mixed vocabulary: a document with `scenario:` must not declare unit-tier fields (found: {found})"
    )]
    MixedVocabulary {
        /// The unit-tier fields found, backticked and comma-joined.
        found: String,
    },
    /// No route source is declared. Same message as the unit-tier
    /// parser.
    #[error(
        "exactly one route source (`routeFiles`, `routeFilesFromRoot`, or `routes`) is required"
    )]
    RouteSourceMissing,
    /// More than one route source is declared. Same message as the
    /// unit-tier parser.
    #[error("route sources {present} are mutually exclusive; exactly one route source is required")]
    RouteSourceConflict {
        /// The declared keys, backticked and comma-joined.
        present: String,
    },
    /// An action failed validation; `index` is the position in the
    /// `scenario:` list. An empty `scenario:` list is rejected with
    /// index 0 (the section, not an action, failed).
    #[error("doc-validation: scenario[{index}]: {message}")]
    Validation {
        /// Zero-based position of the action in the `scenario:` list.
        index: usize,
        /// What failed.
        message: String,
    },
    /// The endpoint declares a provisioning source that is reserved in
    /// v1; only `harness` is supported.
    #[error(
        "doc-validation: unsupported provisioning `{value}` for endpoint `{endpoint}`: only `harness` is supported in v1 (infra-unavailable class)"
    )]
    UnsupportedProvisioning {
        /// The rejected provisioning value.
        value: String,
        /// The endpoint that declared it.
        endpoint: String,
    },
    /// A document `env` key equals an endpoint's `bindVar`. The
    /// reserved set is exactly the `bindVar` values declared by the
    /// document's own endpoints; the harness binding wins.
    #[error(
        "doc-validation: env key `{key}` is reserved: it is the harness bind variable of endpoint `{endpoint}`"
    )]
    ReservedEnvKey {
        /// The reserved key.
        key: String,
        /// The endpoint that reserved it.
        endpoint: String,
    },
    /// Inline `routes` failed to parse.
    #[error("doc-validation: inline routes: {0}")]
    InlineRoutes(String),
}

/// Classifies a compat-layer (serde_yaml) error text, mirroring the
/// unit-tier classifier.
fn classify_yaml_error(raw: &str) -> DocError {
    if raw.contains("unknown field") {
        return DocError::UnknownField(raw.to_string());
    }
    DocError::Yaml(raw.to_string())
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parses and validates a scenario document. Validation order:
/// (a) the path carries a reserved test-document suffix; (b) the text
/// deserializes; (c) a non-empty `scenario:` section exists; (d) no
/// unit-tier section coexists with it; (e) exactly one route source
/// is declared;
/// (f) each action converts (single-key dispatch, deadlines, durations,
/// endpoint provisioning, expectation grammar) with action-index
/// errors; (g) no `env` key collides with a declared `bindVar`.
pub fn parse_scenario_document(path: &Path) -> Result<ScenarioDocument, DocError> {
    if !camel_dsl::discovery::is_test_document(path) {
        return Err(DocError::NotTestDocument {
            path: path.to_path_buf(),
        });
    }
    let text = std::fs::read_to_string(path).map_err(|source| DocError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let raw = serde_yaml::from_str::<RawDocument>(&text)
        .map_err(|e| classify_yaml_error(&e.to_string()))?;

    // (c) This parser accepts scenario documents only, and the
    // scenario list must be non-empty: an empty list would yield a
    // trivially-green FULL document with zero actions (mirrors the
    // unit tier's non-empty `expects` rule).
    let Some(raw_scenario) = raw.scenario else {
        return Err(DocError::MissingScenario);
    };
    if raw_scenario.is_empty() {
        return Err(DocError::Validation {
            index: 0,
            message: "`scenario` must declare at least one action".to_string(),
        });
    }
    // (d) Mixing ban (ADR-0069 section 2).
    let mut unit_tier: Vec<&str> = Vec::new();
    if raw.inputs.is_some() {
        unit_tier.push("inputs");
    }
    if raw.expects.is_some() {
        unit_tier.push("expects");
    }
    if raw.intercepts.is_some() {
        unit_tier.push("intercepts");
    }
    if !unit_tier.is_empty() {
        return Err(DocError::MixedVocabulary {
            found: backticked(&unit_tier),
        });
    }
    // (e) Exactly one route source, with the unit-tier messages.
    let mut present: Vec<&'static str> = Vec::new();
    if raw.route_files.is_some() {
        present.push("routeFiles");
    }
    if raw.route_files_from_root.is_some() {
        present.push("routeFilesFromRoot");
    }
    if raw.routes.is_some() {
        present.push("routes");
    }
    let route_source = match present.as_slice() {
        ["routeFiles"] => RouteSource::RouteFiles(
            raw.route_files
                .unwrap_or_default()
                .into_iter()
                .map(PathBuf::from)
                .collect(),
        ),
        ["routeFilesFromRoot"] => RouteSource::RouteFilesFromRoot(
            raw.route_files_from_root
                .unwrap_or_default()
                .into_iter()
                .map(PathBuf::from)
                .collect(),
        ),
        ["routes"] => {
            let value = raw.routes.unwrap_or(serde_yaml::Value::Null);
            RouteSource::Inline(parse_inline_routes(&value)?)
        }
        [] => return Err(DocError::RouteSourceMissing),
        _ => {
            return Err(DocError::RouteSourceConflict {
                present: backticked(&present),
            });
        }
    };
    // (f) Action conversion.
    let mut scenario = Vec::with_capacity(raw_scenario.len());
    for (index, item) in raw_scenario.into_iter().enumerate() {
        scenario.push(build_action(item, index)?);
    }
    // (g) Reserved env keys: the harness binding wins over document
    // fixtures.
    if let Some(env) = raw.env.as_ref() {
        for action in &scenario {
            for (bind_var, endpoint) in action.bindings() {
                if env.contains_key(bind_var) {
                    return Err(DocError::ReservedEnvKey {
                        key: bind_var.to_string(),
                        endpoint: endpoint.to_string(),
                    });
                }
            }
        }
    }
    Ok(ScenarioDocument {
        route_source,
        scenario,
        env: raw.env,
        env_passthrough: raw.env_passthrough,
        profile: raw.profile,
    })
}

/// Parses inline `routes` through the shared DSL parser. `parse_yaml`
/// expects a top-level `routes:` key; the inline value (the array under
/// `routes:`) is wrapped back into that shape, the same as the unit-tier
/// runner.
fn parse_inline_routes(value: &serde_yaml::Value) -> Result<Vec<RouteDefinition>, DocError> {
    let mut mapping = serde_yaml::Mapping::new();
    mapping.insert("routes", value.clone());
    let text = serde_yaml::to_string(&serde_yaml::Value::Mapping(mapping))
        .map_err(|e| DocError::InlineRoutes(format!("failed to serialize inline routes: {e}")))?;
    camel_dsl::parse_yaml(&text).map_err(|e| DocError::InlineRoutes(e.to_string()))
}

/// Converts one raw action item into the public model. An item is a
/// single-key map (`send`, `receive`, `sleep`, `validate`); dispatch
/// runs here, not in serde, so every failure carries the action index.
fn build_action(item: serde_yaml::Value, index: usize) -> Result<ScenarioAction, DocError> {
    let action_error = |message: String| DocError::Validation { index, message };
    let serde_yaml::Value::Mapping(ref map) = item else {
        return Err(action_error(format!(
            "action must be a single-key map (`send`, `receive`, `sleep`, `validate`), got {item:?}"
        )));
    };
    let Some((key, content)) = map.iter().next() else {
        return Err(action_error(
            "action must be a single-key map (`send`, `receive`, `sleep`, `validate`), got an empty map"
                .to_string(),
        ));
    };
    if map.len() != 1 {
        return Err(action_error(format!(
            "action must declare exactly one key, got {}",
            backticked(&map.keys().map(String::as_str).collect::<Vec<_>>())
        )));
    }
    let action_error_from_serde = |e: serde_yaml::Error| action_error(e.to_string());
    match key.as_str() {
        "send" => {
            let raw: RawSend =
                serde_yaml::from_value(content.clone()).map_err(action_error_from_serde)?;
            let method = match raw.method {
                Some(method) => {
                    let upper = method.trim().to_ascii_uppercase();
                    if !is_http_token(&upper) {
                        return Err(action_error(format!(
                            "send action `method` must be a valid HTTP method name, got `{method}`"
                        )));
                    }
                    upper
                }
                None => {
                    if raw.body.is_some() {
                        "POST".to_string()
                    } else {
                        "GET".to_string()
                    }
                }
            };
            Ok(ScenarioAction::Send {
                to: endpoint_from_raw(raw.to)?,
                body: raw.body,
                headers: raw.headers,
                method,
            })
        }
        "receive" => {
            let raw: RawReceive =
                serde_yaml::from_value(content.clone()).map_err(action_error_from_serde)?;
            let deadline = raw.deadline.ok_or_else(|| {
                action_error(
                    "receive action requires a `deadline` (humantime string, e.g. `5s`)"
                        .to_string(),
                )
            })?;
            Ok(ScenarioAction::Receive {
                from: endpoint_from_raw(raw.from)?,
                deadline: parse_duration(&deadline, index, "deadline")?,
                extract: raw.extract,
            })
        }
        "sleep" => {
            let raw: RawSleep =
                serde_yaml::from_value(content.clone()).map_err(action_error_from_serde)?;
            Ok(ScenarioAction::Sleep {
                duration: parse_duration(&raw.duration, index, "sleep duration")?,
            })
        }
        "validate" => {
            let raw: RawValidate =
                serde_yaml::from_value(content.clone()).map_err(action_error_from_serde)?;
            Ok(ScenarioAction::Validate {
                target: build_target(&raw.target, index)?,
                expectation: expectation_from_value(&raw.expectation, index)?,
            })
        }
        other => Err(action_error(format!(
            "unknown action `{other}`; expected `send`, `receive`, `sleep`, or `validate`"
        ))),
    }
}

/// Builds a `validate` target from the raw `target` node: a single-key
/// map (`lastReceived` or `variable`).
fn build_target(value: &serde_yaml::Value, index: usize) -> Result<ScenarioTarget, DocError> {
    let action_error = |message: String| DocError::Validation { index, message };
    let serde_yaml::Value::Mapping(map) = value else {
        return Err(action_error(format!(
            "validate `target` must be a single-key map (`lastReceived`, `variable`), got {value:?}"
        )));
    };
    let Some((key, content)) = map.iter().next() else {
        return Err(action_error(
            "validate `target` must be a single-key map (`lastReceived`, `variable`), got an empty map"
                .to_string(),
        ));
    };
    match key.as_str() {
        "lastReceived" => {
            let raw: RawEndpointRef =
                serde_yaml::from_value(content.clone()).map_err(|e| action_error(e.to_string()))?;
            Ok(ScenarioTarget::LastReceived(endpoint_from_raw(raw)?))
        }
        "variable" => match content.as_str() {
            Some(name) => Ok(ScenarioTarget::Variable(name.to_string())),
            None => Err(action_error(format!(
                "validate `variable` target must be a string, got {content:?}"
            ))),
        },
        other => Err(action_error(format!(
            "unknown validate target `{other}`; expected `lastReceived` or `variable`"
        ))),
    }
}

/// Applies the provisioning gate: only `harness` (or absent) passes.
fn endpoint_from_raw(raw: RawEndpointRef) -> Result<EndpointRef, DocError> {
    let provisioning = match raw.provisioning.as_deref() {
        None => None,
        Some("harness") => Some(Provisioning::Harness),
        Some(value) => {
            return Err(DocError::UnsupportedProvisioning {
                value: value.to_string(),
                endpoint: raw.endpoint.clone(),
            });
        }
    };
    Ok(EndpointRef {
        endpoint: raw.endpoint,
        provisioning,
        bind_var: raw.bind_var,
    })
}

/// Parses a humantime duration string, naming the action index on
/// failure.
fn parse_duration(raw: &str, index: usize, field: &str) -> Result<Duration, DocError> {
    humantime::parse_duration(raw).map_err(|e| DocError::Validation {
        index,
        message: format!("invalid {field} `{raw}`: {e}"),
    })
}

/// Whether `s` is a valid HTTP token: non-empty and composed only of
/// ASCII alphanumerics or one of ``!#$%&'*+-.^_`|~``. Crate-visible
/// for the parse-test module.
pub(crate) fn is_http_token(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(
                    c,
                    '!' | '#'
                        | '$'
                        | '%'
                        | '&'
                        | '\''
                        | '*'
                        | '+'
                        | '-'
                        | '.'
                        | '^'
                        | '_'
                        | '`'
                        | '|'
                        | '~'
                )
        })
}

/// Recognized expectation matcher keys.
fn is_matcher_key(key: &str) -> bool {
    matches!(
        key,
        "equals" | "regex" | "contains" | "startsWith" | "endsWith" | "exists" | "jsonSubset"
    )
}

/// Applies the expectation dual grammar: a bare value is a literal
/// `equals`; an object whose single key is a recognized matcher key is
/// that matcher; any other object is a literal `equals`. Payload shapes
/// mirror the mock-testkit matcher rules.
fn expectation_from_value(value: &Value, index: usize) -> Result<Expectation, DocError> {
    const FIELD: &str = "expectation";
    let invalid = |message: String| DocError::Validation { index, message };
    if let Value::Object(map) = value
        && map.len() == 1
        && let Some((key, payload)) = map.iter().next()
        && is_matcher_key(key)
    {
        return match key.as_str() {
            "equals" => Ok(Expectation::Equals(payload.clone())),
            "regex" | "contains" | "startsWith" | "endsWith" => {
                let Some(pattern) = payload.as_str() else {
                    return Err(invalid(format!(
                        "{FIELD}: `{key}` requires a string payload"
                    )));
                };
                if key.as_str() == "regex"
                    && let Err(e) = regex::Regex::new(pattern)
                {
                    return Err(invalid(format!("{FIELD}: invalid regex `{pattern}`: {e}")));
                }
                Ok(match key.as_str() {
                    "regex" => Expectation::Regex(pattern.to_string()),
                    "contains" => Expectation::Contains(pattern.to_string()),
                    "startsWith" => Expectation::StartsWith(pattern.to_string()),
                    _ => Expectation::EndsWith(pattern.to_string()),
                })
            }
            "exists" => {
                if payload.is_null() {
                    Ok(Expectation::Exists)
                } else {
                    Err(invalid(format!("{FIELD}: `exists` takes no argument")))
                }
            }
            _ => {
                if payload.is_object() {
                    Ok(Expectation::JsonSubset(payload.clone()))
                } else {
                    Err(invalid(format!("{FIELD}: `jsonSubset` must be an object")))
                }
            }
        };
    }
    Ok(Expectation::Equals(value.clone()))
}

/// Backticks and comma-joins field names for error messages.
fn backticked(fields: &[&str]) -> String {
    fields
        .iter()
        .map(|field| format!("`{field}`"))
        .collect::<Vec<_>>()
        .join(", ")
}
