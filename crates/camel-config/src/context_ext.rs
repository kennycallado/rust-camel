use crate::config::{CamelConfig, KubernetesPlatformCamelConfig, PlatformCamelConfig};
#[cfg(feature = "otel")]
use crate::config::{OtelProtocol, OtelSampler};
use crate::discovery::discover_routes_with_threshold;
use async_trait::async_trait;
use camel_api::{
    CamelError, HealthReport, HealthSource, HealthStatus, PlatformService as PlatformServiceTrait,
    ServiceHealth, ServiceStatus,
};
use camel_core::CamelContext;
use camel_core::OutputFormat;
use camel_core::TracerConfig;
use camel_core::health_registry::HealthCheckRegistry;
use camel_core::route::RouteDefinition;
#[cfg(feature = "otel")]
use camel_otel::{
    OtelConfig, OtelProtocol as OtelProtocolOtel, OtelSampler as OtelSamplerOtel, OtelService,
};
use std::sync::Arc;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

type HealthState = Arc<Mutex<Vec<(String, Arc<AtomicU8>)>>>;

struct ContextHealthSource {
    health_registry: Arc<HealthCheckRegistry>,
    services: HealthState,
}

#[async_trait]
impl HealthSource for ContextHealthSource {
    async fn liveness(&self) -> HealthStatus {
        let guard = self.services.lock().await;
        let has_failed = guard.iter().any(|(_, s)| s.load(Ordering::SeqCst) == 2);
        if has_failed {
            HealthStatus::Unhealthy
        } else {
            HealthStatus::Healthy
        }
    }

    async fn readiness(&self) -> HealthStatus {
        let guard = self.services.lock().await;
        let has_failed = guard.iter().any(|(_, s)| s.load(Ordering::SeqCst) == 2);
        drop(guard);
        if has_failed {
            return HealthStatus::Unhealthy;
        }
        self.health_registry.check_all().await.status
    }

    async fn health_report(&self) -> HealthReport {
        let mut report = self.health_registry.check_all().await;
        let mut worst = report.status;
        let guard = self.services.lock().await;
        for (name, state) in guard.iter() {
            let svc_status = match state.load(Ordering::SeqCst) {
                1 => ServiceStatus::Started,
                0 => ServiceStatus::Stopped,
                _ => ServiceStatus::Failed,
            };
            let health = match svc_status {
                ServiceStatus::Started => HealthStatus::Healthy,
                ServiceStatus::Stopped => HealthStatus::Degraded,
                ServiceStatus::Failed => HealthStatus::Unhealthy,
            };
            if matches!(worst, HealthStatus::Healthy)
                && matches!(health, HealthStatus::Degraded | HealthStatus::Unhealthy)
            {
                worst = health;
            }
            if matches!(worst, HealthStatus::Degraded) && matches!(health, HealthStatus::Unhealthy)
            {
                worst = health;
            }
            report.services.push(ServiceHealth {
                name: name.clone(),
                status: svc_status,
                message: None,
            });
        }
        drop(guard);
        report.status = worst;
        report
    }

