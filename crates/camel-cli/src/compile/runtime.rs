//! Embedded-document runtime for compiled artifacts (openspec changes
//! `cli-compile` Tasks 2.2/2.3 and `multidoc` Task 2.2).
//!
//! [`run_embedded_document`] takes a validated [`EmbeddedRequest`] — a
//! v1 single-document trailer or a v2 decoded
//! [`VirtualDocumentStore`] plus
//! the parsed artifact arguments — and runs it through the EXISTING
//! lifecycles:
//!
//! - v1 route artifacts drive the shared `camel run` lifecycle
//!   (`crate::commands::run::drive_lifecycle`) with the default
//!   in-memory config, the embedded single-document discovery seam,
//!   and `watch = false`;
//! - v2 multi-document route artifacts call
//!   [`camel_dsl::discover_virtual_store`] BEFORE boot (the merged
//!   embedded configuration feeds the context), build the
//!   `CamelConfig` through the deployment-time `${env:}` seam, and
//!   drive the same lifecycle with the discovered routes;
//! - job artifacts drive the existing job report/outcome lifecycle:
//!   v1 with the embedded document as the sole inline route source,
//!   v2 consuming the embedded job/config/route entries (indexed
//!   route files or the inline `routes:` block) with no filesystem
//!   route discovery.
//!
//! Asset resolution (r2embed Task 3.2): the substitution table recorded
//! at compile time is the ONLY runtime resolution path. Before kind
//! dispatch, the in-memory asset registry is populated from the decoded
//! store (memory-served classes read through it, never the filesystem)
//! and the substitution table's recorded byte spans are rewritten —
//! last-offset-first — to the confined per-boot materialization paths of
//! the disk-written classes (TLS cert/key/CA, `xslt:`, `validator:`,
//! `sql:file:`). The runtime never text-searches or canonicalizes
//! declared strings, and TLS-class resolution is embedded-only with no
//! host fallback. The watcher never activates.
//! `${env:NAME}` resolves from the deployment environment through the
//! discovery and config seams. Declared job `args:` resolve at startup
//! through the same parser path as normal jobs with NO dynamic flags
//! (jobargs Task 3.2): embedded runs carry no dynamic flags and use
//! embedded declaration defaults to fill the interpolated fields —
//! typed defaults coerced through the same rules (jobtyped) — and a
//! required declaration without a default exits 2 before boot.
//!
//! [`self_detect_artifact`] is the binary entry point (Task 2.3): the
//! `camel` main calls it BEFORE Clap parses anything, so a self-contained
//! artifact consumes its own argv surface (`--report`, `--help`,
//! `--version`, `--manifest`, and the r4sign `--verify`) while a
//! trailer-free image falls through to the normal CLI unchanged. A
//! valid trailer additionally runs boot verification (r4sign Task 1.3)
//! before any dispatch: a present detached envelope verifies against
//! the streamed artifact bytes, and a schema-4 or schema-5 manifest
//! whose signature is required refuses to boot without it.
//!
//! Exit codes: 0 graceful completion / job Completed; 1 job pipeline
//! failure (route runs end either in graceful completion or a boot-class
//! failure, so 1 stays reserved for them); 2 argument misuse, store or
//! configuration validation, boot failure, or report-write failure.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Serialize;

use ed25519_dalek::{Digest as _, Sha512};

use super::CompileError;
use super::manifest;
use super::signature;
use super::store::{
    SubstitutionContext, SubstitutionEntry, SubstitutionSpan, VirtualDocumentStore,
};
use super::trailer::{self, Trailer, TrailerKind};
use super::trust;
use crate::commands::run::{Discover, LifecycleFailure, LifecycleSpec};

/// Exit code for argument misuse, boot failure, and report-write failure.
const EXIT_REJECTION: i32 = 2;

/// Idle log line while a route artifact runs (watch disabled).
const IDLE_NOTE: &str = "compiled artifact running (hot-reload disabled). Press Ctrl+C to stop.";

/// Parsed artifact argument surface.
///
/// The exclusive modes (`--help`, `--version`, `--manifest`, `--verify`)
/// print or verify and exit 0 (verify: exit 2 on failure) without
/// booting; `--report <path>` pairs with a run. `--truststore <path>`
/// (keypin Task 1.1) is a modifier, not a mode: it pairs with a boot,
/// `--report`, and `--verify` only. The surface is deliberately narrow:
/// dynamic declared-argument flags are unsupported (rejected as
/// unknown) — declared arguments resolve from the embedded declarations
/// alone (jobargs Task 3.2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactArgs {
    /// `--report <path>`: where the run writes its report.
    pub report: Option<PathBuf>,
    /// `--help`: print usage and exit 0.
    pub help: bool,
    /// `--version`: print the runtime version and exit 0.
    pub version: bool,
    /// `--manifest`: print the operational manifest and exit 0.
    pub manifest: bool,
    /// `--verify` (r4sign Task 1.3): verify the detached signature
    /// envelope and exit 0, or fail closed with exit 2 — no boot.
    pub verify: bool,
    /// `--truststore <path>` (keypin Task 1.1): the deployment
    /// truststore for signature pinning. Resolved from
    /// `CAMEL_TRUSTSTORE` in [`ArtifactArgs::parse`] when the argument
    /// is absent (the argument wins); later consumers read only this
    /// field.
    pub truststore: Option<PathBuf>,
}

impl ArtifactArgs {
    /// The flag already present, for exclusive-mode conflict naming.
    fn first_set(&self) -> Option<&'static str> {
        if self.report.is_some() {
            Some("--report")
        } else if self.help {
            Some("--help")
        } else if self.version {
            Some("--version")
        } else if self.manifest {
            Some("--manifest")
        } else if self.verify {
            Some("--verify")
        } else {
            None
        }
    }

    /// The exclusive mode already present that the `--truststore`
    /// modifier cannot pair with, if any: it pairs with a boot,
    /// `--report`, and `--verify` only (keypin Task 1.1).
    fn first_incompatible_mode(&self) -> Option<&'static str> {
        if self.help {
            Some("--help")
        } else if self.version {
            Some("--version")
        } else if self.manifest {
            Some("--manifest")
        } else {
            None
        }
    }
}

/// Artifact-argument rejection. Every variant names the rejected
/// argument; the caller prints the diagnostic and exits 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactArgError {
    /// The same flag appeared twice.
    Duplicate(&'static str),
    /// Two mutually exclusive flags appeared together.
    Exclusive(&'static str, &'static str),
    /// `--report` has no value (end of arguments, or the next token is
    /// another flag).
    MissingValue(&'static str),
    /// An unknown flag.
    Unknown(String),
    /// A positional argument.
    Positional(String),
}

impl fmt::Display for ArtifactArgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Duplicate(flag) => write!(f, "duplicate argument '{flag}'"),
            Self::Exclusive(a, b) => {
                write!(f, "arguments '{a}' and '{b}' are mutually exclusive")
            }
            Self::MissingValue(flag) => {
                write!(f, "argument '{flag}' requires a value")
            }
            Self::Unknown(arg) => write!(f, "unknown argument '{arg}'"),
            Self::Positional(arg) => write!(f, "unexpected positional argument '{arg}'"),
        }
    }
}

impl std::error::Error for ArtifactArgError {}

/// The deployment truststore environment variable (keypin Task 1.1):
/// the fallback source for the artifact truststore when `--truststore`
/// is absent.
pub(crate) const TRUSTSTORE_ENV: &str = "CAMEL_TRUSTSTORE";

impl ArtifactArgs {
    /// Parse the artifact argv (no program name). Accepts `--report
    /// <path>`, `--truststore <path>`, `--help`, `--version`,
    /// `--manifest`, and `--verify`; rejects duplicates, exclusive
    /// combinations, missing report and truststore values, unknown
    /// flags, and positional arguments. `--truststore` is a modifier:
    /// it pairs with a boot, `--report`, and `--verify` only, and
    /// falls back to a non-empty `CAMEL_TRUSTSTORE` when absent (the
    /// argument wins when both are present).
    pub fn parse(args: &[String]) -> Result<Self, ArtifactArgError> {
        let mut parsed = Self::default();
        let mut idx = 0;
        while idx < args.len() {
            let arg = args[idx].as_str();
            match arg {
                "--report" => {
                    if let Some(prev) = parsed.first_set() {
                        return Err(if prev == "--report" {
                            ArtifactArgError::Duplicate(prev)
                        } else {
                            ArtifactArgError::Exclusive(prev, "--report")
                        });
                    }
                    let Some(value) = args.get(idx + 1) else {
                        return Err(ArtifactArgError::MissingValue("--report"));
                    };
                    if value.starts_with('-') {
                        return Err(ArtifactArgError::MissingValue("--report"));
                    }
                    parsed.report = Some(PathBuf::from(value));
                    idx += 2;
                }
                "--truststore" => {
                    if parsed.truststore.is_some() {
                        return Err(ArtifactArgError::Duplicate("--truststore"));
                    }
                    if let Some(mode) = parsed.first_incompatible_mode() {
                        return Err(ArtifactArgError::Exclusive(mode, "--truststore"));
                    }
                    let Some(value) = args.get(idx + 1) else {
                        return Err(ArtifactArgError::MissingValue("--truststore"));
                    };
                    if value.starts_with('-') {
                        return Err(ArtifactArgError::MissingValue("--truststore"));
                    }
                    parsed.truststore = Some(PathBuf::from(value));
                    idx += 2;
                }
                "--help" | "--version" | "--manifest" | "--verify" => {
                    let flag: &'static str = match arg {
                        "--help" => "--help",
                        "--version" => "--version",
                        "--verify" => "--verify",
                        _ => "--manifest",
                    };
                    if let Some(prev) = parsed.first_set() {
                        return Err(if prev == flag {
                            ArtifactArgError::Duplicate(flag)
                        } else {
                            ArtifactArgError::Exclusive(prev, flag)
                        });
                    }
                    if parsed.truststore.is_some() && flag != "--verify" {
                        return Err(ArtifactArgError::Exclusive("--truststore", flag));
                    }
                    match flag {
                        "--help" => parsed.help = true,
                        "--version" => parsed.version = true,
                        "--verify" => parsed.verify = true,
                        _ => parsed.manifest = true,
                    }
                    idx += 1;
                }
                other if other.starts_with('-') => {
                    return Err(ArtifactArgError::Unknown(other.to_string()));
                }
                other => {
                    return Err(ArtifactArgError::Positional(other.to_string()));
                }
            }
        }
        // Env fallback (keypin Task 1.1): a non-empty `CAMEL_TRUSTSTORE`
        // supplies the truststore when no argument did — the argument
        // wins when both are present. Resolved once here so later
        // consumers read only `args.truststore`.
        if parsed.truststore.is_none()
            && let Some(path) = std::env::var_os(TRUSTSTORE_ENV)
            && !path.is_empty()
        {
            parsed.truststore = Some(PathBuf::from(path));
        }
        Ok(parsed)
    }
}

