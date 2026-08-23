//! `camel run` subcommand body.
//!
//! Owns the 7 ADR-0012 `error!` sites migrated in Phase C (see ADR-0012 +
//! Phase C plan). Each site has a `// log-policy: …` annotation that
//! classifies it per the ADR taxonomy.
//!
//! All sites in this file are category (c) or (d) — system-broken /
//! bootstrap. The annotations are added in the Infra cluster task (Task 9),
//! NOT in this split commit.

use camel_api::datasource::DatasourceCatalog;
#[cfg(feature = "wasm")]
use camel_bean::BeanProcessor;
use camel_core::datasource::RuntimeDatasourceCatalog;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

struct BridgeCleanup {
    xslt: Arc<camel_xslt::XsltBridgeRuntime>,
    xj: Arc<camel_xj::XjBridgeRuntime>,
    validator: Option<Arc<camel_component_validator::xsd_bridge::XsdBridgeBackend>>,
}

#[async_trait::async_trait]
impl camel_api::lifecycle::Lifecycle for BridgeCleanup {
    fn name(&self) -> &str {
        "bridge-cleanup"
    }

    async fn start(&mut self) -> Result<(), camel_api::CamelError> {
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), camel_api::CamelError> {
        self.xslt.shutdown().await;
        self.xj.shutdown().await;
        if let Some(validator) = &self.validator {
            validator.shutdown().await;
        }
        Ok(())
    }
}