    async fn startup(&self) -> HealthStatus {
        HealthStatus::Healthy
    }
}

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

    /// Load routes from config file with a security compile context, returning them
    /// without adding to context yet. This allows permission evaluator registries
    /// to be wired before route resolution.
    pub fn load_routes_with_security(
        path: &str,
        security_ctx: camel_dsl::SecurityCompileContext,
    ) -> Result<Vec<RouteDefinition>, CamelError> {
        let config = Self::from_file_with_profile_and_env(path, None)
            .map_err(|e| CamelError::Config(e.to_string()))?;
        if config.routes.is_empty() {
            return Ok(Vec::new());
        }
        crate::discovery::discover_routes_with_threshold_and_security(
            &config.routes,
            config.stream_caching.threshold,
            security_ctx,
        )
        .map_err(|e| CamelError::Config(e.to_string()))
    }

    /// Create a CamelContext configured from this CamelConfig.
    ///
    /// Always installs a unified tracing subscriber (Layers 1–4).
    /// When OTel is enabled, the OtelService is created *before* the subscriber
    /// so the LoggerProvider can be wired into the tracing bridge for log export.
    pub async fn configure_context(config: &CamelConfig) -> Result<CamelContext, CamelError> {
        // TODO(CONFIG-004): config.watch flag is parsed, but hot-reload wiring is not implemented here yet.
        Self::configure_context_with_beans(config, None).await
    }

    pub async fn configure_context_with_beans(
        config: &CamelConfig,
        beans: Option<Arc<std::sync::Mutex<camel_bean::BeanRegistry>>>,
    ) -> Result<CamelContext, CamelError> {
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

        let health_registry = Arc::new({
            let mut reg = HealthCheckRegistry::new(Duration::from_secs(5));
            if let Some(ttl_ms) = config
                .observability
                .health
                .as_ref()
                .and_then(|h| h.forced_ttl_ms)
            {
                reg = reg.with_forced_ttl(Duration::from_millis(ttl_ms));
            }
            reg
        });
        builder = builder.health_registry(Arc::clone(&health_registry));

        let health_state: HealthState = Arc::new(Mutex::new(Vec::new()));
        let health_source: Arc<dyn HealthSource> = Arc::new(ContextHealthSource {
            health_registry: Arc::clone(&health_registry),
            services: Arc::clone(&health_state),
        });

        let k8s_health_source = if matches!(config.platform, PlatformCamelConfig::Kubernetes(_))
            && config
                .observability
                .health
                .as_ref()
                .is_some_and(|h| h.enabled)
        {
            Some(Arc::clone(&health_source))
        } else {
            None
        };

        // Platform service wiring
        let platform_service: Arc<dyn PlatformServiceTrait> =
            Self::build_platform_service(&config.platform, k8s_health_source).await?;
        builder = builder.platform_service(platform_service);

        // Thread language limits from Camel.toml [languages.*.limits] into the registry,
        // so Rhai/JS language engines respect user-configured resource caps.
        let languages = camel_core::languages_from_config(&config.languages);
        builder = builder.languages(languages);

        if let Some(beans) = beans {
            builder = builder.beans(beans);
        }

        let mut ctx = builder.build().await?;

        ctx.set_shutdown_timeout(std::time::Duration::from_millis(config.timeout_ms));

        let tracer_config = config.observability.tracer.clone();

        // Create OtelService *before* subscriber so providers are available
        // for the tracing-opentelemetry layer and the log bridge.
        #[cfg(feature = "otel")]
        let otel_service_opt = if otel_enabled {
            let otel_cfg = config.observability.otel.as_ref().unwrap(); // allow-unwrap

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
            prom_service.set_health_source(Arc::clone(&health_source));
            health_state
                .lock()
                .await
                .push(("prometheus".to_string(), prom_service.status_arc()));
            ctx = ctx.with_lifecycle(prom_service);
        }

        if let Some(ref health_cfg) = config.observability.health
            && health_cfg.enabled
        {
            if health_cfg.handler_timeout_ms == 0 {
                return Err(CamelError::Config(
                    "health.handler_timeout_ms must be > 0".to_string(),
                ));
            }
            let addr: std::net::SocketAddr = format!("{}:{}", health_cfg.host, health_cfg.port)
                .parse()
                .map_err(|_| {
                    CamelError::Config(format!(
                        "Invalid health bind address: {}:{}",
                        health_cfg.host, health_cfg.port
                    ))
                })?;
            let mut health_server = camel_health::HealthServer::new(addr);
            health_server.set_health_source(Arc::clone(&health_source));
            health_server.set_handler_timeout(std::time::Duration::from_millis(
                health_cfg.handler_timeout_ms,
            ));
            health_state
                .lock()
                .await
                .push(("health".to_string(), health_server.status_arc()));
            ctx = ctx.with_lifecycle(health_server);
        }

        Ok(ctx)
    }

    async fn build_platform_service(
        config: &PlatformCamelConfig,
        health_source: Option<Arc<dyn HealthSource>>,
    ) -> Result<Arc<dyn PlatformServiceTrait>, CamelError> {
        match config {
            PlatformCamelConfig::Noop => Ok(Arc::new(camel_api::NoopPlatformService::default())),
            PlatformCamelConfig::Kubernetes(k8s) => {
                Self::build_kubernetes_platform(k8s, health_source).await
            }
        }
    }

    #[cfg(feature = "kubernetes")]
    async fn build_kubernetes_platform(
        k8s: &KubernetesPlatformCamelConfig,
        health_source: Option<Arc<dyn HealthSource>>,
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

        let mut service = camel_platform_kubernetes::KubernetesPlatformService::try_default(config)
            .await
            .map_err(|e| CamelError::Config(e.to_string()))?;

        if let Some(source) = health_source {
            service = service.with_health_source(source);
        }

        Ok(Arc::new(service))
    }

    #[cfg(not(feature = "kubernetes"))]
    async fn build_kubernetes_platform(
        _k8s: &KubernetesPlatformCamelConfig,
        _health_source: Option<Arc<dyn HealthSource>>,
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
        let rust_log = std::env::var("RUST_LOG").ok();
        let env_filter = crate::filter::build_env_filter(log_level, rust_log.as_deref())?;

        // Layer 1+2: general fmt layer — all log events, stdout, plaintext
        let general_layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stdout)
            .with_filter(env_filter.clone())
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
                        .with_filter(env_filter.clone())
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
            tracing::warn!(
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
        let rust_log = std::env::var("RUST_LOG").ok();
        let env_filter = crate::filter::build_env_filter(log_level, rust_log.as_deref())?;

        let general_layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stdout)
            .with_filter(env_filter)
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
            tracing::warn!(
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

    #[test]
    fn load_routes_with_explicit_json_pattern() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let json_file = dir.path().join("route.json");
        std::fs::write(
            &json_file,
            r#"{"routes":[{"id":"config-json-route","from":"timer:tick","steps":[{"to":"log:info"}]}]}"#,
        )
        .unwrap();

        let pattern = dir.path().join("*.json").to_string_lossy().to_string();
        let mut cfg_file = tempfile::NamedTempFile::new().unwrap();
        write!(
            cfg_file,
            r#"
routes = ["{}"]
"#,
            pattern.replace('\\', "\\\\")
        )
        .unwrap();

        let routes = CamelConfig::load_routes(cfg_file.path().to_str().unwrap()).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id(), "config-json-route");
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

    #[tokio::test]
    async fn build_platform_service_noop_returns_ok() {
        let result = CamelConfig::build_platform_service(&PlatformCamelConfig::Noop, None).await;
        assert!(result.is_ok());
    }

    #[cfg(not(feature = "kubernetes"))]
    #[tokio::test]
    async fn build_kubernetes_platform_without_feature_returns_error() {
        let k8s = KubernetesPlatformCamelConfig::default();
        let err = CamelConfig::build_kubernetes_platform(&k8s, None)
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

    #[tokio::test]
    async fn configure_context_with_valid_prometheus_succeeds() {
        let cfg = config::Config::builder()
            .add_source(config::File::from_str(
                r#"
[observability.prometheus]
enabled = true
host = "127.0.0.1"
port = 29090
"#,
                FileFormat::Toml,
            ))
            .build()
            .unwrap()
            .try_deserialize::<CamelConfig>()
            .unwrap();

        let result = CamelConfig::configure_context(&cfg).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn configure_context_with_valid_health_succeeds() {
        let cfg = config::Config::builder()
            .add_source(config::File::from_str(
                r#"
[observability.health]
enabled = true
host = "127.0.0.1"
port = 28081
"#,
                FileFormat::Toml,
            ))
            .build()
            .unwrap()
            .try_deserialize::<CamelConfig>()
            .unwrap();

        let result = CamelConfig::configure_context(&cfg).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn configure_context_with_prometheus_and_health_succeeds() {
        let cfg = config::Config::builder()
            .add_source(config::File::from_str(
                r#"
[observability.prometheus]
enabled = true
host = "127.0.0.1"
port = 29091

[observability.health]
enabled = true
host = "127.0.0.1"
port = 28082
"#,
                FileFormat::Toml,
            ))
            .build()
            .unwrap()
            .try_deserialize::<CamelConfig>()
            .unwrap();

        let result = CamelConfig::configure_context(&cfg).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn configure_context_with_beans_succeeds() {
        let cfg = config::Config::builder()
            .add_source(config::File::from_str("", FileFormat::Toml))
            .build()
            .unwrap()
            .try_deserialize::<CamelConfig>()
            .unwrap();

        let beans = Arc::new(std::sync::Mutex::new(camel_bean::BeanRegistry::default()));
        let result = CamelConfig::configure_context_with_beans(&cfg, Some(beans)).await;
        assert!(result.is_ok());
    }

    #[test]
    fn load_routes_with_yaml_route_file() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let yaml_file = dir.path().join("route.yaml");
        std::fs::write(
            &yaml_file,
            r#"
routes:
  - id: yaml-route
    from: timer:tick
    steps:
      - to: log:info
"#,
        )
        .unwrap();

        let pattern = dir.path().join("*.yaml").to_string_lossy().to_string();
        let mut cfg_file = tempfile::NamedTempFile::new().unwrap();
        write!(
            cfg_file,
            r#"
routes = ["{}"]
"#,
            pattern.replace('\\', "\\\\")
        )
        .unwrap();

        let routes = CamelConfig::load_routes(cfg_file.path().to_str().unwrap()).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id(), "yaml-route");
    }

    #[test]
    fn load_routes_with_yml_extension() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let yml_file = dir.path().join("route.yml");
        std::fs::write(
            &yml_file,
            r#"
routes:
  - id: yml-route
    from: timer:tick
    steps:
      - to: log:info
"#,
        )
        .unwrap();

        let pattern = dir.path().join("*.yml").to_string_lossy().to_string();
        let mut cfg_file = tempfile::NamedTempFile::new().unwrap();
        write!(
            cfg_file,
            r#"
routes = ["{}"]
"#,
            pattern.replace('\\', "\\\\")
        )
        .unwrap();

        let routes = CamelConfig::load_routes(cfg_file.path().to_str().unwrap()).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id(), "yml-route");
    }

    #[test]
    fn load_routes_with_multiple_route_files() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();

        let json_file = dir.path().join("route1.json");
        std::fs::write(
            &json_file,
            r#"{"routes":[{"id":"multi-json","from":"timer:tick","steps":[{"to":"log:info"}]}]}"#,
        )
        .unwrap();

        let yaml_file = dir.path().join("route2.yaml");
        std::fs::write(
            &yaml_file,
            r#"
routes:
  - id: multi-yaml
    from: timer:tick
    steps:
      - to: log:info
"#,
        )
        .unwrap();

        let json_pattern = dir.path().join("*.json").to_string_lossy().to_string();
        let yaml_pattern = dir.path().join("*.yaml").to_string_lossy().to_string();
        let mut cfg_file = tempfile::NamedTempFile::new().unwrap();
        write!(
            cfg_file,
            r#"
routes = ["{}", "{}"]
"#,
            json_pattern.replace('\\', "\\\\"),
            yaml_pattern.replace('\\', "\\\\")
        )
        .unwrap();

        let routes = CamelConfig::load_routes(cfg_file.path().to_str().unwrap()).unwrap();
        assert_eq!(routes.len(), 2);
    }

    #[test]
    fn load_routes_with_glob_matching_no_files_returns_empty() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let pattern = dir.path().join("*.yaml").to_string_lossy().to_string();
        let mut cfg_file = tempfile::NamedTempFile::new().unwrap();
        write!(
            cfg_file,
            r#"
routes = ["{}"]
"#,
            pattern.replace('\\', "\\\\")
        )
        .unwrap();

        let routes = CamelConfig::load_routes(cfg_file.path().to_str().unwrap()).unwrap();
        assert!(routes.is_empty());
    }

    #[tokio::test]
    async fn configure_context_with_tracer_file_output_json_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let trace_path = dir.path().join("trace.json");

        let cfg = config::Config::builder()
            .add_source(config::File::from_str(
                &format!(
                    r#"
[observability.tracer]
enabled = true

[observability.tracer.outputs.file]
enabled = true
path = "{}"
format = "json"
"#,
                    trace_path.to_str().unwrap()
                ),
                FileFormat::Toml,
            ))
            .build()
            .unwrap()
            .try_deserialize::<CamelConfig>()
            .unwrap();

        let result = CamelConfig::configure_context(&cfg).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn configure_context_with_tracer_file_output_plain_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let trace_path = dir.path().join("trace.log");

        let cfg = config::Config::builder()
            .add_source(config::File::from_str(
                &format!(
                    r#"
[observability.tracer]
enabled = true

[observability.tracer.outputs.file]
enabled = true
path = "{}"
format = "plain"
"#,
                    trace_path.to_str().unwrap()
                ),
                FileFormat::Toml,
            ))
            .build()
            .unwrap()
            .try_deserialize::<CamelConfig>()
            .unwrap();

        let result = CamelConfig::configure_context(&cfg).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn configure_context_with_tracer_stdout_plain_succeeds() {
        let cfg = config::Config::builder()
            .add_source(config::File::from_str(
                r#"
[observability.tracer]
enabled = true

[observability.tracer.outputs.stdout]
enabled = true
format = "plain"
"#,
                FileFormat::Toml,
            ))
            .build()
            .unwrap()
            .try_deserialize::<CamelConfig>()
            .unwrap();

        let result = CamelConfig::configure_context(&cfg).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn configure_context_with_tracer_file_and_stdout_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let trace_path = dir.path().join("trace-both.json");

        let cfg = config::Config::builder()
            .add_source(config::File::from_str(
                &format!(
                    r#"
[observability.tracer]
enabled = true

[observability.tracer.outputs.stdout]
enabled = true
format = "json"

[observability.tracer.outputs.file]
enabled = true
path = "{}"
format = "plain"
"#,
                    trace_path.to_str().unwrap()
                ),
                FileFormat::Toml,
            ))
            .build()
            .unwrap()
            .try_deserialize::<CamelConfig>()
            .unwrap();

        let result = CamelConfig::configure_context(&cfg).await;
        assert!(result.is_ok());
    }

    #[cfg(feature = "otel")]
    #[tokio::test]
    async fn configure_context_with_otel_grpc_protocol_succeeds() {
        let cfg = config::Config::builder()
            .add_source(config::File::from_str(
                r#"
[observability.otel]
enabled = true
endpoint = "http://localhost:4317"
service_name = "test-service"
protocol = "grpc"
sampler = "always_on"
"#,
                FileFormat::Toml,
            ))
            .build()
            .unwrap()
            .try_deserialize::<CamelConfig>()
            .unwrap();

        let result = CamelConfig::configure_context(&cfg).await;
        assert!(result.is_ok());
    }

    #[cfg(feature = "otel")]
    #[tokio::test]
    async fn configure_context_with_otel_http_protocol_succeeds() {
        let cfg = config::Config::builder()
            .add_source(config::File::from_str(
                r#"
[observability.otel]
enabled = true
endpoint = "http://localhost:4318"
service_name = "test-service"
protocol = "http"
sampler = "always_off"
"#,
                FileFormat::Toml,
            ))
            .build()
            .unwrap()
            .try_deserialize::<CamelConfig>()
            .unwrap();

        let result = CamelConfig::configure_context(&cfg).await;
        assert!(result.is_ok());
    }

    #[cfg(feature = "otel")]
    #[tokio::test]
    async fn configure_context_with_otel_sampler_ratio_succeeds() {
        let cfg = config::Config::builder()
            .add_source(config::File::from_str(
                r#"
[observability.otel]
enabled = true
endpoint = "http://localhost:4317"
service_name = "test-service"
sampler = "ratio"
sampler_ratio = 0.5
"#,
                FileFormat::Toml,
            ))
            .build()
            .unwrap()
            .try_deserialize::<CamelConfig>()
            .unwrap();

        let result = CamelConfig::configure_context(&cfg).await;
        assert!(result.is_ok());
    }

    #[cfg(feature = "otel")]
    #[tokio::test]
    async fn configure_context_with_otel_resource_attrs_succeeds() {
        let cfg = config::Config::builder()
            .add_source(config::File::from_str(
                r#"
[observability.otel]
enabled = true
endpoint = "http://localhost:4317"
service_name = "test-service"
sampler = "always_on"

[observability.otel.resource_attrs]
"service.version" = "1.0.0"
"deployment.environment" = "test"
"#,
                FileFormat::Toml,
            ))
            .build()
            .unwrap()
            .try_deserialize::<CamelConfig>()
            .unwrap();

        let result = CamelConfig::configure_context(&cfg).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn configure_context_with_tracer_file_open_error_fails() {
        let cfg = config::Config::builder()
            .add_source(config::File::from_str(
                r#"
[observability.tracer]
enabled = true

[observability.tracer.outputs.file]
enabled = true
path = "/nonexistent-dir/trace.log"
format = "json"
"#,
                FileFormat::Toml,
            ))
            .build()
            .unwrap()
            .try_deserialize::<CamelConfig>()
            .unwrap();

        let err = CamelConfig::configure_context(&cfg).await.err().unwrap();
        let msg = err.to_string();
        assert!(msg.contains("Failed to open trace file"));
    }
}

