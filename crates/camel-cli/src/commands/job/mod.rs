//! `camel job <FILE>` — one-shot route execution from a `*.job.yaml`
//! document declaring a top-level `execute:` section. A bare name
//! resolves `{jobs.dir}/<name>.job.yaml`; no argument lists the jobs
//! directory.
//!
//! The job boots the REAL composition root (the same seams `camel run`
//! uses: config load, security compile context, bind-exposure acks, the
//! `camel_bundles` cascade, real `${env:}` resolution through
//! discovery), reuses the document-family route-source keys, then sends
//! exactly one exchange to the document's `direct:`/`seda:` target and
//! shuts down. Route side-effect safety: every discovered route is
//! forced to `auto_startup = false` except the send target, which is
//! forced on — a job's own trigger must start, other consumer routes
//! must not.
//!
//! Exit codes mirror the `camel test` taxonomy (`2 > 1 > 0`): 0 the
//! pipeline completed; 1 the pipeline failed; 2 any load, validation,
//! boot, drain-timeout, shutdown, or report-write error. The process
//! exit is applied only at the `main.rs` boundary; this module returns
//! the code.
//!
//! Spec: openspec/changes/cli-jobs.

mod document;

#[cfg(test)]
mod document_tests;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use camel_api::{Body, CamelError, Exchange, Message};
use camel_component_api::NoOpComponentContext;
use clap::Args;
use noyalib::compat::serde_yaml;
use serde::{Deserialize, Serialize};
use tower::ServiceExt;

use document::{JobBody, JobDocument, JobRouteSource};

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
pub struct JobArgs {
    /// Path to the job document (`*.job.yaml` with an `execute:`
    /// section). A bare name (no separator, no suffix) resolves
    /// `{jobs.dir}/<name>.job.yaml`; omitted lists the jobs directory.
    #[arg(value_name = "FILE")]
    pub document: Option<PathBuf>,
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
}

