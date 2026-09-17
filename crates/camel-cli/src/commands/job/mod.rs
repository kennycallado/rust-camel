//! `camel job <FILE>` — one-shot route execution from a `*.job.yaml`
//! document declaring a top-level `execute:` section. A bare name
//! probes `<name>.job.yaml` across the ordered `[jobs].dirs` roots;
//! no argument lists the discovery set.
//!
//! The job boots the REAL composition root (the same seams `camel run`
//! uses: config load, security compile context, bind-exposure acks, the
//! `camel_bundles` cascade, real `${env:}` resolution through
//! discovery), reuses the document-family route-source keys, then sends
//! exactly one exchange to the document's `direct:`/`seda:` target and
//! shuts down. All discovered routes start; side-effect safety comes
//! from the load-time fail-closed consumer allowlist, with the send
//! target as the sole entry point.
//!
//! Exit codes mirror the `camel test` taxonomy (`2 > 1 > 0`): 0 the
//! pipeline completed; 1 the pipeline failed; 2 any load, validation,
//! boot, interruption, drain-timeout, shutdown, or report-write
//! error. A first SIGINT or SIGTERM cancels the in-flight send or
//! batch drain, runs bounded teardown, and reports outcome
//! `Interrupted` (exit 2); a second signal during that teardown
//! force-exits 1. Every failure after the boot handle is acquired —
//! route loading, consumer validation, target selection, route
//! registration, context start — runs the same bounded teardown
//! before its exit-2 return (the post-boot teardown border, bd
//! rc-7wl19). The process exit is applied only at the `main.rs`
//! boundary; this module returns the code.
//!
//! Spec: openspec/changes/cli-jobs.

mod batch;
mod document;
mod help;
mod signal;

#[cfg(test)]
mod document_tests;

#[cfg(test)]
mod help_tests;

#[cfg(test)]
mod job_effective_config_tests;

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use camel_api::{Body, CamelError, Exchange, Message};
use camel_component_api::NoOpComponentContext;
use clap::Args;
use noyalib::compat::serde_yaml;
use serde::{Deserialize, Serialize};
use tower::ServiceExt;

use document::{JobBody, JobDocument, JobRouteSource};
// Compile-time declaration gate for `camel compile` (jobtyped Task 5);
// the `document` module is private to this one.
pub(crate) use document::validate_job_declarations_for_compile;
use signal::{JobSignals, JobWaitOutcome, await_job_operation_or_signal};

/// Startup-race retry sleep for the send's producer delivery.
const SEND_RETRY_SLEEP: Duration = Duration::from_millis(20);

/// Startup-race retry window for the send: bounded like the unit-tier
/// `deliver_input` deadline (1 s) plus the SEDA consumer-readiness bound
/// (2 s), NOT the overall job timeout — a no-consumer error on `direct:`
/// is a permanent route defect that must surface after a short window,
/// not spin the whole budget.
const SEND_RETRY_WINDOW: Duration = Duration::from_secs(3);

/// Floor for the shutdown budget once the overall timeout is spent: the
/// BootHandle still gets a bounded window to drain pools.
const MIN_SHUTDOWN_BUDGET: Duration = Duration::from_secs(5);

/// CLI args for `camel job`.
#[derive(Args, Debug)]
// The `job` subcommand owns its `--help`/`-h` surface: a bare usage
// print without a name, the declared interface with one. clap's auto
// help flag is disabled for this subcommand only — the top-level
// `camel --help` and every other subcommand keep clap help.
#[command(disable_help_flag = true)]
pub struct JobArgs {
    /// Path to the job document (`*.job.yaml` with an `execute:`
    /// section). A bare name (no separator, no suffix) probes
    /// `<name>.job.yaml` in every `[jobs].dirs` root; omitted lists
    /// the discovery set.
    #[arg(value_name = "FILE")]
    pub document: Option<PathBuf>,
    /// With a job name it renders the job's declared interface;
    /// without a name it prints `camel job` usage.
    #[arg(long = "help", short = 'h', action = clap::ArgAction::SetTrue)]
    pub help: bool,
    /// Write the JSON report to this path instead of stdout.
    #[arg(long, value_name = "FILE")]
    pub report: Option<PathBuf>,
    /// Path to Camel.toml config file.
    ///
    /// Also read from `CAMEL_CONFIG_FILE`, matching `camel run`'s
    /// `--config`; explicit `--config` wins over env.
    #[arg(
        long,
        value_name = "FILE",
        default_value = "Camel.toml",
        env = "CAMEL_CONFIG_FILE"
    )]
    pub config: String,
    /// Repeatable NAME=VALUE pair. On documents WITHOUT `args:` the
    /// pair is injected as a message header at send time (applied after
    /// document headers; last occurrence wins; deprecation note on
    /// stderr). On declared documents the pair must name a declared
    /// argument and resolves through `${arg:}` interpolation instead.
    #[arg(
        long = "arg",
        value_name = "NAME=VALUE",
        value_parser = parse_arg_pair
    )]
    pub args: Vec<(String, String)>,
}

/// Parse one `--arg` value as a NAME=VALUE pair: split at the FIRST `=`
/// (the value may contain further `=`); an empty name is a usage error,
/// an empty value is allowed.
fn parse_arg_pair(raw: &str) -> Result<(String, String), String> {
    match raw.split_once('=') {
        None => Err(format!("invalid --arg value `{raw}`: expected NAME=VALUE")),
        Some(("", _)) => Err(format!("invalid --arg value `{raw}`: name is empty")),
        Some((name, value)) => Ok((name.to_string(), value.to_string())),
    }
}

/// The deprecation note for the legacy implicit-header path: emitted
/// exactly once per run when a document without `args:` receives
/// `--arg` pairs. The wording is a stable output contract (the delta
/// spec requires a note "identifying the legacy behavior"; unit tests
/// pin it verbatim).
const LEGACY_ARG_DEPRECATION: &str = "camel job: --arg header injection on documents \
 without an `args:` block is deprecated; declare arguments in a top-level `args:` block instead";

/// The JSON report of one job run.
#[derive(Serialize)]
struct JobReport {
    /// Displayed path of the job document.
    document: String,
    /// Execution mode (`one-shot` or `batch`).
    mode: String,
    /// Outcome: `Completed` (or `Stopped`, see `terminated_early`),
    /// `Failed`, `Timeout`, or `Interrupted` (first SIGINT/SIGTERM).
    outcome: &'static str,
    /// `true` when the pipeline ended through a `Stop` step. Always
    /// `false` in v1: the producer reply seam deliberately erases the
    /// `Stopped`/`Completed` distinction (ADR-0024 §3.5), and surfacing
    /// it needs a camel-core observation point (deferred).
    terminated_early: bool,
    /// Wall-clock job duration (boot through send completion), in
    /// milliseconds.
    duration_ms: u128,
    /// Captured reply, present only when `capture-reply` is set and a
    /// reply exchange was returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    reply: Option<ReplyReport>,
    /// Error detail for `Failed`/`Timeout` outcomes.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// Teardown failure detail when a shutdown failure follows a recorded
    /// verdict; `error` keeps the pipeline/timeout verdict.
    #[serde(skip_serializing_if = "Option::is_none")]
    shutdown_error: Option<String>,
}

/// The captured reply of the send action.
#[derive(Serialize)]
struct ReplyReport {
    /// Reply body: JSON value, string, or `null` (empty/streaming).
    body: serde_json::Value,
    /// Reply headers.
    headers: serde_json::Map<String, serde_json::Value>,
}

