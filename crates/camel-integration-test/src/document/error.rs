//! Load-time errors for scenario documents, and the endpoint-reference
//! conversion that fails with them (rc-0ahfl, split out of the parent
//! module).
//!
//! Exit-code mapping for the CLI adapter (ADR-0069 section 7):
//! classification is by variant, never by message text. Every variant
//! is a load-time failure and maps to exit 2. The `DocError` type is
//! re-exported at `crate::document::DocError` and at the crate root,
//! so consumers keep the paths they had before the split.

use std::path::PathBuf;
use std::time::Duration;

use noyalib::compat::serde_yaml;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};

use super::{EndpointRef, Provisioning, ref_scheme};

/// Raw endpoint reference: bare string or map with `endpoint`,
/// `provisioning`, and `bindVar`.
#[derive(Debug, Clone)]
pub(super) struct RawEndpointRef {
    /// Endpoint URI as written.
    pub(super) endpoint: String,
    /// Raw provisioning source name (`harness`, or a reserved value).
    pub(super) provisioning: Option<String>,
    /// Raw bind-variable name.
    pub(super) bind_var: Option<String>,
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
///   `ExpectReplyOnUnsupportedSend`, `LogsBlock`.
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
    /// A malformed document-level `logs:` block (rc-tdgh5): an unknown
    /// key, a level outside the accepted set, or a regex that does not
    /// compile. The detail names the offending clause.
    #[error("doc-validation: malformed `logs` block: {detail}")]
    LogsBlock {
        /// The offending clause and why it failed.
        detail: String,
    },
}

/// Classifies a compat-layer (serde_yaml) error text, mirroring the
/// unit-tier classifier.
pub(super) fn classify_yaml_error(raw: &str) -> DocError {
    if raw.contains("unknown field") {
        return DocError::UnknownField(raw.to_string());
    }
    DocError::Yaml(raw.to_string())
}

/// Applies the provisioning gate: only `harness` (or absent) passes,
/// and a harness entry whose reference scheme binds no partner
/// (`direct:`, `fake:`) must not declare a `bindVar` — the variable
/// would never receive a bound authority (rc-j87j). A
/// `direct:`/`fake:` entry without a `bindVar` stays legal.
pub(super) fn endpoint_from_raw(raw: RawEndpointRef) -> Result<EndpointRef, DocError> {
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

/// Parses a humantime duration string, naming the action index on
/// failure.
pub(super) fn parse_duration(raw: &str, index: usize, field: &str) -> Result<Duration, DocError> {
    humantime::parse_duration(raw).map_err(|e| DocError::Validation {
        index,
        message: format!("invalid {field} `{raw}`: {e}"),
    })
}
