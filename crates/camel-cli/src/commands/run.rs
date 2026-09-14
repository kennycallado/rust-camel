//! `camel run` subcommand body.
//!
//! Owns ADR-0012 category (d) `error!` sites for the CLI-owned lifecycle:
//! config/context bootstrap, route discovery/loading, context start, the
//! file watcher, and shutdown. Each site carries a `// log-policy: …`
//! annotation classifying it per the ADR taxonomy.
//!
//! The component bundle cascade and the JMS/CXF pool teardown moved to
//! `camel_bundles` (ADR-0069 section 10); `camel run` prepares the context,
//! calls `camel_bundles::boot`, and drives the returned `BootHandle` at
//! shutdown. The handle logs teardown failures; the exit code never
//! changes.
//!
//! The lifecycle core (`drive_lifecycle`) is shared with compiled
//! artifacts (openspec change `cli-compile`): `run()` contributes config
//! loading, CLI overrides, and filesystem discovery; an embedded artifact
//! contributes the default in-memory config, the embedded discovery seam,
//! and `watch = false`. Boot, route registration, context start, signals,
//! and shutdown are shared verbatim.

#[cfg(feature = "wasm")]
use camel_bean::BeanProcessor;
#[cfg(feature = "wasm")]
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Build the default in-memory `CamelConfig` (serde defaults only): no
/// file read, no `CAMEL_*` environment overrides. Shared by the
/// `Camel.toml`-absent fallback of [`load_config_or_default`] and the
/// compiled-artifact runtime (a compiled document is self-contained by
/// construction — config/profile dependencies are compile-rejected).
pub(crate) fn in_memory_default_config()
-> Result<camel_config::config::CamelConfig, camel_api::CamelError> {
    config::Config::builder()
        .build()
        .and_then(|c| c.try_deserialize())
        .map_err(|e| camel_api::CamelError::Config(format!("Failed to build default config: {e}")))
}

/// Load the Camel.toml at `config_path`, falling back to serde defaults
/// ONLY when the main file does not exist. Shared with `camel job`
/// (same real-boot config seam).
///
/// The not-found decision is made on the main path alone, before loading:
/// a missing INCLUDE also surfaces as file-not-found inside `ConfigError`
/// (dependency-owned; `read_capped` erases the io kind into
/// `ConfigError::Message`), and it must abort, not fall back. Every load
/// error of an existing file — parse failure, broken include, unresolved
/// `${env:...}` placeholder — propagates as `CamelError::Config` so
/// `camel run` fails fast instead of booting on silent defaults.
pub(crate) fn load_config_or_default(
    config_path: &str,
) -> Result<camel_config::config::CamelConfig, camel_api::CamelError> {
    match std::path::Path::new(config_path).try_exists() {
        Ok(false) => in_memory_default_config(),
        Err(e) => Err(camel_api::CamelError::Config(format!(
            "failed to check config path {config_path}: {e}"
        ))),
        Ok(true) => {
            // from_file_with_env applies the allowlisted CAMEL_* env
            // overrides on top of the loaded file; the default-fallback
            // decision above is unaffected.
            camel_config::config::CamelConfig::from_file_with_env(config_path).map_err(|e| {
                camel_api::CamelError::Config(format!("failed to load {config_path}: {e}"))
            })
        }
    }
}

/// Resolve the project root from the config path: the canonicalized parent
/// directory, with an empty parent (a bare `Camel.toml` file name) mapped to
/// the current directory. Both wasm consumers use it: the bean loader and
/// the camel-bundles wasm base dir. Exits with code 1 when the parent cannot
/// be canonicalized; a dangling `--config` parent must fail fast instead of
/// booting on defaults with a broken root. Shared with `camel job`.
pub(crate) fn canonical_project_root(config_path: &std::path::Path) -> std::path::PathBuf {
    match try_canonical_project_root(config_path) {
        Ok(root) => root,
        Err(e) => {
            eprintln!("Error: cannot resolve project root: {e}");
            std::process::exit(1);
        }
    }
}

/// Fallible core of [`canonical_project_root`]: same resolution rules, but
/// returns the error instead of exiting. `camel job` maps the failure into
/// its own exit-2 taxonomy (config-class failure) instead of this crate's
/// exit-1 `camel run` convention.
pub(crate) fn try_canonical_project_root(
    config_path: &std::path::Path,
) -> std::io::Result<std::path::PathBuf> {
    config_path
        .parent()
        .map(|p| {
            if p.as_os_str().is_empty() {
                std::path::Path::new(".")
            } else {
                p
            }
        })
        .unwrap_or(std::path::Path::new("."))
        .canonicalize()
}