/// Send-phase failure classes: the split decides the exit code.
enum SendError {
    /// The route pipeline failed (`PipelineOutcome::Failed` surfaced as
    /// the producer's `Err`) — verdict class, exit 1.
    Pipeline(CamelError),
    /// Producer/endpoint apparatus failure after the retry budget —
    /// exit 2.
    Transport(String),
}

/// Maximum directory depth of the metadata listing walk: the root is
/// depth 0, every nested directory level adds one, and entries deeper
/// than 8 are never inspected.
const LISTING_MAX_DEPTH: usize = 8;

/// Maximum encountered files per listing root — every file counts, not
/// just job documents: the 513th file of a root is never inspected.
const LISTING_MAX_FILES: usize = 512;

/// The ordered job discovery roots, anchored at the Camel.toml root
/// (never the process CWD): `try_canonical_project_root(--config)`
/// joined with each `resolved_dirs()` entry, paired with its
/// configured label for display. Shared by bare-name resolution and
/// no-argument listing so the two surfaces cannot drift. A dangling
/// `--config` parent is an error for the caller to map — this never
/// inherits `camel run`'s exit-1 convention, and never silently falls
/// back to the CWD.
fn jobs_roots(
    args: &JobArgs,
    camel_config: &camel_config::config::CamelConfig,
) -> Result<Vec<(String, PathBuf)>, String> {
    crate::commands::run::try_canonical_project_root(Path::new(&args.config))
        .map(|root| {
            camel_config
                .jobs
                .resolved_dirs()
                .into_iter()
                .map(|label| {
                    let path = root.join(&label);
                    (label, path)
                })
                .collect()
        })
        .map_err(|e| {
            format!(
                "cannot resolve project root from --config {}: {e}",
                args.config
            )
        })
}

