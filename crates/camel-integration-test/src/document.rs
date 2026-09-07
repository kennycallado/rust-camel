//! Scenario document model, parsing, and validation (ADR-0069 sections
//! 1-2).
//!
//! A scenario document is a `.test.yaml` (or `.test.yml`) sidecar that
//! declares one integration-tier test: exactly one route source
//! (`routeFiles`, `routeFilesFromRoot`, or inline `routes`), an ordered
//! `scenario:` action list, an optional `env:` map with fixed fixture
//! values, an optional `envPassthrough:` allowlist, an optional
//! endpoint-keyed `partners:` scripting map, an optional pinned
//! `profile`, an optional document-level `sendDeadline` bounding
//! every send, and an optional document-level `inbound:` listener
//! declaration (feature `http`). Unknown fields are rejected.
//!
//! The scenario vocabulary and the unit-tier vocabulary (`inputs`,
//! `expects`, `intercepts`) never mix in one document. A document with
//! `scenario:` that also declares a unit-tier section is rejected at
//! load time.
//!
//! Durations (`sendDeadline`, `deadline`, `duration`,
//! `elapsedAtLeast`) are humantime strings, for example `"5s"` or
//! `"250ms"`, parsed during validation so errors can name the action
//! index.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use camel_api::Value;
use camel_core::RouteDefinition;
use noyalib::compat::serde_yaml;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};

// The partner-script grammar lives in its own module; the public
// types are re-exported here so the document API stays one surface.
pub use crate::partner_script::{PartnerFault, PartnerScript, PartnerScriptResponse};
// The matcher algebra lives in the shared pure core (camel-matchers);
// re-exported here so the document API stays one surface. The raw
// serde stage below constructs these core types directly.
pub use camel_matchers::RequestExpectation as PartnerExpectation;
pub use camel_matchers::{CountBound, Expectation, PathFilter};

// ---------------------------------------------------------------------------
// Public model
// ---------------------------------------------------------------------------

/// A parsed scenario document. Route file paths stay as declared;
/// resolving them against the document directory or the project root is
/// the runner's job, the same split the unit-tier parser keeps.
#[derive(Debug)]
pub struct ScenarioDocument {
    /// The document's own path as parsed. The boot root may be a
    /// nearest-ancestor `Camel.toml` directory rather than the
    /// document's directory, so the document directory travels with
    /// the model: relative `routeFiles` anchor here (rc-jjzy5).
    pub source_path: std::path::PathBuf,
    /// The single declared route source.
    pub route_source: RouteSource,
    /// Ordered scenario actions.
    pub scenario: Vec<ScenarioAction>,
    /// Document-level partner scripting, keyed by endpoint address.
    /// The grammar lives here; the runner consumes the map.
    pub partners: Option<BTreeMap<String, Vec<PartnerScript>>>,
    /// Fixed fixture values for the scenario; the layered environment
    /// source reads these before any ambient value.
    pub env: Option<BTreeMap<String, String>>,
    /// Ambient variable names allowed to pass through to the scenario.
    pub env_passthrough: Option<Vec<String>>,
    /// Profile pinned per document; an ambient profile would break
    /// hermeticity.
    pub profile: Option<String>,
    /// Document-level bound for every `send` action (rc-tr4w): an
    /// optional tighter deadline than the runner's thirty-second
    /// default, real time only (ADR-0069 §6).
    pub send_deadline: Option<Duration>,
    /// The document-level `inbound:` declaration (rc-5yon): the
    /// harness binds `127.0.0.1:0`, stages the listener on the HTTP
    /// component's global registry (ADR-0070), and exposes the bound
    /// address under the named bind variable so route URIs interpolate
    /// it. Provisioning runs behind the `http` feature; a declaration
    /// in a build without the feature is a named load error (ADR-0069
    /// §8 demand-gated activation).
    pub inbound: Option<InboundListener>,
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
        /// Reply assertion for `direct:` sends (rc-qvz6): the same
        /// matcher grammar `validate` parses, evaluated against the
        /// synchronous route reply the context-stimulus adapter
        /// returns. Load-time rejected on every other scheme.
        expect_reply: Option<Expectation>,
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
        /// a scenario variable, or a partner's recorded traffic.
        target: ScenarioTarget,
        /// Matcher expectation: the message grammar for `lastReceived`
        /// and `variable` targets, the partner count grammar for
        /// `partner` targets.
        expectation: ValidateExpectation,
        /// Optional poll deadline. Only valid on `partner` targets,
        /// whose counts settle asynchronously; without it the partner
        /// assertion reads one immediate snapshot.
        deadline: Option<Duration>,
        /// Optional minimum wire-arrival age. Only valid on
        /// `lastReceived` targets: the last received message must have
        /// arrived at least this long after the scenario started (the
        /// not-before-X control `run.sh` expresses with `awk`). The
        /// assertion anchors to the message's wire arrival, never the
        /// consumption time.
        elapsed_at_least: Option<Duration>,
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
                ScenarioTarget::Partner(_) => Vec::new(),
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
    /// A partner endpoint: the assertion reads the partner's recorded
    /// request traffic. The URI must equal a harness endpoint
    /// reference declared by the scenario's own `send`/`receive`
    /// actions.
    Partner(EndpointRef),
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

/// The document's `inbound:` declaration (rc-5yon): v1 grammar is a
/// single map `inbound: {bindVar: NAME}`. The harness provisions one
/// listener per document, binds `127.0.0.1:0`, stages it on the HTTP
/// component's global registry (ADR-0070 staged consumption), and
/// fills the bind variable with `http://<bound-address>` so route
/// consumer URIs interpolate the staged socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundListener {
    /// Scenario variable name the harness fills with the staged
    /// listener's `http://<bound-address>` URL.
    pub bind_var: String,
}

