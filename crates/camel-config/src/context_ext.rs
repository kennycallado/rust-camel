use crate::config::CamelConfig;
use crate::discovery::discover_routes;
use camel_api::CamelError;
use camel_core::CamelContext;
use camel_core::config::OutputFormat;
use camel_core::route::RouteDefinition;
use camel_otel::{OtelConfig, OtelService};
use tracing::Level;
use tracing_subscriber::Layer;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::filter::filter_fn;

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
    ///
    /// Always installs a unified tracing subscriber (Layers 1–3, plus Layer 4
    /// when OTel is enabled). `OtelService`, if present, only manages providers —
    /// it never installs a subscriber.
    pub fn configure_context(config: &CamelConfig) -> Result<CamelContext, CamelError> {
        let otel_enabled = config
            .observability
            .otel
            .as_ref()
            .is_some_and(|o| o.enabled);

        // Build context with optional supervision
        let mut ctx = if let Some(ref sup) = config.supervision {
            CamelContext::with_supervision(sup.clone().into_supervision_config())
        } else {
            CamelContext::new()
        };

        ctx.set_shutdown_timeout(std::time::Duration::from_millis(config.timeout_ms));

        let tracer_config = config.observability.tracer.clone();

        // Always install the unified subscriber — OtelService no longer owns it
        Self::init_tracing_subscriber(&tracer_config, &config.log_level, otel_enabled)?;

        // OtelService manages providers only — subscriber is already installed above
        if otel_enabled {
            let otel_cfg = config.observability.otel.as_ref().unwrap();
            let otel_config = OtelConfig::new(&otel_cfg.endpoint, &otel_cfg.service_name)
                .with_log_level(&otel_cfg.log_level);
            let otel_service = OtelService::new(otel_config);
            ctx = ctx.with_lifecycle(otel_service);
        }

        ctx.set_tracer_config(tracer_config);
        Ok(ctx)
    }

    fn init_tracing_subscriber(
        config: &camel_core::config::TracerConfig,
        log_level: &str,
        otel_active: bool,
    ) -> Result<(), CamelError> {
        let level = parse_log_level(log_level);

        // Layer 1+2: general fmt layer — all log events, stdout, plaintext
        let general_layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stdout)
            .with_filter(tracing_subscriber::filter::LevelFilter::from_level(level))
            .boxed();

        // Layer 3a: camel_tracer stdout output (JSON or Plain)
        let stdout_layer: Option<Box<dyn tracing_subscriber::Layer<_> + Send + Sync>> =
            if config.enabled && config.outputs.stdout.enabled {
                match config.outputs.stdout.format {
                    OutputFormat::Json => Some(
                        tracing_subscriber::fmt::layer()
                            .json()
                            .with_span_events(FmtSpan::CLOSE)
                            .with_target(true)
                            .with_filter(filter_fn(|meta| meta.target() == "camel_tracer"))
                            .boxed(),
                    ),
                    OutputFormat::Plain => Some(
                        tracing_subscriber::fmt::layer()
                            .with_span_events(FmtSpan::CLOSE)
                            .with_target(true)
                            .with_filter(filter_fn(|meta| meta.target() == "camel_tracer"))
                            .boxed(),
                    ),
                }
            } else {
                None
            };

        // Layer 3b: camel_tracer file output (JSON or Plain)
        let file_layer: Option<Box<dyn tracing_subscriber::Layer<_> + Send + Sync>> =
            if config.enabled
                && let Some(ref file_config) = config.outputs.file
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

                match file_config.format {
                    OutputFormat::Json => Some(
                        tracing_subscriber::fmt::layer()
                            .json()
                            .with_span_events(FmtSpan::CLOSE)
                            .with_writer(std::sync::Mutex::new(file))
                            .with_target(true)
                            .with_filter(filter_fn(|meta| meta.target() == "camel_tracer"))
                            .boxed(),
                    ),
                    OutputFormat::Plain => Some(
                        tracing_subscriber::fmt::layer()
                            .with_span_events(FmtSpan::CLOSE)
                            .with_writer(std::sync::Mutex::new(file))
                            .with_target(true)
                            .with_filter(filter_fn(|meta| meta.target() == "camel_tracer"))
                            .boxed(),
                    ),
                }
            } else {
                None
            };

        // Layer 4: tracing-opentelemetry bridge — only when OTel is active
        #[cfg(feature = "otel")]
        let otel_layer: Option<Box<dyn tracing_subscriber::Layer<_> + Send + Sync>> =
            if otel_active {
                Some(
                    tracing_opentelemetry::layer()
                        .with_filter(filter_fn(|meta| meta.target() == "camel_tracer"))
                        .boxed(),
                )
            } else {
                None
            };
        #[cfg(not(feature = "otel"))]
        let _ = otel_active; // suppress unused variable warning

        let mut layers: Vec<Box<dyn tracing_subscriber::Layer<_> + Send + Sync>> = Vec::new();
        layers.push(general_layer);
        if let Some(l) = stdout_layer {
            layers.push(l);
        }
        if let Some(l) = file_layer {
            layers.push(l);
        }
        #[cfg(feature = "otel")]
        if let Some(l) = otel_layer {
            layers.push(l);
        }

        // try_init() silently ignores "already set" error (expected in tests)
        let _ = tracing_subscriber::registry().with(layers).try_init();

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
