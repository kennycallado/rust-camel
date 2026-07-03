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
    let mut camel_config: camel_config::config::CamelConfig =
        camel_config::config::CamelConfig::from_file(&config_path).unwrap_or_else(|_| {
            // Build an empty config so serde defaults apply
            config::Config::builder()
                .build()
                .and_then(|c| c.try_deserialize())
                .unwrap_or_else(|e| {
                    eprintln!("Failed to build default config: {e}");
                    std::process::exit(1);
                })
        });

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
                component_registry.clone(),
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

    // 3. Determine route patterns
    let patterns: Vec<String> = if let Some(p) = routes_override {
        vec![p]
    } else if !camel_config.routes.is_empty() {
        camel_config.routes.clone()
    } else {
        vec!["routes/*.yaml".to_string()]
    };

    tracing::info!("camel-cli: loading routes from patterns: {:?}", patterns);

    let security_compile_context =
        crate::build_security_compile_context_from_config(&camel_config, ctx.registry_arc())
            .await?;

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

    #[cfg(feature = "wasm")]
    {
        let base_dir = std::path::Path::new(&config_path)
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();
        let wasm_bundle = camel_component_wasm::WasmBundle::new(ctx.registry_arc(), base_dir);
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
            // ADR-0033: register fail-closed ConfigChecks derived from the
            // discovered routes (e.g. SqlDynamicQueryCheck for every `sql:`
            // endpoint). The checks run synchronously at the head of
            // `CamelContext::start()` before any route consumer is started.
            for check in
                camel_core::startup_validation::scan_route_definitions_for_sql_checks(&defs)
            {
                ctx.add_startup_check(check);
            }
            for def in defs {
                let id = def.route_id().to_string();
                if let Err(e) = ctx.add_route_definition(def).await {
                    // log-policy: system-broken
                    tracing::error!("Failed to add route '{}': {}", id, e);
                }
            }
        }
        Err(e) => {
            // log-policy: system-broken
            tracing::error!("Failed to discover routes: {}", e);
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
        let watch_patterns = patterns.clone();
        let watch_security_compile_context = security_compile_context.clone();
        let drain_timeout = std::time::Duration::from_millis(camel_config.drain_timeout_ms);
        let debounce = std::time::Duration::from_millis(camel_config.watch_debounce_ms);
        let watcher_token = watcher_shutdown.clone();
        tokio::spawn(async move {
            let watch_dirs = camel_core::reload_watcher::resolve_watch_dirs(&watch_patterns);
            let result = camel_core::reload_watcher::watch_and_reload(
                watch_dirs,
                ctrl,
                move || {
                    camel_dsl::discover_routes_with_threshold_and_security(
                        &watch_patterns,
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