/// The scripted responses a document's `partners:` entry maps to, for
/// one endpoint key. `None` when the document declares no entry for
/// the endpoint — the caller binds a permissive partner. `Some` maps
/// each script grammar entry to its wire form: absent `status`
/// defaults to 200, absent `times` to 1 (serve once), absent headers
/// to the empty map, and the body follows the client send path's
/// `value_to_wire` encoding (empty when absent); `delay` and `fault`
/// map through.
///
/// The canonical `PartnerScript` → wire-form mapping; the CLI driver
/// and library-level scenarios bind partners through this function so
/// the semantics live in exactly one place.
#[cfg(feature = "http")]
pub fn partner_scripts_for(
    doc: &ScenarioDocument,
    endpoint: &str,
) -> Option<Vec<crate::adapters::http::ScriptedResponse>> {
    use crate::adapters::http::ScriptedResponse;
    let scripts = doc.partners.as_ref()?.get(endpoint)?;
    Some(
        scripts
            .iter()
            .map(|script| {
                let (status, headers, body) = match script.response.as_ref() {
                    Some(response) => (
                        response.status.unwrap_or(200),
                        response.headers.clone().unwrap_or_default(),
                        response.body.as_ref().map_or_else(Vec::new, |value| {
                            crate::adapters::http::value_to_wire(value)
                        }),
                    ),
                    // Fault entries carry no response; the placeholder
                    // keeps the wire form — serve checks the fault
                    // first, so the placeholder never reaches the wire.
                    None => (200, BTreeMap::new(), Vec::new()),
                };
                ScriptedResponse {
                    method: script.method.clone(),
                    path: script.path.clone(),
                    times: script.times.unwrap_or(1),
                    delay: script.delay,
                    fault: script.fault.clone(),
                    status,
                    headers,
                    body,
                }
            })
            .collect(),
    )
}

