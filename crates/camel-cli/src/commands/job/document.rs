//! Document model for `camel job`: the `execute:` section of the
//! `*.job.yaml` family (a `*.test.yaml` declaring `execute:` is a load
//! error with rename guidance).
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
use std::collections::{BTreeMap, HashMap};
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
    /// Declared top-level `args:` map; `None` selects the legacy
    /// implicit-header path for `--arg` pairs.
    pub(crate) args: Option<JobArgumentDeclarations>,
    /// Route files relative to the document's directory.
    pub(crate) route_files: Option<Vec<String>>,
    /// Route files relative to the nearest ancestor `Camel.toml` root.
    pub(crate) route_files_from_root: Option<Vec<String>>,
    /// Inline route definitions (same schema as route files).
    pub(crate) routes: Option<serde_yaml::Value>,
}

impl JobDocument {
    /// Whether `--arg` pairs take the legacy implicit-header path: true
    /// when the document declares no top-level `args:` block. Declared
    /// documents resolve pairs against [`JobDocument::args`] instead
    /// (pair resolution and defaults run inside
    /// [`parse_job_document_with_args`], before any field validation).
    pub(crate) fn legacy_arg_headers(&self) -> bool {
        self.args.is_none()
    }
}

/// The execution mode of a job document's `execute:` section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JobMode {
    /// Run the single send action immediately against the booted routes.
    OneShot,
    /// Batch mode: runs the same send path as one-shot and then drains
    /// until every seda queue is empty (see `commands::job::batch`).
    Batch,
}

impl JobMode {
    /// The document spelling of the mode (the `execute.mode` value).
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::OneShot => "one-shot",
            Self::Batch => "batch",
        }
    }
}