/// Resolve a document argument. An explicit path (any path separator,
/// or a `.yaml`/`.yml`/`.json` suffix) is used as-is — including an
/// explicit `.job.yml`. A bare name probes exactly
/// `<root>/<name>.job.yaml` in every configured root (one
/// deterministic spelling, no alternate-suffix probing, root-level
/// only — nested documents are never bare-resolved), collecting ALL
/// matches before selection so a cross-root stem collision is an
/// explicit error naming every matching path instead of a silent
/// first win; a miss names every probed file.
fn resolve_job_path(raw: &Path, roots: &[(String, PathBuf)]) -> Result<PathBuf, String> {
    let name = raw.to_string_lossy();
    let lower = name.to_lowercase();
    let explicit = raw.components().count() > 1
        || lower.ends_with(".yaml")
        || lower.ends_with(".yml")
        || lower.ends_with(".json");
    if explicit {
        return Ok(raw.to_path_buf());
    }
    let probes: Vec<PathBuf> = roots
        .iter()
        .map(|(_, root)| root.join(format!("{name}.job.yaml")))
        .collect();
    let matches: Vec<PathBuf> = probes
        .iter()
        .filter(|probe| probe.exists())
        .cloned()
        .collect();
    match matches.as_slice() {
        [] => Err(format!(
            "no job `{name}` in any configured root (looked for {})",
            probes
                .iter()
                .map(|probe| probe.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
        [only] => Ok(only.clone()),
        many => Err(format!(
            "job `{name}` is ambiguous: matches {} configured roots: {}",
            many.len(),
            many.iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Cheap listing probe: reads only the optional `description` key.
/// NEVER the full document grammar — a malformed sibling must not abort
/// the listing. No `deny_unknown_fields`: every other key is ignored.
#[derive(Deserialize)]
struct JobListProbe {
    #[serde(default)]
    description: Option<String>,
}

/// Probe the in-memory job document text for its description. Outer
/// `None` = unparseable; `Some(None)` = parseable without
/// `description:`; `Some(Some(d))` = the description string. Shared by
/// the listing probe (file text) and the `--help` path (the text
/// already read for the document parse).
fn probe_description_str(text: &str) -> Option<Option<String>> {
    let probe: JobListProbe = serde_yaml::from_str(text).ok()?;
    Some(probe.description)
}

/// Probe one job document for its description: read the file, then
/// [`probe_description_str`] on its text. Outer `None` covers an
/// unreadable file and an unparseable document alike (the row renders
/// `(unparseable)`).
fn probe_description(path: &Path) -> Option<Option<String>> {
    let text = std::fs::read_to_string(path).ok()?;
    probe_description_str(&text)
}

/// The display stem of a job document file name: the name with its
/// `.job.yaml`/`.job.yml` suffix stripped, or the full name when
/// neither suffix is present. Shared by the listing display names and
/// the `--help` interface header so the two surfaces cannot drift.
fn job_stem(name: &str) -> &str {
    name.strip_suffix(".job.yaml")
        .or_else(|| name.strip_suffix(".job.yml"))
        .unwrap_or(name)
}

/// One listed job document: the display name (the bare stem for files
/// directly under the configured root, `relative/path: stem` for
/// nested ones) and the probed description.
struct ListedJob {
    display: String,
    description: Option<Option<String>>,
}

/// The bounded scan of one configured root: the discovered job rows
/// and, when a cap stopped the walk early, the cap value that
/// truncated it.
struct RootScan {
    jobs: Vec<ListedJob>,
    truncated_at: Option<usize>,
}

/// Scan one configured root (metadata only): a lexical, recursive
/// walk that never follows directory symlinks, bounded by
/// [`LISTING_MAX_DEPTH`] and [`LISTING_MAX_FILES`]. An absent
/// root is the caller's hint case; other read failures of the ROOT
/// surface as `Err`, while failures below the root only skip that
/// subtree.
fn scan_root(root: &Path) -> std::io::Result<RootScan> {
    let mut scan = RootScan {
        jobs: Vec::new(),
        truncated_at: None,
    };
    let mut files_seen = 0usize;
    walk_level(root, root, 0, &mut scan, &mut files_seen)?;
    Ok(scan)
}

/// Walk one directory level of a root scan. `depth` is the depth of
/// the entries directly inside `dir` (the root call passes 0). Entries
/// are visited in lexical order; directories are traversed only while
/// their contents stay within [`LISTING_MAX_DEPTH`] (a directory whose
/// entries would exceed the cap is skipped and marks the scan
/// truncated), and every encountered file — job document or not —
/// counts toward [`LISTING_MAX_FILES`] (the first file past the cap
/// marks the scan truncated and aborts the root). Returns `false`
/// once the file cap aborts the walk so callers stop the whole root.
///
/// The traversal is bounded in memory: the directory is re-read once
/// per yielded entry and each pass keeps only the lexicographically
/// smallest name after the cursor — never the directory's full entry
/// list — so no directory size can force an unbounded allocation and
/// readdir order can never leak into the output.
fn walk_level(
    dir: &Path,
    root: &Path,
    depth: usize,
    scan: &mut RootScan,
    files_seen: &mut usize,
) -> std::io::Result<bool> {
    // Lexical cursor: every pass yields exactly the smallest entry
    // name strictly greater than the last visited one, so the visit
    // order is deterministic and per-directory memory stays at one
    // candidate name.
    let mut cursor: Option<std::ffi::OsString> = None;
    loop {
        let mut next: Option<(std::ffi::OsString, bool)> = None;
        for entry in std::fs::read_dir(dir)? {
            let Ok(entry) = entry else {
                continue;
            };
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            if cursor
                .as_ref()
                .is_some_and(|seen| name.as_os_str() <= seen.as_os_str())
            {
                continue;
            }
            let take = match &next {
                Some((best, _)) => &name < best,
                None => true,
            };
            if take {
                next = Some((name, file_type.is_dir()));
            }
        }
        let Some((name, is_dir)) = next else {
            return Ok(true);
        };
        cursor = Some(name.clone());
        let path = dir.join(&name);
        // Real directories only: the file type comes from
        // `DirEntry::file_type`, which never follows the entry, so a
        // symlinked directory is not traversed.
        if is_dir {
            if depth >= LISTING_MAX_DEPTH {
                scan.truncated_at.get_or_insert(LISTING_MAX_DEPTH);
                continue;
            }
            // A read failure below the root skips the subtree — a
            // malformed sibling must never abort the listing.
            if !walk_level(&path, root, depth + 1, scan, files_seen).unwrap_or(true) {
                return Ok(false);
            }
            continue;
        }
        // Files count whether or not they are job documents; a symlink
        // to a file is followed (flat-listing parity), anything else
        // (dangling symlink, fifo) is skipped.
        if !path.is_file() {
            continue;
        }
        *files_seen += 1;
        if *files_seen > LISTING_MAX_FILES {
            scan.truncated_at.get_or_insert(LISTING_MAX_FILES);
            return Ok(false);
        }
        if !camel_dsl::discovery::is_job_document(&path) {
            continue;
        }
        let name = name.to_string_lossy().into_owned();
        let stem = job_stem(&name).to_string();
        let display = match path.strip_prefix(root) {
            // Nested documents carry their configured-root-relative
            // path; root-level documents keep the bare stem.
            Ok(relative) if relative.components().count() > 1 => {
                format!("{}: {}", relative.display(), stem)
            }
            _ => stem,
        };
        scan.jobs.push(ListedJob {
            display,
            description: probe_description(&path),
        });
    }
}

/// List the configured job discovery roots (`camel job` with no
/// document argument). Exit 0 for found, empty, and absent roots alike
/// — listing is a query, not a usage error (ls semantics). A root the
/// walker cannot read (permissions, ...) is a real error for THAT root
/// while the remaining roots keep scanning. Listing output and the
/// JSON run report never co-occur: the report path requires a
/// document.
fn list_jobs(roots: &[(String, PathBuf)]) -> i32 {
    let mut exit = 0;
    for (label, root) in roots {
        let scan = match scan_root(root) {
            Ok(scan) => scan,
            // Absent is legitimate for a fresh project (ls semantics,
            // exit 0). Any other read failure is a real error for this
            // root; the scan continues with the remaining roots.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                println!(
                    "No jobs found in {label}/. Create a `<name>.job.yaml` there, or run `camel job <path>`."
                );
                continue;
            }
            Err(e) => {
                eprintln!("cannot read jobs dir `{}`: {e}", root.display());
                exit = 2;
                continue;
            }
        };
        // One root-specific truncation warning; the remaining roots
        // still scan.
        if let Some(cap) = scan.truncated_at {
            eprintln!("camel job: root `{label}` listing truncated at {cap}; narrow [jobs].dirs");
        }
        if scan.jobs.is_empty() {
            println!(
                "No jobs found in {label}/. Create a `<name>.job.yaml` there, or run `camel job <path>`."
            );
            continue;
        }
        println!("Jobs in {label}/:");
        for job in &scan.jobs {
            let rendered = match &job.description {
                None => "(unparseable)".to_string(),
                Some(None) => "(no description)".to_string(),
                Some(Some(d)) => d.replace(['\n', '\r'], " "),
            };
            println!("{} — {rendered}", job.display);
        }
    }
    exit
}

/// Run one job document; returns the process exit code (`main.rs`
/// applies it). Every failure path prints to stderr; the JSON report
/// goes to stdout (default) or `--report`.
pub async fn run_job(args: &JobArgs) -> i32 {
    // 0. Register the SIGINT/SIGTERM streams BEFORE config load, but
    //    only when an execution run follows (a document AND no
    //    `--help`): a signal arriving during boot is buffered by the
    //    runtime and consumed by the send race below, instead of
    //    hitting the default disposition and killing the process
    //    (spec: signal during boot is buffered). Both streams stay
    //    preserved until the first signal is consumed; ownership then
    //    moves to the force-exit guard. Every other path — the
    //    no-document listing, `--help` in either spelling — never
    //    installs the streams: handlers whose streams are never
    //    consumed would swallow SIGINT/SIGTERM during listing or help
    //    rendering instead of letting the default disposition
    //    terminate the process.
    let signals = (args.document.is_some() && !args.help).then(JobSignals::arm);
    // Flush the `signal streams armed` marker for subprocess
    // synchronization: stderr is unbuffered and flushed here, while
    // the tracing subscriber installs only inside
    // `configure_context_with_beans` below (a tracing event at this
    // point would go nowhere). Document runs only — the streams (and
    // the marker that synchronizes on them) exist only on the
    // document path; the listing path has no signal-sensitive
    // stretch. The marker is test-synchronization machinery, so it is
    // emitted only when `CAMEL_JOB_SIGNAL_MARKER` opts in: production
    // document runs keep a clean stderr (successful runs must leave it
    // empty).
    if signals.is_some() && std::env::var_os("CAMEL_JOB_SIGNAL_MARKER").is_some() {
        eprintln!("camel job: signal streams armed");
    }

    let started = Instant::now();

    // Config first: bare-name resolution and listing need the ordered
    // `[jobs].dirs` roots.
    let camel_config = match crate::commands::run::load_config_or_default(&args.config) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("camel-cli job failed: {e}");
            return 2;
        }
    };

    let jobs_roots = match jobs_roots(args, &camel_config) {
        Ok(roots) => roots,
        Err(msg) => {
            eprintln!("{msg}");
            return 2;
        }
    };

    // The no-document else branch is the usage/listing path; the
    // document branch internally splits help vs execution on
    // `args.help`. `signals` (armed only for execution runs) passes
    // through as the `Option` `execute_job` already accepts.
    let Some(raw_document) = &args.document else {
        if args.help {
            print!(
                "{}",
                JobArgs::augment_args(clap::Command::new("camel job")).render_help()
            );
            return 0;
        }
        if args.report.is_some() {
            eprintln!("--report requires a job document");
            return 2;
        }
        return list_jobs(&jobs_roots);
    };
    let resolved = match resolve_job_path(raw_document, &jobs_roots) {
        Ok(path) => path,
        Err(msg) => {
            eprintln!("{msg}");
            return 2;
        }
    };

    // ---- Load-time validation (exit 2 class) --------------------------
    let document_path = match std::fs::canonicalize(&resolved) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("{}: {e}", resolved.display());
            return 2;
        }
    };
    let text = match std::fs::read_to_string(&document_path) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("{}: {e}", document_path.display());
            return 2;
        }
    };
    // `--help` renders the declared interface and returns HERE —
    // before pair validation, boot, report write, and route-source
    // resolution. A malformed document still fails loud with the same
    // `{path}: {error}` diagnostic shape the execution path uses.
    if args.help {
        let info = match document::parse_job_document_for_help(&document_path, &text) {
            Ok(info) => info,
            Err(e) => {
                eprintln!("{}: {e}", document_path.display());
                return 2;
            }
        };
        let file_name = document_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| document_path.display().to_string());
        let description = probe_description_str(&text);
        println!(
            "{}",
            help::render_job_help(
                job_stem(&file_name),
                description.flatten().as_deref(),
                &info
            )
        );
        return 0;
    }
    let doc = match document::parse_job_document_with_args(&document_path, &text, &args.args) {
        Ok(doc) => doc,
        Err(e) => {
            eprintln!("{}: {e}", document_path.display());
            return 2;
        }
    };
    // Legacy implicit-header path: pairs stay raw send-time headers, and
    // the command emits exactly ONE deprecation note identifying the
    // legacy behavior (only when the behavior is actually exercised).
    // Declared documents resolved the pairs through their declarations
    // during the parse above and inject no headers.
    let legacy_header_args = doc.legacy_arg_headers();
    if legacy_header_args && !args.args.is_empty() {
        eprintln!("{LEGACY_ARG_DEPRECATION}");
    }
    let doc_dir = document_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let route_load = match document::resolve_route_source(&doc, &doc_dir) {
        Ok(JobRouteSource::Patterns(patterns)) => RouteLoad::Discovery(patterns),
        Ok(JobRouteSource::Inline(text)) => RouteLoad::Inline(text),
        Err(e) => {
            eprintln!("{}: {e}", document_path.display());
            return 2;
        }
    };

    let run = JobRun {
        label: document_path.display().to_string(),
        started,
        route_load,
        project_root: crate::commands::run::canonical_project_root(Path::new(&args.config)),
        report_path: args.report.clone(),
        // Raw header pairs are a LEGACY-path construct: declared
        // documents resolve `--arg` through declarations + interpolation
        // at parse time and must not get implicit headers; embedded
        // runs never carry pairs.
        cli_args: if legacy_header_args {
            args.args.clone()
        } else {
            Vec::new()
        },
    };
    execute_job(doc, run, camel_config, signals).await
}