/// The expectation of a `validate` action, keyed by its target: the
/// message matcher grammar for `lastReceived` and `variable` targets,
/// the partner count grammar for `partner` targets.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ValidateExpectation {
    /// Message matcher expectation (`lastReceived` / `variable`).
    Message(Expectation),
    /// Partner request-count expectation (`partner`).
    Partner(PartnerExpectation),
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
    // Document-level partner scripting: the raw map stays
    // endpoint-keyed with raw sequence values; conversion runs during
    // validation so errors can name the entry key.
    partners: Option<BTreeMap<String, serde_yaml::Value>>,
    // Document-level send bound: raw humantime string; parsed during
    // validation so the error names the field.
    send_deadline: Option<String>,
    // Document-level inbound listener declaration: raw node; the
    // grammar walk runs during validation so unknown fields name
    // themselves in every build, and the `http` feature gate fires
    // after structure (ADR-0069 §8 demand-gated activation).
    inbound: Option<serde_yaml::Value>,
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
    /// Raw `expectReply` node; optional, `direct:` sends only.
    /// Validation converts it through the same matcher grammar
    /// `validate` uses, and rejects it on every other scheme.
    expect_reply: Option<Value>,
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
    /// `variable` / `partner`) converts during validation.
    target: serde_yaml::Value,
    expectation: Value,
    /// Raw humantime string; partner targets only, parsed during
    /// validation so the error can name the action index.
    deadline: Option<String>,
    /// Raw humantime string; `lastReceived` targets only, parsed
    /// during validation so the error can name the action index.
    elapsed_at_least: Option<String>,
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
///   `Validation`, `ReservedEnvKey`, `InlineRoutes`,
///   `InlineRoutesRejected`, `ProvisioningWithoutAuthority`,
///   `ExpectReplyOnUnsupportedSend`.
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
    /// A `provisioning: harness` endpoint reference declares a
    /// `bindVar` while its scheme (`direct:` or `fake:`) binds no
    /// partner, so the variable would never receive a bound authority
    /// and the entry fails later as a verdict-class var-resolution
    /// error (rc-j87j). Rejected at load instead.
    #[error(
        "doc-validation: endpoint `{endpoint}` declares `bindVar` but its `{ref_scheme}:` reference binds no harness partner, so the variable would never receive a bound authority (exit-2 doc-validation class)"
    )]
    ProvisioningWithoutAuthority {
        /// The endpoint whose reference cannot fill the variable.
        endpoint: String,
        /// The scheme of the endpoint reference (`direct` or `fake`).
        ref_scheme: String,
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
    /// A `partners` entry failed validation; `endpoint` is the entry
    /// key of the failing script list.
    #[error("doc-validation: partners[{endpoint}]: {message}")]
    Partners {
        /// The endpoint key of the failing entry.
        endpoint: String,
        /// What failed.
        message: String,
    },
    /// Inline `routes` failed to parse.
    #[error("doc-validation: inline routes: {0}")]
    InlineRoutes(String),
    /// The document's route source is inline `routes`. Inline
    /// definitions cannot boot in v1; the author must declare
    /// `routeFiles`. Rejected at load, before partners bind, instead
    /// of failing the boot afterward (rc-9dpx).
    #[error(
        "doc-validation: inline `routes` are rejected at load: declare `routeFiles` instead (inline definitions cannot boot in the scenario tier; exit 2)"
    )]
    InlineRoutesRejected,
    /// A send declares `expectReply` on a scheme that produces no
    /// synchronous reply: only the context-stimulus `direct:` send
    /// returns one. Partner sends (`http`/`https`) park their
    /// roundtrips for a later `receive`, and `fake:` adapters record
    /// sends without answering, so the assertion could never run
    /// (rc-qvz6). Rejected at load, naming the action index, the
    /// scheme, and the literal `expectReply` field.
    #[error(
        "doc-validation: scenario[{index}]: `expectReply` is only valid on a `direct:` send, not `{scheme}` (exit 2)"
    )]
    ExpectReplyOnUnsupportedSend {
        /// Zero-based position of the action in the `scenario:` list.
        index: usize,
        /// The scheme of the send's endpoint reference.
        scheme: String,
    },
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
/// is declared, and it is not inline (`routes` cannot boot in v1, so
/// the defect fails at load instead of at boot);
/// (f) each action converts (single-key dispatch, deadlines, durations,
/// endpoint provisioning, expectation grammar, the `direct:`-only
/// `expectReply` gate) with action-index errors; (g) each `partners`
/// entry converts (script grammar, response status range) with
/// entry-key errors; (h) no `env` key collides with a declared
/// `bindVar`; (i) each `partner` validate target URI equals
/// a harness endpoint reference declared by the scenario's own
/// `send`/`receive` actions. The optional `inbound:` section converts
/// between (g) and (h): grammar in every build, activation
/// demand-gated behind `http` (ADR-0069 §8).
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
    // (e, rc-9dpx) Inline route sources cannot boot in v1; reject at
    // load, before partners bind, instead of failing the boot after
    // the composition root is up. The boot keeps its own rejection as
    // defense-in-depth.
    if matches!(route_source, RouteSource::Inline(_)) {
        return Err(DocError::InlineRoutesRejected);
    }
    // (f) Action conversion.
    let mut scenario = Vec::with_capacity(raw_scenario.len());
    for (index, item) in raw_scenario.into_iter().enumerate() {
        scenario.push(build_action(item, index)?);
    }
    // (g) Partner scripting: entries convert from the raw sequence
    // with the entry key named on every failure; an empty sequence is
    // a valid, inert entry. The grammar conversion lives in the
    // partner-script module.
    let partners = crate::partner_script::partners_from_raw(raw.partners)?;
    // (g2) Inbound listener declaration: the grammar walk runs in
    // every build so grammar errors read identically with and without
    // the `http` feature; the feature gate fires inside, after
    // structure (ADR-0069 §8).
    let inbound = raw.inbound.map(inbound_from_raw).transpose()?;
    // (h) Reserved env keys: the harness binding wins over document
    // fixtures — both the endpoints' bindVars and, since rc-5yon, the
    // inbound listener's bindVar.
    if let Some(env) = raw.env.as_ref() {
        if let Some(inbound) = inbound.as_ref()
            && env.contains_key(&inbound.bind_var)
        {
            return Err(DocError::ReservedEnvKey {
                key: inbound.bind_var.clone(),
                endpoint: "inbound".to_string(),
            });
        }
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
    // (i) Partner-target cross-check: a `partner` validate target URI
    // must equal a harness endpoint reference declared by the
    // scenario's own `send`/`receive` actions (URI string equality).
    // A typo'd URI would otherwise assert against traffic nobody
    // records.
    let mut harness_uris: Vec<&str> = Vec::new();
    let mut partner_targets: Vec<(usize, &EndpointRef)> = Vec::new();
    for (index, action) in scenario.iter().enumerate() {
        match action {
            ScenarioAction::Send { to, .. } => {
                if to.provisioning == Some(Provisioning::Harness) {
                    harness_uris.push(to.endpoint.as_str());
                }
            }
            ScenarioAction::Receive { from, .. } => {
                if from.provisioning == Some(Provisioning::Harness) {
                    harness_uris.push(from.endpoint.as_str());
                }
            }
            ScenarioAction::Validate {
                target: ScenarioTarget::Partner(endpoint),
                ..
            } => partner_targets.push((index, endpoint)),
            _ => {}
        }
    }
    for (index, endpoint) in partner_targets {
        if !harness_uris.contains(&endpoint.endpoint.as_str()) {
            return Err(DocError::Validation {
                index,
                message: format!(
                    "validate `partner` target `{}` does not match any harness endpoint reference declared by this scenario's `send`/`receive` actions",
                    endpoint.endpoint
                ),
            });
        }
    }
    // Document-level send bound: optional; a present value goes
    // through the same humantime grammar as the action deadlines,
    // naming the field on failure (index 0 — the section, not an
    // action, failed).
    let send_deadline = raw
        .send_deadline
        .as_deref()
        .map(|raw_deadline| parse_duration(raw_deadline, 0, "sendDeadline"))
        .transpose()?;
    Ok(ScenarioDocument {
        source_path: path.to_path_buf(),
        route_source,
        scenario,
        partners,
        env: raw.env,
        env_passthrough: raw.env_passthrough,
        profile: raw.profile,
        send_deadline,
        inbound,
    })
}

