//! Document model for `camel job`: the `execute:` section of the
//! `*.test.yaml` family.
//!
//! The grammar parent is the camel-integration-test scenario action
//! model (a `send` with uri/body/headers), but the types are job-local:
//! reusing `ScenarioAction` would drag the hermetic interpreter
//! machinery (LayeredEnv, partner adapters) into a path that must run
//! against the REAL boot environment, so the sanctioned fallback (a
//! minimal job-local send-action struct parsed directly) applies. Route
//! resolution reuses the document-family semantics verbatim: the same
//! `routeFiles` / `routeFilesFromRoot` / `routes` keys, the same
//! exactly-one conflict rule ([`TestDocError::RouteSourceConflict`]),
//! and the same strict nearest-ancestor `Camel.toml` walk for
//! `routeFilesFromRoot`.

use serde::Deserialize as _;
use std::collections::HashMap;
use std::path::Path;

use noyalib::compat::serde_yaml;

use crate::commands::test::document::TestDocError;
use crate::commands::test::runner::find_camel_toml_root;

/// Sentinel embedded by the body deserializer; extracted during parse so
/// the scalar name survives serde's error rendering (family parity with
/// `commands::test` `BODY_SCALAR_SENTINEL`).
const BODY_SCALAR_SENTINEL: &str = "unsupported body scalar: ";

/// Consumer schemes a one-shot job document may start routes for. The
/// gate is fail-closed: every other `from:` scheme is rejected at load
/// (producers/sinks as `to:` URIs are unrestricted). See the cli-jobs
/// spec delta.
pub(crate) const JOB_SAFE_CONSUMER_SCHEMES: [&str; 4] = ["direct", "seda", "log", "mock"];

/// Schemes the single `send` action may target (v1: in-memory, synchronous
/// request/reply transports).
const JOB_SEND_SCHEMES: [&str; 2] = ["direct", "seda"];

/// A parsed job document: one top-level `execute:` section plus exactly
/// one route-source key.
#[derive(Debug)]
pub(crate) struct JobDocument {
    /// The `execute:` section.
    pub(crate) execute: ExecuteSection,
    /// Route files relative to the document's directory.
    pub(crate) route_files: Option<Vec<String>>,
    /// Route files relative to the nearest ancestor `Camel.toml` root.
    pub(crate) route_files_from_root: Option<Vec<String>>,
    /// Inline route definitions (same schema as route files).
    pub(crate) routes: Option<serde_yaml::Value>,
}

/// The top-level `execute:` section.
#[derive(Debug)]
pub(crate) struct ExecuteSection {
    /// Execution mode; v1 accepts only `one-shot` (`batch` is reserved).
    pub(crate) mode: String,
    /// The single send action.
    pub(crate) send: JobSendAction,
    /// Whether the JSON report carries the reply exchange body/headers.
    pub(crate) capture_reply: bool,
    /// Mandatory overall timeout. Covers the WHOLE run — boot, send,
    /// drain, and teardown — anchored at process start, not at send
    /// time.
    pub(crate) timeout: std::time::Duration,
}

/// The single send action: endpoint URI plus message content.
#[derive(Debug)]
pub(crate) struct JobSendAction {
    /// Target endpoint URI; must use the `direct:` or `seda:` scheme.
    pub(crate) to: String,
    /// Body restricted to string (`Text`) or object/array (`Json`) forms.
    pub(crate) body: Option<JobBody>,
    /// Headers attached to the message.
    pub(crate) headers: Option<HashMap<String, serde_json::Value>>,
}

/// Accepted body forms (family parity with the unit-tier `InputBody`).
#[derive(Debug)]
pub(crate) enum JobBody {
    /// YAML string body.
    Text(String),
    /// YAML object or array body, carried as JSON.
    Json(serde_json::Value),
}