/// How a job run loads its route definitions.
enum RouteLoad {
    /// Filesystem discovery patterns (the CLI file forms): the real-boot
    /// discovery seam with ambient `${env:}`.
    Discovery(Vec<String>),
    /// Inline `routes:` text through `parse_routes_with_env` with the
    /// AMBIENT environment (the CLI inline form; the hermetic
    /// document-env closure is test-family machinery and is deliberately
    /// not used here).
    Inline(String),
    /// Embedded inline text through the camel-dsl embedded seam (compiled
    /// artifacts): virtual identity `compiled://<source_name>`, ambient
    /// (deployment) environment as the `${env:}` lookup.
    Embedded { text: String, source_name: String },
    /// Route definitions already discovered from an embedded virtual
    /// store's indexed route files (v2 compiled artifacts, multidoc
    /// Task 2.2): parsed through `camel_dsl::discover_virtual_store`
    /// with the deployment environment before boot — no filesystem
    /// route discovery ever runs.
    Discovered(Vec<camel_core::RouteDefinition>),
}

/// Everything one job execution needs beyond the parsed document.
struct JobRun {
    /// Display path of the job document (diagnostics and the report's
    /// `document` field).
    label: String,
    /// Deadline anchor: process start, so the overall timeout covers
    /// boot, send, drain, and teardown.
    started: Instant,
    /// Route loading strategy.
    route_load: RouteLoad,
    /// camel-bundles base dir (wasm resolution root).
    project_root: PathBuf,
    /// `--report` path; `None` writes the report to stdout.
    report_path: Option<PathBuf>,
    /// Raw CLI `--arg NAME=VALUE` header pairs — legacy documents only
    /// (declared documents resolve pairs through declarations +
    /// interpolation at parse time and pass none; embedded runs pass
    /// none).
    cli_args: Vec<(String, String)>,
}

/// Run one embedded job document (compiled artifact; openspec change
/// `cli-compile`, Task 2.2). The embedded text is the document's sole
/// route source: file-form route sources are compile-time assets and are
/// rejected here (fail closed, defense in depth behind the compile-time
/// policy), and the inline form loads through the camel-dsl embedded seam
/// with the virtual identity `compiled://<source_name>`. Configuration is
/// the default in-memory config (no Camel.toml, no `CAMEL_*` overrides);
/// the report/outcome lifecycle and exit codes are exactly the existing
/// `camel job` taxonomy.
///
/// Declared `args:` documents (jobargs Task 3.2) resolve through the same
/// parse path as normal jobs with EMPTY `--arg` pairs: the embedded
/// declaration defaults fill `to`, `body`, `headers`, and `timeout` via
/// the shared namespace-specific interpolation stage, and a required
/// declaration without a default fails HERE — exit 2, before boot —
/// because an artifact has no `--arg` surface to fill it.
pub(crate) async fn run_embedded_job(
    source_name: &str,
    text: &str,
    report: Option<PathBuf>,
) -> i32 {
    let started = Instant::now();
    let camel_config = match crate::commands::run::in_memory_default_config() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("camel-cli job failed: {e}");
            return 2;
        }
    };
    let doc = match document::parse_job_document_with_args(Path::new(source_name), text, &[]) {
        Ok(doc) => doc,
        Err(document::JobDocError::MissingRequiredArgument { name }) => {
            eprintln!(
                "compiled://{source_name}: missing required argument `{name}`: compiled \
                 artifacts cannot accept --arg; declare a `default` for the argument in \
                 the document instead"
            );
            return 2;
        }
        Err(e) => {
            eprintln!("compiled://{source_name}: {e}");
            return 2;
        }
    };
    // Sole-source rule: only the inline `routes:` form is compilable; a
    // file form is a compile-time asset and is rejected at runtime.
    // `doc_dir` is never consulted on the inline path.
    let route_load = match document::resolve_route_source(&doc, Path::new(".")) {
        Ok(JobRouteSource::Inline(text)) => RouteLoad::Embedded {
            text,
            source_name: source_name.to_string(),
        },
        Ok(JobRouteSource::Patterns(_)) => {
            eprintln!(
                "compiled://{source_name}: embedded job document declares file route \
                 sources; compiled artifacts reject compile-time route-file assets at \
                 runtime"
            );
            return 2;
        }
        Err(e) => {
            eprintln!("compiled://{source_name}: {e}");
            return 2;
        }
    };
    let run = JobRun {
        label: format!("compiled://{source_name}"),
        started,
        route_load,
        project_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        report_path: report,
        cli_args: Vec::new(),
    };
    execute_job(doc, run, camel_config, None).await
}

/// Validate and reduce the consumed store's source plan to the
/// route-kind references the discovery pass accepts
/// (`discover_virtual_store` parses route-kind plan references only;
/// the entry point job document is parsed separately from its store
/// entry, so it is dropped from the plan).
///
/// The ONLY job-kind reference a plan may legitimately drop is the
/// entry document itself. Any OTHER job-kind reference would be a
/// second entry point — not representable in the single-entry runtime —
/// and is returned here as a named rejection instead of being silently
/// dropped. (`validate_typed_references` bounds plan kinds to
/// route|job, so job is the only reachable non-route kind.)
fn filter_store_source_plan(
    store: &mut crate::compile::store::VirtualDocumentStore,
) -> Option<String> {
    use crate::compile::store::StoreEntryKind;

    let entry_point = store.index.entry_point.clone();
    let unexpected = store
        .index
        .source_plan
        .references
        .iter()
        .find(|path| {
            *path != &entry_point
                && store
                    .index
                    .entries
                    .iter()
                    .any(|entry| entry.path == **path && entry.kind == StoreEntryKind::Job)
        })
        .cloned();
    if unexpected.is_some() {
        return unexpected;
    }
    store.index.source_plan.references.retain(|path| {
        *path != entry_point
            && store
                .index
                .entries
                .iter()
                .any(|entry| entry.path == *path && entry.kind == StoreEntryKind::Route)
    });
    None
}