/// Converts the raw `inbound:` node. The v1 grammar is a single map
/// `inbound: {bindVar: NAME}`; unknown fields are rejected naming the
/// key, mirroring the partners-section strictness. Structure is
/// validated in every build so grammar errors read identically with
/// and without the `http` feature; only a structurally valid
/// declaration reaches the demand gate (ADR-0069 §8), which rejects it
/// naming the section and the feature when the harness is built
/// without `http`. Section-level errors use index 0 — the section, not
/// an action, failed (the `sendDeadline` precedent).
fn inbound_from_raw(value: serde_yaml::Value) -> Result<InboundListener, DocError> {
    let section_error = |message: String| DocError::Validation { index: 0, message };
    let serde_yaml::Value::Mapping(ref map) = value else {
        return Err(section_error(format!(
            "`inbound` must be a map with a `bindVar` key, got {value:?}"
        )));
    };
    let mut bind_var: Option<String> = None;
    for (key, value) in map {
        match key.as_str() {
            "bindVar" => {
                let text = value.as_str().ok_or_else(|| {
                    section_error(format!(
                        "`inbound`: `bindVar` must be a string, got {value:?}"
                    ))
                })?;
                bind_var = Some(text.to_string());
            }
            other => {
                return Err(section_error(format!(
                    "`inbound`: unknown field `{other}`; expected `bindVar`"
                )));
            }
        }
    }
    let bind_var =
        bind_var.ok_or_else(|| section_error("`inbound` requires a `bindVar` key".to_string()))?;
    // Demand-gated activation (ADR-0069 §8): the grammar parsed; the
    // activation needs the `http` feature, which provisions the
    // listener.
    #[cfg(not(feature = "http"))]
    {
        let _ = bind_var;
        Err(section_error(
            "`inbound` requires the `http` feature, which this harness build does \
             not enable: rebuild with `--features http` (demand-gated activation)"
                .to_string(),
        ))
    }
    #[cfg(feature = "http")]
    Ok(InboundListener { bind_var })
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
            // (rc-qvz6) `expectReply` reads the synchronous reply only
            // the context-stimulus `direct:` send produces; partner
            // sends park their roundtrips for a later `receive` and
            // fake adapters record sends without answering, so the
            // assertion is rejected at load on every other scheme.
            let scheme = ref_scheme(&raw.to.endpoint);
            if raw.expect_reply.is_some() && scheme != Some("direct") {
                return Err(DocError::ExpectReplyOnUnsupportedSend {
                    index,
                    // A scheme-less reference names no scheme to
                    // render; the explicit phrase keeps the
                    // diagnostic from degrading to an empty name.
                    scheme: scheme.unwrap_or("no scheme").to_string(),
                });
            }
            let expect_reply = raw
                .expect_reply
                .map(|value| expectation_from_value(&value, index, "expectReply"))
                .transpose()?;
            Ok(ScenarioAction::Send {
                to: endpoint_from_raw(raw.to)?,
                body: raw.body,
                headers: raw.headers,
                method,
                expect_reply,
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
            let target = build_target(&raw.target, index)?;
            let deadline = match raw.deadline.as_deref() {
                None => None,
                // The poll deadline exists because a partner count
                // settles asynchronously; on any other target it has
                // no meaning and is a grammar error.
                Some(raw_deadline) if matches!(target, ScenarioTarget::Partner(_)) => {
                    Some(parse_duration(raw_deadline, index, "deadline")?)
                }
                Some(raw_deadline) => {
                    return Err(action_error(format!(
                        "`deadline` is only valid on a `partner` validate target, got `{raw_deadline}`"
                    )));
                }
            };
            let elapsed_at_least = match raw.elapsed_at_least.as_deref() {
                None => None,
                // The elapsed bound measures the wire arrival of the
                // last received message against the scenario start;
                // only that target carries an arrival to measure.
                Some(raw_bound) if matches!(target, ScenarioTarget::LastReceived(_)) => {
                    Some(parse_duration(raw_bound, index, "elapsedAtLeast")?)
                }
                Some(raw_bound) => {
                    return Err(action_error(format!(
                        "`elapsedAtLeast` is only valid on a `lastReceived` validate target, got `{raw_bound}`"
                    )));
                }
            };
            let expectation = match &target {
                ScenarioTarget::Partner(_) => ValidateExpectation::Partner(
                    partner_expectation_from_value(&raw.expectation, index)?,
                ),
                _ => ValidateExpectation::Message(expectation_from_value(
                    &raw.expectation,
                    index,
                    "expectation",
                )?),
            };
            Ok(ScenarioAction::Validate {
                target,
                expectation,
                deadline,
                elapsed_at_least,
            })
        }
        other => Err(action_error(format!(
            "unknown action `{other}`; expected `send`, `receive`, `sleep`, or `validate`"
        ))),
    }
}