/// Parse and validation errors for job documents.
#[derive(Debug)]
pub(crate) enum JobDocError {
    /// The path lacks the reserved job-document suffix.
    NotJobSuffix { path: String },
    /// Malformed YAML or a type mismatch at the serde layer.
    Yaml(String),
    /// `deny_unknown_fields` rejection.
    UnknownField(String),
    /// The document declares no `execute:` section.
    MissingExecute,
    /// The document declares both `execute:` and `scenario:`.
    ExclusiveWithScenario,
    /// The document mixes `execute:` with unit-tier test sections.
    /// Belt-and-suspenders after the suffix split: the families are
    /// suffix-separated now, so this catches a copy-paste author who
    /// renamed a test to `.job.yaml` but left test vocabulary inside.
    MixedVocabulary { sections: Vec<&'static str> },
    /// The `execute:` section declares no `mode`.
    MissingMode,
    /// `mode: batch` is reserved for a future release.
    BatchReserved,
    /// `mode` holds an unrecognized value.
    UnsupportedMode(String),
    /// The `execute:` section declares no `timeout`.
    MissingTimeout,
    /// `timeout` is missing, unparsable, or non-positive.
    InvalidTimeout(String),
    /// The send target lacks the required scheme.
    UnsupportedSendScheme { to: String },
    /// A body scalar (null/boolean/number) is not a supported body form.
    UnsupportedBodyScalar(String),
    /// Route-source resolution failed (conflict, no project root).
    RouteSource(TestDocError),
}

impl std::fmt::Display for JobDocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotJobSuffix { path } => write!(
                f,
                "job document {path} must use the reserved .job.yaml/.job.yml suffix (rename it — .test.yaml names a camel test document)"
            ),
            Self::Yaml(raw) => write!(f, "invalid job document: {raw}"),
            Self::UnknownField(raw) => write!(f, "unknown field in job document: {raw}"),
            Self::MissingExecute => {
                write!(f, "job document requires an execute: section")
            }
            Self::ExclusiveWithScenario => {
                write!(f, "execute: and scenario: are mutually exclusive sections")
            }
            Self::MixedVocabulary { sections } => write!(
                f,
                "execute: is mutually exclusive with the test sections {}",
                sections
                    .iter()
                    .map(|s| format!("`{s}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::MissingMode => write!(f, "execute.mode is required"),
            Self::BatchReserved => write!(
                f,
                "execute.mode `batch` is not available yet; only `one-shot` is supported"
            ),
            Self::UnsupportedMode(mode) => write!(
                f,
                "unsupported execute.mode `{mode}`: expected `one-shot` (`batch` is reserved)"
            ),
            Self::MissingTimeout => write!(f, "execute.timeout is required"),
            Self::InvalidTimeout(raw) => write!(
                f,
                "invalid execute.timeout `{raw}`: expected a positive duration (e.g. `30s`)"
            ),
            Self::UnsupportedSendScheme { to } => {
                write!(f, "send target `{to}` must start with `direct:` or `seda:`")
            }
            Self::UnsupportedBodyScalar(raw) => write!(
                f,
                "unsupported body scalar `{raw}`: only string, object, and array bodies are supported"
            ),
            Self::RouteSource(err) => write!(f, "{err}"),
        }
    }
}

/// Raw serde shape of the `execute:` section; validation runs after
/// deserialization so each rule produces its own precise error.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ExecuteSectionDoc {
    mode: Option<String>,
    #[serde(default)]
    send: Option<JobSendActionDoc>,
    /// The brief's grammar spells the key `capture-reply` (kebab), unlike
    /// the camelCase family fields; renamed explicitly.
    #[serde(rename = "capture-reply")]
    capture_reply: Option<bool>,
    timeout: Option<String>,
}

/// Raw serde shape of the send action.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct JobSendActionDoc {
    to: String,
    #[serde(default, deserialize_with = "deserialize_option_job_body")]
    body: Option<JobBody>,
    headers: Option<HashMap<String, serde_json::Value>>,
}

/// Top-level raw serde shape of a job document.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct JobDocumentDoc {
    execute: ExecuteSectionDoc,
    /// Optional one-line description shown by `camel job` listing
    /// (read via the listing probe; the full grammar only accepts it).
    #[serde(default)]
    description: Option<String>,
    route_files: Option<Vec<String>>,
    route_files_from_root: Option<Vec<String>>,
    routes: Option<serde_yaml::Value>,
}

/// Maps a deserialized JSON value to [`JobBody`], rejecting scalars with
/// a message whose [`BODY_SCALAR_SENTINEL`] prefix is the classification
/// protocol used by [`parse_job_document`].
fn job_body_from_value(value: serde_json::Value) -> Result<Option<JobBody>, String> {
    match &value {
        serde_json::Value::String(s) => Ok(Some(JobBody::Text(s.clone()))),
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
            Ok(Some(JobBody::Json(value)))
        }
        scalar => Err(format!("{BODY_SCALAR_SENTINEL}{scalar}")),
    }
}

/// Field-level deserializer for the optional `body` field: a missing
/// field never reaches this helper, while an explicit `body: null`
/// arrives as `Value::Null` and is rejected (family parity).
fn deserialize_option_job_body<'de, D>(deserializer: D) -> Result<Option<JobBody>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    job_body_from_value(value).map_err(serde::de::Error::custom)
}

