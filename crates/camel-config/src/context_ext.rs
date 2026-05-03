use crate::config::{CamelConfig, KubernetesPlatformCamelConfig, PlatformCamelConfig};
#[cfg(feature = "otel")]
use crate::config::{OtelProtocol, OtelSampler};
use crate::discovery::discover_routes_with_threshold;
use camel_api::{CamelError, PlatformService as PlatformServiceTrait};
use camel_core::CamelContext;
use camel_core::OutputFormat;
use camel_core::TracerConfig;
use camel_core::route::RouteDefinition;
#[cfg(feature = "otel")]
use camel_otel::{
    OtelConfig, OtelProtocol as OtelProtocolOtel, OtelSampler as OtelSamplerOtel, OtelService,
};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU8;
#[cfg(feature = "kubernetes")]
use std::time::Duration;
use tracing::Level;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

type HealthState = Arc<Mutex<Vec<(String, Arc<AtomicU8>)>>>;

impl CamelConfig {
    /// Load routes from config file and return them (without adding to context yet)
    /// This allows components to be registered before routes are resolved
    pub fn load_routes(path: &str) -> Result<Vec<RouteDefinition>, CamelError> {
        let config = Self::from_file_with_profile_and_env(path, None)
            .map_err(|e| CamelError::Config(e.to_string()))?;

        if config.routes.is_empty() {
            return Ok(Vec::new());
        }

        discover_routes_with_threshold(&config.routes, config.stream_caching.threshold)
            .map_err(|e| CamelError::Config(e.to_string()))
    }