/// Run one embedded virtual-store job artifact (v2 multi-document,
/// openspec change `multidoc`, Task 2.2). The store is the document's
/// sole source universe: the job document is the indexed entry point,
/// configuration comes from the merged embedded config/include/profile
/// entries (deployment-time `${env:}` resolution only — no ambient
/// `Camel.toml`, no `CAMEL_*` overrides), and routes come from the
/// indexed route files of the source plan (or the job document's inline
/// `routes:` block, the exactly-one-source rule). No filesystem
/// route discovery ever runs; the report/outcome lifecycle and exit codes are
/// exactly the existing `camel job` taxonomy. The store is consumed:
/// its source plan is filtered in place (see [`filter_store_source_plan`])
/// instead of cloning the content blob.
pub(crate) async fn run_embedded_job_store(
    mut store: crate::compile::store::VirtualDocumentStore,
    report: Option<PathBuf>,
) -> i32 {
    let started = Instant::now();
    let entry_point = store.index.entry_point.clone();
    let identity = format!("compiled://{entry_point}");
    // Route files: the source plan minus the job entry document. A plan
    // carrying any OTHER job-kind reference is a named rejection.
    if let Some(path) = filter_store_source_plan(&mut store) {
        eprintln!("{identity}: store plan carries an unexpected job document reference: {path}");
        return 2;
    }
    let ambient = |name: &str| std::env::var(name).ok();
    let (camel_config, routes) =
        match crate::compile::runtime::resolve_virtual_store(&store, &ambient) {
            Ok(resolved) => resolved,
            Err(crate::compile::runtime::VirtualStoreResolveError::Discovery(e)) => {
                eprintln!("{identity}: {e}");
                return 2;
            }
            Err(crate::compile::runtime::VirtualStoreResolveError::Config(e)) => {
                eprintln!("camel-cli job failed: {e}");
                return 2;
            }
        };
    let text = match store.read_text(&entry_point) {
        Some(text) => text.to_string(),
        None => {
            eprintln!("{identity}: entry point names no store entry");
            return 2;
        }
    };
    let doc = match document::parse_job_document_with_args(Path::new(&entry_point), &text, &[]) {
        Ok(doc) => doc,
        Err(document::JobDocError::MissingRequiredArgument { name }) => {
            eprintln!(
                "{identity}: missing required argument `{name}`: compiled artifacts cannot accept \
                 --arg; declare a `default` for the argument in the document instead"
            );
            return 2;
        }
        Err(e) => {
            eprintln!("{identity}: {e}");
            return 2;
        }
    };
    // Route source: the exactly-one-source rule makes the inline
    // `routes:` block and the indexed route files mutually exclusive.
    // The inline branch never consults the filesystem (the file-form
    // fields are absent); the file forms deliberately bypass
    // `resolve_route_source` — its `routeFilesFromRoot` arm would walk
    // ancestor directories for a `Camel.toml`, a forbidden runtime
    // read, and the compile already resolved those sources into the
    // store.
    let route_load = if doc.routes.is_some() {
        match document::resolve_route_source(&doc, Path::new(".")) {
            Ok(JobRouteSource::Inline(text)) => RouteLoad::Embedded {
                text,
                source_name: entry_point,
            },
            Ok(JobRouteSource::Patterns(_)) | Err(_) => {
                eprintln!("{identity}: job document route source did not resolve inline");
                return 2;
            }
        }
    } else {
        RouteLoad::Discovered(routes)
    };
    let run = JobRun {
        label: identity,
        started,
        route_load,
        project_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        report_path: report,
        cli_args: Vec::new(),
    };
    execute_job(doc, run, camel_config, None).await
}

/// Await one job wait operation (the send under its overall deadline, or
/// the batch drain) through the registered signal streams when they are
/// armed — [`await_job_operation_or_signal`] semantics, signal-first —
/// or directly when they are not: embedded runs carry no signal streams,
/// so the plain await preserves the timeout-only semantics verbatim.
async fn await_job_operation<T>(
    signals: Option<&mut JobSignals>,
    operation: impl Future<Output = T>,
) -> JobWaitOutcome<T> {
    match signals {
        Some(signals) => await_job_operation_or_signal(signals.next(), operation).await,
        None => JobWaitOutcome::Completed(operation.await),
    }
}

/// Project the boot config for a one-shot `camel job` run: the runtime
/// journal and the whole observability stack are neutralized so a job
/// coexisting with a long-running server never opens the server's
/// journal file nor re-binds its metrics/health/OTel listeners. The
/// allowlist is exact — only those two fields are assigned; everything
/// else is carried through the clone untouched.
/// Spec: openspec/changes/jobcoexist (projection allowlist is exact).
fn job_effective_config(
    config: &camel_config::config::CamelConfig,
) -> camel_config::config::CamelConfig {
    let mut projected = config.clone();
    projected.runtime_journal = None;
    projected.observability = camel_config::config::ObservabilityConfig::default();
    projected
}