/// The top-level `execute:` section.
#[derive(Debug)]
pub(crate) struct ExecuteSection {
    /// Execution mode; accepted values are `one-shot` and `batch`.
    pub(crate) mode: JobMode,
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

/// One declared job argument: the strict per-entry shape of the
/// top-level `args:` map. Only `required`, `default`, and `description`
/// are admitted, values are string-only, and the name is the map key
/// validated against the identifier grammar.
#[derive(Debug, Clone)]
pub(crate) struct JobArgumentDeclaration {
    /// Whether the CLI must supply a value.
    pub(crate) required: bool,
    /// Value applied when the CLI omits the argument.
    pub(crate) default: Option<String>,
    /// Author documentation for the argument.
    pub(crate) description: Option<String>,
}

/// The declared top-level `args:` map, normalized: declarations keyed
/// by validated argument name (the ordered map keeps iteration and
/// diagnostics deterministic). Read by [`resolve_job_args`].
#[derive(Debug, Clone, Default)]
pub(crate) struct JobArgumentDeclarations {
    pub(crate) entries: BTreeMap<String, JobArgumentDeclaration>,
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
    /// A top-level `args:` name violates the identifier grammar.
    InvalidArgumentName { name: String },
    /// A top-level `args:` declaration contains a field outside the
    /// allowed `required`/`default`/`description` set.
    UnknownArgumentField { argument: String, field: String },
    /// A top-level `args:` declaration is not a mapping, or one of its
    /// fields has the wrong type (values remain string-only).
    InvalidArgumentDeclaration { argument: String, detail: String },
    /// A `--arg` pair names an argument the document does not declare
    /// (declared mode only; legacy documents accept any name as a
    /// header).
    UnknownArgumentName { name: String },
    /// A declared `required: true` argument without a `default` got no
    /// `--arg` value.
    MissingRequiredArgument { name: String },
    /// A `${arg:NAME}` token in a declared document's fields resolved
    /// to nothing: `NAME` was neither declared (no default, no CLI
    /// value) nor an `${arg:NAME:-fallback}` rejection, or an
    /// `${env:NAME}` reference had no matching environment variable.
    /// The scanner's `Err(var_name)` shape carries the name only.
    UnresolvedArgument { name: String },
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
            Self::UnsupportedMode(mode) => write!(
                f,
                "unsupported execute.mode `{mode}`: expected `one-shot` or `batch`"
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
            Self::InvalidArgumentName { name } => write!(
                f,
                "invalid argument name `{name}` in `args:`: names must match [A-Za-z_][A-Za-z0-9_]*"
            ),
            Self::UnknownArgumentField { argument, field } => write!(
                f,
                "unknown field `{field}` in the declaration of argument `{argument}`: expected `required`, `default`, or `description`"
            ),
            Self::InvalidArgumentDeclaration { argument, detail } => {
                write!(f, "invalid declaration for argument `{argument}`: {detail}")
            }
            Self::UnknownArgumentName { name } => write!(
                f,
                "unknown argument `{name}` in `--arg`: not declared in the document's `args:` block"
            ),
            Self::MissingRequiredArgument { name } => write!(
                f,
                "missing required argument `{name}`: pass --arg {name}=<value>"
            ),
            Self::UnresolvedArgument { name } => write!(
                f,
                "unresolved argument `{name}` in job document: no declared value \
                 and no matching environment variable"
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
    /// (read via the listing probe; the full grammar only admits the
    /// key — its value is intentionally dropped here).
    #[serde(default)]
    #[expect(dead_code, reason = "admit-only under deny_unknown_fields")]
    description: Option<String>,
    /// Optional declared-argument map. Held raw (`serde_yaml::Value`)
    /// and validated per declaration so every diagnostic can name its
    /// argument.
    #[serde(default)]
    args: Option<BTreeMap<String, serde_yaml::Value>>,
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
/// shape, and v1 grammar rules (`mode: one-shot`/`batch`, mandatory
/// `timeout`, one `direct:`/`seda:` send, exactly one route source).
/// Pure document grammar: no CLI `--arg` machinery — declared
/// arguments are normalized into the model but never resolved or
/// interpolated (fields stay raw). See
/// [`parse_job_document_with_args`].
///
/// The grammar test suite (`document_tests`) is the only consumer since
/// the embedded-artifact path moved to [`parse_job_document_with_args`]
/// with EMPTY pairs (jobargs Task 3.2); the pair-free bare parse is
/// deliberately preserved as the pure-grammar seam.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn parse_job_document(path: &Path, text: &str) -> Result<JobDocument, JobDocError> {
    parse_job_document_impl(path, text, None)
}

/// [`parse_job_document`] with the CLI's repeatable `--arg NAME=VALUE`
/// pairs. For a declared document (`args:` present) the pairs are
/// resolved against the declarations FIRST — unknown names and missing
/// required values fail before any field validation and before boot —
/// then defaults fill the omissions, and the resolved values (plus the
/// ambient environment, the same namespace-specific shared stage the
/// route sources use) are interpolated into `to`, `body`, `headers`,
/// and `timeout` BEFORE those fields are validated. Legacy documents
/// (no `args:`) never see pair validation or field interpolation:
/// their pairs stay raw send-time headers.
pub(crate) fn parse_job_document_with_args(
    path: &Path,
    text: &str,
    cli_args: &[(String, String)],
) -> Result<JobDocument, JobDocError> {
    parse_job_document_impl(path, text, Some(cli_args))
}

/// Shared document parser. `cli_args` is `Some` on the CLI execution
/// path (the raw pairs) and on the embedded-artifact path (EMPTY pairs —
/// embedded defaults only, jobargs Task 3.2): pair resolution,
/// defaulting, and field interpolation are execution concerns, while the
/// bare parse (model inspection) stays pair-free.
fn parse_job_document_impl(
    path: &Path,
    text: &str,
    cli_args: Option<&[(String, String)]>,
) -> Result<JobDocument, JobDocError> {
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

    let mut raw =
        serde_yaml::from_str::<JobDocumentDoc>(text).map_err(|e| classify(&e.to_string()))?;

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

    // Declared arguments: strict per-declaration validation with
    // argument-specific diagnostics.
    let args = normalize_job_args(raw.args.take())?;

    // Declared mode with CLI pairs: resolve the pairs against the
    // declarations (unknown/missing-required fail HERE, before field
    // validation and before boot), apply defaults, then interpolate
    // the resolved values into the four field surfaces so validation
    // sees final text. Legacy pairs are untouched (raw send-time
    // headers) and the bare parse (cli_args = None) keeps raw fields.
    let resolved = match cli_args {
        Some(pairs) => resolve_job_args(args.as_ref(), pairs)?,
        None => None,
    };
    if let Some(resolved) = &resolved {
        interpolate_declared_fields(&mut raw, resolved)?;
    }

    // Grammar rules with per-rule errors.
    let execute = raw.execute;
    let mode = match execute.mode.ok_or(JobDocError::MissingMode)?.as_str() {
        "one-shot" => JobMode::OneShot,
        "batch" => JobMode::Batch,
        other => return Err(JobDocError::UnsupportedMode(other.to_string())),
    };
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
        args,
    })
}

/// Whether `name` matches the argument identifier grammar
/// `[A-Za-z_][A-Za-z0-9_]*` (shared with the `${arg:NAME}` token form).
fn is_argument_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        _ => false,
    }
}