/// Unit-tier test sections that must not appear next to `execute:`.
/// Test vocabulary does not belong in a job document; the families are
/// suffix-separated and these guards are belt-and-suspenders.
const TEST_VOCABULARY_KEYS: [&str; 8] = [
    "inputs",
    "expects",
    "intercepts",
    "beans",
    "repositories",
    "sequence",
    "settle",
    "env",
];

/// Parse one job document: suffix contract, section exclusivity, serde
/// shape, and v1 grammar rules (`mode: one-shot`, mandatory `timeout`,
/// one `direct:`/`seda:` send, exactly one route source).
pub(crate) fn parse_job_document(path: &Path, text: &str) -> Result<JobDocument, JobDocError> {
    if !camel_dsl::discovery::is_job_document(path) {
        return Err(JobDocError::NotJobSuffix {
            path: path.display().to_string(),
        });
    }
    let value = serde_yaml::from_str::<serde_yaml::Value>(text)
        .map_err(|e| JobDocError::Yaml(e.to_string()))?;
    let has = |key: &str| value.get(key).is_some();
    if !has("execute") {
        return Err(JobDocError::MissingExecute);
    }
    if has("scenario") {
        return Err(JobDocError::ExclusiveWithScenario);
    }
    let mixed: Vec<&'static str> = TEST_VOCABULARY_KEYS
        .iter()
        .copied()
        .filter(|key| has(key))
        .collect();
    if !mixed.is_empty() {
        return Err(JobDocError::MixedVocabulary { sections: mixed });
    }

    let raw = serde_yaml::from_str::<JobDocumentDoc>(text).map_err(|e| classify(&e.to_string()))?;

    // Route-source conflict: the family rule, verbatim.
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
    if present.len() != 1 {
        return Err(JobDocError::RouteSource(
            TestDocError::RouteSourceConflict { present },
        ));
    }

    // Grammar rules with per-rule errors.
    let execute = raw.execute;
    let mode = execute.mode.ok_or(JobDocError::MissingMode)?;
    match mode.as_str() {
        "one-shot" => {}
        "batch" => return Err(JobDocError::BatchReserved),
        other => return Err(JobDocError::UnsupportedMode(other.to_string())),
    }
    let timeout_raw = execute.timeout.ok_or(JobDocError::MissingTimeout)?;
    let timeout = humantime::parse_duration(&timeout_raw)
        .ok()
        .filter(|d| *d > std::time::Duration::ZERO)
        .ok_or_else(|| JobDocError::InvalidTimeout(timeout_raw.clone()))?;
    let send = execute.send.ok_or(JobDocError::Yaml(
        "execute.send is required: exactly one send action".to_string(),
    ))?;
    if !JOB_SEND_SCHEMES
        .iter()
        .any(|scheme| send.to.starts_with(&format!("{scheme}:")))
    {
        return Err(JobDocError::UnsupportedSendScheme { to: send.to });
    }

    Ok(JobDocument {
        execute: ExecuteSection {
            mode,
            send: JobSendAction {
                to: send.to,
                body: send.body,
                headers: send.headers,
            },
            capture_reply: execute.capture_reply.unwrap_or(false),
            timeout,
        },
        route_files: raw.route_files,
        route_files_from_root: raw.route_files_from_root,
        routes: raw.routes,
    })
}

/// Classify a noyalib (serde_yaml compat) error text: the body-scalar
/// sentinel is extracted first so the scalar name survives; `unknown
/// field` keeps its serde rendering; everything else stays raw (family
/// parity with `commands::test::document_parse::classify_yaml_error`).
fn classify(raw: &str) -> JobDocError {
    if let Some((_, after)) = raw.split_once(BODY_SCALAR_SENTINEL) {
        let scalar = after.split_whitespace().next().unwrap_or_default();
        return JobDocError::UnsupportedBodyScalar(scalar.to_string());
    }
    if raw.contains("unknown field") {
        return JobDocError::UnknownField(raw.to_string());
    }
    JobDocError::Yaml(raw.to_string())
}

/// The resolved route source of a job document.
pub(crate) enum JobRouteSource {
    /// Discovery patterns (absolute paths) for the file forms; loaded
    /// through the real-boot discovery seam (ambient `${env:}`).
    Patterns(Vec<String>),
    /// Inline `routes:` re-serialized for `parse_routes_with_env`.
    Inline(String),
}