/// The single-document job execution/report lifecycle, shared by the
/// CLI argv path and embedded artifacts: boot composition (mirrors
/// `camel run` steps 1-5), the post-boot setup behind the teardown
/// border, the one-shot/batch send, the outcome report, and teardown.
/// Returns the process exit code. `signals` carries the argv path's
/// registered SIGINT/SIGTERM streams (first-signal interruption);
/// embedded runs pass `None`.
async fn execute_job(
    doc: JobDocument,
    run: JobRun,
    camel_config: camel_config::config::CamelConfig,
    mut signals: Option<JobSignals>,
) -> i32 {
    let JobRun {
        label: document_label,
        started,
        route_load,
        project_root,
        report_path,
        cli_args,
    } = run;

    // Job boot projection (jobcoexist): every downstream consumer — the
    // beans registry emptiness check, `configure_context_with_beans`, the
    // security compile context, bind acks, and the component cascade —
    // sees the projected config, never the ambient caller's.
    let camel_config = job_effective_config(&camel_config);

    // ---- Boot composition: mirrors `camel run` steps 1-5 ---------------
    let beans_registry = {
        let bean_reg = std::sync::Arc::new(std::sync::Mutex::new(camel_bean::BeanRegistry::new()));
        if camel_config.beans.is_empty() {
            None
        } else {
            Some(bean_reg)
        }
    };

    let mut ctx = match camel_config::config::CamelConfig::configure_context_with_beans(
        &camel_config,
        beans_registry.clone(),
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("camel-cli job failed: {e}");
            return 2;
        }
    };

    // R4-L4 trust-model parity: a job executes route scripts/WASM/beans
    // from the current working directory, like `camel run`.
    tracing::warn!(
        "camel job trusts the current working directory and will execute route \
         scripts and WASM route components resolved from it; only run from a \
         trusted directory"
    );

    match camel_function::FunctionRuntimeService::with_default_container_provider(
        camel_function::FunctionConfig::default(),
    ) {
        Ok(svc) => ctx = ctx.with_lifecycle(svc),
        Err(e) => tracing::warn!("Function runtime disabled: {e}"),
    }

    // Security compile context: the same fail-closed seams as `camel run`
    // (scenario-shared-boot task 2.3).
    #[cfg(feature = "security")]
    let security_compile_context =
        match camel_bundles::security_boot::build_security_compile_context_from_config(
            &camel_config,
            ctx.registry_arc(),
        )
        .await
        {
            Ok(context) => context,
            Err(e) => {
                eprintln!("camel-cli job failed: {e}");
                return 2;
            }
        };
    #[cfg(not(feature = "security"))]
    let security_compile_context =
        match camel_bundles::security_boot::ensure_security_supported(&camel_config) {
            Ok(()) => camel_dsl::SecurityCompileContext::default(),
            Err(e) => {
                eprintln!("camel-cli job failed: {e}");
                return 2;
            }
        };

    camel_bundles::security_boot::install_bind_exposure_acks(&mut ctx, &camel_config).await;

    let boot_handle = match camel_bundles::boot(&mut ctx, &camel_config, &project_root).await {
        Ok(handle) => handle,
        Err(e) => {
            // log-policy: system-broken
            tracing::error!("Failed to boot component cascade: {e}");
            eprintln!("camel-cli job failed: {e}");
            return 2;
        }
    };

    // ---- Post-boot teardown border (bd rc-7wl19) -----------------------
    // The deadline was anchored at process start: it covers the WHOLE
    // run — boot, setup, send, drain, and teardown — and feeds the
    // border's early-failure budget below. Every setup failure returns
    // through this border, which owns the one bounded shutdown; the
    // send-phase paths that already shut down stay inside
    // `execute_job` unchanged.
    let deadline = started + doc.execute.timeout;
    let batch_probe = match setup_booted_job(
        &mut ctx,
        &doc,
        &document_label,
        route_load,
        &security_compile_context,
        &camel_config,
    )
    .await
    {
        Ok(batch_probe) => batch_probe,
        Err(EarlyJobFailure) => {
            let budget = shutdown_budget(doc.execute.mode, deadline);
            if let Err(detail) = shutdown(&mut ctx, &boot_handle, budget).await {
                eprintln!("{detail}");
            }
            return 2;
        }
    };

    // ---- Send under the mandatory overall timeout ----------------------
    let tokio_deadline = tokio::time::Instant::from_std(deadline);
    // Verdict fidelity for seda: the producer defaults to fire-and-forget
    // on InOnly sends, so force `waitForTaskToComplete=Always` — a
    // failing route must surface as `Failed`, and `capture-reply` must
    // report the route's outcome, not the echoed input.
    let send_to = if document::scheme_of_uri(&doc.execute.send.to) == Some("seda") {
        document::seda_send_uri(&doc.execute.send.to)
    } else {
        doc.execute.send.to.clone()
    };
    let send = send_with_startup_retry(&ctx, &doc.execute.send, &send_to, &cli_args);
    // Shared shape for every overall-deadline expiry: the send-timeout
    // arm and the batch drain-timeout path report identically.
    let timeout_report = || JobReport {
        document: document_label.clone(),
        mode: doc.execute.mode.as_str().to_string(),
        outcome: "Timeout",
        terminated_early: false,
        duration_ms: started.elapsed().as_millis(),
        reply: None,
        error: Some(format!(
            "job timed out after {}",
            humantime::format_duration(doc.execute.timeout)
        )),
        shutdown_error: None,
    };
    // Shared shape for the first-signal interruption: the in-flight
    // send or batch drain was cancelled; the verdict is the signal.
    let interrupted_report = || JobReport {
        document: document_label.clone(),
        mode: doc.execute.mode.as_str().to_string(),
        outcome: "Interrupted",
        terminated_early: false,
        duration_ms: started.elapsed().as_millis(),
        reply: None,
        error: Some("interrupted by signal (SIGINT/SIGTERM)".to_string()),
        shutdown_error: None,
    };
    let mut interrupted = false;
    let mut report = {
        // Signal-first race: a signal ready at the same poll point as
        // send completion or deadline expiry wins; dropping the
        // operation future on a signal win cancels the in-flight send.
        // Without streams (embedded runs) the race degrades to the
        // plain timeout await.
        let operation = tokio::time::timeout_at(tokio_deadline, send);
        match await_job_operation(signals.as_mut(), operation).await {
            JobWaitOutcome::Signaled => {
                interrupted = true;
                interrupted_report()
            }
            JobWaitOutcome::Completed(Err(_)) => timeout_report(),
            JobWaitOutcome::Completed(Ok(Err(SendError::Transport(detail)))) => {
                // log-policy: system-broken
                tracing::error!("Job send apparatus failure: {detail}");
                eprintln!("{detail}");
                // The context is booted; run the shutdown path before exiting.
                // Batch keeps the no-floor rule: teardown cannot run past the
                // overall deadline (same branch as the post-verdict budget).
                let transport_budget = shutdown_budget(doc.execute.mode, deadline);
                if let Err(shutdown_detail) =
                    shutdown(&mut ctx, &boot_handle, transport_budget).await
                {
                    eprintln!("{shutdown_detail}");
                }
                return 2;
            }
            JobWaitOutcome::Completed(Ok(Err(SendError::Pipeline(e)))) => JobReport {
                document: document_label.clone(),
                mode: doc.execute.mode.as_str().to_string(),
                outcome: "Failed",
                terminated_early: false,
                duration_ms: started.elapsed().as_millis(),
                reply: None,
                error: Some(e.to_string()),
                shutdown_error: None,
            },
            JobWaitOutcome::Completed(Ok(Ok(reply))) => {
                // Batch drain: the trigger send's seda hops are
                // fire-and-forget, so wait until every expected queue has
                // ten consecutive zero-depth samples (a 2.5 s quiescence
                // window) — see the `batch` module docs — before the
                // verdict; pre-send zero samples were reset away. The
                // drain waits on the same registered signal streams, so
                // an interruption during drain follows the same path as
                // one during send. One-shot skips the drain.
                let drained = match &batch_probe {
                    Some(probe) => {
                        probe.reset();
                        match await_job_operation(
                            signals.as_mut(),
                            batch::drain_until_empty(probe, tokio_deadline),
                        )
                        .await
                        {
                            JobWaitOutcome::Completed(drained) => drained,
                            JobWaitOutcome::Signaled => {
                                interrupted = true;
                                false
                            }
                        }
                    }
                    None => true,
                };
                if interrupted {
                    interrupted_report()
                } else if !drained {
                    timeout_report()
                } else {
                    JobReport {
                        document: document_label.clone(),
                        mode: doc.execute.mode.as_str().to_string(),
                        outcome: "Completed",
                        terminated_early: false,
                        duration_ms: started.elapsed().as_millis(),
                        reply: doc
                            .execute
                            .capture_reply
                            .then(|| reply_report(&reply))
                            .map(|(body, headers)| ReplyReport { body, headers }),
                        error: None,
                        shutdown_error: None,
                    }
                }
            }
        }
    };

    // ---- First-signal interruption: bounded teardown + report --------
    if interrupted {
        // The force-exit guard owns the streams from now: a second
        // SIGINT/SIGTERM during teardown exits 1 immediately without
        // waiting for the bounded shutdown. The first signal was
        // consumed by the race above, so the guard only sees later
        // signals; it is aborted once teardown completes so the normal
        // exit path stays untouched. Embedded runs never reach this
        // path (no streams → no interruption), so the guard is spawned
        // exactly when streams exist.
        let budget = shutdown_budget(doc.execute.mode, deadline);
        let guard = signals
            .take()
            .map(|signals| tokio::spawn(signals.force_exit()));
        let shutdown_result = shutdown(&mut ctx, &boot_handle, budget).await;
        if let Some(guard) = guard {
            guard.abort();
        }
        if let Err(detail) = shutdown_result {
            eprintln!("{detail}");
            record_shutdown_failure(&mut report, detail, budget);
        }
        if !write_report(report_path.as_deref(), &report) {
            return 2;
        }
        return exit_code_for(report.outcome);
    }

    // ---- Drain + teardown under the remaining budget --------------------
    // Batch: teardown cannot run past the overall deadline (no floor).
    let budget = shutdown_budget(doc.execute.mode, deadline);
    if let Err(detail) = shutdown(&mut ctx, &boot_handle, budget).await {
        eprintln!("{detail}");
        // A zero-budget teardown failure is the timeout's tail, not an
        // independent shutdown failure: on the batch Timeout path the
        // deadline has already fired, so a 0-budget shutdown call is a
        // foregone timeout artifact. Keep the stderr line, but the
        // report carries only the Timeout verdict.
        record_shutdown_failure(&mut report, detail, budget);
        // Apparatus class outranks the verdict (2 > 1 > 0).
        write_report(report_path.as_deref(), &report);
        return 2;
    }

    let code = exit_code_for(report.outcome);
    if !write_report(report_path.as_deref(), &report) {
        return 2;
    }
    code
}