    /// Create a CamelContext configured from this CamelConfig.
    ///
    /// Always installs a unified tracing subscriber (Layers 1–4).
    /// When OTel is enabled, the OtelService is created *before* the subscriber
    /// so the LoggerProvider can be wired into the tracing bridge for log export.
    pub async fn configure_context(config: &CamelConfig) -> Result<CamelContext, CamelError> {
        let otel_enabled = config
            .observability
            .otel
            .as_ref()
            .is_some_and(|o| o.enabled);

        // Build context with optional supervision + durable runtime journal
        let mut builder = CamelContext::builder();

        if let Some(ref sup) = config.supervision {
            builder = builder.supervision(sup.clone().into_supervision_config());
        }

        if let Some(ref jcfg) = config.runtime_journal {
            let options: camel_core::RedbJournalOptions = jcfg.into();
            let journal =
                camel_core::RedbRuntimeEventJournal::new(jcfg.path.clone(), options).await?;
            let store = camel_core::InMemoryRuntimeStore::default().with_journal(Arc::new(journal));
            builder = builder.runtime_store(store);
        }

        // Platform service wiring
        let platform_service: Arc<dyn PlatformServiceTrait> =
            Self::build_platform_service(&config.platform).await?;
        builder = builder.platform_service(platform_service);

        let mut ctx = builder.build().await?;

        ctx.set_shutdown_timeout(std::time::Duration::from_millis(config.timeout_ms));

        let tracer_config = config.observability.tracer.clone();

        // Create OtelService *before* subscriber so providers are available
        // for the tracing-opentelemetry layer and the log bridge.
        #[cfg(feature = "otel")]
        let otel_service_opt = if otel_enabled {
            let otel_cfg = config.observability.otel.as_ref().unwrap();

            let protocol = match otel_cfg.protocol {
                OtelProtocol::Grpc => OtelProtocolOtel::Grpc,
                OtelProtocol::Http => OtelProtocolOtel::HttpProtobuf,
            };

            let sampler = match &otel_cfg.sampler {
                OtelSampler::AlwaysOn => OtelSamplerOtel::AlwaysOn,
                OtelSampler::AlwaysOff => OtelSamplerOtel::AlwaysOff,
                OtelSampler::Ratio => {
                    let ratio = otel_cfg.sampler_ratio.unwrap_or(1.0).clamp(0.0, 1.0);
                    OtelSamplerOtel::TraceIdRatioBased(ratio)
                }
            };

            let mut otel_config = OtelConfig::new(&otel_cfg.endpoint, &otel_cfg.service_name)
                .with_protocol(protocol)
                .with_sampler(sampler)
                .with_log_level(&otel_cfg.log_level)
                .with_logs_enabled(otel_cfg.logs_enabled)
                .with_metrics_interval_ms(otel_cfg.metrics_interval_ms);

            for (key, value) in &otel_cfg.resource_attrs {
                otel_config = otel_config.with_resource_attr(key, value);
            }

            let mut otel_service = OtelService::new(otel_config);

            // Initialize LoggerProvider early so the log bridge can attach to the subscriber
            let logger_provider = otel_service.init_logger_provider()?;

            // Initialize TracerProvider early so the tracing-opentelemetry layer
            // picks up the global tracer provider at subscriber install time
            otel_service.init_tracer_provider()?;

            Some((otel_service, logger_provider))
        } else {
            None
        };
        #[cfg(not(feature = "otel"))]
        let _otel_service_opt: Option<()> = None;

        // Install subscriber with all layers including OTel log bridge + tracing
        Self::init_tracing_subscriber(
            &tracer_config,
            &config.log_level,
            otel_enabled,
            #[cfg(feature = "otel")]
            otel_service_opt.as_ref().map(|(_, lp)| lp.clone()),
        )?;

        // Register OtelService as lifecycle (start()/stop() manage providers)
        #[cfg(feature = "otel")]
        if let Some((otel_service, _)) = otel_service_opt {
            ctx = ctx.with_lifecycle(otel_service);
        }

        // Enable tracer pipeline when OTel is active (spans + metrics per route)
        let final_tracer_config = effective_tracer_config(tracer_config, otel_enabled);
        ctx = ctx.with_tracer_config(final_tracer_config).await;

        let health_state: HealthState = Arc::new(Mutex::new(Vec::new()));

        let create_checker = || {
            let state = Arc::clone(&health_state);
            Arc::new(move || {
                let guard = state.lock().unwrap();
                let services: Vec<camel_api::ServiceHealth> = guard
                    .iter()
                    .map(|(name, status_arc)| camel_api::ServiceHealth {
                        name: name.clone(),
                        status: match status_arc.load(std::sync::atomic::Ordering::SeqCst) {
                            0 => camel_api::ServiceStatus::Stopped,
                            1 => camel_api::ServiceStatus::Started,
                            _ => camel_api::ServiceStatus::Failed,
                        },
                    })
                    .collect();
                let status = if services
                    .iter()
                    .all(|s| s.status == camel_api::ServiceStatus::Started)
                {
                    camel_api::HealthStatus::Healthy
                } else {
                    camel_api::HealthStatus::Unhealthy
                };
                camel_api::HealthReport {
                    status,
                    services,
                    ..Default::default()
                }
            }) as camel_api::HealthChecker
        };

        if let Some(ref prom) = config.observability.prometheus
            && prom.enabled
        {
            let addr: std::net::SocketAddr = format!("{}:{}", prom.host, prom.port)
                .parse()
                .map_err(|_| {
                    CamelError::Config(format!(
                        "Invalid prometheus bind address: {}:{}",
                        prom.host, prom.port
                    ))
                })?;
            let mut prom_service = camel_prometheus::PrometheusService::new(addr);
            prom_service.set_health_checker(create_checker());
            health_state
                .lock()
                .unwrap()
                .push(("prometheus".to_string(), prom_service.status_arc()));
            ctx = ctx.with_lifecycle(prom_service);
        }

        if let Some(ref health_cfg) = config.observability.health
            && health_cfg.enabled
        {
            let addr: std::net::SocketAddr = format!("{}:{}", health_cfg.host, health_cfg.port)
                .parse()
                .map_err(|_| {
                    CamelError::Config(format!(
                        "Invalid health bind address: {}:{}",
                        health_cfg.host, health_cfg.port
                    ))
                })?;
            let health_server =
                camel_health::HealthServer::new_with_checker(addr, Some(create_checker()));
            health_state
                .lock()
                .unwrap()
                .push(("health".to_string(), health_server.status_arc()));
            ctx = ctx.with_lifecycle(health_server);
        }

        Ok(ctx)
    }

    async fn build_platform_service(
        config: &PlatformCamelConfig,
    ) -> Result<Arc<dyn PlatformServiceTrait>, CamelError> {
        match config {
            PlatformCamelConfig::Noop => Ok(Arc::new(camel_api::NoopPlatformService::default())),
            PlatformCamelConfig::Kubernetes(k8s) => Self::build_kubernetes_platform(k8s).await,
        }
    }