/// The JSON report of one job run.
#[derive(Serialize)]
struct JobReport {
    /// Displayed path of the job document.
    document: String,
    /// Execution mode (`one-shot`).
    mode: String,
    /// Outcome: `Completed` (or `Stopped`, see `terminated_early`),
    /// `Failed`, or `Timeout`.
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
    /// Error detail for `Failed`/`Timeout` outcomes and shutdown
    /// failures after a recorded verdict.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
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

/// The jobs directory, anchored at the Camel.toml root (never the
/// process CWD): `canonical_project_root(--config)` joined with
/// `[jobs].dir`. Shared by bare-name resolution and no-argument
/// listing so the two surfaces cannot drift.
fn jobs_root(args: &JobArgs, camel_config: &camel_config::config::CamelConfig) -> PathBuf {
    crate::commands::run::canonical_project_root(Path::new(&args.config))
        .join(&camel_config.jobs.dir)
}

/// Resolve a document argument. An explicit path (any path separator,
/// or a `.yaml`/`.yml`/`.json` suffix) is used as-is — including an
/// explicit `.job.yml`. A bare name probes exactly
/// `{jobs_root}/<name>.job.yaml` (one deterministic spelling, no
/// alternate-suffix probing) and a miss fails with one error naming
/// the probed file.
fn resolve_job_path(raw: &Path, jobs_root: &Path) -> Result<PathBuf, String> {
    let name = raw.to_string_lossy();
    let lower = name.to_lowercase();
    let explicit = raw.components().count() > 1
        || lower.ends_with(".yaml")
        || lower.ends_with(".yml")
        || lower.ends_with(".json");
    if explicit {
        return Ok(raw.to_path_buf());
    }
    let probe = jobs_root.join(format!("{name}.job.yaml"));
    if probe.exists() {
        Ok(probe)
    } else {
        Err(format!(
            "no job `{name}` in `{}` (looked for {name}.job.yaml)",
            jobs_root.display()
        ))
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

/// Probe one job document for its description. Outer `None` = unreadable
/// or unparseable (row renders `(unparseable)`); `Some(None)` = parseable
/// without `description:`; `Some(Some(d))` = the description string.
fn probe_description(path: &Path) -> Option<Option<String>> {
    let text = std::fs::read_to_string(path).ok()?;
    let probe: JobListProbe = serde_yaml::from_str(&text).ok()?;
    Some(probe.description)
}

/// List the jobs directory (`camel job` with no document argument).
/// Exit 0 for found, empty, and absent directories alike — listing is a
/// query, not a usage error (ls semantics). Listing output and the JSON
/// run report never co-occur: the report path requires a document.
fn list_jobs(args: &JobArgs, camel_config: &camel_config::config::CamelConfig) -> i32 {
    let root = jobs_root(args, camel_config);
    let dir_label = camel_config.jobs.dir.as_str();
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(_) => {
            println!(
                "No jobs found in {dir_label}/. Create a `<name>.job.yaml` there, or run `camel job <path>`."
            );
            return 0;
        }
    };

    let mut jobs: Vec<(String, Option<Option<String>>)> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && camel_dsl::discovery::is_job_document(path))
        .map(|path| {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let display = name
                .strip_suffix(".job.yaml")
                .or_else(|| name.strip_suffix(".job.yml"))
                .unwrap_or(&name)
                .to_string();
            (display, probe_description(&path))
        })
        .collect();
    jobs.sort_by(|a, b| a.0.cmp(&b.0));

    if jobs.is_empty() {
        println!(
            "No jobs found in {dir_label}/. Create a `<name>.job.yaml` there, or run `camel job <path>`."
        );
        return 0;
    }

    println!("Jobs in {dir_label}/:");
    for (name, description) in &jobs {
        let rendered = match description {
            None => "(unparseable)".to_string(),
            Some(None) => "(no description)".to_string(),
            Some(Some(d)) => d.replace(['\n', '\r'], " "),
        };
        println!("  {name}      {rendered}");
    }
    0
}

/// Run one job document; returns the process exit code (`main.rs`
/// applies it). Every failure path prints to stderr; the JSON report
/// goes to stdout (default) or `--report`.
pub async fn run_job(args: &JobArgs) -> i32 {
    let started = Instant::now();

    // Config first: bare-name resolution needs `[jobs].dir`.
    let camel_config = match crate::commands::run::load_config_or_default(&args.config) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("camel-cli job failed: {e}");
            return 2;
        }
    };

    let Some(raw_document) = &args.document else {
        if args.report.is_some() {
            eprintln!("--report requires a job document");
            return 2;
        }
        return list_jobs(args, &camel_config);
    };
    let resolved = match resolve_job_path(raw_document, &jobs_root(args, &camel_config)) {
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
    let doc = match document::parse_job_document(&document_path, &text) {
        Ok(doc) => doc,
        Err(e) => {
            eprintln!("{}: {e}", document_path.display());
            return 2;
        }
    };
    let doc_dir = document_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

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

    let project_root = crate::commands::run::canonical_project_root(Path::new(&args.config));
    let boot_handle = match camel_bundles::boot(&mut ctx, &camel_config, &project_root).await {
        Ok(handle) => handle,
        Err(e) => {
            // log-policy: system-broken
            tracing::error!("Failed to boot component cascade: {e}");
            eprintln!("camel-cli job failed: {e}");
            return 2;
        }
    };

    // ---- Route loading (real-boot seam: ambient ${env:}) ---------------
    let defs =
        match load_route_definitions(&doc, &doc_dir, &camel_config, &security_compile_context) {
            Ok(defs) => defs,
            Err(e) => {
                // log-policy: system-broken
                tracing::error!("Failed to load job routes: {e}");
                eprintln!("{e}");
                return 2;
            }
        };
    if defs.is_empty() {
        eprintln!(
            "{}: job route source resolved zero route definitions",
            document_path.display()
        );
        return 2;
    }

    // Fail-closed consumer gate (load time): only job-safe schemes may
    // consume; producers/sinks as to: URIs are unrestricted.
    for def in &defs {
        if let Err(e) = document::validate_consumer_uri(def.from_uri()) {
            eprintln!(
                "{}: route `{}` rejected: {e}",
                document_path.display(),
                def.route_id()
            );
            return 2;
        }
    }

    // Route-target safety: nothing auto-starts except the send target.
    // Exactly one route must match the target base: zero means the send
    // has nowhere to land; more than one is ambiguous — both would be
    // auto-started and either could consume the send.
    let target_base = document::uri_base(&doc.execute.send.to);
    let target_ids = document::target_route_ids(&defs, target_base);
    match target_ids.len() {
        0 => {
            eprintln!(
                "{}: send target `{}` has no matching consumer route",
                document_path.display(),
                doc.execute.send.to
            );
            return 2;
        }
        1 => {}
        count => {
            eprintln!(
                "{}: send target `{}` is ambiguous: {} consumer routes share its base: {}",
                document_path.display(),
                doc.execute.send.to,
                count,
                target_ids.join(", ")
            );
            return 2;
        }
    }
    let defs: Vec<_> = defs
        .into_iter()
        .map(|def| {
            let is_target = document::uri_base(def.from_uri()) == target_base;
            def.with_auto_startup(is_target)
        })
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
                &mut ctx,
                &camel_config,
            )
        {
            eprintln!("camel-cli job failed: {e}");
            return 2;
        }
    }

    // ADR-0033: fail-closed ConfigChecks derived from the routes (e.g.
    // SqlDynamicQueryCheck for every `sql:` endpoint).
    camel_bundles::security_boot::install_sql_startup_checks(&mut ctx, &defs);

    for def in defs {
        let id = def.route_id().to_string();
        if let Err(e) = ctx.add_route_definition(def).await {
            // log-policy: system-broken
            tracing::error!("Failed to add route '{id}': {e}");
            eprintln!("camel-cli job failed: {e}");
            return 2;
        }
    }

    if let Err(e) = ctx.start().await {
        // log-policy: system-broken
        tracing::error!("Failed to start CamelContext: {e}");
        eprintln!("camel-cli job failed: {e}");
        return 2;
    }

    // ---- Send under the mandatory overall timeout ----------------------
    // The deadline was anchored at process start: it covers the WHOLE
    // run — boot, send, drain, and teardown.
    let deadline = started + doc.execute.timeout;
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
    let send = send_with_startup_retry(&ctx, &doc.execute.send, &send_to);
    let mut report = match tokio::time::timeout_at(tokio_deadline, send).await {
        Err(_) => JobReport {
            document: document_path.display().to_string(),
            mode: doc.execute.mode.clone(),
            outcome: "Timeout",
            terminated_early: false,
            duration_ms: started.elapsed().as_millis(),
            reply: None,
            error: Some(format!(
                "job timed out after {}",
                humantime::format_duration(doc.execute.timeout)
            )),
        },
        Ok(Err(SendError::Transport(detail))) => {
            // log-policy: system-broken
            tracing::error!("Job send apparatus failure: {detail}");
            eprintln!("{detail}");
            // The context is booted; run the shutdown path before exiting.
            if let Err(shutdown_detail) =
                shutdown(&mut ctx, &boot_handle, MIN_SHUTDOWN_BUDGET).await
            {
                eprintln!("{shutdown_detail}");
            }
            return 2;
        }
        Ok(Err(SendError::Pipeline(e))) => JobReport {
            document: document_path.display().to_string(),
            mode: doc.execute.mode.clone(),
            outcome: "Failed",
            terminated_early: false,
            duration_ms: started.elapsed().as_millis(),
            reply: None,
            error: Some(e.to_string()),
        },
        Ok(Ok(reply)) => JobReport {
            document: document_path.display().to_string(),
            mode: doc.execute.mode.clone(),
            outcome: "Completed",
            terminated_early: false,
            duration_ms: started.elapsed().as_millis(),
            reply: doc
                .execute
                .capture_reply
                .then(|| reply_report(&reply))
                .map(|(body, headers)| ReplyReport { body, headers }),
            error: None,
        },
    };

    // ---- Drain + teardown under the remaining budget --------------------
    let remaining = deadline.saturating_duration_since(Instant::now());
    let budget = remaining.max(MIN_SHUTDOWN_BUDGET);
    if let Err(detail) = shutdown(&mut ctx, &boot_handle, budget).await {
        eprintln!("{detail}");
        if report.error.is_none() {
            report.error = Some(detail);
        }
        // Apparatus class outranks the verdict (2 > 1 > 0).
        write_report(args, &report);
        return 2;
    }

    let code = match report.outcome {
        "Completed" => 0,
        "Failed" => 1,
        // Timeout: the mandatory overall budget expired.
        _ => 2,
    };
    if !write_report(args, &report) {
        return 2;
    }
    code
}