/// Route-artifact status report: the exact JSON object
/// `{"kind":"route","status":"completed"|"failed","error":string|null}`
/// written to `--report` after boot/runtime completion or failure.
#[derive(Debug, Serialize)]
pub struct RouteReport {
    kind: &'static str,
    status: &'static str,
    error: Option<String>,
}

impl RouteReport {
    /// A gracefully completed run.
    fn completed() -> Self {
        Self {
            kind: "route",
            status: "completed",
            error: None,
        }
    }

    /// A failed run (boot/discovery class carries the diagnostic).
    fn failed(error: String) -> Self {
        Self {
            kind: "route",
            status: "failed",
            error: Some(error),
        }
    }

    /// Compact JSON with the exact key order `kind`, `status`, `error`.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("route report serialization cannot fail") // allow-unwrap
    }
}

/// One validated embedded run request: the decoded artifact plus the
/// parsed artifact arguments.
///
/// - v1 (`[`EmbeddedRequest::SingleDocument`]`, built by
///   [`EmbeddedRequest::from_trailer`]): the single-document trailer
///   payload runs through the single-document seams — default
///   in-memory config, embedded-text discovery. v1 artifacts keep this
///   path verbatim.
/// - v2 ([`EmbeddedRequest::VirtualStore`], built by
///   [`EmbeddedRequest::from_v2`]; multidoc Task 2.2): the decoded
///   multi-document store runs through the virtual-store runtime —
///   merged embedded configuration, ordered source-plan routes,
///   deployment-time `${env:}` resolution. Single-document v2 stores
///   take the same path.
#[derive(Debug, Clone)]
pub enum EmbeddedRequest {
    /// v1 single-document artifact.
    SingleDocument {
        /// Embedded artifact kind (route/job).
        kind: TrailerKind,
        /// Logical source name; the runtime source identity is
        /// `compiled://<source_name>`.
        source_name: String,
        /// Normalized document text (pre-interpolation authoring text).
        document: String,
        /// Canonical manifest JSON (printed verbatim by `--manifest`).
        manifest_json: String,
        /// Parsed artifact arguments.
        args: ArtifactArgs,
    },
    /// v2 multi-document virtual-store artifact (multidoc Task 2.2).
    VirtualStore {
        /// Embedded artifact kind (route/job).
        kind: TrailerKind,
        /// Decoded virtual-document store; the entry point is
        /// `store.index.entry_point` and every document is read through
        /// the store, never the filesystem.
        store: super::store::VirtualDocumentStore,
        /// Canonical manifest JSON (printed verbatim by `--manifest`).
        manifest_json: String,
        /// Parsed artifact arguments.
        args: ArtifactArgs,
    },
}

impl EmbeddedRequest {
    /// Build the request from a decoded v1 trailer and parsed
    /// arguments. Both payload and manifest are validated UTF-8 (the
    /// encoder normalized the document and serialized the manifest as
    /// canonical UTF-8 JSON); the manifest must carry its
    /// `source_name`.
    pub fn from_trailer(trailer: Trailer, args: ArtifactArgs) -> Result<Self, CompileError> {
        let document = String::from_utf8(trailer.payload).map_err(|_| CompileError::InvalidUtf8)?;
        let manifest_json =
            String::from_utf8(trailer.manifest).map_err(|_| CompileError::InvalidUtf8)?;
        let source_name = serde_json::from_str::<serde_json::Value>(&manifest_json)
            .ok()
            .and_then(|value| {
                value
                    .get("source_name")
                    .and_then(|name| name.as_str())
                    .map(str::to_string)
            })
            .ok_or_else(|| {
                CompileError::InvalidDocument("manifest carries no source_name".to_string())
            })?;
        Ok(Self::SingleDocument {
            kind: trailer.kind,
            source_name,
            document,
            manifest_json,
            args,
        })
    }

    /// Build the request from a decoded v2 multi-document trailer and
    /// parsed arguments (multidoc Task 2.2). `decode_artifact` already
    /// verified the checksums, the store index, the typed references,
    /// and the manifest/store agreement; the store decodes again here
    /// and the typed reference invariants re-check — defense in depth
    /// so a hand-routed `TrailerV2` fails by name too, before boot.
    pub fn from_v2(v2: trailer::TrailerV2, args: ArtifactArgs) -> Result<Self, CompileError> {
        let store = super::store::VirtualDocumentStore::decode(v2.content, &v2.index)
            .map_err(|e| CompileError::InvalidDocument(format!("invalid virtual store: {e}")))?;
        super::store::validate_typed_references(&store.index, v2.kind)
            .map_err(|e| CompileError::InvalidDocument(format!("invalid virtual store: {e}")))?;
        let manifest_json =
            String::from_utf8(v2.manifest).map_err(|_| CompileError::InvalidUtf8)?;
        Ok(Self::VirtualStore {
            kind: v2.kind,
            store,
            manifest_json,
            args,
        })
    }
}

/// Run one validated embedded request; the process termination seam for
/// the binary self-detect path (Task 2.3 wires this into `main`).
pub async fn run_embedded_document(request: EmbeddedRequest) -> ExitCode {
    ExitCode::from(run_embedded_document_code(request).await as u8)
}

/// URI-site percent-encoding set: everything except the RFC 3986
/// unreserved set (`A-Za-z0-9-._~`) and the path separator `/`. The
/// confined path must survive as a literal file path for the legacy
/// readers that do NOT percent-decode (`camel-xslt`, `camel-sql` open
/// the substituted value directly), so already-safe characters stay
/// raw; whitespace and URI-meaningful characters (`%`, `&`, `=`, `?`,
/// `#`, controls, non-ASCII) encode to `%XX` and round-trip through the
/// decoding consumers (`camel-validator` percent-decodes its schema
/// path).
const URI_SITE_SET: &percent_encoding::AsciiSet = &NON_ALPHANUMERIC
    .remove(b'/')
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Pre-boot asset preparation failure: registry population, path-safety
/// enforcement, or confined materialization. Every variant is a
/// fail-closed exit-2 diagnostic; nothing boots.
#[derive(Debug)]
enum PrepareError {
    /// The embedded manifest could not be re-parsed (defense in depth;
    /// decode already validated it).
    Manifest(String),
    /// Registry population failed a positional/digest check.
    Registry(camel_bundles::AssetRegistryError),
    /// A disk-written class needs a writable temp location and
    /// `std::env::temp_dir()` is not one. Names the class and the OS
    /// cause (read-only contract, r2embed Task 3.2 Step 5; proto-class
    /// targets never reach this — they resolve in memory, mission 350).
    TempUnavailable {
        /// Class of the first disk-class substitution target.
        class: String,
        /// Underlying OS error.
        source: std::io::Error,
    },
    /// Confined materialization failed (planted entry, symlink,
    /// confinement check).
    Materialize(super::materialize::MaterializeError),
    /// The substitution path-safety rule rejected a site.
    Rule(CompileError),
    /// A store invariant the substitution rewrite relies on does not
    /// hold (unreadable entry, out-of-bounds range). Defensive: decode
    /// validates these.
    StoreInconsistent(String),
}

impl fmt::Display for PrepareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest(reason) => write!(f, "compiled artifact manifest is invalid: {reason}"),
            Self::Registry(e) => write!(f, "asset registry population failed: {e}"),
            Self::TempUnavailable { class, source } => write!(
                f,
                "{class} asset requires a writable temp directory for confined \
                 materialization: {source}"
            ),
            Self::Materialize(e) => write!(f, "confined materialization failed: {e}"),
            Self::Rule(e) => e.fmt(f),
            Self::StoreInconsistent(reason) => write!(f, "substitution rewrite failed: {reason}"),
        }
    }
}

impl std::error::Error for PrepareError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Registry(e) => Some(e),
            Self::TempUnavailable { source, .. } => Some(source),
            Self::Materialize(e) => Some(e),
            Self::Rule(e) => Some(e),
            _ => None,
        }
    }
}

/// Populates the process-global asset registry from the decoded store's
/// entries, positionally paired with the manifest's `embedded_files`
/// (same canonical order), verifying every entry's kind, length, and
/// BLAKE3 digest. Population happens exactly once per process; normal
/// `camel run` never populates the registry.
fn populate_asset_registry(
    store: &VirtualDocumentStore,
    manifest: &manifest::Manifest,
) -> Result<(), PrepareError> {
    let registry = camel_bundles::AssetRegistry::global();
    if registry.is_populated() {
        return Ok(());
    }
    let mut assets = Vec::with_capacity(store.index.entries.len());
    for entry in &store.index.entries {
        let Some(bytes) = store.read(&entry.path) else {
            return Err(PrepareError::StoreInconsistent(format!(
                "store entry {} is unreadable",
                entry.path
            )));
        };
        assets.push(camel_bundles::RegistryAsset {
            logical_path: entry.path.clone(),
            kind: entry.kind.as_str().to_string(),
            bytes: bytes.to_vec(),
        });
    }
    let declared: Vec<camel_bundles::RegistryManifestEntry> = manifest
        .embedded_files
        .iter()
        .map(|file| camel_bundles::RegistryManifestEntry {
            kind: file.kind.as_str().to_string(),
            length: file.length,
            digest: file.digest.clone(),
        })
        .collect();
    registry
        .populate(assets, &declared)
        .map_err(PrepareError::Registry)
}

/// Enforces the substitution path-safety rule at one substitution site
/// (r2embed Task 3.2, Step 2): every site requires the confined path to
/// be valid UTF-8 (guaranteed by the `&str` parameter; the caller
/// rejects non-UTF-8 confined paths with the same diagnostic) with no
/// ASCII control characters; `literal` sites additionally restrict the
/// characters to `[A-Za-z0-9._/+-]`. A violation fails closed with the
/// named diagnostic `substitution path-safety rule violation`, naming
/// the site and the offending character. URI sites pass the charset
/// clause: their value is percent-encoded afterwards.
fn enforce_substitution_path_safety(
    site: &str,
    confined: &str,
    context: SubstitutionContext,
) -> Result<(), CompileError> {
    let violation = |detail: String| {
        CompileError::InvalidDocument(format!(
            "substitution path-safety rule violation at site '{site}': {detail}"
        ))
    };
    for c in confined.chars() {
        if c.is_ascii_control() {
            return Err(violation(format!(
                "character {c:?} (U+{:04X}) is an ASCII control character, rejected at every \
                 site",
                c as u32
            )));
        }
    }
    if context == SubstitutionContext::Literal {
        for c in confined.chars() {
            if !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '+' | '-')) {
                return Err(violation(format!(
                    "character {c:?} (U+{:04X}) is outside the literal-site safe set \
                     [A-Za-z0-9._/+-]",
                    c as u32
                )));
            }
        }
    }
    Ok(())
}