// ---------------------------------------------------------------------------
// Shared runtime lifecycle (camel run + compiled artifacts)
// ---------------------------------------------------------------------------

/// How `drive_lifecycle` obtains route definitions.
#[derive(Debug, Clone)]
pub(crate) enum Discover {
    /// `camel run`'s filesystem discovery: glob patterns through the
    /// real-boot discovery seam (ambient `${env:}`, stream-caching
    /// threshold, security compile context built inside the lifecycle).
    Patterns {
        /// Route patterns verbatim (unexpanded globs; discovery owns
        /// expansion, the test-suffix skip, and errors).
        patterns: Vec<String>,
    },
    /// A compiled artifact's embedded document: the normalized payload
    /// through `camel_dsl::discover_embedded_text` with the virtual
    /// identity `compiled://<source_name>` and the ambient (deployment)
    /// environment as the `${env:}` lookup. No config, glob, external
    /// route file, or temporary file is ever touched.
    Embedded {
        /// Normalized document text (pre-interpolation authoring text).
        text: String,
        /// Logical source name from the artifact manifest.
        source_name: String,
        /// Route/job document kind recorded in the trailer.
        kind: camel_dsl::EmbeddedDocumentKind,
    },
}

/// Watcher ingredients for [`LifecycleSpec`] (camel run only; compiled
/// artifacts never watch).
#[derive(Debug, Clone)]
pub(crate) struct WatchSpec {
    /// Initial route patterns (for watch-directory derivation and the
    /// watching log line).
    pub patterns: Vec<String>,
    /// `--routes` override, re-applied on every reload pass.
    pub routes_override: Option<String>,
    /// Camel.toml routes entries, re-applied on every reload pass.
    pub config_routes: Option<Vec<String>>,
}

/// Lifecycle inputs shared by `camel run` and compiled artifacts.
pub(crate) struct LifecycleSpec {
    /// Boot configuration (already CLI-overridden for `camel run`;
    /// default in-memory for a compiled artifact).
    pub config: camel_config::config::CamelConfig,
    /// camel-bundles base dir (wasm bean/plugin resolution root).
    pub project_root: std::path::PathBuf,
    /// Route source (filesystem discovery or embedded document).
    pub discover: Discover,
    /// `Some` enables the reload watcher; `None` disables it
    /// unconditionally (compiled artifacts always pass `None`).
    pub watch: Option<WatchSpec>,
    /// Emit the `camel run` CWD-trust WARN after context configure.
    /// Compiled artifacts skip it: their documents cannot reference
    /// scripts/WASM (compile-time asset policy).
    pub trust_note: bool,
    /// Idle log line emitted when the watcher is disabled.
    pub idle_note: &'static str,
}

/// Lifecycle failure classes; the caller owns rendering and exit codes
/// (`camel run` logs and exits 1, an artifact writes its report and exits
/// 2).
pub(crate) enum LifecycleFailure {
    /// Route discovery failed. `camel run` renders the
    /// `MaterializationFailures` detail lines itself.
    Discovery(camel_dsl::DiscoveryError),
    /// Config/context/bootstrap/start class failure.
    Boot(camel_api::CamelError),
}

