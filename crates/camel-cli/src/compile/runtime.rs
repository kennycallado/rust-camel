//! Embedded-document runtime for compiled artifacts (openspec change
//! `cli-compile`, Tasks 2.2 and 2.3).
//!
//! [`run_embedded_document`] takes a validated [`EmbeddedRequest`] — the
//! decoded trailer payload plus the parsed artifact arguments — and runs
//! it through the EXISTING lifecycles:
//!
//! - route artifacts drive the shared `camel run` lifecycle
//!   ([`crate::commands::run::drive_lifecycle`]) with the default
//!   in-memory config, the embedded discovery seam, and `watch = false`;
//! - job artifacts drive the existing single-document job
//!   report/outcome lifecycle with the embedded document as their sole
//!   route source.
//!
//! No compile-time asset is resolved at runtime, nothing is extracted to
//! a temporary location, and the watcher never activates. `${env:NAME}`
//! resolves from the deployment environment through the discovery path.
//! Declared job `args:` resolve at startup through the same parser path
//! as normal jobs with an EMPTY `--arg` list (jobargs Task 3.2):
//! embedded declaration defaults fill the interpolated fields, and a
//! required declaration without a default exits 2 before boot.
//!
//! [`self_detect_artifact`] is the binary entry point (Task 2.3): the
//! `camel` main calls it BEFORE Clap parses anything, so a self-contained
//! artifact consumes its own argv surface (`--report`, `--help`,
//! `--version`, `--manifest`) while a trailer-free image falls through to
//! the normal CLI unchanged.
//!
//! Exit codes: 0 graceful completion / job Completed; 1 job pipeline
//! failure (route runs end either in graceful completion or a boot-class
//! failure, so 1 stays reserved for them); 2 argument misuse, boot
//! failure, or report-write failure.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::Serialize;

use super::CompileError;
use super::manifest;
use super::trailer::{self, Trailer, TrailerKind};
use crate::commands::run::{Discover, LifecycleFailure, LifecycleSpec};

/// Exit code for argument misuse, boot failure, and report-write failure.
const EXIT_REJECTION: i32 = 2;

/// Idle log line while a route artifact runs (watch disabled).
const IDLE_NOTE: &str = "compiled artifact running (hot-reload disabled). Press Ctrl+C to stop.";

/// Parsed artifact argument surface.
///
/// The exclusive modes (`--help`, `--version`, `--manifest`) print and
/// exit 0 without booting; `--report <path>` pairs with a run. The
/// surface is deliberately narrow: job arguments (`--arg`) are
/// unsupported (rejected as unknown) — declared arguments resolve from
/// the embedded declarations alone (jobargs Task 3.2).
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

impl ArtifactArgs {
    /// Parse the artifact argv (no program name). Accepts `--report
    /// <path>`, `--help`, `--version`, and `--manifest`; rejects
    /// duplicates, exclusive combinations, missing report values,
    /// unknown flags, and positional arguments.
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
                "--help" | "--version" | "--manifest" => {
                    let flag: &'static str = match arg {
                        "--help" => "--help",
                        "--version" => "--version",
                        _ => "--manifest",
                    };
                    if let Some(prev) = parsed.first_set() {
                        return Err(if prev == flag {
                            ArtifactArgError::Duplicate(flag)
                        } else {
                            ArtifactArgError::Exclusive(prev, flag)
                        });
                    }
                    match flag {
                        "--help" => parsed.help = true,
                        "--version" => parsed.version = true,
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

/// One validated embedded run request: the decoded trailer document plus
/// the parsed artifact arguments. Built from a `Trailer` by
/// [`EmbeddedRequest::from_trailer`] (integrity was verified at decode).
#[derive(Debug, Clone)]
pub struct EmbeddedRequest {
    /// Embedded artifact kind (route/job).
    pub kind: TrailerKind,
    /// Logical source name; the runtime source identity is
    /// `compiled://<source_name>`.
    pub source_name: String,
    /// Normalized document text (pre-interpolation authoring text).
    pub document: String,
    /// Canonical manifest JSON (printed verbatim by `--manifest`).
    pub manifest_json: String,
    /// Parsed artifact arguments.
    pub args: ArtifactArgs,
}

impl EmbeddedRequest {
    /// Build the request from a decoded trailer and parsed arguments.
    /// Both payload and manifest are validated UTF-8 (the encoder
    /// normalized the document and serialized the manifest as canonical
    /// UTF-8 JSON); the manifest must carry its `source_name`.
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
        Ok(Self {
            kind: trailer.kind,
            source_name,
            document,
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

/// Self-detect a compiled artifact before any CLI parsing (Task 2.3).
///
/// Probes `current_exe()` and decodes its trailer:
///
/// - absent trailer (no exact terminal magic; also an unreadable or
///   missing executable image) → `None`: the caller falls through to the
///   normal Clap CLI unchanged, because absence is indistinguishable
///   from an ordinary executable;
/// - marked corruption (terminal magic present but fields, bounds, or
///   checksum invalid) → fail closed: an integrity diagnostic on stderr
///   and exit 2, never a fall-through;
/// - valid trailer → the artifact argv (`std::args` minus the program
///   name) is parsed with [`ArtifactArgs::parse`] (misuse exits 2 naming
///   the argument), the payload decodes into an [`EmbeddedRequest`], and
///   the request dispatches through [`run_embedded_document_code`]:
///   `--help`, `--version`, and `--manifest` print and exit 0 without
///   booting.
pub async fn self_detect_artifact() -> Option<i32> {
    // A missing or unreadable executable image carries no trailer
    // evidence: that is absence, not corruption, so fall through.
    let exe = std::env::current_exe().ok()?;
    let bytes = std::fs::read(exe).ok()?;
    let trailer = match trailer::decode(&bytes) {
        Ok(Some(trailer)) => trailer,
        Ok(None) => return None,
        Err(e) => {
            eprintln!("compiled artifact integrity error: {e}");
            return Some(EXIT_REJECTION);
        }
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match ArtifactArgs::parse(&argv) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{e}");
            return Some(EXIT_REJECTION);
        }
    };
    let request = match EmbeddedRequest::from_trailer(trailer, args) {
        Ok(request) => request,
        Err(e) => {
            eprintln!("compiled artifact integrity error: {e}");
            return Some(EXIT_REJECTION);
        }
    };
    Some(run_embedded_document_code(request).await)
}

/// Same dispatch returning the raw process code (0/1/2); the seam for
/// harness children that re-exit with [`std::process::exit`] (an
/// `ExitCode` cannot be read back out).
pub async fn run_embedded_document_code(request: EmbeddedRequest) -> i32 {
    let EmbeddedRequest {
        kind,
        source_name,
        document,
        manifest_json,
        args,
    } = request;
    if args.help {
        print_artifact_usage();
        return 0;
    }
    if args.version {
        println!("camel {}", manifest::RUNTIME_VERSION);
        return 0;
    }
    if args.manifest {
        println!("{manifest_json}");
        return 0;
    }
    match kind {
        TrailerKind::Route => {
            run_embedded_route(&source_name, &document, args.report.as_deref()).await
        }
        TrailerKind::Job => {
            crate::commands::job::run_embedded_job(&source_name, &document, args.report).await
        }
    }
}

/// Artifact `--help` text (no boot).
fn print_artifact_usage() {
    println!("camel compiled artifact usage:");
    println!("  --report <path>  write the run report to <path>");
    println!("  --manifest       print the operational manifest and exit");
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
    use super::{ArtifactArgError, ArtifactArgs, RouteReport};

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
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
}