/// Load the document's route definitions through the real-boot seams:
/// the file forms feed the resolved paths as discovery patterns (ambient
/// `${env:}`, stream-caching threshold, security compile context — the
/// same loader `camel run` uses); the inline form goes through
/// `parse_routes_with_env` with the AMBIENT environment as lookup (the
/// hermetic document-env closure is test-family machinery and is
/// deliberately not used here).
fn load_route_definitions(
    doc: &JobDocument,
    doc_dir: &Path,
    camel_config: &camel_config::config::CamelConfig,
    security_compile_context: &camel_dsl::SecurityCompileContext,
) -> Result<Vec<camel_core::RouteDefinition>, String> {
    match document::resolve_route_source(doc, doc_dir).map_err(|e| e.to_string())? {
        JobRouteSource::Patterns(patterns) => {
            camel_dsl::discover_routes_with_threshold_and_security(
                &patterns,
                camel_config.stream_caching.threshold,
                security_compile_context.clone(),
            )
            .map_err(|e| e.to_string())
        }
        JobRouteSource::Inline(text) => {
            let ambient = &|name: &str| std::env::var(name).ok();
            match camel_dsl::parse_routes_with_env(&text, ambient) {
                Ok(defs) => Ok(defs),
                Err(camel_dsl::RoutesEnvError::Unresolved(var)) => Err(format!(
                    "Environment variable '{var}' not set (required by inline routes)"
                )),
                Err(camel_dsl::RoutesEnvError::Parse(e)) => Err(format!("inline routes: {e}")),
            }
        }
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

/// Tear the context down through the BootHandle with a bounded budget;
/// the deadline-wrapped pool teardown mirrors `camel run`. Returns the
/// first failure as a display string (apparatus class, exit 2).
async fn shutdown(
    ctx: &mut camel_core::CamelContext,
    boot_handle: &camel_bundles::BootHandle,
    budget: Duration,
) -> Result<(), String> {
    match tokio::time::timeout(budget, boot_handle.shutdown_with_deadline(ctx, budget)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(format!("shutdown failure: {e}")),
        Err(_) => Err(format!(
            "drain timeout: job teardown exceeded {}",
            humantime::format_duration(budget)
        )),
    }
}

/// Write the JSON report to `--report` or stdout. Returns success.
fn write_report(args: &JobArgs, report: &JobReport) -> bool {
    let rendered = match serde_json::to_string_pretty(report) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("failed to render job report: {e}");
            return false;
        }
    };
    match &args.report {
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