/// Applies the recorded substitution spans to one site entry's bytes,
/// LAST-OFFSET-FIRST: later spans are rewritten before earlier ones so
/// every earlier offset — and with it every line start — stays stable
/// across applications. Each recorded span is replaced exactly once;
/// non-recorded text is untouched. An out-of-bounds, inverted, or
/// overlapping span pair fails closed.
fn apply_span_rewrites(
    bytes: &[u8],
    replacements: &[(SubstitutionSpan, Vec<u8>)],
) -> Result<Vec<u8>, CompileError> {
    let mut ordered: Vec<&(SubstitutionSpan, Vec<u8>)> = replacements.iter().collect();
    ordered.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
    for window in ordered.windows(2) {
        let (later, earlier) = (window[0].0, window[1].0);
        if later.start < earlier.end {
            return Err(CompileError::InvalidDocument(format!(
                "overlapping substitution spans {}..{} and {}..{}",
                earlier.start, earlier.end, later.start, later.end
            )));
        }
    }
    let mut out = bytes.to_vec();
    for (span, replacement) in ordered {
        let start = span.start as usize;
        let end = span.end as usize;
        if start > end || end > out.len() {
            return Err(CompileError::InvalidDocument(format!(
                "substitution span {}..{} is out of bounds for a {}-byte entry",
                span.start,
                span.end,
                out.len()
            )));
        }
        out.splice(start..end, replacement.iter().cloned());
    }
    Ok(out)
}

/// Rewrites every site entry's recorded spans inside the store: site
/// bytes are replaced, and the content blob is rebuilt with every
/// entry's offset and length recomputed, so the store's canonical
/// invariants (contiguous coverage, per-entry ranges) hold for the
/// by-value consumers downstream.
fn rewrite_store_entries(
    store: &mut VirtualDocumentStore,
    sites: &BTreeMap<String, Vec<(SubstitutionSpan, Vec<u8>)>>,
) -> Result<(), CompileError> {
    if sites.is_empty() {
        return Ok(());
    }
    let old = std::mem::take(&mut store.content);
    let mut new_content = Vec::with_capacity(old.len());
    let mut cursor = 0usize;
    for entry in &mut store.index.entries {
        let start = entry.offset as usize;
        let end = start + entry.length as usize;
        if start > end || end > old.len() {
            return Err(CompileError::InvalidDocument(format!(
                "store entry {} has an out-of-bounds range",
                entry.path
            )));
        }
        if start > cursor {
            new_content.extend_from_slice(&old[cursor..start]);
        }
        cursor = cursor.max(end);
        match sites.get(&entry.path) {
            Some(spans) => {
                let rewritten = apply_span_rewrites(&old[start..end], spans)?;
                entry.offset = new_content.len() as u64;
                entry.length = rewritten.len() as u64;
                new_content.extend_from_slice(&rewritten);
            }
            None => {
                entry.offset = new_content.len() as u64;
                new_content.extend_from_slice(&old[start..end]);
            }
        }
    }
    if cursor < old.len() {
        new_content.extend_from_slice(&old[cursor..]);
    }
    store.content = new_content;
    Ok(())
}

/// The reserved in-memory reference prefix, shared with
/// `camel_proto_compiler::embedded::EMBEDDED_REF_PREFIX` on grpc builds.
/// The string rewrite below MUST work in every flavor (an artifact must
/// never carry unrewritten protoFile spans), so the non-grpc fallback
/// keeps the literal; the grpc build reads the constant.
#[cfg(feature = "grpc")]
fn embedded_ref(asset: &str) -> String {
    format!(
        "{}{asset}",
        camel_proto_compiler::embedded::EMBEDDED_REF_PREFIX
    )
}
#[cfg(not(feature = "grpc"))]
fn embedded_ref(asset: &str) -> String {
    format!("camel-embedded:{asset}")
}

/// Pushes the replacement for every recorded span of `entry` into `sites`.
///
/// `raw` is the site value: the embedded reference for the in-memory class,
/// or the confined path for a disk class. A `literal` site keeps the raw
/// bytes; a `uri` site percent-encodes them.
fn push_replacement(
    sites: &mut BTreeMap<String, Vec<(SubstitutionSpan, Vec<u8>)>>,
    entry: &SubstitutionEntry,
    raw: &str,
) {
    let replacement = match entry.context {
        SubstitutionContext::Literal => raw.as_bytes().to_vec(),
        SubstitutionContext::Uri => utf8_percent_encode(raw, URI_SITE_SET)
            .to_string()
            .into_bytes(),
    };
    let site_spans = sites.entry(entry.document.clone()).or_default();
    for span in &entry.spans {
        site_spans.push((*span, replacement.clone()));
    }
}

/// The pre-boot asset preparation at the DECIDED single swap point
/// (r2embed Task 3.2; the proto class re-plumbed to in-memory resolution,
/// mission 350): after decode, before the route/job kind dispatch.
///
/// 1. The asset registry is populated from the store's entries
///    (positional manifest pairing, digest verification) — the
///    memory-served class's FS of record.
/// 2. Substitution targets partition by class. The `proto file` class
///    resolves IN MEMORY: each asset registers into the in-process proto
///    compiler's embedded registry and its site rewrites to a
///    `camel-embedded:` reference — no disk write, so a proto-only
///    artifact boots on a host with no writable temp directory. Every
///    other class (TLS cert/key/CA, `xslt:`, `validator:`, `sql:file:`)
///    is a legacy path reader: its assets materialize into a per-boot
///    confined directory (static-file entries stay memory-served and
///    never touch the filesystem).
/// 3. The substitution path-safety rule is enforced at every site
///    BEFORE anything is registered or written; `uri` sites then receive
///    the percent-encoded replacement (confined path or embedded
///    reference), `literal` sites the raw value.
/// 4. The recorded byte spans — and only those — are rewritten in the
///    store's document and config entries, applied last-offset-first.
///    Runtime never text-searches or canonicalizes declared strings, and
///    TLS-class resolution is embedded-only: no host fallback exists.
///
/// Returns the materialization guard when a per-boot directory exists —
/// exactly when disk-class targets exist; dropping it removes the
/// directory on shutdown AND boot failure. Proto-only artifacts return
/// `None` with ZERO temp writes.
fn prepare_embedded_assets(
    store: &mut VirtualDocumentStore,
    manifest_json: &str,
) -> Result<Option<super::materialize::Materialization>, PrepareError> {
    let manifest = manifest::Manifest::from_canonical_json(manifest_json.as_bytes())
        .map_err(|e| PrepareError::Manifest(e.to_string()))?;
    populate_asset_registry(store, &manifest)?;

    // The substitution targets in canonical table order, with their
    // relative path under the per-boot directory and their class (for
    // the read-only diagnostic and the boot partition below).
    let mut targets: Vec<(&SubstitutionEntry, String, String)> = Vec::new();
    for entry in &store.index.substitutions {
        let Some(rel) = entry.asset.strip_prefix("assets/") else {
            return Err(PrepareError::StoreInconsistent(format!(
                "substitution target {} is not an asset entry",
                entry.asset
            )));
        };
        let Some(class) = store
            .index
            .entries
            .iter()
            .find(|candidate| candidate.path == entry.asset)
            .and_then(|candidate| candidate.asset_class.clone())
        else {
            return Err(PrepareError::StoreInconsistent(format!(
                "substitution target {} is not a store asset entry",
                entry.asset
            )));
        };
        targets.push((entry, rel.to_string(), class));
    }
    if targets.is_empty() {
        return Ok(None);
    }

    // Partition FIRST: the proto class never materializes, so the
    // materialization directory (and its writability requirement) is
    // created only when a disk class actually needs one.
    let mut sites: BTreeMap<String, Vec<(SubstitutionSpan, Vec<u8>)>> = BTreeMap::new();
    let mut disk_targets: Vec<(&SubstitutionEntry, String, String)> = Vec::new();
    for (entry, rel, class) in targets {
        if class == super::policy::PROTO_ASSET_CLASS {
            // In-memory class: rewrite the site to the reserved
            // reference and register the bytes with the in-process proto
            // compiler. The reference carries only `:` `/` `.` `_` `-`
            // and alphanumerics, so a `uri` site passes the safety rule;
            // a `literal` site fails the literal charset clause (`:` is
            // outside `[A-Za-z0-9._/+-]`), keeping hand-crafted stores
            // fail-closed. Proto sites are URI parameters, so the uri
            // context is the only producer shape.
            let reference = embedded_ref(&entry.asset);
            enforce_substitution_path_safety(&entry.document, &reference, entry.context)
                .map_err(PrepareError::Rule)?;
            #[cfg(feature = "grpc")]
            {
                let Some(bytes) = store.read(&entry.asset) else {
                    return Err(PrepareError::StoreInconsistent(format!(
                        "substitution target {} is unreadable",
                        entry.asset
                    )));
                };
                camel_proto_compiler::embedded::register(&entry.asset, bytes.to_vec());
            }
            push_replacement(&mut sites, entry, &reference);
        } else {
            disk_targets.push((entry, rel, class));
        }
    }

    // A disk-written class exists, so a writable temp location is
    // required: the read-only contract's materialized-class side. A
    // proto-only artifact never reaches this branch.
    let materialization = if disk_targets.is_empty() {
        None
    } else {
        let first_class = disk_targets[0].2.clone();
        Some(
            super::materialize::Materialization::create().map_err(|source| {
                PrepareError::TempUnavailable {
                    class: first_class,
                    source,
                }
            })?,
        )
    };

    if let Some(materialization) = &materialization {
        // Path-safety rule at EVERY disk site before anything is written.
        for (entry, rel, _class) in &disk_targets {
            let confined_path = materialization
                .confined_path(rel)
                .map_err(PrepareError::Materialize)?;
            let Some(confined) = confined_path.to_str() else {
                return Err(PrepareError::Rule(CompileError::InvalidDocument(format!(
                    "substitution path-safety rule violation at site '{}': the confined path is \
                     not valid UTF-8",
                    entry.document
                ))));
            };
            enforce_substitution_path_safety(&entry.document, confined, entry.context)
                .map_err(PrepareError::Rule)?;
            push_replacement(&mut sites, entry, confined);
        }

        // Write exactly the targeted assets (deduped), never the whole
        // store.
        let mut written: Vec<&str> = Vec::new();
        for (entry, rel, _class) in &disk_targets {
            if written.contains(&entry.asset.as_str()) {
                continue;
            }
            let Some(bytes) = store.read(&entry.asset) else {
                return Err(PrepareError::StoreInconsistent(format!(
                    "substitution target {} is unreadable",
                    entry.asset
                )));
            };
            materialization
                .write(rel, bytes)
                .map_err(PrepareError::Materialize)?;
            written.push(entry.asset.as_str());
        }
    }

    rewrite_store_entries(store, &sites).map_err(PrepareError::Rule)?;
    Ok(materialization)
}