/// Builds a `validate` target from the raw `target` node: a single-key
/// map (`lastReceived`, `variable`, or `partner`).
fn build_target(value: &serde_yaml::Value, index: usize) -> Result<ScenarioTarget, DocError> {
    let action_error = |message: String| DocError::Validation { index, message };
    let serde_yaml::Value::Mapping(map) = value else {
        return Err(action_error(format!(
            "validate `target` must be a single-key map (`lastReceived`, `variable`, `partner`), got {value:?}"
        )));
    };
    let Some((key, content)) = map.iter().next() else {
        return Err(action_error(
            "validate `target` must be a single-key map (`lastReceived`, `variable`, `partner`), got an empty map"
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
        "partner" => {
            let raw: RawEndpointRef =
                serde_yaml::from_value(content.clone()).map_err(|e| action_error(e.to_string()))?;
            Ok(ScenarioTarget::Partner(endpoint_from_raw(raw)?))
        }
        other => Err(action_error(format!(
            "unknown validate target `{other}`; expected `lastReceived`, `variable`, or `partner`"
        ))),
    }
}

/// Applies the provisioning gate: only `harness` (or absent) passes,
/// and a harness entry whose reference scheme binds no partner
/// (`direct:`, `fake:`) must not declare a `bindVar` — the variable
/// would never receive a bound authority (rc-j87j). A
/// `direct:`/`fake:` entry without a `bindVar` stays legal.
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
    if provisioning == Some(Provisioning::Harness)
        && raw.bind_var.is_some()
        && let Some(scheme) = ref_scheme(&raw.endpoint)
        && (scheme == "direct" || scheme == "fake")
    {
        return Err(DocError::ProvisioningWithoutAuthority {
            endpoint: raw.endpoint.clone(),
            ref_scheme: scheme.to_string(),
        });
    }
    Ok(EndpointRef {
        endpoint: raw.endpoint,
        provisioning,
        bind_var: raw.bind_var,
    })
}

/// The scheme prefix of an endpoint URI: the non-empty text before
/// the first `:`, or `None` when the URI carries no scheme — which
/// requires the separator; a colon-less string (`orders`) is a bare
/// name, not a scheme.
fn ref_scheme(endpoint: &str) -> Option<&str> {
    let (scheme, _) = endpoint.split_once(':')?;
    (!scheme.is_empty()).then_some(scheme)
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
/// mirror the mock-testkit matcher rules. The field name parameter
/// (`expectation`, `expectReply`) keeps one verb parser behind both
/// readers (rc-qvz6): the verbs never fork between `validate` and
/// send-level reply assertions.
fn expectation_from_value(
    value: &Value,
    index: usize,
    field: &'static str,
) -> Result<Expectation, DocError> {
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
                        "{field}: `{key}` requires a string payload"
                    )));
                };
                if key.as_str() == "regex"
                    && let Err(e) = regex::Regex::new(pattern)
                {
                    return Err(invalid(format!("{field}: invalid regex `{pattern}`: {e}")));
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
                    Err(invalid(format!("{field}: `exists` takes no argument")))
                }
            }
            _ => {
                if payload.is_object() {
                    Ok(Expectation::JsonSubset(payload.clone()))
                } else {
                    Err(invalid(format!("{field}: `jsonSubset` must be an object")))
                }
            }
        };
    }
    Ok(Expectation::Equals(value.clone()))
}

