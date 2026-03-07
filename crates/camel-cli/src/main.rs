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
    },
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
        } => {
            // Resolve CLI watch override: --watch → Some(true), --no-watch → Some(false), neither → None
            let cli_watch = if watch {
                Some(true)
            } else if no_watch {
                Some(false)
            } else {
                None
            };
            run(routes, config, cli_watch).await
        }
    }
}

async fn run(routes_override: Option<String>, config_path: String, cli_watch: Option<bool>) {
    // 1. Load config (fall back to empty config with serde defaults if Camel.toml not found)
    let camel_config: camel_config::config::CamelConfig =
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

    // 2. Build context (also initialises tracing subscriber)
    let mut ctx = camel_config::config::CamelConfig::configure_context(&camel_config)
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

    // 4. Register built-in components
    ctx.register_component(camel_component_timer::TimerComponent::new());
    ctx.register_component(camel_component_log::LogComponent::new());
    ctx.register_component(camel_component_direct::DirectComponent::new());
    ctx.register_component(camel_component_file::FileComponent::new());
    ctx.register_component(camel_component_http::HttpComponent::new());
    ctx.register_component(camel_component_mock::MockComponent::new());

    // 5. Discover and load initial routes
    match camel_dsl::discover_routes(&patterns) {
        Ok(defs) => {
            for def in defs {
                let id = def.route_id().to_string();
                if let Err(e) = ctx.add_route_definition(def) {
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
        let ctrl = ctx.route_controller().clone();
        let watch_patterns = patterns.clone();
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

    tracing::info!("camel-cli: stopped");
}