/// Drive the full runtime lifecycle: arm stop signals, build the context,
/// run the shared boot cascade, load and register routes, start the
/// context, optionally watch, wait for the first stop signal, and tear
/// down gracefully. A second stop signal force-exits 1 (rc-kz85m). Boot
/// failures return [`LifecycleFailure`]; teardown failures are logged and
/// never change the outcome (ADR-0069 section 10).
pub(crate) async fn drive_lifecycle(spec: LifecycleSpec) -> Result<(), LifecycleFailure> {
    // 0. Register the SIGTERM stream BEFORE boot: a TERM arriving while
    //    config load, the bundle cascade, discovery, or ctx.start() still
    //    run would otherwise hit the default disposition and kill the
    //    process before the shutdown select below arms (rc-z5zch — the
    //    empty-discovery harness SIGTERMs on the zero-routes WARN, which
    //    fires mid-boot). tokio buffers the pending signal; the run loop
    //    consumes it as soon as it awaits, so a boot-time TERM completes
    //    boot and then shuts down gracefully. Boot itself is not
    //    interrupted mid-flight.
    #[cfg(unix)]
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("Failed to install SIGTERM handler"); // allow-unwrap
    // rc-ukwlt (SIGINT twin of rc-z5zch): tokio::signal::ctrl_c() is only
    // armed at the shutdown select below, so an INT arriving mid-boot would
    // hit the default disposition and kill the process. This entry-registered
    // stream buffers the pending interrupt the same way; the select consumes
    // it as soon as it awaits.
    #[cfg(unix)]
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .expect("Failed to install SIGINT handler"); // allow-unwrap

    let camel_config = spec.config;

    // 2. Build context with beans registry (also initialises tracing subscriber)
    let beans_registry = {
        let bean_reg = std::sync::Arc::new(std::sync::Mutex::new(camel_bean::BeanRegistry::new()));
        if camel_config.beans.is_empty() {
            None
        } else {
            Some(bean_reg)
        }
    };

    let mut ctx = camel_config::config::CamelConfig::configure_context_with_beans(
        &camel_config,
        beans_registry.clone(),
    )
    .await
    .map_err(LifecycleFailure::Boot)?;

    // R4-L4: CWD trust model — camel run executes route scripts/WASM/beans
    // from the current working directory (dev-tool model, like cargo run).
    // Compiled artifacts never execute CWD-resolved assets (the compile
    // asset policy rejects them), so the note is camel-run-only.
    if spec.trust_note {
        tracing::warn!(
            "camel run trusts the current working directory and will execute route \
             scripts, WASM modules, and beans resolved from it; only run from a \
             trusted directory"
        );
    }

    match camel_function::FunctionRuntimeService::with_default_container_provider(
        camel_function::FunctionConfig::default(),
    ) {
        Ok(svc) => ctx = ctx.with_lifecycle(svc),
        Err(e) => tracing::warn!("Function runtime disabled: {e}"),
    }

    // Load WASM beans after context is created (needs component registry)
    #[cfg(feature = "wasm")]
    if let Some(ref bean_reg) = beans_registry {
        let component_registry = ctx.registry_arc();
        let plugins_dir_raw = camel_config
            .components
            .raw
            .get("wasm")
            .and_then(|v| v.get("plugins_dir"))
            .and_then(|v| v.as_str())
            .unwrap_or("plugins");
        let camel_root = spec.project_root.clone();
        crate::commands::plugin::validate_plugins_dir(&camel_root, plugins_dir_raw).unwrap_or_else(
            |e| {
                eprintln!("Error: invalid plugins_dir: {e}");
                std::process::exit(1);
            },
        );
        let plugins_dir = camel_root.join(plugins_dir_raw);
        for (bean_name, bean_cfg) in &camel_config.beans {
            tracing::info!(bean = %bean_name, plugin = %bean_cfg.plugin, "registering WASM bean");

            if !bean_cfg
                .plugin
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
            {
                eprintln!(
                    "Invalid bean plugin name '{}': must be alphanumeric with - or _",
                    bean_cfg.plugin
                );
                std::process::exit(1);
            }

            let wasm_path = plugins_dir.join(format!("{}.wasm", bean_cfg.plugin));
            let canonical_plugins = plugins_dir.canonicalize().unwrap_or_else(|_| {
                eprintln!("Plugins directory not found: {}", plugins_dir.display());
                std::process::exit(1);
            });
            let canonical_path = wasm_path.canonicalize().unwrap_or_else(|_| {
                eprintln!("WASM bean plugin not found: {}", wasm_path.display());
                std::process::exit(1);
            });
            if !canonical_path.starts_with(&canonical_plugins) {
                eprintln!(
                    "Bean plugin path escapes plugins directory: {}",
                    bean_cfg.plugin
                );
                std::process::exit(1);
            }
            let wasm_config =
                camel_component_wasm::config::WasmConfig::from_limits(&bean_cfg.limits);
            let wasm_bean = camel_component_wasm::bean::WasmBean::new(
                &wasm_path,
                wasm_config,
                Arc::new(camel_core::RegistryComponentContext::new(
                    component_registry.clone(),
                    Some(ctx.metrics()),
                    camel_component_api::ComponentContext::component_metrics_enabled(&ctx),
                )),
                bean_cfg.config.clone(),
            )
            .await
            .unwrap_or_else(|e| {
                eprintln!("Failed to load WASM bean '{}': {}", bean_name, e);
                std::process::exit(1);
            });
            tracing::info!(
                bean = %bean_name,
                plugin = %bean_cfg.plugin,
                methods = ?wasm_bean.methods(),
                "WASM bean loaded"
            );
            bean_reg
                .lock()
                .expect("beans registry lock") // allow-unwrap
                .register(bean_name, wasm_bean)
                .unwrap_or_else(|e| {
                    eprintln!("Bean registration failed for '{}': {}", bean_name, e);
                    std::process::exit(1);
                });
        }
    }

    // Security compile context (scenario-shared-boot task 2.3): the
    // builder moved to camel-bundles' security_boot module behind the
    // `security` feature (default-on). Without the feature, any
    // configured `[security.*]` section fails closed here, naming the
    // required feature, before any route compiles.
    #[cfg(feature = "security")]
    let security_compile_context =
        camel_bundles::security_boot::build_security_compile_context_from_config(
            &camel_config,
            ctx.registry_arc(),
        )
        .await
        .map_err(LifecycleFailure::Boot)?;
    #[cfg(not(feature = "security"))]
    camel_bundles::security_boot::ensure_security_supported(&camel_config)
        .map_err(LifecycleFailure::Boot)?;
    #[cfg(not(feature = "security"))]
    let security_compile_context = camel_dsl::SecurityCompileContext::default();

    // ADR-0061: install per-bind public-exposure acknowledgements from
    // `[binds."<addr>"]` before any route starts staging (task 2.3: the
    // wiring lives in camel-bundles' security_boot; the helper threads
    // the ack map into the route controller and, under their own cfg
    // gates, the MCP registry and the wasm source-bind gate — mirroring
    // the previous inline block 1:1).
    camel_bundles::security_boot::install_bind_exposure_acks(&mut ctx, &camel_config).await;

    // 4. Boot the component bundle cascade via camel-bundles (ADR-0069
    //    section 10). Boot registers the built-ins, every bundle of the
    //    cascade, the datasource catalog, and the BridgeCleanup lifecycle
    //    on this prepared context, and returns the BootHandle that owns
    //    pool teardown. Route loading, discovery, startup checks, and
    //    ctx.start() stay with the CLI.
    //
    //    project_root feeds the wasm bundle base dir; resolution is shared
    //    with the wasm bean loader (the caller supplies it).
    let boot_handle = camel_bundles::boot(&mut ctx, &camel_config, &spec.project_root)
        .await
        .map_err(LifecycleFailure::Boot)?;

    // 5. Discover and load initial routes
    let defs = match spec.discover {
        Discover::Patterns { patterns } => {
            // Log the patterns: globs are returned unexpanded, so the glob
            // itself stays visible even when no `routes/` dir exists
            // (bd rc-1110 operator diagnosis).
            tracing::info!("camel-cli: loading routes from patterns: {:?}", patterns);
            match camel_dsl::discover_routes_with_threshold_and_security(
                &patterns,
                camel_config.stream_caching.threshold,
                security_compile_context.clone(),
            ) {
                Ok(defs) => {
                    if defs.is_empty() {
                        // Name the patterns: discovery matched no files,
                        // which would otherwise hide the glob the operator
                        // needs to diagnose (bd rc-1110).
                        tracing::warn!(
                            "route discovery matched zero route files for patterns {:?}; \
                             starting with no routes",
                            patterns
                        );
                    }
                    // Benchmark instrumentation: when BENCH_LATENCY_FILE is
                    // set, wrap every top-level `To` step with timing
                    // processors (default), or bracket each whole route when
                    // BENCH_LATENCY_MODE=route (bench_instrument module).
                    crate::commands::bench_instrument::maybe_instrument_routes(defs)
                }
                Err(e) => return Err(LifecycleFailure::Discovery(e)),
            }
        }
        Discover::Embedded {
            text,
            source_name,
            kind,
        } => {
            // Embedded seam: the normalized payload parses through the
            // shared discovery pipeline with the virtual identity
            // `compiled://<source_name>` and the ambient (deployment)
            // environment as the `${env:}` lookup — no filesystem access.
            tracing::info!("camel-cli: loading routes from compiled://{source_name}");
            let ambient = &|name: &str| std::env::var(name).ok();
            match camel_dsl::discover_embedded_text(&text, &source_name, kind, ambient) {
                Ok(defs) => {
                    if defs.is_empty() {
                        tracing::warn!(
                            "embedded document compiled://{source_name} declared zero \
                             routes; starting with no routes"
                        );
                    }
                    defs
                }
                Err(e) => return Err(LifecycleFailure::Discovery(e)),
            }
        }
    };

    let defs = {
        // Conditionally register ExecBundle: only when a discovered route
        // references `exec:` or the operator declared `[components.exec]`.
        // CLI-owned (route-content-conditional), so it stays outside the
        // camel-bundles boot cascade (ADR-0069 section 10) and goes
        // through the single-bundle seam instead.
        #[cfg(feature = "exec")]
        {
            let exec_used =
                camel_core::startup_validation::route_definitions_reference_scheme(&defs, "exec");
            let exec_configured = camel_config.components.raw.contains_key("exec");
            if exec_used || exec_configured {
                camel_bundles::register_bundle::<camel_component_exec::ExecBundle>(
                    &mut ctx,
                    &camel_config,
                )
                .map_err(LifecycleFailure::Boot)?;
            }
        }

        // ADR-0033: register fail-closed ConfigChecks derived from the
        // discovered routes (e.g. SqlDynamicQueryCheck for every `sql:`
        // endpoint). The checks run synchronously at the head of
        // `CamelContext::start()` before any route consumer is started
        // (task 2.3: shared installer in camel-bundles).
        camel_bundles::security_boot::install_sql_startup_checks(&mut ctx, &defs);
        defs
    };

    for def in defs {
        let id = def.route_id().to_string();
        if let Err(e) = ctx.add_route_definition(def).await {
            // log-policy: system-broken
            tracing::error!("Failed to add route '{}': {}", id, e);
        }
    }

    // 6. Start context
    if let Err(e) = ctx.start().await {
        // log-policy: system-broken
        tracing::error!("Failed to start CamelContext: {}", e);
        return Err(LifecycleFailure::Boot(e));
    }

    tracing::info!("camel-cli: context started");

    // jemalloc memory gauges: publish allocator stats every 5s through the
    // context's late-bound metrics handle (OpenSpec memory-gauges task 2.2).
    #[cfg(feature = "jemalloc")]
    crate::allocator_metrics::spawn_allocator_sampler(ctx.metrics());

    // 7/8. Optionally start the reload watcher in background (camel run
    //     only; compiled artifacts disable it unconditionally).
    let watcher_shutdown = CancellationToken::new();
    if let Some(watch) = spec.watch {
        let ctrl = ctx.runtime_execution_handle();
        let watch_routes_override = watch.routes_override.clone();
        let watch_config_routes = watch.config_routes.clone();
        let watch_patterns = watch.patterns.clone();
        let watch_security_compile_context = security_compile_context.clone();
        let drain_timeout = std::time::Duration::from_millis(camel_config.drain_timeout_ms);
        let debounce = std::time::Duration::from_millis(camel_config.watch_debounce_ms);
        let watcher_token = watcher_shutdown.clone();
        tokio::spawn(async move {
            // Watch dirs derive from the unexpanded globs: an
            // initially-empty routes dir must still yield a watched root so
            // newly created files trigger reload.
            let watch_dirs = camel_core::reload_watcher::resolve_watch_dirs(&watch_patterns);
            let result = camel_core::reload_watcher::watch_and_reload(
                watch_dirs,
                ctrl,
                move || {
                    // Re-resolve patterns on every reload pass so discovery
                    // re-globs newly created/deleted files and re-applies
                    // the test-document skip per pass.
                    let patterns =
                        resolve_route_patterns(&watch_routes_override, &watch_config_routes);
                    camel_dsl::discover_routes_with_threshold_and_security(
                        &patterns,
                        camel_config.stream_caching.threshold,
                        watch_security_compile_context.clone(),
                    )
                    .map_err(|e| camel_api::CamelError::RouteError(e.to_string()))
                },
                Some(watcher_token),
                drain_timeout,
                debounce,
            )
            .await;
            if let Err(e) = result {
                // log-policy: system-broken
                tracing::error!("File watcher failed: {}", e);
            }
        });
        tracing::info!(
            "camel-cli: hot-reload watching {:?}. Press Ctrl+C to stop.",
            watch.patterns
        );
    } else {
        tracing::info!("{}", spec.idle_note);
    }

    tokio::select! {
        // The stream was registered before boot (step 0); awaiting here
        // consumes an interrupt that arrived during boot (rc-ukwlt, the
        // SIGINT twin of rc-z5zch).
        _ = async {
            #[cfg(unix)]
            {
                sigint.recv().await
            }
            // No entry-registered SIGINT stream off unix: the portable
            // ctrl_c() listener IS the first-signal handler there. A
            // pending() here would leave non-unix with no shutdown
            // trigger at all (the SIGTERM arm is unix-only).
            #[cfg(not(unix))]
            {
                tokio::signal::ctrl_c().await.ok()
            }
        } => tracing::info!("Received Ctrl+C"),
        // The stream was registered before boot (step 0); awaiting here
        // consumes a TERM that arrived during boot (rc-z5zch).
        _ = async {
            #[cfg(unix)]
            {
                sigterm.recv().await
            }
            #[cfg(not(unix))]
            {
                std::future::pending::<()>().await
            }
        } => tracing::info!("Received SIGTERM"),
    }

    // Second stop signal = force exit (rc-kz85m). Orchestrators (systemd,
    // docker stop) resend the stop signal after their grace period, so the
    // escape hatch must accept the signal they actually send: a second
    // Ctrl+C OR a second TERM. The entry-registered streams are moved here
    // now that the shutdown select above consumed the first signal; polling
    // them (instead of a fresh ctrl_c()) keeps boot-time buffered signals
    // force-exit-eligible and avoids a duplicate SIGINT listener. The task
    // only lives across teardown — it is aborted once shutdown completes —
    // so a signal during normal running still just shuts down gracefully.
    #[cfg(unix)]
    let force_exit = tokio::spawn(async move {
        tokio::select! {
            _ = sigint.recv() => {
                tracing::warn!("Second Ctrl+C — forcing exit");
            }
            _ = sigterm.recv() => {
                tracing::warn!("Second SIGTERM — forcing exit");
            }
        }
        std::process::exit(1);
    });
    #[cfg(not(unix))]
    let force_exit = tokio::spawn(async {
        tokio::signal::ctrl_c().await.ok();
        tracing::warn!("Second Ctrl+C — forcing exit");
        std::process::exit(1);
    });

    tracing::info!("camel-cli: shutting down...");
    watcher_shutdown.cancel();

    // Pool begin_shutdown, ctx.stop, and the deadline-wrapped pool
    // teardown moved into the BootHandle (ADR-0069 section 10). The handle
    // logs each failure (system-broken); `camel run` teardown is non-fatal,
    // so the value is discarded and a teardown failure never changes the
    // exit code.
    let _ = boot_handle.shutdown(&mut ctx).await;

    force_exit.abort();

    tracing::info!("camel-cli: stopped");
    Ok(())
}