/// Load the Camel.toml at `config_path`, falling back to serde defaults
/// ONLY when the main file does not exist.
///
/// The not-found decision is made on the main path alone, before loading:
/// a missing INCLUDE also surfaces as file-not-found inside `ConfigError`
/// (dependency-owned; `read_capped` erases the io kind into
/// `ConfigError::Message`), and it must abort, not fall back. Every load
/// error of an existing file — parse failure, broken include, unresolved
/// `${env:...}` placeholder — propagates as `CamelError::Config` so
/// `camel run` fails fast instead of booting on silent defaults.
fn load_config_or_default(
    config_path: &str,
) -> Result<camel_config::config::CamelConfig, camel_api::CamelError> {
    match std::path::Path::new(config_path).try_exists() {
        Ok(false) => {
            // Build an empty config so serde defaults apply.
            config::Config::builder()
                .build()
                .and_then(|c| c.try_deserialize())
                .map_err(|e| {
                    camel_api::CamelError::Config(format!("Failed to build default config: {e}"))
                })
        }
        Err(e) => Err(camel_api::CamelError::Config(format!(
            "failed to check config path {config_path}: {e}"
        ))),
        Ok(true) => camel_config::config::CamelConfig::from_file(config_path).map_err(|e| {
            camel_api::CamelError::Config(format!("failed to load {config_path}: {e}"))
        }),
    }
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
    .unwrap_or_else(|e| {
        eprintln!("Failed to configure CamelContext: {e}");
        std::process::exit(1);
    });

    // R4-L4: CWD trust model — camel run executes route scripts/WASM/beans
    // from the current working directory (dev-tool model, like cargo run).
    tracing::warn!(
        "camel run trusts the current working directory and will execute route \
         scripts, WASM modules, and beans resolved from it; only run from a \
         trusted directory"
    );

    match camel_function::FunctionRuntimeService::with_default_container_provider(
        camel_function::FunctionConfig::default(),
    ) {
        Ok(svc) => ctx = ctx.with_lifecycle(svc),
        Err(e) => tracing::warn!("Function runtime disabled: {e}"),
    }

    // 3a. Create datasource catalog from configured datasources, wiring health registry
    let datasource_catalog: Arc<dyn DatasourceCatalog> = {
        let catalog = RuntimeDatasourceCatalog::new(camel_config.datasources.clone())
            .with_health_registry(ctx.health_registry());
        Arc::new(catalog)
    };

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
        let config_dir = std::path::Path::new(&config_path)
            .parent()
            .map(|p| {
                if p.as_os_str().is_empty() {
                    std::path::Path::new(".")
                } else {
                    p
                }
            })
            .unwrap_or(std::path::Path::new("."));
        let camel_root = config_dir.canonicalize().unwrap_or_else(|e| {
            eprintln!("Error: cannot resolve project root: {e}");
            std::process::exit(1);
        });
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

    // 3. Determine route patterns. Every path (default glob, `--routes`
    //    override, Camel.toml `routes` entries) returns patterns verbatim;
    //    discovery (camel-dsl) owns the reserved test-suffix skip and error.
    //    The unexpanded globs also feed watch-directory derivation so an
    //    initially-empty routes dir still yields a watched root.
    let config_routes = Some(camel_config.routes.clone());
    let patterns: Vec<String> = resolve_route_patterns(&routes_override, &config_routes);

    // Log the patterns: globs are returned unexpanded, so the glob itself
    // stays visible even when no `routes/` dir exists (bd rc-1110 operator
    // diagnosis).
    tracing::info!("camel-cli: loading routes from patterns: {:?}", patterns);

    let security_compile_context = crate::security::build_security_compile_context_from_config(
        &camel_config,
        ctx.registry_arc(),
    )
    .await?;

    // ADR-0061: install per-bind public-exposure acknowledgements from
    // `[binds."<addr>"]` before any route starts staging.
    let bind_acks: std::collections::HashMap<String, bool> = camel_config
        .binds
        .iter()
        .map(|(k, v)| (k.clone(), v.allow_public_exposure))
        .collect();
    // MCP binds are invisible to the route-level gate (`mcp:` from-URIs
    // carry no authority; the listener binds via `McpServerConfig.bind`),
    // so the same ack map threads into the MCP registry's per-bind gate
    // (Task 2.6) — refuse-without-ack on non-loopback Public, warn when
    // acked. Only when the mcp component is compiled in.
    #[cfg(feature = "mcp")]
    camel_component_mcp::McpServerRegistry::global().set_bind_exposure_acks(bind_acks.clone());
    // Wasm source binds are likewise invisible to the route-level gate
    // (the `wasm:` gate runs inside the source consumer; there is no
    // shared listener registry), so the same ack map threads into the
    // wasm component's per-bind gate — fail-closed without ack. Only
    // when the wasm component is compiled in.
    #[cfg(feature = "wasm")]
    install_wasm_bind_acks(&bind_acks);
    ctx.set_bind_exposure_acks(camel_core::route_controller::BindExposureAcks::new(
        bind_acks,
    ))
    .await;

    // Define register_bundle! macro — looks up config key in ComponentsConfig::raw,
    // falling back to an empty table so bundles always register with their serde defaults.
    // Uses UFCS to invoke ComponentBundle methods without requiring trait in scope
    macro_rules! register_bundle {
        ($ctx:expr, $cfg:expr, $Bundle:ty) => {
            let raw = $cfg
                .components
                .raw
                .get(<$Bundle as camel_component_api::ComponentBundle>::config_key())
                .cloned()
                .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));
            match <$Bundle as camel_component_api::ComponentBundle>::from_toml(raw) {
                Ok(bundle) => <$Bundle as camel_component_api::ComponentBundle>::register_all(
                    bundle, &mut $ctx,
                ),
                Err(e) => {
                    return Err(camel_api::CamelError::Config(format!(
                        "Failed to load {} config: {}",
                        <$Bundle as camel_component_api::ComponentBundle>::config_key(),
                        e
                    )));
                }
            }
        };
    }

    // Register built-in components (no config needed)
    ctx.register_component(camel_component_timer::TimerComponent::new());
    ctx.register_component(camel_component_cron::CronComponent::new());
    ctx.register_component(camel_component_log::LogComponent::new());
    ctx.register_component(camel_component_direct::DirectComponent::new());
    ctx.register_component(camel_component_seda::SedaComponent::new());
    ctx.register_component(camel_component_mock::MockComponent::new());
    ctx.register_component(camel_component_controlbus::ControlBusComponent::new());
    let validator_component = camel_component_validator::ValidatorComponent::new();
    let validator_backend = validator_component.xsd_bridge_backend();
    ctx.register_component(validator_component);

    let xslt_component = camel_xslt::XsltComponent::default();
    let xslt_runtime = xslt_component.bridge_runtime();
    ctx.register_component(xslt_component);

    let xj_component = camel_xj::XjComponent::default();
    let xj_runtime = xj_component.bridge_runtime();
    ctx.register_component(xj_component);

    ctx = ctx.with_lifecycle(BridgeCleanup {
        xslt: xslt_runtime,
        xj: xj_runtime,
        validator: validator_backend,
    });

    // Register HTTP, WS, File, Container (always-on in camel-cli, no feature flag)
    register_bundle!(ctx, camel_config, camel_component_http::HttpBundle);
    #[cfg(feature = "http-static")]
    register_bundle!(ctx, camel_config, camel_component_http::HttpStaticBundle);
    register_bundle!(ctx, camel_config, camel_component_ws::WsBundle);
    register_bundle!(ctx, camel_config, camel_component_file::FileBundle);
    register_bundle!(
        ctx,
        camel_config,
        camel_component_container::ContainerBundle
    );
    // External template renderer (ADR-0047 Stage 2): always-on built-in.
    register_bundle!(ctx, camel_config, camel_template::TemplateBundle);

    // Register optional/feature-gated bundles
    let jms_pool = {
        let raw = camel_config
            .components
            .raw
            .get("jms")
            .cloned()
            .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));
        match <camel_component_jms::JmsBundle as camel_component_api::ComponentBundle>::from_toml(
            raw,
        ) {
            Ok(bundle) => {
                let pool = bundle.pool();
                <camel_component_jms::JmsBundle as camel_component_api::ComponentBundle>::register_all(bundle, &mut ctx);
                pool
            }
            Err(e) => {
                return Err(camel_api::CamelError::Config(format!(
                    "Failed to load jms config: {e}"
                )));
            }
        }
    };

    let cxf_pool = {
        let raw = camel_config
            .components
            .raw
            .get("cxf")
            .cloned()
            .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));
        match <camel_component_cxf::CxfBundle as camel_component_api::ComponentBundle>::from_toml(
            raw,
        ) {
            Ok(bundle) => {
                let pool = bundle.pool();
                <camel_component_cxf::CxfBundle as camel_component_api::ComponentBundle>::register_all(bundle, &mut ctx);
                pool
            }
            Err(e) => {
                return Err(camel_api::CamelError::Config(format!(
                    "Failed to load cxf config: {e}"
                )));
            }
        }
    };

    #[cfg(feature = "kafka")]
    register_bundle!(ctx, camel_config, camel_component_kafka::KafkaBundle);
    #[cfg(feature = "mqtt")]
    register_bundle!(ctx, camel_config, camel_component_mqtt::MqttBundle);
    register_bundle!(ctx, camel_config, camel_master::MasterBundle);
    register_bundle!(
        ctx,
        camel_config,
        camel_component_opensearch::OpenSearchBundle
    );
    register_bundle!(ctx, camel_config, camel_component_redis::RedisBundle);
    {
        let sql_raw = camel_config
            .components
            .raw
            .get(<camel_component_sql::SqlBundle as camel_component_api::ComponentBundle>::config_key())
            .cloned()
            .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));
        match <camel_component_sql::SqlBundle as camel_component_api::ComponentBundle>::from_toml(
            sql_raw,
        ) {
            Ok(bundle) => {
                let bundle = bundle.with_catalog(Arc::clone(&datasource_catalog));
                <camel_component_sql::SqlBundle as camel_component_api::ComponentBundle>::register_all(bundle, &mut ctx);
            }
            Err(e) => {
                // log-policy: system-broken
                tracing::error!("failed to initialize SQL bundle: {}", e);
            }
        }
    }
    #[cfg(feature = "surrealdb")]
    {
        let surrealdb_raw = camel_config
            .components
            .raw
            .get(<camel_component_surrealdb::SurrealDbBundle as camel_component_api::ComponentBundle>::config_key())
            .cloned()
            .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));
        match <camel_component_surrealdb::SurrealDbBundle as camel_component_api::ComponentBundle>::from_toml(
            surrealdb_raw,
        ) {
            Ok(bundle) => {
                let bundle = bundle.with_catalog(Arc::clone(&datasource_catalog));
                <camel_component_surrealdb::SurrealDbBundle as camel_component_api::ComponentBundle>::register_all(
                    bundle, &mut ctx,
                );
            }
            Err(e) => {
                // log-policy: system-broken
                tracing::error!("failed to initialize SurrealDB bundle: {}", e);
            }
        }
    }
    #[cfg(feature = "grpc")]
    register_bundle!(ctx, camel_config, camel_component_grpc::GrpcBundle);

    #[cfg(feature = "llm")]
    register_bundle!(ctx, camel_config, camel_component_llm::LlmBundle);

    #[cfg(feature = "mcp")]
    register_bundle!(ctx, camel_config, camel_component_mcp::McpBundle);

    #[cfg(feature = "wasm")]
    {
        let base_dir = std::path::Path::new(&config_path)
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();
        let wasm_bundle = camel_component_wasm::WasmBundle::new(
            Arc::new(camel_core::RegistryComponentContext::new(
                ctx.registry_arc(),
            )),
            base_dir,
        );
        <camel_component_wasm::WasmBundle as camel_component_api::ComponentBundle>::register_all(
            wasm_bundle,
            &mut ctx,
        );
    }

    // Languages are registered in `configure_context_with_beans` (camel-config)
    // via `camel_core::languages_from_config(&config.languages)`, which applies
    // Camel.toml [languages.*.limits] and registers js/javascript/rhai/
    // jsonpath/xpath under feature gates. A direct `CamelContext::builder().build()`
    // caller gets `LanguagesConfig::default()` (rust-camel runtime defaults).

    // 5. Discover and load initial routes
    match camel_dsl::discover_routes_with_threshold_and_security(
        &patterns,
        camel_config.stream_caching.threshold,
        security_compile_context.clone(),
    ) {
        Ok(defs) => {
            if defs.is_empty() {
                // Name the patterns: discovery matched no files, which would
                // otherwise hide the glob the operator needs to diagnose
                // (bd rc-1110).
                tracing::warn!(
                    "route discovery matched zero route files for patterns {:?}; \
                     starting with no routes",
                    patterns
                );
            }
            // Conditionally register ExecBundle: only when a discovered route
            // references `exec:` or the operator declared `[components.exec]`.
            #[cfg(feature = "exec")]
            {
                let exec_used = camel_core::startup_validation::route_definitions_reference_scheme(
                    &defs, "exec",
                );
                let exec_configured = camel_config.components.raw.contains_key("exec");
                if exec_used || exec_configured {
                    register_bundle!(ctx, camel_config, camel_component_exec::ExecBundle);
                }
            }

            // ADR-0033: register fail-closed ConfigChecks derived from the
            // discovered routes (e.g. SqlDynamicQueryCheck for every `sql:`
            // endpoint). The checks run synchronously at the head of
            // `CamelContext::start()` before any route consumer is started.
            for check in
                camel_core::startup_validation::scan_route_definitions_for_sql_checks(&defs)
            {
                ctx.add_startup_check(check);
            }
            // Benchmark instrumentation: when BENCH_LATENCY_FILE is set,
            // wrap every top-level `To` step with timing processors.
            let defs = crate::commands::bench_instrument::maybe_instrument_routes(defs);
            for def in defs {
                let id = def.route_id().to_string();
                if let Err(e) = ctx.add_route_definition(def).await {
                    // log-policy: system-broken
                    tracing::error!("Failed to add route '{}': {}", id, e);
                }
            }
        }
        Err(e) => {
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
            std::process::exit(1);
        }
    }

    // 6. Start context
    if let Err(e) = ctx.start().await {
        // log-policy: system-broken
        tracing::error!("Failed to start CamelContext: {}", e);
        std::process::exit(1);
    }

    tracing::info!("camel-cli: context started");

    // 7. Resolve whether to enable the file watcher:
    //    CLI flag takes precedence; falls back to Camel.toml `watch` field (default: false).
    let watch_enabled = cli_watch.unwrap_or(camel_config.watch);

    // 8. Optionally start file watcher in background
    let watcher_shutdown = CancellationToken::new();
    if watch_enabled {
        let ctrl = ctx.runtime_execution_handle();
        let watch_routes_override = routes_override.clone();
        let watch_config_routes = config_routes.clone();
        let watch_patterns = patterns.clone();
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
            patterns
        );
    } else {
        tracing::info!("camel-cli: running (hot-reload disabled). Press Ctrl+C to stop.");
    }

    tokio::select! {
        _ = tokio::signal::ctrl_c() => tracing::info!("Received Ctrl+C"),
        _ = async {
            #[cfg(unix)]
            {
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("Failed to install SIGTERM handler") // allow-unwrap
                    .recv()
                    .await
            }
            #[cfg(not(unix))]
            {
                std::future::pending::<()>().await
            }
        } => tracing::info!("Received SIGTERM"),
    }

    // Second Ctrl+C = force exit
    let force_exit = tokio::spawn(async {
        tokio::signal::ctrl_c().await.ok();
        tracing::warn!("Second Ctrl+C — forcing exit");
        std::process::exit(1);
    });

    tracing::info!("camel-cli: shutting down...");
    watcher_shutdown.cancel();

    // Signal pools to stop restarting BEFORE context shutdown
    jms_pool.begin_shutdown();
    cxf_pool.begin_shutdown();

    // Stop context (routes + lifecycle services)
    ctx.stop().await.unwrap_or_else(|e| {
        // log-policy: system-broken
        tracing::error!("Error during shutdown: {}", e);
    });

    // Tear down bridge pools with timeouts
    const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

    match tokio::time::timeout(SHUTDOWN_TIMEOUT, jms_pool.shutdown()).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            // log-policy: system-broken
            tracing::error!("JMS pool shutdown failed: {}", e);
        }
        Err(_) => tracing::warn!("JMS pool shutdown timed out after 30s"),
    }

    match tokio::time::timeout(SHUTDOWN_TIMEOUT, cxf_pool.shutdown()).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            // log-policy: system-broken
            tracing::error!("CXF pool shutdown failed: {}", e);
        }
        Err(_) => tracing::warn!("CXF pool shutdown timed out after 30s"),
    }

    force_exit.abort();

    tracing::info!("camel-cli: stopped");
    Ok(())
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