    #[cfg(feature = "kubernetes")]
    async fn build_kubernetes_platform(
        k8s: &KubernetesPlatformCamelConfig,
    ) -> Result<Arc<dyn PlatformServiceTrait>, CamelError> {
        let namespace = k8s
            .namespace
            .clone()
            .or_else(|| std::env::var("POD_NAMESPACE").ok())
            .unwrap_or_else(|| "default".to_string());

        let config = camel_platform_kubernetes::KubernetesPlatformConfig {
            namespace,
            lease_name_prefix: k8s.lease_name_prefix.clone(),
            lease_duration: Duration::from_secs(k8s.lease_duration_secs),
            renew_deadline: Duration::from_secs(k8s.renew_deadline_secs),
            retry_period: Duration::from_secs(k8s.retry_period_secs),
            jitter_factor: k8s.jitter_factor,
        };

        let service = camel_platform_kubernetes::KubernetesPlatformService::try_default(config)
            .await
            .map_err(|e| CamelError::Config(e.to_string()))?;

        Ok(Arc::new(service))
    }

    #[cfg(not(feature = "kubernetes"))]
    async fn build_kubernetes_platform(
        _k8s: &KubernetesPlatformCamelConfig,
    ) -> Result<Arc<dyn PlatformServiceTrait>, CamelError> {
        Err(CamelError::Config(
            "platform.type = \"kubernetes\" requires camel-config feature `kubernetes`".into(),
        ))
    }

    #[cfg(feature = "otel")]
    fn init_tracing_subscriber(
        config: &TracerConfig,
        log_level: &str,
        otel_active: bool,
        logger_provider: Option<camel_otel::SdkLoggerProvider>,
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
        let file_layer: Option<Box<dyn tracing_subscriber::Layer<_> + Send + Sync>> = if config
            .enabled
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

        // Layer 4a: tracing-opentelemetry span bridge — all targets when OTel active
        let otel_span_layer: Option<Box<dyn tracing_subscriber::Layer<_> + Send + Sync>> =
            if otel_active {
                Some(tracing_opentelemetry::layer().boxed())
            } else {
                None
            };

        // Layer 4b: OTel log bridge — exports tracing logs via OTLP (respects log_level)
        let otel_log_layer: Option<Box<dyn tracing_subscriber::Layer<_> + Send + Sync>> =
            if let Some(lp) = logger_provider {
                Some(
                    opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(&lp)
                        .with_filter(tracing_subscriber::filter::LevelFilter::from_level(level))
                        .boxed(),
                )
            } else {
                None
            };

        let mut layers: Vec<Box<dyn tracing_subscriber::Layer<_> + Send + Sync>> = Vec::new();
        layers.push(general_layer);
        if let Some(l) = stdout_layer {
            layers.push(l);
        }
        if let Some(l) = file_layer {
            layers.push(l);
        }
        if let Some(l) = otel_span_layer {
            layers.push(l);
        }
        if let Some(l) = otel_log_layer {
            layers.push(l);
        }

        let result = tracing_subscriber::registry().with(layers).try_init();
        if result.is_err() {
            eprintln!(
                "WARNING: OTel tracing subscriber not installed — a global subscriber \
                 was already set. OTel span/log bridge is inactive. \
                 Ensure no other crate calls tracing_subscriber::init() before camel."
            );
        }

        Ok(())
    }

    #[cfg(not(feature = "otel"))]
    fn init_tracing_subscriber(
        config: &TracerConfig,
        log_level: &str,
        _otel_active: bool,
    ) -> Result<(), CamelError> {
        let level = parse_log_level(log_level);

        let general_layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stdout)
            .with_filter(tracing_subscriber::filter::LevelFilter::from_level(level))
            .boxed();

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

        let file_layer: Option<Box<dyn tracing_subscriber::Layer<_> + Send + Sync>> = if config
            .enabled
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

        let mut layers: Vec<Box<dyn tracing_subscriber::Layer<_> + Send + Sync>> = Vec::new();
        layers.push(general_layer);
        if let Some(l) = stdout_layer {
            layers.push(l);
        }
        if let Some(l) = file_layer {
            layers.push(l);
        }

        let result = tracing_subscriber::registry().with(layers).try_init();
        if result.is_err() {
            eprintln!(
                "WARNING: Tracing subscriber not installed — a global subscriber \
                 was already set. Trace output may be incomplete."
            );
        }

        Ok(())
    }
}

