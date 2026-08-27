// Global allocator overrides (feature-gated, binary-only). Library crates
// must never set a global allocator. If both features are enabled, jemalloc
// wins. Soak/migration plan: bd rc-vnm8.
#[cfg(feature = "jemalloc")]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(all(feature = "mimalloc", not(feature = "jemalloc")))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

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
    /// Start a Camel context from YAML route files.
    ///
    /// Trust model: `camel run` executes route scripts, WASM modules, and beans
    /// resolved from the current working directory. Only run from a trusted directory.
    Run {
        /// Glob pattern for route YAML files (overrides Camel.toml routes config)
        #[arg(long, value_name = "GLOB")]
        routes: Option<String>,

        /// Path to Camel.toml config file.
        ///
        /// Also read from `CAMEL_CONFIG_FILE` (rc-6gqy), matching
        /// `from_env_or_default()`; explicit `--config` wins over env.
        #[arg(
            long,
            value_name = "FILE",
            default_value = "Camel.toml",
            env = "CAMEL_CONFIG_FILE"
        )]
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

    /// Lint a route file against the production component catalog.
    Lint(commands::lint::LintArgs),

    /// Run declarative mock tests from *.test.yaml documents.
    ///
    /// Trust model: `camel test` executes route scripts and beans resolved from
    /// the current working directory. Only run from a trusted directory.
    Test(commands::test::TestArgs),

    /// Start Language Server Protocol server over stdio.
    Lsp,
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
        Commands::Lint(args) => {
            commands::lint::run(args).await;
        }
        Commands::Lsp => {
            let code = commands::lsp::run().await;
            std::process::exit(code);
        }
        Commands::Test(args) => {
            // Config validation happens before any document runs and before
            // any report path is touched: an invalid --filter-file pattern
            // is misuse (stderr + exit 2, no report written).
            let config = match commands::test::config_from_args(&args) {
                Ok(config) => config,
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(2);
                }
            };
            let mut out = std::io::stdout().lock();
            let mut err = std::io::stderr().lock();
            let summary = commands::test::run_tests_full(&config, &mut out, &mut err).await;
            std::process::exit(summary.exit_code);
        }
    }
}

#[cfg(test)]
mod run_config_env_tests {
    use super::Cli;
    use clap::CommandFactory as _;

    /// rc-6gqy: `camel run` must honor CAMEL_CONFIG_FILE while keeping the
    /// Camel.toml default when the env var is unset. Both phases live in ONE
    /// test because they mutate shared process env.
    #[test]
    fn run_config_arg_honors_camel_config_file_env() {
        // SAFETY: single-threaded interaction via this one env-touching test
        // in this target; restored below before returning.
        unsafe { std::env::set_var("CAMEL_CONFIG_FILE", "EnvSelected.toml") };

        let parsed = Cli::command()
            .try_get_matches_from(["camel", "run"])
            .expect("run parses without explicit --config");

        // Restore process state before asserting on captured values.
        unsafe { std::env::remove_var("CAMEL_CONFIG_FILE") }

        let run_matches = parsed
            .subcommand_matches("run")
            .expect("run subcommand matched");

        assert_eq!(
            run_matches.get_one::<String>("config").map(String::as_str),
            Some("EnvSelected.toml"),
            "--config must pick up CAMEL_CONFIG_FILE"
        );

        let defaulted_parsed = Cli::command()
            .try_get_matches_from(["camel", "run"])
            .expect("run parses when env unset");
        let defaulted = defaulted_parsed
            .subcommand_matches("run")
            .expect("run subcommand matched");
        assert_eq!(
            defaulted.get_one::<String>("config").map(String::as_str),
            Some("Camel.toml"),
            "default_value must stay intact when CAMEL_CONFIG_FILE is unset"
        );
    }
}