/// Install per-bind public-exposure acks into the wasm component's
/// source-bind gate (wasm-source-auth-kernel, Task 1.5). Wasm source
/// consumers bind outside the route-level gate, so the same ack map the
/// route controller gets must also reach `WasmSourceBindAcks` before
/// any route starts staging. Only exists when the wasm component is
/// compiled in.
#[cfg(feature = "wasm")]
fn install_wasm_bind_acks(bind_acks: &std::collections::HashMap<String, bool>) {
    camel_component_wasm::WasmSourceBindAcks::global().set(bind_acks.clone());
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The run function must emit exactly one startup warning about the CWD trust model.
    #[test]
    fn startup_warning_emitted() {
        let source = include_str!("run.rs");
        // Build the search string from two parts so the concatenated form
        // never appears literally in test code — only in the warn! call.
        let a = "camel run trusts the current working directory";
        let b = " and will execute route";
        let msg = format!("{a}{b}");
        let count = source.matches(&msg).count();
        assert_eq!(
            count, 1,
            "expected exactly one tracing::warn! with the trust-model message in run.rs; found {count}"
        );
    }

    /// The run command's clap help must document the trust model.
    #[test]
    fn clap_help_documents_trust_model() {
        let source = include_str!("../main.rs");
        let has_trust_doc = source
            .contains("Trust model: `camel run` executes route scripts, WASM modules, and beans")
            || source
                .contains("Trust model: camel run executes route scripts, WASM modules, and beans");
        assert!(
            has_trust_doc,
            "expected trust model documentation in the Run subcommand help in main.rs"
        );
    }

    /// Minimal valid route text for fixtures (string form). Pattern
    /// resolution never reads file content; fixtures exist to prove the
    /// resolver ignores matching files and returns globs verbatim.
    const ROUTE_TEXT: &str = r#"routes: [- from: "direct:x", steps: [{to: "mock:m"}]]"#;

    #[test]
    fn none_returns_defaults_verbatim() {
        let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
        let routes_dir = dir.path().join("routes");
        std::fs::create_dir_all(&routes_dir).expect("create routes dir"); // allow-unwrap
        std::fs::write(routes_dir.join("demo.yaml"), ROUTE_TEXT).expect("write demo.yaml"); // allow-unwrap
        std::fs::write(routes_dir.join("demo.test.yaml"), b"expects: {}")
            .expect("write demo.test.yaml"); // allow-unwrap

        let pat = format!("{}/routes/*.yaml", dir.path().display());
        let result = resolve_route_patterns_with(std::slice::from_ref(&pat), &None, &None);
        assert_eq!(
            result,
            vec![pat],
            "defaults must pass through verbatim: no expansion, no test-doc filtering"
        );
    }

    #[test]
    fn resolver_returns_unexpanded_globs() {
        let glob = "routes/**/*.yaml".to_string();
        assert_eq!(
            resolve_route_patterns(&Some(glob.clone()), &None),
            vec![glob.clone()],
            "override globs must stay unexpanded (watch-root guard)"
        );
        assert_eq!(
            resolve_route_patterns(&None, &Some(vec![glob.clone()])),
            vec![glob],
            "config-route globs must stay unexpanded (watch-root guard)"
        );
    }

    #[test]
    fn override_passthrough_untouched() {
        let result = resolve_route_patterns(&Some("routes/*.test.yaml".to_string()), &None);
        assert_eq!(result, vec!["routes/*.test.yaml".to_string()]);
    }

    #[test]
    fn config_routes_passthrough_untouched() {
        let result = resolve_route_patterns(&None, &Some(vec!["custom/*.yaml".to_string()]));
        assert_eq!(result, vec!["custom/*.yaml".to_string()]);
    }

    #[test]
    fn literal_test_doc_path_reaches_discovery() {
        let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
        let routes_dir = dir.path().join("routes");
        std::fs::create_dir_all(&routes_dir).expect("create routes dir"); // allow-unwrap
        std::fs::write(routes_dir.join("demo.test.yaml"), b"expects: {}")
            .expect("write demo.test.yaml"); // allow-unwrap

        let p = format!("{}/routes/demo.test.yaml", dir.path().display());
        let result = resolve_route_patterns(&Some(p.clone()), &None);
        assert_eq!(
            result,
            vec![p],
            "a literal test-doc path must reach discovery unfiltered; \
             ReservedTestSuffix is discovery's job"
        );
    }

    /// Task 8 (unify-config-interpolation-on-env): the empty-config fallback
    /// applies ONLY to a missing main file; every load error of an existing
    /// file aborts instead of silently booting on defaults.
    #[test]
    fn missing_config_file_yields_defaults() {
        let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
        let path = dir.path().join("nope.toml");
        let config = load_config_or_default(&path.display().to_string())
            .expect("missing file must fall back to serde defaults"); // allow-unwrap
        assert_eq!(config.log_level, "INFO");
        assert_eq!(config.timeout_ms, 5000);
    }

    #[test]
    fn malformed_config_aborts_instead_of_defaults() {
        let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
        let path = dir.path().join("Camel.toml");
        std::fs::write(&path, "[observability").expect("write malformed Camel.toml"); // allow-unwrap
        let err = load_config_or_default(&path.display().to_string())
            .expect_err("malformed config must abort, not fall back to defaults"); // allow-unwrap
        let msg = err.to_string();
        assert!(
            msg.contains(&path.display().to_string()),
            "error must name the config path: {msg}"
        );
        assert!(
            msg.contains("failed to load"),
            "error must carry the load prefix: {msg}"
        );
        assert!(
            msg.contains("Failed to parse TOML"),
            "error must carry the parse cause, not only the prefix: {msg}"
        );
    }

    #[test]
    fn broken_include_aborts_instead_of_defaults() {
        let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
        let path = dir.path().join("Camel.toml");
        std::fs::write(&path, "include = [\"missing.toml\"]\n").expect("write Camel.toml"); // allow-unwrap
        let err = load_config_or_default(&path.display().to_string())
            .expect_err("broken include must abort, not fall back to defaults"); // allow-unwrap
        let msg = err.to_string();
        assert!(
            msg.contains(&path.display().to_string()),
            "error must name the main config path: {msg}"
        );
        assert!(
            msg.contains("missing.toml"),
            "error must name the missing include: {msg}"
        );
    }

    /// Restores an env var to its prior value on drop, so a panicking
    /// assertion cannot leak the test's env mutation into other tests.
    struct EnvVarGuard {
        key: &'static str,
        prior: Option<String>,
    }

    impl EnvVarGuard {
        fn unset(key: &'static str) -> Self {
            let prior = std::env::var(key).ok();
            // SAFETY: test-scoped; the guard restores the prior value on drop.
            unsafe { std::env::remove_var(key) };
            Self { key, prior }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.prior {
                Some(value) => {
                    // SAFETY: test-scoped restore of the value captured at guard creation.
                    unsafe { std::env::set_var(self.key, value) };
                }
                None => {
                    // SAFETY: test-scoped; the var was unset before the test.
                    unsafe { std::env::remove_var(self.key) };
                }
            }
        }
    }

    #[test]
    fn unresolved_placeholder_aborts_instead_of_defaults() {
        // Env hygiene: the referenced var must be unset for the duration of
        // the test; the guard restores any prior value on drop.
        let _guard = EnvVarGuard::unset("RUST_CAMEL_TEST_RUN_A");

        let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
        let path = dir.path().join("Camel.toml");
        std::fs::write(
            &path,
            "[observability.otel]\nendpoint = \"${env:RUST_CAMEL_TEST_RUN_A}\"\n",
        )
        .expect("write Camel.toml"); // allow-unwrap

        let err = load_config_or_default(&path.display().to_string())
            .expect_err("unresolved ${env:} must abort, not fall back to defaults"); // allow-unwrap
        let msg = err.to_string();
        assert!(
            msg.contains(&path.display().to_string()),
            "error must name the config path: {msg}"
        );
        assert!(
            msg.contains("RUST_CAMEL_TEST_RUN_A"),
            "error must name the unresolved env var: {msg}"
        );
    }

    #[test]
    fn try_exists_error_aborts_instead_of_defaults() {
        let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
        let file_path = dir.path().join("Camel.toml");
        std::fs::write(&file_path, "").expect("write Camel.toml"); // allow-unwrap
        let child = file_path.join("x");
        let child_str = child.display().to_string();
        match load_config_or_default(&child_str) {
            Err(err) => {
                let msg = err.to_string();
                assert!(
                    msg.contains(&child_str),
                    "error must name the config path: {msg}"
                );
            }
            Ok(config) => {
                assert_eq!(config.log_level, "INFO");
                assert_eq!(config.timeout_ms, 5000);
            }
        }
    }

    /// Task 1.5 (wasm-source-auth-kernel): `camel run` threads the
    /// per-bind exposure acks from `[binds."<addr>"]` into the wasm
    /// component's source-bind gate. The wiring helper invoked at run
    /// startup must install the config-built ack map so
    /// `WasmSourceBindAcks::acknowledged` reflects what the config set.
    #[cfg(feature = "wasm")]
    #[test]
    fn wasm_bind_acks_wired_from_config() {
        const TEST_BIND: &str = "0.0.0.0:41234"; // distinctive; no other test acks it

        let camel_config: camel_config::CamelConfig = toml::from_str(&format!(
            r#"[binds."{TEST_BIND}"]
allow_public_exposure = true
"#
        ))
        .expect("parse test CamelConfig"); // allow-unwrap

        // Same construction as the run command's wiring site.
        let bind_acks: std::collections::HashMap<String, bool> = camel_config
            .binds
            .iter()
            .map(|(k, v)| (k.clone(), v.allow_public_exposure))
            .collect();

        install_wasm_bind_acks(&bind_acks);

        assert!(
            camel_component_wasm::WasmSourceBindAcks::global().acknowledged(TEST_BIND),
            "run wiring must install wasm bind acks from CamelConfig.binds"
        );
    }
}