pub async fn run(
    routes_override: Option<String>,
    config_path: String,
    cli_watch: Option<bool>,
    otel: bool,
    otel_endpoint: Option<String>,
    service_name: Option<String>,
    health_port: Option<u16>,
) -> Result<(), camel_api::CamelError> {
    // 1. Load config (fall back to empty config with serde defaults if Camel.toml not found)
    let mut camel_config: camel_config::config::CamelConfig = load_config_or_default(&config_path)?;

    // 1b. Apply OTel CLI overrides (--otel-endpoint and --service-name imply --otel)
    let otel_enabled = otel || otel_endpoint.is_some() || service_name.is_some();
    if otel_enabled {
        let otel_cfg =
            camel_config
                .observability
                .otel
                .get_or_insert(camel_config::OtelCamelConfig {
                    enabled: true,
                    endpoint: "http://localhost:4317".to_string(),
                    service_name: "rust-camel".to_string(),
                    ..Default::default()
                });
        otel_cfg.enabled = true;
        if let Some(ep) = otel_endpoint {
            otel_cfg.endpoint = ep;
        }
        if let Some(name) = service_name {
            otel_cfg.service_name = name;
        }
    }

    if let Some(port) = health_port {
        let health_cfg = camel_config
            .observability
            .health
            .get_or_insert(camel_config::config::HealthCamelConfig::default());
        health_cfg.enabled = true;
        health_cfg.port = port;
    }

    // 3. Determine route patterns (before the lifecycle: pure config
    //    resolution). Every path (default glob, `--routes` override,
    //    Camel.toml `routes` entries) returns patterns verbatim;
    //    discovery (camel-dsl) owns the reserved test-suffix skip and
    //    error. The unexpanded globs also feed watch-directory derivation
    //    so an initially-empty routes dir still yields a watched root.
    let config_routes = Some(camel_config.routes.clone());
    let patterns: Vec<String> = resolve_route_patterns(&routes_override, &config_routes);

    // 7. Resolve whether to enable the file watcher:
    //    CLI flag takes precedence; falls back to Camel.toml `watch` field
    //    (default: false).
    let watch_enabled = cli_watch.unwrap_or(camel_config.watch);
    let watch = watch_enabled.then(|| WatchSpec {
        patterns: patterns.clone(),
        routes_override: routes_override.clone(),
        config_routes: config_routes.clone(),
    });

    // project_root feeds the wasm bundle base dir; resolution is shared
    // with the wasm bean loader through `canonical_project_root`.
    let project_root = canonical_project_root(std::path::Path::new(&config_path));

    let spec = LifecycleSpec {
        config: camel_config,
        project_root,
        discover: Discover::Patterns { patterns },
        watch,
        trust_note: true,
        idle_note: "camel-cli: running (hot-reload disabled). Press Ctrl+C to stop.",
    };

    match drive_lifecycle(spec).await {
        Ok(()) => Ok(()),
        Err(LifecycleFailure::Boot(e)) => Err(e),
        Err(LifecycleFailure::Discovery(e)) => {
            match &e {
                camel_dsl::DiscoveryError::MaterializationFailures { failures } => {
                    // log-policy: system-broken
                    tracing::error!("Failed to discover routes: template materialization failed:");
                    for failure in failures {
                        match &failure.route_id {
                            Some(route_id) => {
                                // log-policy: system-broken
                                tracing::error!(
                                    "  {} (template '{}', route '{}'): {}",
                                    failure.path,
                                    failure.template_ref,
                                    route_id,
                                    failure.error
                                );
                            }
                            None => {
                                // log-policy: system-broken
                                tracing::error!(
                                    "  {} (template '{}'): {}",
                                    failure.path,
                                    failure.template_ref,
                                    failure.error
                                );
                            }
                        }
                    }
                }
                _ => {
                    // log-policy: system-broken
                    tracing::error!("Failed to discover routes: {}", e);
                }
            }
            crate::commands::errors::report_cli_failure_and_exit(
                "run",
                &camel_api::CamelError::RouteError(e.to_string()),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Route pattern resolution
// ---------------------------------------------------------------------------

/// The default route glob used when neither `--routes` nor Camel.toml
/// `routes` entries are present.
fn default_patterns() -> Vec<String> {
    vec!["routes/*.yaml".to_string()]
}

/// Three-way route pattern selection with injectable defaults.
///
/// Every branch returns patterns verbatim — no glob expansion, no
/// test-document filtering. Discovery (camel-dsl) owns the reserved
/// test-suffix skip and error on every path.
///
/// - `Some(override)` ⇒ the override verbatim.
/// - `None` + non-empty config routes ⇒ the config entries verbatim.
/// - otherwise ⇒ `defaults` verbatim.
fn resolve_route_patterns_with(
    defaults: &[String],
    routes_override: &Option<String>,
    config_routes: &Option<Vec<String>>,
) -> Vec<String> {
    if let Some(ov) = routes_override {
        vec![ov.clone()]
    } else if let Some(routes) = config_routes {
        if routes.is_empty() {
            defaults.to_vec()
        } else {
            routes.clone()
        }
    } else {
        defaults.to_vec()
    }
}

/// Production entry: resolve the route patterns `camel run` loads routes from.
fn resolve_route_patterns(
    routes_override: &Option<String>,
    config_routes: &Option<Vec<String>>,
) -> Vec<String> {
    resolve_route_patterns_with(&default_patterns(), routes_override, config_routes)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Serializes tests that write or read the process-global
/// `WasmSourceBindAcks` singleton: `set()` replaces the whole map, so
/// parallel test writers would clobber each other's installs.
///
/// Gated to the union of its users' feature gates: the `run` tests
/// behind `wasm`/`security` and the scenario tests behind
/// `integration-http`.
#[cfg(all(
    test,
    any(feature = "security", feature = "wasm", feature = "integration-http")
))]
pub(crate) static WASM_ACKS_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
#[path = "run_tests.rs"]
pub(crate) mod tests;
