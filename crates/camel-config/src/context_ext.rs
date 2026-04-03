use crate::config::{CamelConfig, OtelProtocol, OtelSampler};
use crate::discovery::discover_routes;
use camel_api::CamelError;
use camel_core::CamelContext;
use camel_core::OutputFormat;
use camel_core::TracerConfig;
use camel_core::route::RouteDefinition;
use camel_otel::{
    OtelConfig, OtelProtocol as OtelProtocolOtel, OtelSampler as OtelSamplerOtel, OtelService,
};
use tracing::Level;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[cfg(feature = "http")]
impl From<&crate::config::HttpCamelConfig> for camel_component_http::HttpConfig {
    fn from(c: &crate::config::HttpCamelConfig) -> Self {
        camel_component_http::HttpConfig {
            connect_timeout_ms: c.connect_timeout_ms,
            response_timeout_ms: c.response_timeout_ms,
            pool_max_idle_per_host: c.pool_max_idle_per_host,
            pool_idle_timeout_ms: c.pool_idle_timeout_ms,
            follow_redirects: c.follow_redirects,
            max_body_size: c.max_body_size,
            max_request_body: c.max_request_body,
            allow_private_ips: c.allow_private_ips,
            blocked_hosts: c.blocked_hosts.clone(),
        }
    }
}

#[cfg(feature = "kafka")]
impl From<&crate::config::KafkaCamelConfig> for camel_component_kafka::KafkaConfig {
    fn from(c: &crate::config::KafkaCamelConfig) -> Self {
        camel_component_kafka::KafkaConfig {
            brokers: c.brokers.clone(),
            group_id: c.group_id.clone(),
            session_timeout_ms: c.session_timeout_ms,
            request_timeout_ms: c.request_timeout_ms,
            auto_offset_reset: c.auto_offset_reset.clone(),
            security_protocol: c.security_protocol.clone(),
        }
    }
}

#[cfg(feature = "jms")]
impl From<&crate::config::JmsCamelConfig> for camel_component_jms::JmsConfig {
    fn from(c: &crate::config::JmsCamelConfig) -> Self {
        let broker_type: camel_component_jms::BrokerType = c.broker_type.parse().unwrap();
        if matches!(broker_type, camel_component_jms::BrokerType::Generic)
            && !matches!(c.broker_type.to_lowercase().as_str(), "" | "generic")
        {
            tracing::warn!(
                "JMS: unrecognized broker_type '{}', falling back to Generic. Valid values: activemq, artemis, generic",
                c.broker_type
            );
        }

        camel_component_jms::JmsConfig {
            broker_url: c.broker_url.clone(),
            broker_type,
            username: c.username.clone(),
            password: c.password.clone(),
            bridge_version: c.bridge_version.clone(),
            bridge_cache_dir: c.bridge_cache_dir.clone(),
            bridge_start_timeout_ms: c.bridge_start_timeout_ms,
            broker_reconnect_interval_ms: c.broker_reconnect_interval_ms,
        }
    }
}

#[cfg(feature = "redis")]
impl From<&crate::config::RedisCamelConfig> for camel_component_redis::RedisConfig {
    fn from(c: &crate::config::RedisCamelConfig) -> Self {
        camel_component_redis::RedisConfig {
            host: c.host.clone(),
            port: c.port,
        }
    }
}

#[cfg(feature = "sql")]
impl From<&crate::config::SqlCamelConfig> for camel_component_sql::SqlGlobalConfig {
    fn from(c: &crate::config::SqlCamelConfig) -> Self {
        camel_component_sql::SqlGlobalConfig::default()
            .with_max_connections(c.max_connections)
            .with_min_connections(c.min_connections)
            .with_idle_timeout_secs(c.idle_timeout_secs)
            .with_max_lifetime_secs(c.max_lifetime_secs)
    }
}

#[cfg(feature = "file")]
impl From<&crate::config::FileCamelConfig> for camel_component_file::FileGlobalConfig {
    fn from(c: &crate::config::FileCamelConfig) -> Self {
        camel_component_file::FileGlobalConfig::default()
            .with_delay_ms(c.delay_ms)
            .with_initial_delay_ms(c.initial_delay_ms)
            .with_read_timeout_ms(c.read_timeout_ms)
            .with_write_timeout_ms(c.write_timeout_ms)
    }
}