fn effective_tracer_config(mut tracer_config: TracerConfig, otel_enabled: bool) -> TracerConfig {
    if otel_enabled {
        tracer_config.enabled = true;
    }
    tracer_config
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

#[cfg(test)]
mod configure_context_smoke_tests {
    use super::*;
    use config::FileFormat;

    #[tokio::test]
    async fn test_configure_context_empty_config() {
        let cfg = config::Config::builder()
            .add_source(config::File::from_str("", FileFormat::Toml))
            .build()
            .unwrap()
            .try_deserialize::<CamelConfig>()
            .unwrap();
        // configure_context compiles and runs without error on empty config
        let result = CamelConfig::configure_context(&cfg).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn journal_config_deserializes_with_defaults() {
        let cfg = config::Config::builder()
            .add_source(config::File::from_str(
                r#"
                [runtime_journal]
                path = "/tmp/test.db"
                "#,
                FileFormat::Toml,
            ))
            .build()
            .unwrap()
            .try_deserialize::<CamelConfig>()
            .unwrap();

        let jcfg = cfg.runtime_journal.unwrap();
        assert_eq!(jcfg.path, std::path::PathBuf::from("/tmp/test.db"));
        assert_eq!(jcfg.durability, crate::config::JournalDurability::Immediate);
        assert_eq!(jcfg.compaction_threshold_events, 10_000);
    }

    #[tokio::test]
    async fn journal_config_deserializes_durability_eventual() {
        let cfg = config::Config::builder()
            .add_source(config::File::from_str(
                r#"
                [runtime_journal]
                path = "/tmp/test.db"
                durability = "eventual"
                "#,
                FileFormat::Toml,
            ))
            .build()
            .unwrap()
            .try_deserialize::<CamelConfig>()
            .unwrap();

        let jcfg = cfg.runtime_journal.unwrap();
        assert_eq!(jcfg.durability, crate::config::JournalDurability::Eventual);
    }

    #[tokio::test]
    async fn configure_context_without_journal_creates_ephemeral_context() {
        let cfg = config::Config::builder()
            .add_source(config::File::from_str("", FileFormat::Toml))
            .build()
            .unwrap()
            .try_deserialize::<CamelConfig>()
            .unwrap();

        assert!(cfg.runtime_journal.is_none());
        let result = CamelConfig::configure_context(&cfg).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn configure_context_with_supervision_and_journal_creates_context() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("sup-journal.db");
        let path_str = db_path.to_str().unwrap();

        let toml_str = format!(
            r#"
            [supervision]
            max_attempts = 5
            initial_delay_ms = 1000
            backoff_multiplier = 2.0
            max_delay_ms = 60000

            [runtime_journal]
            path = "{}"
            "#,
            path_str
        );

        let cfg = config::Config::builder()
            .add_source(config::File::from_str(&toml_str, FileFormat::Toml))
            .build()
            .unwrap()
            .try_deserialize::<CamelConfig>()
            .unwrap();

        assert!(cfg.supervision.is_some());
        assert!(cfg.runtime_journal.is_some());

        let result = CamelConfig::configure_context(&cfg).await;
        assert!(
            result.is_ok(),
            "supervision+journal context creation must succeed: {:?}",
            result.err()
        );
        assert!(
            db_path.exists(),
            "redb journal file must be created on disk"
        );
    }

    #[tokio::test]
    async fn journal_config_without_path_fails_deserialization() {
        let result = config::Config::builder()
            .add_source(config::File::from_str(
                r#"
                [runtime_journal]
                durability = "eventual"
                "#,
                FileFormat::Toml,
            ))
            .build()
            .unwrap()
            .try_deserialize::<CamelConfig>();

        assert!(
            result.is_err(),
            "JournalConfig without 'path' field must fail deserialization"
        );
    }

    #[test]
    fn parse_log_level_covers_all_branches() {
        assert_eq!(parse_log_level("trace"), Level::TRACE);
        assert_eq!(parse_log_level("debug"), Level::DEBUG);
        assert_eq!(parse_log_level("info"), Level::INFO);
        assert_eq!(parse_log_level("warn"), Level::WARN);
        assert_eq!(parse_log_level("warning"), Level::WARN);
        assert_eq!(parse_log_level("error"), Level::ERROR);
        assert_eq!(parse_log_level("unknown"), Level::INFO);
    }

    #[test]
    fn load_routes_returns_empty_when_routes_not_declared() {
        use std::io::Write;

        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(
            br#"
log_level = "info"
"#,
        )
        .unwrap();

        let routes = CamelConfig::load_routes(file.path().to_str().unwrap()).unwrap();
        assert!(routes.is_empty());
    }

    #[test]
    fn load_routes_propagates_discovery_error_for_invalid_glob() {
        use std::io::Write;

        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(
            br#"
routes = ["["]
"#,
        )
        .unwrap();

        let err = CamelConfig::load_routes(file.path().to_str().unwrap())
            .err()
            .expect("invalid glob should error");
        assert!(matches!(err, CamelError::Config(_)));
    }

    #[test]
    fn load_routes_propagates_config_error_for_missing_file() {
        let err = CamelConfig::load_routes("/definitely/missing/camel-config.toml")
            .err()
            .expect("missing file should error");
        assert!(matches!(err, CamelError::Config(_)));
    }

    #[tokio::test]
    async fn configure_context_with_noop_platform_succeeds() {
        let cfg = config::Config::builder()
            .add_source(config::File::from_str("", FileFormat::Toml))
            .build()
            .unwrap()
            .try_deserialize::<CamelConfig>()
            .unwrap();

        let result = CamelConfig::configure_context(&cfg).await;
        assert!(result.is_ok());
    }

    #[cfg(not(feature = "kubernetes"))]
    #[tokio::test]
    async fn configure_context_rejects_kubernetes_without_feature() {
        let cfg = config::Config::builder()
            .add_source(config::File::from_str(
                r#"
[platform]
type = "kubernetes"
"#,
                FileFormat::Toml,
            ))
            .build()
            .unwrap()
            .try_deserialize::<CamelConfig>()
            .unwrap();

        let err = CamelConfig::configure_context(&cfg).await.err();
        assert!(
            err.is_some(),
            "kubernetes platform without feature should fail"
        );
        let msg = err.unwrap().to_string();
        assert!(
            msg.contains("kubernetes"),
            "error should mention kubernetes feature: {msg}"
        );
    }

    #[test]
    fn effective_tracer_config_enables_tracing_when_otel_is_enabled() {
        let cfg = TracerConfig {
            enabled: false,
            ..Default::default()
        };

        let out = effective_tracer_config(cfg, true);
        assert!(out.enabled);
    }

    #[test]
    fn effective_tracer_config_preserves_tracing_when_otel_is_disabled() {
        let cfg = TracerConfig {
            enabled: false,
            ..Default::default()
        };

        let out = effective_tracer_config(cfg, false);
        assert!(!out.enabled);
    }

    #[test]
    fn parse_log_level_is_case_insensitive_and_defaults_to_info() {
        assert_eq!(parse_log_level("TRACE"), Level::TRACE);
        assert_eq!(parse_log_level("DeBuG"), Level::DEBUG);
        assert_eq!(parse_log_level("WARNING"), Level::WARN);
        assert_eq!(parse_log_level(""), Level::INFO);
    }

    #[tokio::test]
    async fn build_platform_service_noop_returns_ok() {
        let result = CamelConfig::build_platform_service(&PlatformCamelConfig::Noop).await;
        assert!(result.is_ok());
    }

    #[cfg(not(feature = "kubernetes"))]
    #[tokio::test]
    async fn build_kubernetes_platform_without_feature_returns_error() {
        let k8s = KubernetesPlatformCamelConfig::default();
        let err = CamelConfig::build_kubernetes_platform(&k8s)
            .await
            .err()
            .unwrap();
        assert!(
            err.to_string()
                .contains("requires camel-config feature `kubernetes`")
        );
    }

    #[tokio::test]
    async fn configure_context_rejects_invalid_prometheus_bind_address() {
        let cfg = config::Config::builder()
            .add_source(config::File::from_str(
                r#"
[observability.prometheus]
enabled = true
host = "bad host"
port = 9000
"#,
                FileFormat::Toml,
            ))
            .build()
            .unwrap()
            .try_deserialize::<CamelConfig>()
            .unwrap();

        let err = CamelConfig::configure_context(&cfg).await.err().unwrap();
        let msg = err.to_string();
        assert!(msg.contains("Invalid prometheus bind address"));
    }

    #[tokio::test]
    async fn configure_context_rejects_invalid_health_bind_address() {
        let cfg = config::Config::builder()
            .add_source(config::File::from_str(
                r#"
[observability.health]
enabled = true
host = "bad host"
port = 8080
"#,
                FileFormat::Toml,
            ))
            .build()
            .unwrap()
            .try_deserialize::<CamelConfig>()
            .unwrap();

        let err = CamelConfig::configure_context(&cfg).await.err().unwrap();
        let msg = err.to_string();
        assert!(msg.contains("Invalid health bind address"));
    }
}
