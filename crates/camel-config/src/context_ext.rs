use crate::config::CamelConfig;
use crate::discovery::discover_routes;
use camel_api::CamelError;
use camel_core::CamelContext;
use camel_core::route::RouteDefinition;
use tracing::Level;
use tracing_subscriber::Layer;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

impl CamelConfig {
    /// Load routes from config file and return them (without adding to context yet)
    /// This allows components to be registered before routes are resolved
    pub fn load_routes(path: &str) -> Result<Vec<RouteDefinition>, CamelError> {
        let config = Self::from_file_with_profile_and_env(path, None)
            .map_err(|e| CamelError::Config(e.to_string()))?;

        if config.routes.is_empty() {
            return Ok(Vec::new());
        }

        discover_routes(&config.routes).map_err(|e| CamelError::Config(e.to_string()))
    }

    /// Create a CamelContext configured from this CamelConfig.
    /// Initializes tracing subscriber if tracing is enabled.
    pub fn configure_context(config: &CamelConfig) -> Result<CamelContext, CamelError> {
        // Task 1: Use supervision when configured
        let mut ctx = if let Some(ref sup) = config.supervision {
            CamelContext::with_supervision(sup.clone().into_supervision_config())
        } else {
            CamelContext::new()
        };

        // Task 3: Apply shutdown timeout from config
        ctx.set_shutdown_timeout(std::time::Duration::from_millis(config.timeout_ms));

        let tracer_config = config.observability.tracer.clone();

        // Task 2: Always initialize tracing subscriber (general log_level layer + optional tracer layer)
        Self::init_tracing_subscriber(&tracer_config, &config.log_level)?;

        ctx.set_tracer_config(tracer_config);
        Ok(ctx)
    }

    fn init_tracing_subscriber(
        config: &camel_core::config::TracerConfig,
        log_level: &str,
    ) -> Result<(), CamelError> {
        let mut layers: Vec<Box<dyn tracing_subscriber::Layer<_> + Send + Sync>> = Vec::new();

        // Task 2: General fmt layer using log_level from config — always added
        let level = parse_log_level(log_level);
        let general_layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stdout)
            .with_filter(tracing_subscriber::filter::LevelFilter::from_level(level))
            .boxed();
        layers.push(general_layer);

        // Critical 3: Filter spans to only capture those with target "camel_tracer"
        if config.enabled && config.outputs.stdout.enabled {
            let layer = tracing_subscriber::fmt::layer()
                .json()
                .with_span_events(FmtSpan::CLOSE)
                .with_target(true)
                .with_filter(tracing_subscriber::filter::filter_fn(|meta| {
                    meta.target() == "camel_tracer"
                }))
                .boxed();
            layers.push(layer);
        }

        // Critical 4: Add file output support when configured
        if config.enabled {
            if let Some(ref file_config) = config.outputs.file
                && file_config.enabled
            {
                let file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&file_config.path)
                    .map_err(|e| {
                        CamelError::Config(format!(
                            "Failed to open trace file '{}': {}",
                            file_config.path, e
                        ))
                    })?;
                let layer = tracing_subscriber::fmt::layer()
                    .json()
                    .with_span_events(FmtSpan::CLOSE)
                    .with_writer(std::sync::Mutex::new(file))
                    .with_target(true)
                    .with_filter(tracing_subscriber::filter::filter_fn(|meta| {
                        meta.target() == "camel_tracer"
                    }))
                    .boxed();
                layers.push(layer);
            }
        }

        // Critical 5: Initialize subscriber with proper error handling
        // Ignore "already set" error (expected in tests) but propagate file creation errors
        let registry = tracing_subscriber::registry().with(layers);
        // Ignore error if subscriber already set (OK in tests), but this won't mask file errors
        let _ = registry.try_init();

        Ok(())
    }
}

/// Parse a log level string, defaulting to INFO on failure.
fn parse_log_level(s: &str) -> Level {
    match s.to_lowercase().as_str() {
        "trace" => Level::TRACE,
        "debug" => Level::DEBUG,
        "info" => Level::INFO,
        "warn" | "warning" => Level::WARN,
        "error" => Level::ERROR,
        _ => Level::INFO,
    }
}