/// Self-detect a compiled artifact before any CLI parsing (Task 2.3).
///
/// Probes `current_exe()` and decodes its trailer (version-aware:
/// [`trailer::decode_artifact`]):
///
/// - absent trailer (no exact terminal magic; also an unreadable or
///   missing executable image) → `None`: the caller falls through to the
///   normal Clap CLI unchanged, because absence is indistinguishable
///   from an ordinary executable;
/// - marked corruption (terminal magic present but fields, bounds, or
///   checksum invalid) → fail closed: an integrity diagnostic on stderr
///   and exit 2, never a fall-through;
/// - valid trailer → the artifact argv (`std::args` minus the program
///   name) is parsed with [`ArtifactArgs::parse`] FIRST (misuse exits 2
///   naming the argument), then routed (r4sign Task 1.3): `--verify`
///   runs the verify-only chain and never reaches the boot-verification
///   path; every other invocation runs boot verification BEFORE its
///   normal dispatch, so the artifact is SHA-512-streamed at most once
///   per invocation.
///
/// Version dispatch (multidoc Task 2.2): a v1 artifact feeds the
/// single-document runtime (default in-memory config, embedded-text
/// discovery); a v2 multi-document artifact feeds the virtual-store
/// runtime — `discover_virtual_store` assembles the merged embedded
/// configuration and the ordered source-plan routes before boot, with
/// the deployment environment as the `${env:}` lookup.
/// `decode_artifact` has already validated the store, its references,
/// and the manifest/store agreement before this point; the request
/// builder re-validates as defense in depth.
pub async fn self_detect_artifact() -> Option<i32> {
    // A missing or unreadable executable image carries no trailer
    // evidence: that is absence, not corruption, so fall through.
    let exe = std::env::current_exe().ok()?;
    // Bounded tail probe (rc-j329x): the trailer lives at the image end,
    // so only the footer window plus declared sections are read. The
    // previous whole-image read cost ~80 ms and one-binary-size RSS on
    // every CLI startup, including `--help`.
    let mut image = std::fs::File::open(&exe).ok()?;
    let bytes = trailer::read_probe_tail(&mut image).ok().flatten()?;
    let decoded = match trailer::decode_artifact(&bytes) {
        Ok(Some(decoded)) => decoded,
        Ok(None) => return None,
        Err(e) => {
            eprintln!("compiled artifact integrity error: {e}");
            return Some(EXIT_REJECTION);
        }
    };
    // Arguments parse FIRST (r4sign Task 1.3): the route decision
    // precedes every signature step, so `--verify` never reaches the
    // boot-verification path and every other invocation verifies before
    // its dispatch — at most one artifact SHA-512 stream per invocation.
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match ArtifactArgs::parse(&argv) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{e}");
            return Some(EXIT_REJECTION);
        }
    };
    let manifest = match artifact_manifest(&decoded) {
        Ok(manifest) => manifest,
        Err(()) => return Some(EXIT_REJECTION),
    };
    // The deployment truststore path (keypin Task 1.2), resolved once
    // during argument parsing (argument or `CAMEL_TRUSTSTORE`): both
    // verify sites receive the same value.
    let truststore = args.truststore.as_deref();
    if args.verify {
        return Some(run_verify_only(&exe, &manifest, truststore));
    }
    if let Some(code) = verify_for_boot(&exe, &manifest, truststore) {
        return Some(code);
    }
    let request = match decoded {
        trailer::DecodedArtifact::V1(v1) => EmbeddedRequest::from_trailer(v1, args),
        trailer::DecodedArtifact::V2(v2) => EmbeddedRequest::from_v2(v2, args),
    };
    match request {
        Ok(request) => Some(run_embedded_document_code(request).await),
        Err(e) => {
            eprintln!("compiled artifact integrity error: {e}");
            Some(EXIT_REJECTION)
        }
    }
}

/// Parse the decoded artifact's manifest into the typed
/// [`manifest::Manifest`] the signing decisions read (schema and
/// signing block). `decode_artifact` has already validated the manifest
/// bytes per trailer version, so a failure here is defensive and fails
/// closed with the integrity diagnostic already on stderr.
fn artifact_manifest(decoded: &trailer::DecodedArtifact) -> Result<manifest::Manifest, ()> {
    let bytes = match decoded {
        trailer::DecodedArtifact::V1(v1) => &v1.manifest,
        trailer::DecodedArtifact::V2(v2) => &v2.manifest,
    };
    manifest::Manifest::from_canonical_json(bytes).map_err(|e| {
        eprintln!("compiled artifact integrity error: {e}");
    })
}

/// The detached signature envelope path of `exe`: the artifact path
/// with `.sig` appended.
fn envelope_path(exe: &Path) -> PathBuf {
    let mut name = exe.as_os_str().to_owned();
    name.push(".sig");
    PathBuf::from(name)
}

/// Stream `artifact` through `Sha512` exactly once and return the
/// finalized 64-byte Ed25519ph prehash. Flat memory: `io::copy` feeds
/// the file through the hasher in bounded chunks — the artifact is
/// never buffered.
fn stream_sha512(artifact: &Path) -> std::io::Result<[u8; 64]> {
    let mut file = std::fs::File::open(artifact)?;
    let mut hasher = Sha512::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hasher.finalize().into())
}

/// Whether the manifest marks a boot without the envelope unacceptable:
/// exactly the schema-4 and schema-5 manifests — the only schemas that
/// carry a signing block — whose required bit is set. The decision keys
/// on `manifest_schema` being 4 or 5, never on the signing block's
/// presence: the lenient legacy parse never populates `signing`
/// for a schema other than 4 or 5, so keying on the schema keeps the
/// decision total.
fn requires_signature(manifest: &manifest::Manifest) -> bool {
    matches!(
        manifest.manifest_schema,
        manifest::MANIFEST_SCHEMA_V4 | manifest::MANIFEST_SCHEMA_V5
    ) && manifest.signing.as_ref().is_some_and(|s| s.required)
}

/// Verify a PRESENT envelope against the artifact and the manifest
/// signing block — the shared chain of the `--verify` surface and boot
/// verification (r4sign Task 1.3): the unpaired-schema check first
/// (keyed on manifest schema 4 or 5; schema 3 or earlier plus a present
/// envelope stays the unpaired rejection), then one
/// artifact stream through [`stream_sha512`], then
/// [`signature::verify_envelope`] with the manifest's fingerprint and
/// algorithm. Every failure has already printed its exit-2 diagnostic
/// naming the failing step on stderr.
fn verify_envelope_bytes(
    envelope: &[u8],
    exe: &Path,
    manifest: &manifest::Manifest,
) -> Result<signature::VerifiedEnvelope, ()> {
    // Schema-agnostic rejection: every manifest below schema 4 pairs
    // with no signing block, so a present envelope is stray regardless
    // of the integer printed. Boot-level twins cover schemas 1 and 3
    // (v1_artifact_with_stray_envelope_fails_closed,
    // unsigned_with_stray_envelope_fails_closed); the schema-2
    // boot-level twin is deferred — hand-assembling a store-1 (R1-era)
    // artifact outweighs the marginal coverage, and schema-2 decode and
    // pairing rejection are unit-tested in manifest.rs
    // (manifest_reader_accepts_schema2_and_rejects_unknown_and_unpaired).
    // Reopen the defer only if a store-1 artifact builder ever lands
    // (bd rc-pbezt).
    if !matches!(
        manifest.manifest_schema,
        manifest::MANIFEST_SCHEMA_V4 | manifest::MANIFEST_SCHEMA_V5
    ) {
        eprintln!(
            "compiled artifact signature verification failed: unpaired signature envelope: \
             the manifest (schema {}) carries no signing block; remove the stray envelope \
             or recompile with --sign",
            manifest.manifest_schema
        );
        return Err(());
    }
    let Some(signing) = &manifest.signing else {
        // Unreachable: the schema check above gates on the only schemas
        // whose validated form must carry the signing block. Kept
        // total instead of panicking.
        eprintln!(
            "compiled artifact signature verification failed: the manifest carries no \
             signing block"
        );
        return Err(());
    };
    let digest = stream_sha512(exe).map_err(|e| {
        eprintln!(
            "compiled artifact signature verification failed: the artifact could not be \
             read for verification: {e}"
        );
    })?;
    signature::verify_envelope(
        envelope,
        &digest,
        &signing.key_fingerprint,
        &signing.algorithm,
    )
    .map_err(|e| {
        eprintln!("compiled artifact signature verification failed: {e}");
    })
}

/// The `--verify` chain (r4sign Task 1.3, no boot): read the detached
/// envelope at `<exe>.sig` and verify it against the artifact, apply the
/// shared trust policy (keypin Task 1.2), then print the verified
/// identity and exit 0; any failure exits 2 with the diagnostic on
/// stderr. A missing envelope is its own case: the diagnostic names a
/// required signature when the manifest marks one, the truststore strip
/// rule when a supplied store is in force, or `strict-unsigned` when the
/// store is strict and the manifest carries no signing block. An
/// unreadable envelope, an unpaired envelope, a named verification step,
/// a truststore parse failure (which fails closed for unsigned artifacts
/// too), or an unpinned key each exit 2 as well.
fn run_verify_only(exe: &Path, manifest: &manifest::Manifest, truststore: Option<&Path>) -> i32 {
    let sig = envelope_path(exe);
    let envelope = match std::fs::read(&sig) {
        Ok(envelope) => envelope,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if trust_policy_rejection(truststore, manifest, exe, false, TrustMode::Verify, &sig)
                .is_some()
            {
                return EXIT_REJECTION;
            }
            if requires_signature(manifest) {
                eprintln!(
                    "compiled artifact signature verification failed: no signature envelope \
                     present at {}; the manifest marks the signature required",
                    sig.display()
                );
            } else {
                eprintln!(
                    "compiled artifact signature verification failed: no signature envelope \
                     present at {}",
                    sig.display()
                );
            }
            return EXIT_REJECTION;
        }
        Err(e) => {
            eprintln!(
                "compiled artifact signature verification failed: the signature envelope at \
                 {} could not be read: {e}",
                sig.display()
            );
            return EXIT_REJECTION;
        }
    };
    let verified = match verify_envelope_bytes(&envelope, exe, manifest) {
        Ok(verified) => verified,
        Err(()) => return EXIT_REJECTION,
    };
    if trust_policy_rejection(truststore, manifest, exe, true, TrustMode::Verify, &sig).is_some() {
        return EXIT_REJECTION;
    }
    println!("algorithm: {}", verified.algorithm_name);
    println!("key_fingerprint: {}", verified.fingerprint);
    0
}

/// Which verify site owns the trust policy (keypin Task 2.2): `--verify`
/// is a dry run (reads floors without locking or writing); boot takes the
/// lock and records floors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrustMode {
    /// The `--verify` dry run.
    Verify,
    /// The boot path.
    Boot,
}

/// Why the trust policy refused an artifact — the diagnostic step token
/// the helper printed (keypin Tasks 1.2 and 2.2). `None` from
/// [`trust_policy_rejection`] means the artifact may proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrustRejection {
    /// `truststore-pin`.
    Pin,
    /// `truststore-parse`.
    Parse,
    /// `strict-unsigned`.
    StrictUnsigned,
    /// `freshness-rollback`.
    Rollback,
    /// `truststore-update`.
    Update,
}

/// One boot decision taken inside the truststore lock (keypin Task 2.2).
enum BootLockOutcome {
    /// Marker at the floor (or both absent): proceed, no write needed.
    Accepted,
    /// The pin re-check passed and the floor was recorded.
    Recorded,
    /// The pin was removed while this boot waited for the lock.
    Depinned,
    /// The under-lock marker is below the under-lock floor.
    Rollback { floor: Option<u64> },
}

