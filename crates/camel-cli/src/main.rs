use camel_cli::commands;

use clap::{Parser, Subcommand};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(
    name = "camel",
    version,
    about = "Command-line interface for Apache Camel in Rust"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start a Camel context from YAML route files
    Run {
        /// Glob pattern for route YAML files (overrides Camel.toml routes config)
        #[arg(long, value_name = "GLOB")]
        routes: Option<String>,

        /// Path to Camel.toml config file
        #[arg(long, value_name = "FILE", default_value = "Camel.toml")]
        config: String,

        /// Enable file-watcher hot-reload (overrides Camel.toml watch setting)
        #[arg(long, overrides_with = "no_watch")]
        watch: bool,

        /// Disable file-watcher hot-reload (overrides Camel.toml watch setting)
        #[arg(long, overrides_with = "watch")]
        no_watch: bool,

        /// Enable OpenTelemetry export (traces, metrics, logs)
        #[arg(long)]
        otel: bool,

        /// OTLP endpoint URL (implies --otel)
        #[arg(long, value_name = "URL")]
        otel_endpoint: Option<String>,

        /// OTel service name (implies --otel)
        #[arg(long, value_name = "NAME")]
        service_name: Option<String>,

        /// Override health server port (starts standalone health server)
        #[arg(long, value_name = "PORT")]
        health_port: Option<u16>,
    },

    /// Inspect a runtime journal file.
    Journal {
        #[command(subcommand)]
        action: JournalAction,
    },

    /// Scaffold a new Camel project
    New(commands::new::NewArgs),
}

#[derive(Subcommand)]
enum JournalAction {
    /// Inspect events in a redb journal file.
    Inspect(commands::journal::JournalInspectArgs),
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            routes,
            config,
            watch,
            no_watch,
            otel,
            otel_endpoint,
            service_name,
            health_port,
        } => {
            // Resolve CLI watch override: --watch → Some(true), --no-watch → Some(false), neither → None
            let cli_watch = if watch {
                Some(true)
            } else if no_watch {
                Some(false)
            } else {
                None
            };
            if let Err(e) = run(
                routes,
                config,
                cli_watch,
                otel,
                otel_endpoint,
                service_name,
                health_port,
            )
            .await
            {
                eprintln!("camel-cli run failed: {e}");
                std::process::exit(1);
            }
        }
        Commands::Journal { action } => match action {
            JournalAction::Inspect(args) => {
                commands::journal::run_inspect(args).await;
            }
        },
        Commands::New(args) => {
            commands::new::run_new(args);
        }
    }
}

async fn run(
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
                    log_level: "info".to_string(),
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

    // 2. Build context (also initialises tracing subscriber)
    let mut ctx = camel_config::config::CamelConfig::configure_context(&camel_config)
        .await
        .unwrap_or_else(|e| {
            eprintln!("Failed to configure CamelContext: {e}");
            std::process::exit(1);
        });

    // 3. Determine route patterns
    let patterns: Vec<String> = if let Some(p) = routes_override {
        vec![p]
    } else if !camel_config.routes.is_empty() {
        camel_config.routes.clone()
    } else {
        vec!["routes/*.yaml".to_string()]
    };

    tracing::info!("camel-cli: loading routes from patterns: {:?}", patterns);

    // Define register_bundle! macro — looks up config key in ComponentsConfig::raw
    // Uses UFCS to invoke ComponentBundle methods without requiring trait in scope
    macro_rules! register_bundle {
        ($ctx:expr, $cfg:expr, $Bundle:ty) => {
            if let Some(raw) = $cfg
                .components
                .raw
                .get(<$Bundle as camel_component_api::ComponentBundle>::config_key())
                .cloned()
            {
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
            }
        };
    }

    // Register built-in components (no config needed)
    ctx.register_component(camel_component_timer::TimerComponent::new());
    ctx.register_component(camel_component_log::LogComponent::new());
    ctx.register_component(camel_component_direct::DirectComponent::new());
    ctx.register_component(camel_component_mock::MockComponent::new());
    ctx.register_component(camel_component_controlbus::ControlBusComponent::new());

    // Register HTTP, WS, File, Container (always-on in camel-cli, no feature flag)
    register_bundle!(ctx, camel_config, camel_component_http::HttpBundle);
    register_bundle!(ctx, camel_config, camel_component_ws::WsBundle);
    register_bundle!(ctx, camel_config, camel_component_file::FileBundle);
    register_bundle!(
        ctx,
        camel_config,
        camel_component_container::ContainerBundle
    );

    // Register optional/feature-gated bundles
    #[cfg(feature = "jms")]
    // JMS needs explicit pool capture for shutdown — handled below
    let jms_pool = {
        if let Some(raw) = camel_config.components.raw.get("jms").cloned() {
            match <camel_component_jms::JmsBundle as camel_component_api::ComponentBundle>::from_toml(raw) {
                Ok(bundle) => {
                    let pool = bundle.pool();
                    <camel_component_jms::JmsBundle as camel_component_api::ComponentBundle>::register_all(bundle, &mut ctx);
                    Some(pool)
                }
                Err(e) => {
                    return Err(camel_api::CamelError::Config(format!(
                        "Failed to load jms config: {e}"
                    )));
                }
            }
        } else {
            None
        }
    };
    #[cfg(not(feature = "jms"))]
    let jms_pool: Option<()> = None;

    #[cfg(feature = "kafka")]
    register_bundle!(ctx, camel_config, camel_component_kafka::KafkaBundle);
    #[cfg(feature = "redis")]
    register_bundle!(ctx, camel_config, camel_component_redis::RedisBundle);
    #[cfg(feature = "sql")]
    register_bundle!(ctx, camel_config, camel_component_sql::SqlBundle);

    // 5. Discover and load initial routes
    match camel_dsl::discover_routes(&patterns) {
        Ok(defs) => {
            for def in defs {
                let id = def.route_id().to_string();
                if let Err(e) = ctx.add_route_definition(def).await {
                    tracing::error!("Failed to add route '{}': {}", id, e);
                }
            }
        }
        Err(e) => {
            tracing::error!("Failed to discover routes: {}", e);
            std::process::exit(1);
        }
    }

    // 6. Start context
    if let Err(e) = ctx.start().await {
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
        let drain_timeout = std::time::Duration::from_millis(camel_config.drain_timeout_ms);
        let debounce = std::time::Duration::from_millis(camel_config.watch_debounce_ms);
        let watcher_token = watcher_shutdown.clone();
        tokio::spawn(async move {
            let watch_dirs = camel_core::reload_watcher::resolve_watch_dirs(&watch_patterns);
            let result = camel_core::reload_watcher::watch_and_reload(
                watch_dirs,
                ctrl,
                move || {
                    camel_dsl::discover_routes(&watch_patterns)
                        .map_err(|e| camel_api::CamelError::RouteError(e.to_string()))
                },
                Some(watcher_token),
                drain_timeout,
                debounce,
            )
            .await;
            if let Err(e) = result {
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

    // 9. Wait for Ctrl+C
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to listen for Ctrl+C");

    tracing::info!("camel-cli: shutting down...");
    watcher_shutdown.cancel();
    ctx.stop().await.unwrap_or_else(|e| {
        tracing::error!("Error during shutdown: {}", e);
    });

    #[cfg(feature = "jms")]
    if let Some(pool) = jms_pool
        && let Err(e) = pool.shutdown().await
    {
        tracing::error!("JMS pool shutdown error: {e}");
    }

    tracing::info!("camel-cli: stopped");
    Ok(())
}