/// Marker for a post-boot setup failure whose diagnostic was already
/// printed at the failure site. The caller — the post-boot teardown
/// border in [`execute_job`] — owns the single bounded context shutdown
/// before the exit-2 return (bd rc-7wl19), so no setup branch calls
/// `shutdown` itself and a future early exit cannot leak the booted
/// context.
struct EarlyJobFailure;

/// The post-boot setup stretch between the boot handle and the send:
/// route loading (real-boot seam), the fail-closed consumer gate, the
/// send-target selection, conditional bundle registration, route
/// registration, the batch probe, and `ctx.start()`. Every failure
/// prints its existing diagnostic here and returns
/// [`EarlyJobFailure`] — the diagnostic list and exit outcomes are
/// carried over verbatim from the pre-border control flow — so the
/// teardown border can run the one bounded shutdown for all of them.
/// On success it returns the batch drain probe (`None` for one-shot);
/// the probe is registered before `ctx.start()` so the seda samplers'
/// emissions fan out to it from their first tick.
async fn setup_booted_job(
    ctx: &mut camel_core::CamelContext,
    doc: &JobDocument,
    document_label: &str,
    route_load: RouteLoad,
    security_compile_context: &camel_dsl::SecurityCompileContext,
    camel_config: &camel_config::config::CamelConfig,
) -> Result<Option<std::sync::Arc<batch::BatchDepthProbe>>, EarlyJobFailure> {
    // ---- Route loading (real-boot seam: ambient ${env:}) ---------------
    let defs = match load_route_definitions(route_load, camel_config, security_compile_context) {
        Ok(defs) => defs,
        Err(e) => {
            // log-policy: system-broken
            tracing::error!("Failed to load job routes: {e}");
            eprintln!("{e}");
            return Err(EarlyJobFailure);
        }
    };
    if defs.is_empty() {
        eprintln!("{document_label}: job route source resolved zero route definitions");
        return Err(EarlyJobFailure);
    }

    // Fail-closed consumer gate (load time): only job-safe schemes may
    // consume; producers/sinks as to: URIs are unrestricted.
    for def in &defs {
        if let Err(e) = document::validate_consumer_uri(def.from_uri()) {
            eprintln!("{document_label}: route `{}` rejected: {e}", def.route_id());
            return Err(EarlyJobFailure);
        }
    }

    // Batch drain expectation set: every seda consumer route's URI
    // base. The queue-depth gauge label is exactly the seda URI base
    // (`seda:<name>`), so the drain waits on precisely these labels.
    let mut expected_queues = HashSet::new();
    for def in &defs {
        if document::scheme_of_uri(def.from_uri()) == Some("seda") {
            expected_queues.insert(document::uri_base(def.from_uri()).to_string());
        }
    }

    // Route-target safety: ALL document routes start. Side-effect safety
    // comes from the fail-closed consumer allowlist at load, and the
    // send target stays the sole entry point. The missing- and
    // ambiguous-target checks below stay: with all routes started, two
    // consumer routes sharing one base would round-robin both the
    // target send and any `to:` hops.
    let target_base = document::uri_base(&doc.execute.send.to);
    let target_ids = document::target_route_ids(&defs, target_base);
    match target_ids.len() {
        0 => {
            eprintln!(
                "{document_label}: send target `{}` has no matching consumer route",
                doc.execute.send.to
            );
            return Err(EarlyJobFailure);
        }
        1 => {}
        count => {
            eprintln!(
                "{document_label}: send target `{}` is ambiguous: {} consumer routes share its base: {}",
                doc.execute.send.to,
                count,
                target_ids.join(", ")
            );
            return Err(EarlyJobFailure);
        }
    }
    let defs: Vec<_> = defs
        .into_iter()
        .map(|def| def.with_auto_startup(true))
        .collect();

    // Conditionally register ExecBundle (route-content-conditional, the
    // single-bundle seam — mirrors `camel run`).
    #[cfg(feature = "exec")]
    {
        let exec_used =
            camel_core::startup_validation::route_definitions_reference_scheme(&defs, "exec");
        let exec_configured = camel_config.components.raw.contains_key("exec");
        if (exec_used || exec_configured)
            && let Err(e) = camel_bundles::register_bundle::<camel_component_exec::ExecBundle>(
                ctx,
                camel_config,
            )
        {
            eprintln!("camel-cli job failed: {e}");
            return Err(EarlyJobFailure);
        }
    }

    // ADR-0033: fail-closed ConfigChecks derived from the routes (e.g.
    // SqlDynamicQueryCheck for every `sql:` endpoint).
    camel_bundles::security_boot::install_sql_startup_checks(ctx, &defs);

    for def in defs {
        let id = def.route_id().to_string();
        if let Err(e) = ctx.add_route_definition(def).await {
            // log-policy: system-broken
            tracing::error!("Failed to add route '{id}': {e}");
            eprintln!("camel-cli job failed: {e}");
            return Err(EarlyJobFailure);
        }
    }

    // Batch mode: register the drain probe BEFORE `ctx.start()` so the
    // seda samplers' emissions fan out to it from their first tick (the
    // metrics handle composes; every emission reaches the composite).
    let batch_probe = match doc.execute.mode {
        document::JobMode::Batch => {
            let probe = std::sync::Arc::new(batch::BatchDepthProbe::new(expected_queues));
            ctx.add_lifecycle(batch::BatchProbeLifecycle(std::sync::Arc::clone(&probe)));
            Some(probe)
        }
        document::JobMode::OneShot => None,
    };

    if let Err(e) = ctx.start().await {
        // log-policy: system-broken
        tracing::error!("Failed to start CamelContext: {e}");
        eprintln!("camel-cli job failed: {e}");
        return Err(EarlyJobFailure);
    }

    Ok(batch_probe)
}

/// Load the run's route definitions through the real-boot seams:
/// the filesystem and embedded forms feed their inputs to the shared
/// discovery pipeline (ambient `${env:}`, stream-caching threshold,
/// security compile context — the same loader `camel run` uses); the
/// CLI inline form goes through `parse_routes_with_env` with the
/// AMBIENT environment as lookup (the hermetic document-env closure is
/// test-family machinery and is deliberately not used here).
fn load_route_definitions(
    load: RouteLoad,
    camel_config: &camel_config::config::CamelConfig,
    security_compile_context: &camel_dsl::SecurityCompileContext,
) -> Result<Vec<camel_core::RouteDefinition>, String> {
    let ambient = &|name: &str| std::env::var(name).ok();
    match load {
        RouteLoad::Discovery(patterns) => camel_dsl::discover_routes_with_threshold_and_security(
            &patterns,
            camel_config.stream_caching.threshold,
            security_compile_context.clone(),
        )
        .map_err(|e| e.to_string()),
        RouteLoad::Inline(text) => match camel_dsl::parse_routes_with_env(&text, ambient) {
            Ok(defs) => Ok(defs),
            Err(camel_dsl::RoutesEnvError::Unresolved(var)) => Err(format!(
                "Environment variable '{var}' not set (required by inline routes)"
            )),
            Err(camel_dsl::RoutesEnvError::Parse(e)) => Err(format!("inline routes: {e}")),
        },
        RouteLoad::Embedded { text, source_name } => camel_dsl::discover_embedded_text(
            &text,
            &source_name,
            camel_dsl::EmbeddedDocumentKind::Job,
            ambient,
        )
        .map_err(|e| e.to_string()),
        // Already discovered from the store's indexed route files with
        // the deployment environment — nothing to load.
        RouteLoad::Discovered(defs) => Ok(defs),
    }
}

