use camel_cli::commands;
use clap::{Parser, Subcommand};

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

    /// Manage WASM plugins (processors and beans)
    Plugin {
        #[command(subcommand)]
        action: commands::plugin::PluginAction,
    },

    /// Generate OpenAPI document from REST route files
    Openapi {
        #[command(subcommand)]
        action: commands::openapi::OpenapiAction,
    },
}

#[derive(Subcommand)]
enum JournalAction {
    /// Inspect events in a redb journal file.
    Inspect(commands::journal::JournalInspectArgs),
}

#[tokio::main]
async fn main() {
    // Install rustls crypto provider before any TLS operations.
    // The dep graph enables both ring and aws-lc-rs, so explicit selection
    // is required to avoid a runtime panic.
    let _ = rustls::crypto::ring::default_provider().install_default();

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
            if let Err(e) = commands::run::run(
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
                commands::errors::report_cli_failure_and_exit("run", &e);
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
        Commands::Plugin { action } => {
            commands::plugin::run_plugin(action);
        }
        Commands::Openapi { action } => {
            commands::openapi::run(action);
        }
    }
}