/// The shared deployment truststore policy (keypin Tasks 1.2 and 2.2),
/// called by BOTH verify sites so the boot and verify-only chains cannot
/// drift. `envelope_present` selects the hook point:
///
/// - absent (the envelope-missing branch of either site): the strip
///   rule — a manifest carrying a signing block (schema 4 or 5) may not
///   pass without its envelope while a truststore is in force, WHATEVER
///   its required bit. The pre-existing required-signature diagnostic
///   keeps its form only on the no-truststore path. Schema-3-or-earlier
///   manifests (no signing block) read the strict directive: a strict
///   store rejects the unsigned artifact (`strict-unsigned`) and a
///   malformed store fails closed (`truststore-parse`); a well-formed
///   non-strict store leaves the unsigned path unchanged. That branch
///   costs exactly one store read.
/// - present (after [`verify_envelope_bytes`] fully passes): parse the
///   truststore — a [`trust::TrustError`] prints its `truststore-parse`
///   diagnostic — require the manifest `key_fingerprint` to be pinned
///   (`truststore-pin` otherwise), then apply the freshness policy.
///   [`TrustMode::Verify`] reads the parsed floors and never locks or
///   writes; [`TrustMode::Boot`] takes the `<truststore>.lock` critical
///   section, re-parses UNDER it, re-checks the pin, decides on the fresh
///   floors, and records on accept.
///
/// Returns `Some(step)` after printing the exit-2 diagnostic when the
/// artifact must not proceed. With no truststore path supplied this
/// touches no file: zero new reads on the R4 path.
fn trust_policy_rejection(
    truststore: Option<&Path>,
    manifest: &manifest::Manifest,
    exe: &Path,
    envelope_present: bool,
    mode: TrustMode,
    sig: &Path,
) -> Option<TrustRejection> {
    // No truststore path supplied: zero new file reads on the R4 path.
    let path = truststore?;
    let Some(signing) = &manifest.signing else {
        // Schema 3 or earlier carries no signing block; a stray envelope
        // on such a manifest is already rejected inside
        // `verify_envelope_bytes` before this hook. The supplied store is
        // still read once: a malformed store fails closed for unsigned
        // artifacts too, and a strict store rejects every unsigned
        // artifact; a well-formed non-strict store keeps the historical
        // unsigned path.
        match trust::TrustStore::parse(path) {
            Err(e) => {
                eprintln!("{e}");
                return Some(TrustRejection::Parse);
            }
            Ok(store) if store.is_strict() => {
                eprintln!(
                    "strict-unsigned: the artifact at {} carries no signing block \
                     (schema {}); the truststore at {} is strict",
                    exe.display(),
                    manifest.manifest_schema,
                    path.display()
                );
                return Some(TrustRejection::StrictUnsigned);
            }
            Ok(_) => return None,
        }
    };
    if !envelope_present {
        eprintln!(
            "truststore-pin: no signature envelope present at {}; the manifest key \
             fingerprint {} must be envelope-verified while the truststore at {} is in force",
            sig.display(),
            signing.key_fingerprint,
            path.display()
        );
        return Some(TrustRejection::Pin);
    }
    match mode {
        TrustMode::Verify => verify_freshness_dry_run(path, signing),
        TrustMode::Boot => boot_freshness_under_lock(path, signing),
    }
}

/// The `--verify` freshness dry run (keypin Task 2.2): parse the
/// truststore, require the pin, and decide on the parsed floors — no lock,
/// no write.
fn verify_freshness_dry_run(
    path: &Path,
    signing: &manifest::SigningBlock,
) -> Option<TrustRejection> {
    let store = match trust::TrustStore::parse(path) {
        Ok(store) => store,
        Err(e) => {
            eprintln!("{e}");
            return Some(TrustRejection::Parse);
        }
    };
    if !store.is_pinned(&signing.key_fingerprint) {
        eprintln!(
            "truststore-pin: the manifest key fingerprint {} is not pinned by the \
             truststore at {}",
            signing.key_fingerprint,
            path.display()
        );
        return Some(TrustRejection::Pin);
    }
    let floor = store.floor(&signing.key_fingerprint);
    match trust::decide(signing.freshness, floor) {
        trust::FreshnessVerdict::Rollback => {
            print_freshness_rollback(signing, floor);
            Some(TrustRejection::Rollback)
        }
        // `Accept` and `AcceptRecord` both boot unchanged: `--verify` is a
        // dry run and never records.
        trust::FreshnessVerdict::Accept | trust::FreshnessVerdict::AcceptRecord => None,
    }
}

/// The boot freshness critical section (keypin Task 2.2): take the
/// truststore lock, re-parse the snapshot UNDER it, re-check the pin,
/// decide on the FRESH floors, and record on accept. A lock or write
/// failure is the underlying [`trust::TrustError`], step
/// `truststore-update`.
fn boot_freshness_under_lock(
    path: &Path,
    signing: &manifest::SigningBlock,
) -> Option<TrustRejection> {
    let lock_path = trust::lock_path_for(path);
    let result = trust::with_truststore_lock(&lock_path, trust::TRUSTSTORE_LOCK_DEADLINE, || {
        let fresh = trust::TrustStore::parse(path)?;
        // A pin removed while this boot waited for the lock must fail
        // closed before the freshness logic runs.
        if !fresh.is_pinned(&signing.key_fingerprint) {
            return Ok(BootLockOutcome::Depinned);
        }
        let floor = fresh.floor(&signing.key_fingerprint);
        match trust::decide(signing.freshness, floor) {
            trust::FreshnessVerdict::Rollback => Ok(BootLockOutcome::Rollback { floor }),
            trust::FreshnessVerdict::Accept => Ok(BootLockOutcome::Accepted),
            trust::FreshnessVerdict::AcceptRecord => {
                // `AcceptRecord` implies a present marker; the `if let`
                // keeps the path total without a panic.
                if let Some(marker) = signing.freshness {
                    trust::record_floor(&fresh, path, &signing.key_fingerprint, marker)?;
                }
                Ok(BootLockOutcome::Recorded)
            }
        }
    });
    match result {
        Ok(BootLockOutcome::Accepted | BootLockOutcome::Recorded) => None,
        Ok(BootLockOutcome::Depinned) => {
            eprintln!(
                "truststore-pin: the manifest key fingerprint {} is no longer pinned by \
                 the truststore at {}",
                signing.key_fingerprint,
                path.display()
            );
            Some(TrustRejection::Pin)
        }
        Ok(BootLockOutcome::Rollback { floor }) => {
            print_freshness_rollback(signing, floor);
            Some(TrustRejection::Rollback)
        }
        Err(e) => {
            eprintln!("{e}");
            Some(rejection_step_for(&e))
        }
    }
}

/// The diagnostic step token of a [`trust::TrustError`]: the lock and
/// write variants are `truststore-update`, every parse variant is
/// `truststore-parse`.
fn rejection_step_for(error: &trust::TrustError) -> TrustRejection {
    match error {
        trust::TrustError::LockFailure { .. }
        | trust::TrustError::Unwritable { .. }
        | trust::TrustError::Io { .. } => TrustRejection::Update,
        trust::TrustError::Unreadable { .. }
        | trust::TrustError::NotUtf8 { .. }
        | trust::TrustError::MalformedEntry { .. }
        | trust::TrustError::DuplicatePin { .. } => TrustRejection::Parse,
    }
}

/// Print the exit-2 `freshness-rollback` diagnostic naming the
/// fingerprint and the floor (keypin Task 2.2). A missing marker (legacy
/// schema 4) names the schema-4 form instead.
fn print_freshness_rollback(signing: &manifest::SigningBlock, floor: Option<u64>) {
    let floor = match floor {
        Some(value) => value.to_string(),
        None => "unknown".to_string(),
    };
    match signing.freshness {
        Some(marker) => eprintln!(
            "freshness-rollback: the freshness marker {marker} of key {} is below the \
             recorded floor {floor}",
            signing.key_fingerprint
        ),
        None => eprintln!(
            "freshness-rollback: the schema-4 artifact key {} carries no freshness marker \
             and the recorded floor is {floor}",
            signing.key_fingerprint
        ),
    }
}

/// Boot verification for every non-`--verify` invocation, including the
/// bare boot (r4sign Task 1.3): a missing envelope refuses the boot
/// only when the manifest marks the signature required — or, under a
/// supplied truststore, when the manifest carries a signing block (the
/// keypin Task 1.2 strip rule) or the store is strict and the manifest
/// carries no signing block (`strict-unsigned`) — otherwise the
/// artifact proceeds with ZERO hashing (v1 compatibility). A present envelope runs the shared
/// verification chain and then the trust policy before the dispatch; any
/// failure exits 2 with the named step on stderr and no boot.
///
/// Returns `Some(EXIT_REJECTION)` when the boot must not proceed.
fn verify_for_boot(
    exe: &Path,
    manifest: &manifest::Manifest,
    truststore: Option<&Path>,
) -> Option<i32> {
    let sig = envelope_path(exe);
    let envelope = match std::fs::read(&sig) {
        Ok(envelope) => envelope,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if trust_policy_rejection(truststore, manifest, exe, false, TrustMode::Boot, &sig)
                .is_some()
            {
                return Some(EXIT_REJECTION);
            }
            if requires_signature(manifest) {
                eprintln!(
                    "compiled artifact boot refused: the manifest marks the signature \
                     required, but no signature envelope is present at {}",
                    sig.display()
                );
                return Some(EXIT_REJECTION);
            }
            // No envelope and no requirement: boot unchanged, v1
            // compatible — zero hashing.
            return None;
        }
        Err(e) => {
            eprintln!(
                "compiled artifact signature verification failed: the signature envelope at \
                 {} could not be read: {e}",
                sig.display()
            );
            return Some(EXIT_REJECTION);
        }
    };
    match verify_envelope_bytes(&envelope, exe, manifest) {
        Ok(_) => {}
        Err(()) => return Some(EXIT_REJECTION),
    }
    trust_policy_rejection(truststore, manifest, exe, true, TrustMode::Boot, &sig)
        .map(|_| EXIT_REJECTION)
}