/// Normalize the raw top-level `args:` map: each name must match the
/// identifier grammar and each declaration the strict three-field
/// shape. An empty map stays `Some` (the declared mode); only an
/// absent `args:` key yields `None` (the legacy path).
fn normalize_job_args(
    raw: Option<BTreeMap<String, serde_yaml::Value>>,
) -> Result<Option<JobArgumentDeclarations>, JobDocError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let mut entries = BTreeMap::new();
    for (name, value) in raw {
        if !is_argument_identifier(&name) {
            return Err(JobDocError::InvalidArgumentName { name });
        }
        let declaration = job_argument_declaration(&name, &value)?;
        entries.insert(name, declaration);
    }
    Ok(Some(JobArgumentDeclarations { entries }))
}

/// Validate one declaration mapping and build its normalized form.
/// Unknown fields and non-string/non-boolean values produce
/// argument-specific diagnostics.
fn job_argument_declaration(
    name: &str,
    value: &serde_yaml::Value,
) -> Result<JobArgumentDeclaration, JobDocError> {
    let invalid = |detail: String| JobDocError::InvalidArgumentDeclaration {
        argument: name.to_string(),
        detail,
    };
    let mapping = value.as_mapping().ok_or_else(|| {
        invalid("expected a mapping of `required`, `default`, and `description` fields".to_string())
    })?;
    let mut declaration = JobArgumentDeclaration {
        required: false,
        default: None,
        description: None,
    };
    for (key, val) in mapping {
        // The compat `Mapping` is string-keyed, so every field name is
        // a string by construction.
        match key.as_str() {
            "required" => {
                declaration.required = val
                    .as_bool()
                    .ok_or_else(|| invalid("`required` must be a boolean".to_string()))?;
            }
            "default" => {
                let raw = val
                    .as_str()
                    .ok_or_else(|| invalid("`default` must be a string".to_string()))?;
                declaration.default = Some(raw.to_string());
            }
            "description" => {
                let raw = val
                    .as_str()
                    .ok_or_else(|| invalid("`description` must be a string".to_string()))?;
                declaration.description = Some(raw.to_string());
            }
            other => {
                return Err(JobDocError::UnknownArgumentField {
                    argument: name.to_string(),
                    field: other.to_string(),
                });
            }
        }
    }
    Ok(declaration)
}