/// Resolve the document's route source into discovery patterns or inline
/// text. `routeFiles` resolves against the document's directory;
/// `routeFilesFromRoot` against the nearest ancestor `Camel.toml`
/// directory (the strict family walk, no such root is a document error).
pub(crate) fn resolve_route_source(
    doc: &JobDocument,
    doc_dir: &Path,
) -> Result<JobRouteSource, JobDocError> {
    if let Some(files) = &doc.route_files_from_root {
        let root = find_camel_toml_root(doc_dir).ok_or_else(|| {
            JobDocError::RouteSource(TestDocError::NoProjectRoot {
                doc_dir: doc_dir.display().to_string(),
            })
        })?;
        Ok(JobRouteSource::Patterns(
            files
                .iter()
                .map(|p| root.join(p).display().to_string())
                .collect(),
        ))
    } else if let Some(files) = &doc.route_files {
        Ok(JobRouteSource::Patterns(
            files
                .iter()
                .map(|p| doc_dir.join(p).display().to_string())
                .collect(),
        ))
    } else if let Some(value) = &doc.routes {
        let mut mapping = serde_yaml::Mapping::new();
        mapping.insert("routes", value.clone());
        let text = serde_yaml::to_string(&serde_yaml::Value::Mapping(mapping))
            .map_err(|e| JobDocError::Yaml(format!("failed to serialize inline routes: {e}")))?;
        Ok(JobRouteSource::Inline(text))
    } else {
        // Unreachable: parse_job_document enforces exactly one source.
        Err(JobDocError::RouteSource(
            TestDocError::RouteSourceConflict {
                present: Vec::new(),
            },
        ))
    }
}

/// The scheme of a consumer URI: the text before the first `:`. `None`
/// means the URI has no scheme separator (fail-closed at the gate).
pub(crate) fn scheme_of_uri(uri: &str) -> Option<&str> {
    uri.split_once(':').map(|(scheme, _)| scheme)
}

/// The base of a URI: everything before query options, so `direct:x?a=1`
/// matches a send to `direct:x`.
pub(crate) fn uri_base(uri: &str) -> &str {
    uri.split('?').next().unwrap_or(uri)
}

/// Force synchronous completion semantics on a `seda:` send target.
///
/// The seda producer defaults to `waitForTaskToComplete=IfReplyExpected`,
/// which for an InOnly job send means fire-and-forget: a failing route
/// would report `Completed` and `capture-reply` would echo the input.
/// `Always` makes the producer attach a reply channel and await the
/// pipeline result — the component honors it regardless of exchange
/// pattern (`WaitForTaskToComplete::Always => true` in the producer),
/// so the verdict and the captured reply reflect the route's outcome.
/// Any existing `waitForTaskToComplete` value is replaced.
pub(crate) fn seda_send_uri(to: &str) -> String {
    const PARAM: &str = "waitForTaskToComplete";
    match to.split_once('?') {
        None => format!("{to}?{PARAM}=Always"),
        Some((base, query)) => {
            let kept: Vec<&str> = query
                .split('&')
                .filter(|pair| !pair.starts_with(&format!("{PARAM}=")))
                .collect();
            let mut uri = String::from(base);
            uri.push('?');
            if !kept.is_empty() {
                uri.push_str(&kept.join("&"));
                uri.push('&');
            }
            uri.push_str(&format!("{PARAM}=Always"));
            uri
        }
    }
}

/// Route IDs of every route whose consumer (`from:`) base matches the
/// send target base. The runner accepts exactly one match: zero is a
/// missing-target error, more than one an ambiguous-target error (both
/// routes would be auto-started and either could consume the send).
pub(crate) fn target_route_ids(
    defs: &[camel_core::RouteDefinition],
    target_base: &str,
) -> Vec<String> {
    defs.iter()
        .filter(|def| uri_base(def.from_uri()) == target_base)
        .map(|def| def.route_id().to_string())
        .collect()
}

/// Validate one route's consumer URI against the fail-closed job-safe
/// allowlist. Producers/sinks as `to:` URIs are NOT checked — only the
/// `from:` scheme decides whether a route may auto-consume.
pub(crate) fn validate_consumer_uri(from_uri: &str) -> Result<(), String> {
    let scheme = scheme_of_uri(from_uri).unwrap_or_default();
    if JOB_SAFE_CONSUMER_SCHEMES.contains(&scheme) {
        Ok(())
    } else {
        Err(format!(
            "route consumes from `{from_uri}`; one-shot job documents allow only {} \
             consumers (producers/sinks as to: URIs are unrestricted); scheme `{scheme}` \
             is rejected",
            JOB_SAFE_CONSUMER_SCHEMES
                .iter()
                .map(|s| format!("`{s}:`"))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}