/// Same dispatch returning the raw process code (0/1/2); the seam for
/// harness children that re-exit with [`std::process::exit`] (an
/// `ExitCode` cannot be read back out).
pub async fn run_embedded_document_code(request: EmbeddedRequest) -> i32 {
    // The exclusive print-and-exit modes are common to both artifact
    // versions and never boot.
    let (help, version, manifest, report) = match &request {
        EmbeddedRequest::SingleDocument { args, .. }
        | EmbeddedRequest::VirtualStore { args, .. } => {
            (args.help, args.version, args.manifest, args.report.clone())
        }
    };
    if help {
        print_artifact_usage();
        return 0;
    }
    if version {
        println!("camel {}", manifest::RUNTIME_VERSION);
        return 0;
    }
    if manifest {
        let manifest_json = match &request {
            EmbeddedRequest::SingleDocument { manifest_json, .. }
            | EmbeddedRequest::VirtualStore { manifest_json, .. } => manifest_json,
        };
        println!("{manifest_json}");
        return 0;
    }
    match request {
        EmbeddedRequest::SingleDocument {
            kind,
            source_name,
            document,
            ..
        } => match kind {
            TrailerKind::Route => {
                run_embedded_route(&source_name, &document, report.as_deref()).await
            }
            TrailerKind::Job => {
                crate::commands::job::run_embedded_job(&source_name, &document, report).await
            }
        },
        EmbeddedRequest::VirtualStore {
            kind,
            mut store,
            manifest_json,
            ..
        } => {
            // The DECIDED single swap point (r2embed Task 3.2): after
            // decode, BEFORE the kind dispatch. The store is rewritten
            // IN-LEASE and passed by value below, so neither
            // `drive_lifecycle` nor `run_embedded_job_store` changes.
            let materialization = match prepare_embedded_assets(&mut store, &manifest_json) {
                Ok(prepared) => prepared,
                Err(e) => {
                    eprintln!("compiled artifact error: {e}");
                    return EXIT_REJECTION;
                }
            };
            let code = match kind {
                TrailerKind::Route => run_embedded_store_route(&store, report.as_deref()).await,
                TrailerKind::Job => {
                    crate::commands::job::run_embedded_job_store(store, report).await
                }
            };
            // The confined per-boot directory is removed on BOTH the
            // graceful return and every boot-failure return above: the
            // guard drops at this scope's end either way.
            drop(materialization);
            code
        }
    }
}

/// Artifact `--help` text (no boot).
fn print_artifact_usage() {
    println!("camel compiled artifact usage:");
    println!("  --report <path>  write the run report to <path>");
    println!("  --manifest       print the operational manifest and exit");
    println!("  --verify         verify the detached signature envelope and exit");
    println!(
        "  --truststore <path>  deployment truststore pin file (pairs with a boot and --verify)"
    );
    println!("  --version        print the runtime version and exit");
    println!("  --help           print this usage and exit");
}

/// Route-artifact lifecycle: default in-memory config, embedded source,
/// `watch = false` — the same boot, route registration, context start,
/// signal, and shutdown path `camel run` drives. Writes the
/// [`RouteReport`] to `report` when given.
///
/// Exit codes: 0 graceful completion; 2 boot/discovery/report-write
/// failure. Pipeline failure (1) stays reserved: a signal-driven route
/// run ends either in graceful completion or a boot-class failure.
async fn run_embedded_route(source_name: &str, document: &str, report: Option<&Path>) -> i32 {
    let config = match crate::commands::run::in_memory_default_config() {
        Ok(config) => config,
        Err(e) => return fail_route(report, e.to_string()),
    };
    let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let spec = LifecycleSpec {
        config,
        project_root,
        discover: Discover::Embedded {
            text: document.to_string(),
            source_name: source_name.to_string(),
            kind: camel_dsl::EmbeddedDocumentKind::Route,
        },
        watch: None,
        trust_note: false,
        idle_note: IDLE_NOTE,
    };
    match crate::commands::run::drive_lifecycle(spec).await {
        Ok(()) => {
            if let Err(e) = write_route_report(report, &RouteReport::completed()) {
                eprintln!("failed to write route report: {e}");
                return EXIT_REJECTION;
            }
            0
        }
        Err(LifecycleFailure::Discovery(e)) => fail_route(report, e.to_string()),
        Err(LifecycleFailure::Boot(e)) => fail_route(report, e.to_string()),
    }
}

/// Pre-boot failure of the shared virtual-store resolution. The two
/// phases fail for disjoint reasons, and the job lifecycle reports them
/// under different labels, so the variant is preserved for the caller.
#[derive(Debug)]
pub(crate) enum VirtualStoreResolveError {
    /// Store validation or source-plan discovery failed.
    Discovery(camel_dsl::DiscoveryError),
    /// The merged configuration failed to deserialize into a
    /// `CamelConfig`.
    Config(config::ConfigError),
}