/// Resolve the CLI `--arg NAME=VALUE` pairs against the declared
/// arguments (pure; declared mode only — `None` declarations are the
/// legacy path and yield `Ok(None)` without validating the pairs, which
/// stay raw send-time headers). Semantics, in order:
///
/// 1. Every pair must name a declared argument — the first unknown name
///    fails with [`JobDocError::UnknownArgumentName`].
/// 2. Repeated names take the LAST value (sequential overwrite;
///    deterministic).
/// 3. Every `required: true` declaration without a `default` must have
///    received a pair — the first (lexical) omission fails with
///    [`JobDocError::MissingRequiredArgument`].
/// 4. Declarations with a `default` fill omissions; an explicit pair
///    always wins over the default.
///
/// The returned map is the complete `${arg:NAME}` lookup for
/// [`interpolate_declared_fields`].
pub(crate) fn resolve_job_args(
    declarations: Option<&JobArgumentDeclarations>,
    pairs: &[(String, String)],
) -> Result<Option<BTreeMap<String, String>>, JobDocError> {
    let Some(declarations) = declarations else {
        return Ok(None);
    };
    let mut resolved = BTreeMap::new();
    for (name, value) in pairs {
        if !declarations.entries.contains_key(name) {
            return Err(JobDocError::UnknownArgumentName { name: name.clone() });
        }
        resolved.insert(name.clone(), value.clone());
    }
    for (name, declaration) in &declarations.entries {
        if declaration.required && declaration.default.is_none() && !resolved.contains_key(name) {
            return Err(JobDocError::MissingRequiredArgument { name: name.clone() });
        }
        if let Some(default) = &declaration.default {
            resolved
                .entry(name.clone())
                .or_insert_with(|| default.clone());
        }
    }
    Ok(Some(resolved))
}

/// Resolve one job-document field string through the shared
/// interpolation seam (`camel_dsl::interpolate_with_args`): the same
/// scanner and stage the route sources use, with namespace dispatch
/// before lookup — `${arg:NAME}` consults ONLY the resolved argument
/// values (never the environment) and `${env:NAME}` ONLY the ambient
/// environment. An unresolved name (including the rejected
/// `${arg:NAME:-fallback}` form) fails with
/// [`JobDocError::UnresolvedArgument`], naming the name.
fn interpolate_job_string(
    src: &str,
    resolved: &BTreeMap<String, String>,
) -> Result<String, JobDocError> {
    let env_lookup = |name: &str| std::env::var(name).ok();
    let arg_lookup = |name: &str| resolved.get(name).cloned();
    camel_dsl::interpolate_with_args(src, &env_lookup, &arg_lookup)
        .map_err(|name| JobDocError::UnresolvedArgument { name })
}

/// Interpolate every string VALUE of a JSON body/header value,
/// recursively. Object keys are deliberately left raw: an interpolated
/// key could collide in the header map, and the job header surface has
/// no last-wins collapse rule.
fn interpolate_json_strings(
    value: &mut serde_json::Value,
    resolved: &BTreeMap<String, String>,
) -> Result<(), JobDocError> {
    match value {
        serde_json::Value::String(text) => {
            *text = interpolate_job_string(text, resolved)?;
        }
        serde_json::Value::Array(items) => {
            for item in items {
                interpolate_json_strings(item, resolved)?;
            }
        }
        serde_json::Value::Object(map) => {
            for item in map.values_mut() {
                interpolate_json_strings(item, resolved)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Interpolate a declared document's four field surfaces — `to`,
/// `body`, `headers`, and `timeout` — in place, before their grammar
/// validation (`timeout: "${arg:wait}"` must resolve to a duration
/// string BEFORE the humantime check, `to:` before the scheme check).
/// Declared mode only: legacy documents keep raw fields verbatim.
fn interpolate_declared_fields(
    raw: &mut JobDocumentDoc,
    resolved: &BTreeMap<String, String>,
) -> Result<(), JobDocError> {
    if let Some(send) = raw.execute.send.as_mut() {
        send.to = interpolate_job_string(&send.to, resolved)?;
        if let Some(JobBody::Text(text)) = send.body.as_mut() {
            *text = interpolate_job_string(text, resolved)?;
        }
        if let Some(JobBody::Json(value)) = send.body.as_mut() {
            interpolate_json_strings(value, resolved)?;
        }
        if let Some(headers) = send.headers.as_mut() {
            for header in headers.values_mut() {
                interpolate_json_strings(header, resolved)?;
            }
        }
    }
    if let Some(timeout) = raw.execute.timeout.as_mut() {
        *timeout = interpolate_job_string(timeout, resolved)?;
    }
    Ok(())
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
/// missing-target error, more than one an ambiguous-target error —
/// with all routes started, duplicate consumer bases would round-robin
/// the send and any `to:` hops, so exactly one match stays mandatory.
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
