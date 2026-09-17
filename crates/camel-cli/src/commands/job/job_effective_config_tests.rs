//! Structural tests for the `camel job` boot projection
//! (`job_effective_config`): the journal and observability stack are
//! neutralized, and every other field survives the projection
//! untouched — the allowlist is exact.

use std::collections::HashMap;

use super::job_effective_config;
use camel_api::datasource::DatasourceConfig;
use camel_config::config::{
    BeanConfig, BindExposureConfig, CacheRepoConfig, CamelConfig, ComponentsConfig,
    HealthCamelConfig, IdempotentRepoConfig, JobsCamelConfig, JournalConfig, JournalDurability,
    KubernetesPlatformCamelConfig, ObservabilityConfig, OidcSecurityConfig, OtelCamelConfig,
    PlatformCamelConfig, PrometheusCamelConfig, SecurityConfig, StreamCachingConfig,
    SupervisionCamelConfig,
};
use camel_config::{LanguagesConfig, RhaiEngineConfig, RhaiLimitsConfig};

/// A `CamelConfig` with every non-projected field set away from its
/// default where constructible, so survivor assertions can never pass
/// vacuously. The projected fields (`runtime_journal`, `observability`)
/// stay at their defaults.
fn sample_job_config() -> CamelConfig {
    CamelConfig {
        routes: vec!["direct:job-probe".to_string()],
        watch: true,
        idempotent_repo: Some(IdempotentRepoConfig {
            backend: "redb".to_string(),
            name: None,
            path: Some("/tmp/keep.db".to_string()),
            durability: None,
            url: None,
            sentinel_nodes: None,
            master_name: None,
            sentinel_username: None,
            sentinel_password: None,
            password: None,
            username: None,
            db: None,
            key_prefix: None,
        }),
        cache_repo: Some(CacheRepoConfig {
            backend: "memory".to_string(),
            max_capacity: Some(99),
            ..Default::default()
        }),
        log_level: "debug".to_string(),
        timeout_ms: 4321,
        drain_timeout_ms: 8765,
        watch_debounce_ms: 321,
        components: ComponentsConfig {
            raw: HashMap::from([("timer".to_string(), toml::Value::String("keep".to_string()))]),
        },
        supervision: Some(SupervisionCamelConfig {
            max_attempts: Some(7),
            ..Default::default()
        }),
        platform: PlatformCamelConfig::Kubernetes(KubernetesPlatformCamelConfig::default()),
        stream_caching: StreamCachingConfig { threshold: 4096 },
        beans: HashMap::from([(
            "greeter".to_string(),
            BeanConfig {
                plugin: "mem".to_string(),
                ..Default::default()
            },
        )]),
        languages: LanguagesConfig {
            rhai: RhaiEngineConfig {
                limits: RhaiLimitsConfig {
                    max_operations: Some(4242),
                    ..Default::default()
                },
            },
            ..Default::default()
        },
        security: SecurityConfig {
            oidc: Some(OidcSecurityConfig {
                issuer: "https://issuer.example".to_string(),
                jwks_uri: None,
                audience: Vec::new(),
                client_id: None,
                client_secret: None,
                token_endpoint: None,
                introspection_endpoint: None,
            }),
            ..Default::default()
        },
        binds: HashMap::from([(
            "0.0.0.0:8080".to_string(),
            BindExposureConfig {
                allow_public_exposure: true,
            },
        )]),
        datasources: HashMap::from([(
            "primary".to_string(),
            DatasourceConfig {
                db_url: "postgres://db.example/keep".to_string(),
                provider: None,
                max_connections: None,
                min_connections: None,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                ssl_mode: None,
                ssl_root_cert: None,
                ssl_cert: None,
                ssl_key: None,
                extra: HashMap::new(),
            },
        )]),
        jobs: JobsCamelConfig {
            dir: Some("job-roots".to_string()),
            dirs: None,
        },
        // Projected fields stay at their defaults (see module doc).
        runtime_journal: None,
        observability: ObservabilityConfig::default(),
        _extra: HashMap::new(),
    }
}

#[test]
fn job_effective_config_neutralizes_journal_and_observability() {
    let config = CamelConfig {
        runtime_journal: Some(JournalConfig {
            path: std::path::PathBuf::from("/tmp/jobcoexist-probe.db"),
            durability: JournalDurability::Immediate,
            compaction_threshold_events: 10_000,
        }),
        observability: ObservabilityConfig {
            otel: Some(OtelCamelConfig {
                enabled: true,
                ..Default::default()
            }),
            prometheus: Some(PrometheusCamelConfig {
                enabled: true,
                ..Default::default()
            }),
            health: Some(HealthCamelConfig {
                enabled: true,
                ..Default::default()
            }),
            ..Default::default()
        },
        ..CamelConfig::default()
    };

    let effective = job_effective_config(&config);

    assert!(effective.runtime_journal.is_none());
    assert!(effective.observability.otel.is_none());
    assert!(effective.observability.prometheus.is_none());
    assert!(effective.observability.health.is_none());
}

#[test]
fn job_effective_config_preserves_all_other_fields() {
    let input = sample_job_config();

    let effective = job_effective_config(&input);

    assert_eq!(effective.routes, input.routes);
    assert_eq!(effective.watch, input.watch);
    assert_eq!(effective.idempotent_repo, input.idempotent_repo);
    assert_eq!(effective.cache_repo, input.cache_repo);
    assert_eq!(effective.log_level, input.log_level);
    assert_eq!(effective.timeout_ms, input.timeout_ms);
    assert_eq!(effective.drain_timeout_ms, input.drain_timeout_ms);
    assert_eq!(effective.watch_debounce_ms, input.watch_debounce_ms);
    assert_eq!(effective.components, input.components);
    assert_eq!(effective.supervision, input.supervision);
    assert_eq!(effective.platform, input.platform);
    assert_eq!(effective.stream_caching, input.stream_caching);
    assert_eq!(effective.beans, input.beans);
    assert_eq!(effective.languages, input.languages);
    assert_eq!(effective.security, input.security);
    assert_eq!(effective.binds, input.binds);
    assert_eq!(effective.datasources, input.datasources);
    assert_eq!(effective.jobs, input.jobs);
    assert_eq!(effective._extra, input._extra);
}