impl fmt::Display for VirtualStoreResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Discovery(e) => e.fmt(f),
            Self::Config(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for VirtualStoreResolveError {}

/// Shared pre-boot resolution for v2 virtual-store artifacts (multidoc
/// Task 2.2): discovery assembles the merged embedded configuration and
/// the ordered source-plan routes, then the merged TOML tree
/// deserializes into the deployment `CamelConfig` with `${env:}`
/// resolution through `env`. Both steps run strictly BEFORE boot; the
/// caller owns the error reporting and the lifecycle hand-off.
pub(crate) fn resolve_virtual_store(
    store: &super::store::VirtualDocumentStore,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<
    (
        camel_config::config::CamelConfig,
        Vec<camel_core::RouteDefinition>,
    ),
    VirtualStoreResolveError,
> {
    let discovery = camel_dsl::discover_virtual_store(store, env)
        .map_err(VirtualStoreResolveError::Discovery)?;
    let config = camel_config::config::CamelConfig::from_toml_value_with_env(discovery.config, env)
        .map_err(VirtualStoreResolveError::Config)?;
    Ok((config, discovery.routes))
}

/// Route-artifact lifecycle for a v2 virtual store (multidoc Task 2.2):
/// merged embedded configuration, ordered source-plan routes, `watch =
/// false` — the same boot, route registration, context start, signal,
/// and shutdown path `camel run` drives. Writes the [`RouteReport`] to
/// `report` when given.
///
/// Store validation, configuration assembly, and route discovery run
/// BEFORE boot ([`resolve_virtual_store`]): an unknown schema, invalid
/// reference, malformed range, kind mismatch, or malformed
/// configuration entry exits 2 with a named diagnostic and no boot.
/// Exit codes: 0 graceful completion; 2
/// validation/discovery/config/boot/report-write failure.
async fn run_embedded_store_route(
    store: &super::store::VirtualDocumentStore,
    report: Option<&Path>,
) -> i32 {
    let identity = store.index.entry_point.clone();
    let ambient = |name: &str| std::env::var(name).ok();
    let (config, routes) = match resolve_virtual_store(store, &ambient) {
        Ok(resolved) => resolved,
        Err(e) => {
            eprintln!("compiled://{identity}: {e}");
            return fail_route(report, e.to_string());
        }
    };
    let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let spec = LifecycleSpec {
        config,
        project_root,
        discover: Discover::VirtualStore { routes },
        watch: None,
        trust_note: false,
        idle_note: IDLE_NOTE,
    };
    match crate::commands::run::drive_lifecycle(spec).await {
        Ok(()) => {
            if let Err(e) = write_route_report(report, &RouteReport::completed()) {
                eprintln!("failed to write route report: {e}");
                return EXIT_REJECTION;
            }
            0
        }
        Err(LifecycleFailure::Discovery(e)) => fail_route(report, e.to_string()),
        Err(LifecycleFailure::Boot(e)) => fail_route(report, e.to_string()),
    }
}

/// Record a failed run: write the failed report (best effort) and return
/// the boot-failure exit code.
fn fail_route(report: Option<&Path>, error: String) -> i32 {
    if let Err(e) = write_route_report(report, &RouteReport::failed(error.clone())) {
        eprintln!("failed to write route report: {e}");
    }
    EXIT_REJECTION
}

/// Write the report JSON (with trailing newline) when a path was given.
fn write_route_report(report: Option<&Path>, value: &RouteReport) -> std::io::Result<()> {
    match report {
        Some(path) => std::fs::write(path, format!("{}\n", value.to_json())),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::{ArtifactArgError, ArtifactArgs, RouteReport, TRUSTSTORE_ENV, TrailerKind};

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    /// Serializes every env-var mutation in this module's tests so
    /// parallel lib tests cannot observe a half-set variable.
    static TRUSTSTORE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Restores `CAMEL_TRUSTSTORE` to its prior value on drop, so a
    /// panicking assertion cannot leak the test's env mutation into
    /// other tests.
    struct TruststoreEnvRestore(Option<std::ffi::OsString>);

    impl TruststoreEnvRestore {
        fn set(value: &str) -> Self {
            let prior = std::env::var_os(TRUSTSTORE_ENV);
            // SAFETY: test-scoped; the guard restores the prior value on drop.
            unsafe { std::env::set_var(TRUSTSTORE_ENV, value) };
            Self(prior)
        }
    }

    impl Drop for TruststoreEnvRestore {
        fn drop(&mut self) {
            match self.0.take() {
                Some(prior) => {
                    // SAFETY: test-scoped restore of the value captured
                    // at guard creation.
                    unsafe { std::env::set_var(TRUSTSTORE_ENV, prior) };
                }
                None => {
                    // SAFETY: test-scoped; the var was unset before the test.
                    unsafe { std::env::remove_var(TRUSTSTORE_ENV) };
                }
            }
        }
    }

    /// The four accepted forms parse into their flags.
    #[test]
    fn artifact_args_accept_the_documented_surface() {
        let args = ArtifactArgs::parse(&argv(&["--report", "out.json"])).expect("report parses");
        assert_eq!(args.report, Some(std::path::PathBuf::from("out.json")));
        assert!(ArtifactArgs::parse(&argv(&["--help"])).expect("help").help);
        assert!(
            ArtifactArgs::parse(&argv(&["--version"]))
                .expect("version")
                .version
        );
        assert!(
            ArtifactArgs::parse(&argv(&["--manifest"]))
                .expect("manifest")
                .manifest
        );
        assert!(
            ArtifactArgs::parse(&argv(&["--verify"]))
                .expect("verify")
                .verify
        );
        assert!(
            ArtifactArgs::parse(&argv(&[]))
                .expect("bare run")
                .report
                .is_none()
        );
    }

    /// Duplicate, missing-value, exclusive, unknown, and positional forms
    /// are rejected with the argument named.
    #[test]
    fn artifact_args_reject_misuse() {
        assert_eq!(
            ArtifactArgs::parse(&argv(&["--report", "a", "--report", "b"])),
            Err(ArtifactArgError::Duplicate("--report"))
        );
        assert_eq!(
            ArtifactArgs::parse(&argv(&["--report"])),
            Err(ArtifactArgError::MissingValue("--report"))
        );
        assert_eq!(
            ArtifactArgs::parse(&argv(&["--report", "--manifest"])),
            Err(ArtifactArgError::MissingValue("--report"))
        );
        assert_eq!(
            ArtifactArgs::parse(&argv(&["--help", "--version"])),
            Err(ArtifactArgError::Exclusive("--help", "--version"))
        );
        assert_eq!(
            ArtifactArgs::parse(&argv(&["--report", "r", "--manifest"])),
            Err(ArtifactArgError::Exclusive("--report", "--manifest"))
        );
        assert_eq!(
            ArtifactArgs::parse(&argv(&["--watch"])),
            Err(ArtifactArgError::Unknown("--watch".to_string()))
        );
        assert_eq!(
            ArtifactArgs::parse(&argv(&["routes.yaml"])),
            Err(ArtifactArgError::Positional("routes.yaml".to_string()))
        );
    }

    /// `--verify` (r4sign Task 1.3) stays exclusive with every other
    /// artifact flag, in either order, and rejects as a duplicate of
    /// itself.
    #[test]
    fn artifact_args_verify_stays_exclusive() {
        assert_eq!(
            ArtifactArgs::parse(&argv(&["--verify", "--manifest"])),
            Err(ArtifactArgError::Exclusive("--verify", "--manifest"))
        );
        assert_eq!(
            ArtifactArgs::parse(&argv(&["--manifest", "--verify"])),
            Err(ArtifactArgError::Exclusive("--manifest", "--verify"))
        );
        assert_eq!(
            ArtifactArgs::parse(&argv(&["--verify", "--report", "r"])),
            Err(ArtifactArgError::Exclusive("--verify", "--report"))
        );
        assert_eq!(
            ArtifactArgs::parse(&argv(&["--report", "r", "--verify"])),
            Err(ArtifactArgError::Exclusive("--report", "--verify"))
        );
        assert_eq!(
            ArtifactArgs::parse(&argv(&["--verify", "--help"])),
            Err(ArtifactArgError::Exclusive("--verify", "--help"))
        );
        assert_eq!(
            ArtifactArgs::parse(&argv(&["--verify", "--version"])),
            Err(ArtifactArgError::Exclusive("--verify", "--version"))
        );
        assert_eq!(
            ArtifactArgs::parse(&argv(&["--verify", "--verify"])),
            Err(ArtifactArgError::Duplicate("--verify"))
        );
    }

    /// The `CAMEL_TRUSTSTORE` fallback (keypin Task 1.1): with no
    /// `--truststore` argument the env var supplies the path; with
    /// both, the argument wins.
    #[test]
    fn artifact_args_resolve_env_truststore() {
        let _env_lock = TRUSTSTORE_ENV_LOCK.lock().expect("env lock poisoned");
        let _env = TruststoreEnvRestore::set("/deploy/env-truststore.keys");

        // (a) No argument: the env var supplies the truststore.
        let from_env = ArtifactArgs::parse(&argv(&[])).expect("bare run parses");
        assert_eq!(
            from_env.truststore,
            Some(std::path::PathBuf::from("/deploy/env-truststore.keys"))
        );

        // (b) Both present: the argument wins.
        let both = ArtifactArgs::parse(&argv(&["--truststore", "/deploy/arg-truststore.keys"]))
            .expect("argument parses");
        assert_eq!(
            both.truststore,
            Some(std::path::PathBuf::from("/deploy/arg-truststore.keys"))
        );
    }

    /// Reverse-order enforcement (keypin Task 1.1): `--truststore`
    /// following an exclusive mode is rejected the same way, naming
    /// both flags.
    #[test]
    fn artifact_args_truststore_after_mode_is_exclusive() {
        assert_eq!(
            ArtifactArgs::parse(&argv(&["--help", "--truststore", "t"])),
            Err(ArtifactArgError::Exclusive("--help", "--truststore"))
        );
    }

    /// The route report serializes to the exact documented JSON object.
    #[test]
    fn route_report_serializes_exact_json() {
        assert_eq!(
            RouteReport::completed().to_json(),
            r#"{"kind":"route","status":"completed","error":null}"#
        );
        assert_eq!(
            RouteReport::failed("boom".to_string()).to_json(),
            r#"{"kind":"route","status":"failed","error":"boom"}"#
        );
    }

    /// The substitution path-safety rule rejects an ASCII control
    /// character at EVERY site (literal and uri alike), rejects a space
    /// at a `literal` site (outside `[A-Za-z0-9._/+-]`) naming the
    /// offending character, and passes a space at a `uri` site (the
    /// value percent-encodes) — r2embed Task 3.2.
    #[test]
    fn substitution_path_safety_rule_rejects_control_and_unsafe_literal_chars() {
        use super::SubstitutionContext;
        use super::enforce_substitution_path_safety;

        let phrase = "substitution path-safety rule violation";
        let ctrl: String = std::iter::once('c')
            .chain(std::iter::once('\u{7}'))
            .collect();
        let ctrl_path = format!("/tmp/boot/{ctrl}/svc.crt");

        // Control character: rejected at both site kinds, naming the
        // site and the diagnostic phrase.
        for context in [SubstitutionContext::Literal, SubstitutionContext::Uri] {
            let err = enforce_substitution_path_safety("routes/app.yaml", &ctrl_path, context)
                .expect_err("control characters are rejected at every site");
            assert!(
                err.to_string().contains(phrase),
                "diagnostic names the rule: {err}"
            );
            assert!(
                err.to_string().contains("routes/app.yaml"),
                "diagnostic names the site: {err}"
            );
        }

        // Space at a literal site: outside the safe set, rejected with
        // the offending character named.
        let spaced = "/tmp/boot dir/svc.crt";
        let err = enforce_substitution_path_safety(
            "routes/app.yaml",
            spaced,
            SubstitutionContext::Literal,
        )
        .expect_err("a literal-site space is outside the safe set");
        assert!(
            err.to_string().contains(phrase),
            "diagnostic names the rule: {err}"
        );
        assert!(
            err.to_string().contains(' '),
            "diagnostic names the offending character: {err}"
        );

        // The same space at a uri site PASSES: the value percent-encodes.
        enforce_substitution_path_safety("routes/app.yaml", spaced, SubstitutionContext::Uri)
            .expect("a uri-site space passes (percent-encoded)");

        // Clean paths pass at both site kinds.
        let clean = "/tmp/camel-assets-ab12cd34/certs/svc.crt";
        for context in [SubstitutionContext::Literal, SubstitutionContext::Uri] {
            enforce_substitution_path_safety("routes/app.yaml", clean, context)
                .expect("a safe path passes at every site");
        }
    }

    /// The span rewrite applies recorded spans LAST-OFFSET-FIRST and is
    /// surgical: every recorded span is replaced exactly once, byte
    /// offsets of line starts stay stable across applications, and
    /// non-recorded text is untouched — r2embed Task 3.2.
    #[test]
    fn span_rewrite_applies_last_offset_first_and_is_surgical() {
        use super::SubstitutionSpan;
        use super::apply_span_rewrites;

        // Offsets: "alpha XX mid YY end ZZ\nnext XX line\nlast YY\n"
        // spans: 6..8, 13..15, 20..22, 28..30, 41..43; line starts:
        // "next" at 23, "last" at 36.
        let text = b"alpha XX mid YY end ZZ\nnext XX line\nlast YY\n";
        assert_eq!(&text[6..8], b"XX", "fixture span sanity");
        assert_eq!(&text[23..27], b"next", "fixture line-start sanity");
        assert_eq!(&text[36..40], b"last", "fixture line-start sanity");

        let span = |start: u64, end: u64| SubstitutionSpan { start, end };
        // All replacements are length-preserving except the LAST one
        // (41..43, 2 -> 6 bytes): applying last-offset-first means that
        // expansion is applied FIRST and shifts no recorded span and no
        // line start before it. A first-offset-first application would
        // inflate the earlier spans' offsets and corrupt the rewrite.
        let replacements = vec![
            (span(6, 8), b"P1".to_vec()),
            (span(13, 15), b"P2".to_vec()),
            (span(20, 22), b"P3".to_vec()),
            (span(28, 30), b"P4".to_vec()),
            (span(41, 43), b"LONGER".to_vec()),
        ];

        let out = apply_span_rewrites(text, &replacements).expect("rewrite succeeds");

        // Every recorded span replaced exactly once, in order.
        assert_eq!(
            out.as_slice(),
            &b"alpha P1 mid P2 end P3\nnext P4 line\nlast LONGER\n"[..],
            "all recorded spans are replaced exactly once"
        );

        // Surgical: non-recorded text untouched.
        for literal in ["alpha ", " mid ", " end ", "\nnext ", " line\nlast ", "\n"] {
            assert!(
                out.windows(literal.len()).any(|w| w == literal.as_bytes()),
                "non-recorded text survives: {literal:?} in {}",
                String::from_utf8_lossy(&out)
            );
        }

        // Line-start offsets stable across applications: with the
        // last-offset-first order, the length-changing final rewrite
        // never moves an earlier line start.
        assert_eq!(&out[23..27], b"next", "second line start keeps its offset");
        assert_eq!(&out[36..40], b"last", "third line start keeps its offset");
    }

    /// `from_v2` decodes the store and re-validates the typed reference
    /// invariants as defense in depth: a hand-constructed store whose
    /// entry point kind disagrees with the artifact kind fails by name
    /// (multidoc Task 2.2 — the runtime-level rejection is pinned by
    /// `compiled_runtime_rejects_invalid_store_before_boot`, which
    /// replaced the interim fail-closed bridge test).
    #[test]
    fn from_v2_rejects_kind_mismatched_stores() {
        use super::super::store::{StoreDocument, StoreEntryKind};

        let route_doc = |path: &str| StoreDocument {
            path: path.to_string(),
            kind: StoreEntryKind::Route,
            bytes: b"routes:\n  - id: demo\n".to_vec(),
        };
        let job_doc = |path: &str| StoreDocument {
            path: path.to_string(),
            kind: StoreEntryKind::Job,
            bytes: b"execute:\n  mode: one-shot\n".to_vec(),
        };
        let v2 = |store: &super::super::store::VirtualDocumentStore| {
            use super::super::trailer::TrailerV2;
            TrailerV2 {
                kind: TrailerKind::Route,
                content: store.content.clone(),
                index: store.index.encode_canonical().expect("canonical index"),
                manifest: br#"{"manifest_schema":2,"source_name":"app.yaml"}"#.to_vec(),
            }
        };

        // Kind agreement: a route artifact over a route entry point
        // builds the virtual-store request.
        let route_store = super::super::store::VirtualDocumentStore::build(
            "app.yaml",
            &[route_doc("app.yaml")],
            &[],
            &["app.yaml".to_string()],
        )
        .expect("route store builds");
        let request = super::EmbeddedRequest::from_v2(v2(&route_store), Default::default())
            .expect("kind-agreeing store builds a request");
        assert!(matches!(
            request,
            super::EmbeddedRequest::VirtualStore { .. }
        ));

        // Kind mismatch: a route artifact over a job entry point fails
        // by name before any boot.
        let job_store = super::super::store::VirtualDocumentStore::build(
            "ingest.job.yaml",
            &[job_doc("ingest.job.yaml")],
            &[],
            &["ingest.job.yaml".to_string()],
        )
        .expect("job store builds");
        let err = super::EmbeddedRequest::from_v2(v2(&job_store), Default::default())
            .expect_err("kind-mismatched store must fail closed");
        assert!(
            err.to_string().contains("expected route"),
            "the rejection must name the kind mismatch: {err}"
        );
    }

    /// A synthetic schema-4 manifest for `fingerprint` — no freshness
    /// marker — so the policy boundary is testable without constructing a
    /// signed schema-4 artifact (impossible once the compiler emits
    /// schema 5: the manifest lives inside the signed bytes and the
    /// trailer checksum).
    fn schema4_manifest(fingerprint: &str) -> super::manifest::Manifest {
        super::manifest::Manifest {
            manifest_schema: super::manifest::MANIFEST_SCHEMA_V4,
            source_name: "app.yaml".to_string(),
            runtime_version: super::manifest::RUNTIME_VERSION.to_string(),
            kind: TrailerKind::Job,
            artifact_kind: "job".to_string(),
            components: Vec::new(),
            env_names: Vec::new(),
            listeners: Vec::new(),
            embedded_files: Vec::new(),
            total_embedded_bytes: 0,
            signing: Some(super::manifest::SigningBlock {
                algorithm: "ed25519ph".to_string(),
                freshness: None,
                key_fingerprint: fingerprint.to_string(),
                required: false,
            }),
        }
    }

    /// The schema-4 boundary (keypin Task 2.2): a pinned schema-4
    /// artifact passes pin-only while no floor is recorded for its key,
    /// and rolls back once any floor exists.
    #[test]
    fn schema4_boundary_is_the_first_recorded_floor() {
        let dir = tempfile::tempdir().expect("tempdir");
        let fingerprint = format!("blake3:{}", "a0".repeat(32));
        let manifest = schema4_manifest(&fingerprint);
        let exe = dir.path().join("app.bin");
        let sig = dir.path().join("app.bin.sig");

        // (a) No floor recorded: the pin check passes and the boot
        // proceeds (plain pin-only acceptance).
        let no_floor = dir.path().join("no-floor.keys");
        std::fs::write(&no_floor, format!("{fingerprint}\n")).expect("write truststore");
        assert_eq!(
            super::trust_policy_rejection(
                Some(&no_floor),
                &manifest,
                &exe,
                true,
                super::TrustMode::Boot,
                &sig,
            ),
            None,
            "a schema-4 artifact with no recorded floor proceeds"
        );

        // (b) Any floor recorded: the marker-less schema-4 artifact is
        // below it and rolls back.
        let floored = dir.path().join("floored.keys");
        std::fs::write(&floored, format!("{fingerprint} 7\n")).expect("write truststore");
        assert_eq!(
            super::trust_policy_rejection(
                Some(&floored),
                &manifest,
                &exe,
                true,
                super::TrustMode::Boot,
                &sig,
            ),
            Some(super::TrustRejection::Rollback),
            "a schema-4 artifact below a recorded floor rolls back"
        );
    }

    /// A pin removed while a boot holds the truststore lock fails closed
    /// on the under-lock re-parse (keypin Task 2.2): the pin re-check
    /// fires before the freshness step ever runs.
    #[test]
    fn pin_removal_under_lock_fails_closed() {
        use fs4::FileExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let fingerprint = format!("blake3:{}", "ae".repeat(32));
        let manifest = schema4_manifest(&fingerprint);
        let exe = dir.path().join("app.bin");
        let sig = dir.path().join("app.bin.sig");
        let truststore = dir.path().join("pins.keys");
        std::fs::write(&truststore, format!("{fingerprint}\n")).expect("write truststore");
        let lock_path = super::trust::lock_path_for(&truststore);

        // Another boot holds the lock while the pin is removed; the
        // release lets this boot proceed to its under-lock re-parse.
        let held = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .expect("open the lock file");
        FileExt::lock(&held).expect("hold the lock");
        std::fs::write(&truststore, "# pin removed\n").expect("rewrite without the pin");
        FileExt::unlock(&held).expect("release the lock");

        assert_eq!(
            super::trust_policy_rejection(
                Some(&truststore),
                &manifest,
                &exe,
                true,
                super::TrustMode::Boot,
                &sig,
            ),
            Some(super::TrustRejection::Pin),
            "the under-lock re-parse must fail closed when the pin was removed"
        );
    }

    // ── Embedded proto boot (mission 350) ──────────────────────────────

    use super::super::store::{StoreAsset, StoreDocument, StoreEntryKind};
    use super::{
        SubstitutionContext, SubstitutionEntry, SubstitutionSpan, URI_SITE_SET,
        VirtualDocumentStore, utf8_percent_encode,
    };

    /// Serializes the two `prepare_embedded_assets` tests: the asset
    /// registry is process-global and populate-once, so concurrent
    /// preparation fixtures would race the population check.
    static PREPARE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Byte span of the first occurrence of `declared` inside the site
    /// text — the fixture twin of the seal-time span scan.
    fn declared_span(text: &str, declared: &str) -> SubstitutionSpan {
        let start = text.find(declared).expect("declared present in site") as u64;
        SubstitutionSpan {
            start,
            end: start + declared.len() as u64,
        }
    }

    /// T5: a proto-only artifact never materializes a per-boot
    /// directory (`Ok(None)`, zero temp writes), its URI site rewrites
    /// to the percent-encoded `camel-embedded:` reference, and the
    /// registry holds the asset bytes for the in-process compiler.
    #[test]
    fn prepare_embeds_proto_assets_without_materialization() {
        let _serial = PREPARE_LOCK.lock().expect("prepare lock");

        let asset_name = "protos/hello350a.proto";
        let proto_source =
            "syntax = \"proto3\";\npackage p350a;\nmessage M350a { string a = 1; }\n";
        let doc = format!(
            "routes:\n  - id: grpc-serve\n    from: \
             'grpc://127.0.0.1:50051/p350a.Svc/M?protoFile={asset_name}&transport=plaintext'\n    \
             steps:\n      - log: req\n"
        );
        let span = declared_span(&doc, asset_name);
        let store = VirtualDocumentStore::build_with_assets(
            "app.yaml",
            &[StoreDocument {
                path: "app.yaml".to_string(),
                kind: StoreEntryKind::Route,
                bytes: doc.clone().into_bytes(),
            }],
            &[StoreAsset {
                path: asset_name.to_string(),
                class: Some(super::super::policy::PROTO_ASSET_CLASS.to_string()),
                bytes: proto_source.as_bytes().to_vec(),
            }],
            &[],
            &["app.yaml".to_string()],
            &[SubstitutionEntry {
                asset: format!("assets/{asset_name}"),
                context: SubstitutionContext::Uri,
                declared: asset_name.to_string(),
                document: "app.yaml".to_string(),
                spans: vec![span],
            }],
        )
        .expect("store builds");
        let manifest =
            super::manifest::derive_for_store(&store, TrailerKind::Route, &[]).expect("manifest");
        let manifest_json = manifest.to_canonical_json();

        let mut store = store;
        let prepared = super::prepare_embedded_assets(&mut store, &manifest_json)
            .expect("prepare succeeds for a proto-only store");
        assert!(
            prepared.is_none(),
            "a proto-only artifact must not create a per-boot directory"
        );

        let rewritten =
            String::from_utf8(store.read("app.yaml").expect("site entry present").to_vec())
                .expect("site entry is UTF-8");
        let expected = utf8_percent_encode(
            &super::embedded_ref("assets/protos/hello350a.proto"),
            URI_SITE_SET,
        )
        .to_string();
        assert!(
            rewritten.contains(&expected),
            "the URI site must carry the embedded reference ({expected}): {rewritten}"
        );

        #[cfg(feature = "grpc")]
        assert_eq!(
            &camel_proto_compiler::embedded::get("assets/protos/hello350a.proto")
                .expect("asset registered")[..],
            proto_source.as_bytes(),
            "the registry must hold the embedded proto bytes"
        );
    }

    /// T6: mixed classes materialize ONLY the disk-class asset (the
    /// proto asset never touches disk), the disk site rewrites to the
    /// percent-encoded confined path, and the proto site rewrites to the
    /// embedded reference.
    #[test]
    fn prepare_materializes_disk_classes_alongside_embedded_proto() {
        let _serial = PREPARE_LOCK.lock().expect("prepare lock");

        let proto_asset = "protos/hello350b.proto";
        let proto_source =
            "syntax = \"proto3\";\npackage p350b;\nmessage M350b { string a = 1; }\n";
        let cert_asset = "grpc/server350b.crt";
        let cert_source = "-----BEGIN CERTIFICATE-----\nfixture350b\n-----END CERTIFICATE-----\n";
        let doc = format!(
            "routes:\n  - id: grpc-serve\n    from: \
             'grpc://127.0.0.1:50051/p350b.Svc/M?protoFile={proto_asset}&transport=tls&\
             serverCertPath={cert_asset}'\n    steps:\n      - log: req\n"
        );
        let proto_span = declared_span(&doc, proto_asset);
        let cert_span = declared_span(&doc, cert_asset);
        let store = VirtualDocumentStore::build_with_assets(
            "app.yaml",
            &[StoreDocument {
                path: "app.yaml".to_string(),
                kind: StoreEntryKind::Route,
                bytes: doc.into_bytes(),
            }],
            &[
                StoreAsset {
                    path: cert_asset.to_string(),
                    class: Some("certificate".to_string()),
                    bytes: cert_source.as_bytes().to_vec(),
                },
                StoreAsset {
                    path: proto_asset.to_string(),
                    class: Some(super::super::policy::PROTO_ASSET_CLASS.to_string()),
                    bytes: proto_source.as_bytes().to_vec(),
                },
            ],
            &[],
            &["app.yaml".to_string()],
            &[
                SubstitutionEntry {
                    asset: format!("assets/{proto_asset}"),
                    context: SubstitutionContext::Uri,
                    declared: proto_asset.to_string(),
                    document: "app.yaml".to_string(),
                    spans: vec![proto_span],
                },
                SubstitutionEntry {
                    asset: format!("assets/{cert_asset}"),
                    context: SubstitutionContext::Uri,
                    declared: cert_asset.to_string(),
                    document: "app.yaml".to_string(),
                    spans: vec![cert_span],
                },
            ],
        )
        .expect("store builds");
        let manifest =
            super::manifest::derive_for_store(&store, TrailerKind::Route, &[]).expect("manifest");
        let manifest_json = manifest.to_canonical_json();

        let mut store = store;
        let guard = super::prepare_embedded_assets(&mut store, &manifest_json)
            .expect("prepare succeeds for the mixed store")
            .expect("disk classes materialize a per-boot directory");

        // ONLY the TLS asset is on disk; the proto asset is absent.
        assert!(
            guard.path().join(cert_asset).is_file(),
            "the certificate asset must materialize"
        );
        assert!(
            !guard.path().join("protos").exists(),
            "the proto asset must never touch disk"
        );

        let rewritten =
            String::from_utf8(store.read("app.yaml").expect("site entry present").to_vec())
                .expect("site entry is UTF-8");
        let embedded_expected = utf8_percent_encode(
            &super::embedded_ref("assets/protos/hello350b.proto"),
            URI_SITE_SET,
        )
        .to_string();
        assert!(
            rewritten.contains(&embedded_expected),
            "the proto site must carry the embedded reference ({embedded_expected}): {rewritten}"
        );
        let confined = guard
            .confined_path(cert_asset)
            .expect("confined path computes");
        let confined_expected =
            utf8_percent_encode(confined.to_str().expect("utf-8"), URI_SITE_SET).to_string();
        assert!(
            rewritten.contains(&confined_expected),
            "the TLS site must carry the confined path ({confined_expected}): {rewritten}"
        );
    }
}