/// Send the job's single exchange, retrying the consumer-startup race
/// (the `deliver_input` discipline: `EndpointCreationFailed` /
/// not-registered races, plus the SEDA no-active-consumers gate — safe
/// to retry here because a gate rejection is pre-enqueue and the job
/// send is the first and only send, so no side effects can have run).
/// A persistent failure maps to [`SendError::Pipeline`] when the
/// pipeline itself failed, [`SendError::Transport`] otherwise.
async fn send_with_startup_retry(
    ctx: &camel_core::CamelContext,
    send: &document::JobSendAction,
    send_to: &str,
    cli_args: &[(String, String)],
) -> Result<Exchange, SendError> {
    let body = match &send.body {
        Some(JobBody::Text(s)) => Body::Text(s.clone()),
        Some(JobBody::Json(v)) => Body::Json(v.clone()),
        None => Body::Empty,
    };
    let mut message = Message::new(body);
    if let Some(headers) = &send.headers {
        for (k, v) in headers {
            message.set_header(k.clone(), v.clone());
        }
    }
    // CLI values are applied LAST: they override colliding document
    // headers, and a repeated name resolves to the last occurrence.
    // Legacy path only — declared documents pass an empty pair list
    // (`JobRun::cli_args`), their values having already been resolved
    // through `${arg:}` interpolation.
    for (k, v) in cli_args {
        message.set_header(k.clone(), serde_json::Value::String(v.clone()));
    }
    let exchange = Exchange::new(message);
    let scheme = document::scheme_of_uri(send_to)
        .unwrap_or_default()
        .to_string();
    let retry_until = Instant::now() + SEND_RETRY_WINDOW;

    loop {
        match attempt_send(ctx, &scheme, send_to, exchange.clone()).await {
            Ok(Ok(reply)) => return Ok(reply),
            Ok(Err(e)) => {
                let retryable = camel_component_seda::is_no_active_consumers_gate(&e)
                    || matches!(e, CamelError::EndpointCreationFailed(_))
                    || e.to_string().contains("not registered");
                if retryable && Instant::now() < retry_until {
                    tokio::time::sleep(SEND_RETRY_SLEEP).await;
                    continue;
                }
                return Err(SendError::Pipeline(e));
            }
            Err(detail) => {
                if Instant::now() < retry_until {
                    tokio::time::sleep(SEND_RETRY_SLEEP).await;
                    continue;
                }
                return Err(SendError::Transport(detail));
            }
        }
    }
}

/// One producer-creation + send attempt. The outer `Err` carries a
/// transport detail (producer/endpoint apparatus); the inner `Result`
/// is the producer's reply — `Err` there is the route pipeline failing
/// (`PipelineOutcome::Failed` per ADR-0024).
async fn attempt_send(
    ctx: &camel_core::CamelContext,
    scheme: &str,
    uri: &str,
    exchange: Exchange,
) -> Result<Result<Exchange, CamelError>, String> {
    let producer = {
        // Producer creation under the registry lock, mirroring the
        // `deliver_input` / `DirectStimulus` discipline; the guard drops
        // before the awaited send.
        let registry = ctx.registry();
        let component = registry.get(scheme).ok_or_else(|| {
            format!("failed to send to {uri}: `{scheme}:` component not registered")
        })?;
        let endpoint = component
            .create_endpoint(uri, ctx)
            .map_err(|e| format!("failed to create endpoint {uri}: {e}"))?;
        let producer_ctx = ctx.producer_context();
        endpoint
            .create_producer(std::sync::Arc::new(NoOpComponentContext), &producer_ctx)
            .map_err(|e| format!("failed to create producer for {uri}: {e}"))?
    };
    Ok(producer.oneshot(exchange).await)
}

/// Extract the reply report parts from the reply exchange.
fn reply_report(
    reply: &Exchange,
) -> (
    serde_json::Value,
    serde_json::Map<String, serde_json::Value>,
) {
    let message = reply.output.as_ref().unwrap_or(&reply.input);
    let body = match &message.body {
        Body::Json(value) => value.clone(),
        body => body
            .as_text()
            .map(|s| serde_json::Value::String(s.to_string()))
            .unwrap_or(serde_json::Value::Null),
    };
    let headers = message
        .headers
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    (body, headers)
}

/// Compute the teardown budget for a shutdown call: the wall clock
/// remaining to the overall deadline, floored to [`MIN_SHUTDOWN_BUDGET`]
/// for one-shot only. Batch keeps the no-floor rule — teardown cannot
/// run past the overall deadline — so a spent deadline computes a zero
/// budget there (the Timeout path's teardown artifact).
fn shutdown_budget(mode: document::JobMode, deadline: Instant) -> Duration {
    let remaining = deadline.saturating_duration_since(Instant::now());
    match mode {
        document::JobMode::Batch => remaining,
        document::JobMode::OneShot => remaining.max(MIN_SHUTDOWN_BUDGET),
    }
}

/// Map a report outcome to the process exit code (`2 > 1 > 0`):
/// `Completed` 0, `Failed` 1, and the apparatus class — `Timeout`
/// (mandatory overall budget expired) and `Interrupted` (first
/// SIGINT/SIGTERM) — 2.
fn exit_code_for(outcome: &str) -> i32 {
    match outcome {
        "Completed" => 0,
        "Failed" => 1,
        _ => 2,
    }
}

/// Record a teardown failure on a verdict-carrying report:
/// `shutdown_error` carries the detail ONLY when teardown had a
/// non-zero budget. A zero-budget failure is the timeout path's
/// foregone artifact (stderr-only; the verdict keeps the report).
fn record_shutdown_failure(report: &mut JobReport, detail: String, budget: Duration) {
    if budget > Duration::ZERO {
        report.shutdown_error = Some(detail);
    }
}

/// Tear the context down through the BootHandle with a bounded budget;
/// the deadline-wrapped pool teardown mirrors `camel run`. Returns the
/// first failure as a display string (apparatus class, exit 2).
///
/// Every teardown goes through this helper — the send-failure,
/// interruption, final, and post-boot-border paths alike — so it is the
/// single observation point for exactly-one-shutdown assertions: when
/// `CAMEL_JOB_SHUTDOWN_MARKER` is set, exactly one
/// `camel job: shutdown complete` line is emitted to stderr after the
/// bounded shutdown attempt (test seam, mirroring
/// `CAMEL_JOB_SIGNAL_MARKER`; production output is unchanged by
/// default).
async fn shutdown(
    ctx: &mut camel_core::CamelContext,
    boot_handle: &camel_bundles::BootHandle,
    budget: Duration,
) -> Result<(), String> {
    let result =
        tokio::time::timeout(budget, boot_handle.shutdown_with_deadline(ctx, budget)).await;
    if std::env::var_os("CAMEL_JOB_SHUTDOWN_MARKER").is_some() {
        eprintln!("camel job: shutdown complete");
    }
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(format!("shutdown failure: {e}")),
        Err(_) => Err(format!(
            "drain timeout: job teardown exceeded {}",
            humantime::format_duration(budget)
        )),
    }
}

/// Write the JSON report to the report path or stdout. Returns success.
fn write_report(report_path: Option<&Path>, report: &JobReport) -> bool {
    let rendered = match serde_json::to_string_pretty(report) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("failed to render job report: {e}");
            return false;
        }
    };
    match report_path {
        Some(path) => match std::fs::write(path, format!("{rendered}\n")) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("failed to write {}: {e}", path.display());
                false
            }
        },
        None => {
            println!("{rendered}");
            true
        }
    }
}