/// Applies the partner expectation grammar: a map with exactly one
/// count bound (`count`; or `atLeast`, `atMost`, or their range), an
/// optional `method` string, at most one path filter (`path`,
/// `pathContains`, `pathMatches` — the regex compiled at load), and
/// an optional `query` subset map of string keys to string values;
/// unknown keys fail. Field-by-field extraction, like the
/// endpoint-reference reader, so errors name the offending key.
fn partner_expectation_from_value(
    value: &Value,
    index: usize,
) -> Result<PartnerExpectation, DocError> {
    const FIELD: &str = "partner expectation";
    const KEYS: &[&str] = &[
        "count",
        "atLeast",
        "atMost",
        "method",
        "path",
        "pathContains",
        "pathMatches",
        "query",
    ];
    let invalid = |message: String| DocError::Validation { index, message };
    let Value::Object(map) = value else {
        return Err(invalid(format!(
            "{FIELD} must be a map with a count bound, got {value:?}"
        )));
    };
    let mut count: Option<u64> = None;
    let mut at_least: Option<u64> = None;
    let mut at_most: Option<u64> = None;
    let mut method: Option<String> = None;
    let mut path: Option<PathFilter> = None;
    let mut path_key: Option<&str> = None;
    let mut query: Option<BTreeMap<String, String>> = None;
    for (key, payload) in map {
        match key.as_str() {
            "count" | "atLeast" | "atMost" => {
                let bound = payload.as_u64().ok_or_else(|| {
                    invalid(format!(
                        "{FIELD}: `{key}` must be a non-negative integer, got {payload}"
                    ))
                })?;
                match key.as_str() {
                    "count" => count = Some(bound),
                    "atLeast" => at_least = Some(bound),
                    _ => at_most = Some(bound),
                }
            }
            "method" => {
                let text = payload.as_str().ok_or_else(|| {
                    invalid(format!("{FIELD}: `{key}` must be a string, got {payload}"))
                })?;
                method = Some(text.to_string());
            }
            "path" | "pathContains" | "pathMatches" => {
                if let Some(first) = path_key {
                    return Err(invalid(format!(
                        "{FIELD}: `{first}` and `{key}` are exclusive: at most one path filter"
                    )));
                }
                let text = payload.as_str().ok_or_else(|| {
                    invalid(format!("{FIELD}: `{key}` must be a string, got {payload}"))
                })?;
                path = Some(match key.as_str() {
                    "path" => PathFilter::Exact(text.to_string()),
                    "pathContains" => PathFilter::Contains(text.to_string()),
                    _ => {
                        if let Err(e) = regex::Regex::new(text) {
                            return Err(invalid(format!("{FIELD}: invalid regex `{text}`: {e}")));
                        }
                        PathFilter::Matches(text.to_string())
                    }
                });
                path_key = Some(key.as_str());
            }
            "query" => {
                let Value::Object(pairs) = payload else {
                    return Err(invalid(format!(
                        "{FIELD}: `query` must be a map of string keys to string values, got {payload}"
                    )));
                };
                let mut subset = BTreeMap::new();
                for (name, pair) in pairs {
                    let Some(text) = pair.as_str() else {
                        return Err(invalid(format!(
                            "{FIELD}: `query` value for `{name}` must be a string, got {pair}"
                        )));
                    };
                    subset.insert(name.clone(), text.to_string());
                }
                query = Some(subset);
            }
            other => {
                return Err(invalid(format!(
                    "{FIELD}: unknown field `{other}`; expected {}",
                    backticked(KEYS)
                )));
            }
        }
    }
    if count.is_some() && (at_least.is_some() || at_most.is_some()) {
        let mut others: Vec<&str> = Vec::new();
        if at_least.is_some() {
            others.push("atLeast");
        }
        if at_most.is_some() {
            others.push("atMost");
        }
        return Err(invalid(format!(
            "{FIELD}: `count` and {} are exclusive: declare exactly one bound form",
            backticked(&others)
        )));
    }
    let bound = if let Some(exact) = count {
        CountBound::Exact(exact)
    } else if let (Some(min), Some(max)) = (at_least, at_most) {
        if min > max {
            return Err(invalid(format!(
                "{FIELD}: `atLeast` ({min}) must not exceed `atMost` ({max})"
            )));
        }
        CountBound::Range(min, max)
    } else if let Some(n) = at_least {
        CountBound::AtLeast(n)
    } else if let Some(n) = at_most {
        CountBound::AtMost(n)
    } else {
        return Err(invalid(format!(
            "{FIELD}: requires a count bound: `count`, `atLeast`, or `atMost`"
        )));
    };
    Ok(PartnerExpectation {
        bound,
        method,
        path,
        query,
    })
}

/// Backticks and comma-joins field names for error messages.
fn backticked(fields: &[&str]) -> String {
    fields
        .iter()
        .map(|field| format!("`{field}`"))
        .collect::<Vec<_>>()
        .join(", ")
}