#[cfg(test)]
mod context_health_source_tests {
    use super::*;
    use camel_api::{AsyncHealthCheck, CheckResult};

    struct FixedCheck {
        check_name: String,
        result: CheckResult,
    }

    #[async_trait::async_trait]
    impl AsyncHealthCheck for FixedCheck {
        fn name(&self) -> &str {
            &self.check_name
        }
        async fn check(&self) -> CheckResult {
            self.result.clone()
        }
    }

    fn make_source(
        registry: Arc<HealthCheckRegistry>,
        services: HealthState,
    ) -> ContextHealthSource {
        ContextHealthSource {
            health_registry: registry,
            services,
        }
    }

    #[tokio::test]
    async fn health_report_populates_services_from_registry() {
        let registry = Arc::new(HealthCheckRegistry::new(Duration::from_secs(5)));
        registry.register_for_route(
            "datasource:cartodb",
            Arc::new(FixedCheck {
                check_name: "datasource:cartodb".to_string(),
                result: CheckResult::unhealthy("datasource:cartodb", "timed out"),
            }),
        );
        registry.mark_route_started("datasource:cartodb");

        let source = make_source(Arc::clone(&registry), Arc::new(Mutex::new(Vec::new())));
        let report = source.health_report().await;

        assert_eq!(report.status, HealthStatus::Unhealthy);
        let ds = report
            .services
            .iter()
            .find(|s| s.name == "datasource:cartodb")
            .expect("datasource check should appear in report");
        assert_eq!(ds.status, ServiceStatus::Failed);
        assert_eq!(ds.message.as_deref(), Some("timed out"));
    }

    #[tokio::test]
    async fn health_report_reflects_failed_platform_service() {
        let registry = Arc::new(HealthCheckRegistry::new(Duration::from_secs(5)));
        let state: HealthState = Arc::new(Mutex::new(vec![(
            "health".to_string(),
            Arc::new(AtomicU8::new(2)),
        )]));

        let source = make_source(registry, state);
        let report = source.health_report().await;

        assert_eq!(report.status, HealthStatus::Unhealthy);
        assert!(
            report
                .services
                .iter()
                .any(|s| s.name == "health" && s.status == ServiceStatus::Failed),
            "failed platform service should appear: {:?}",
            report.services
        );
    }

    #[tokio::test]
    async fn health_report_empty_is_healthy() {
        let registry = Arc::new(HealthCheckRegistry::new(Duration::from_secs(5)));
        let source = make_source(registry, Arc::new(Mutex::new(Vec::new())));
        let report = source.health_report().await;

        assert_eq!(report.status, HealthStatus::Healthy);
        assert!(report.services.is_empty());
    }
}