#[cfg(feature = "container")]
impl From<&crate::config::ContainerCamelConfig>
    for camel_component_container::ContainerGlobalConfig
{
    fn from(c: &crate::config::ContainerCamelConfig) -> Self {
        camel_component_container::ContainerGlobalConfig::default()
            .with_docker_host(c.docker_host.clone())
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

        discover_routes(&config.routes).map_err(|e| CamelError::Config(e.to_string()))
    }

    /// Create a CamelContext configured from this CamelConfig.
    ///
    /// Always installs a unified tracing subscriber (Layers 1–3, plus Layer 4
    /// when OTel is enabled). `OtelService`, if present, only manages providers —
    /// it never installs a subscriber.
    pub async fn configure_context(config: &CamelConfig) -> Result<CamelContext, CamelError> {
        let otel_enabled = config
            .observability
            .otel
            .as_ref()
            .is_some_and(|o| o.enabled);

        // Build context with optional supervision
        let mut ctx = if let Some(ref sup) = config.supervision {
            if let Some(ref jcfg) = config.runtime_journal {
                CamelContext::with_supervision_and_metrics_and_redb_journal(
                    sup.clone().into_supervision_config(),
                    std::sync::Arc::new(camel_api::NoOpMetrics),
                    jcfg.path.clone(),
                    jcfg.into(),
                )
                .await?
            } else {
                CamelContext::with_supervision(sup.clone().into_supervision_config())
            }
        } else if let Some(ref jcfg) = config.runtime_journal {
            CamelContext::new_with_redb_journal(jcfg.path.clone(), jcfg.into()).await?
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

            let otel_service = OtelService::new(otel_config);
            ctx = ctx.with_lifecycle(otel_service);
        }

        // Prometheus — replaces loose metrics_enabled / metrics_port
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
            let prom_service = camel_prometheus::PrometheusService::new(addr);
            ctx = ctx.with_lifecycle(prom_service);
        }

        #[cfg(feature = "http")]
        {
            let http_config: camel_component_http::HttpConfig = config
                .components
                .http
                .as_ref()
                .map(camel_component_http::HttpConfig::from)
                .unwrap_or_default();
            ctx.set_component_config(http_config);
        }

        #[cfg(feature = "kafka")]
        {
            let kafka_config: camel_component_kafka::KafkaConfig = config
                .components
                .kafka
                .as_ref()
                .map(camel_component_kafka::KafkaConfig::from)
                .unwrap_or_default();
            ctx.set_component_config(kafka_config);
        }

        #[cfg(feature = "jms")]
        {
            let jms_config: camel_component_jms::JmsConfig = config
                .components
                .jms
                .as_ref()
                .map(camel_component_jms::JmsConfig::from)
                .unwrap_or_default();
            ctx.set_component_config(jms_config);
        }

        #[cfg(feature = "redis")]
        {
            let redis_config: camel_component_redis::RedisConfig = config
                .components
                .redis
                .as_ref()
                .map(camel_component_redis::RedisConfig::from)
                .unwrap_or_default();
            ctx.set_component_config(redis_config);
        }

        #[cfg(feature = "sql")]
        {
            let sql_config: camel_component_sql::SqlGlobalConfig = config
                .components
                .sql
                .as_ref()
                .map(camel_component_sql::SqlGlobalConfig::from)
                .unwrap_or_default();
            ctx.set_component_config(sql_config);
        }

        #[cfg(feature = "file")]
        {
            let file_config: camel_component_file::FileGlobalConfig = config
                .components
                .file
                .as_ref()
                .map(camel_component_file::FileGlobalConfig::from)
                .unwrap_or_default();
            ctx.set_component_config(file_config);
        }

        #[cfg(feature = "container")]
        {
            let container_config: camel_component_container::ContainerGlobalConfig = config
                .components
                .container
                .as_ref()
                .map(camel_component_container::ContainerGlobalConfig::from)
                .unwrap_or_default();
            ctx.set_component_config(container_config);
        }

        ctx.set_tracer_config(tracer_config);
        Ok(ctx)
    }

    fn init_tracing_subscriber(
        config: &TracerConfig,
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

        // Layer 4: tracing-opentelemetry bridge — only when OTel is active
        #[cfg(feature = "otel")]
        let otel_layer: Option<Box<dyn tracing_subscriber::Layer<_> + Send + Sync>> = if otel_active
        {
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
}
