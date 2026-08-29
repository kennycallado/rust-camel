use super::*;

#[test]
fn test_config_zero_timeout_rejected() {
    let config = CamelConfig {
        timeout_ms: 0,
        ..CamelConfig::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn test_config_zero_drain_timeout_rejected() {
    let config = CamelConfig {
        drain_timeout_ms: 0,
        ..CamelConfig::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn test_config_empty_journal_path_rejected() {
    let config = CamelConfig {
        runtime_journal: Some(JournalConfig {
            path: std::path::PathBuf::from(""),
            durability: JournalDurability::default(),
            compaction_threshold_events: 10_000,
        }),
        ..CamelConfig::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn redb_idempotent_config_empty_path_rejected() {
    let config = CamelConfig {
        idempotent_repo: Some(IdempotentRepoConfig {
            backend: "redb".to_string(),
            path: Some(String::new()),
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
        ..CamelConfig::default()
    };
    let err = config
        .validate()
        .expect_err("empty idempotent_repo.path must fail validation");
    let msg = err.to_string();
    assert!(
        msg.contains("idempotent_repo") && msg.contains("path"),
        "error must name `idempotent_repo.path`, got: {msg}"
    );
}

#[test]
fn test_config_empty_bean_plugin_rejected() {
    let mut beans = HashMap::new();
    beans.insert(
        "my-bean".to_string(),
        BeanConfig {
            plugin: "".to_string(),
            config: HashMap::new(),
            limits: Default::default(),
        },
    );
    let config = CamelConfig {
        beans,
        ..CamelConfig::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn test_config_whitespace_bean_plugin_rejected() {
    let mut beans = HashMap::new();
    beans.insert(
        "my-bean".to_string(),
        BeanConfig {
            plugin: "   ".to_string(),
            config: HashMap::new(),
            limits: Default::default(),
        },
    );
    let config = CamelConfig {
        beans,
        ..CamelConfig::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn test_config_valid_defaults_pass() {
    let config = CamelConfig::default();
    assert!(config.validate().is_ok());
}

#[test]
fn test_config_zero_watch_debounce_rejected() {
    let config = CamelConfig {
        watch_debounce_ms: 0,
        ..CamelConfig::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn test_config_zero_journal_compaction_threshold_rejected() {
    let config = CamelConfig {
        runtime_journal: Some(JournalConfig {
            path: std::path::PathBuf::from("/tmp/test.db"),
            durability: JournalDurability::default(),
            compaction_threshold_events: 0,
        }),
        ..CamelConfig::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn test_config_zero_supervision_initial_delay_rejected() {
    let config = CamelConfig {
        supervision: Some(SupervisionCamelConfig {
            max_attempts: Some(5),
            initial_delay_ms: 0,
            backoff_multiplier: 2.0,
            max_delay_ms: 60000,
        }),
        ..CamelConfig::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn test_config_zero_supervision_max_delay_rejected() {
    let config = CamelConfig {
        supervision: Some(SupervisionCamelConfig {
            max_attempts: Some(5),
            initial_delay_ms: 1000,
            backoff_multiplier: 2.0,
            max_delay_ms: 0,
        }),
        ..CamelConfig::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn test_config_supervision_backoff_below_one_rejected() {
    let config = CamelConfig {
        supervision: Some(SupervisionCamelConfig {
            max_attempts: Some(5),
            initial_delay_ms: 1000,
            backoff_multiplier: 0.5,
            max_delay_ms: 60000,
        }),
        ..CamelConfig::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn test_config_zero_otel_metrics_interval_rejected() {
    let otel = OtelCamelConfig {
        metrics_interval_ms: 0,
        ..Default::default()
    };
    let config = CamelConfig {
        observability: ObservabilityConfig {
            otel: Some(otel),
            ..Default::default()
        },
        ..CamelConfig::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn test_config_kubernetes_zero_lease_duration_rejected() {
    let config = CamelConfig {
        platform: PlatformCamelConfig::Kubernetes(KubernetesPlatformCamelConfig {
            namespace: None,
            lease_name_prefix: "camel-".to_string(),
            lease_duration_secs: 0,
            renew_deadline_secs: 10,
            retry_period_secs: 2,
            jitter_factor: 0.2,
        }),
        ..CamelConfig::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn test_config_kubernetes_zero_renew_deadline_rejected() {
    let config = CamelConfig {
        platform: PlatformCamelConfig::Kubernetes(KubernetesPlatformCamelConfig {
            namespace: None,
            lease_name_prefix: "camel-".to_string(),
            lease_duration_secs: 15,
            renew_deadline_secs: 0,
            retry_period_secs: 2,
            jitter_factor: 0.2,
        }),
        ..CamelConfig::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn test_config_kubernetes_zero_retry_period_rejected() {
    let config = CamelConfig {
        platform: PlatformCamelConfig::Kubernetes(KubernetesPlatformCamelConfig {
            namespace: None,
            lease_name_prefix: "camel-".to_string(),
            lease_duration_secs: 15,
            renew_deadline_secs: 10,
            retry_period_secs: 0,
            jitter_factor: 0.2,
        }),
        ..CamelConfig::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn test_config_kubernetes_jitter_out_of_range_rejected() {
    let config = CamelConfig {
        platform: PlatformCamelConfig::Kubernetes(KubernetesPlatformCamelConfig {
            namespace: None,
            lease_name_prefix: "camel-".to_string(),
            lease_duration_secs: 15,
            renew_deadline_secs: 10,
            retry_period_secs: 2,
            jitter_factor: 1.5,
        }),
        ..CamelConfig::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn test_config_kubernetes_negative_jitter_rejected() {
    let config = CamelConfig {
        platform: PlatformCamelConfig::Kubernetes(KubernetesPlatformCamelConfig {
            namespace: None,
            lease_name_prefix: "camel-".to_string(),
            lease_duration_secs: 15,
            renew_deadline_secs: 10,
            retry_period_secs: 2,
            jitter_factor: -0.1,
        }),
        ..CamelConfig::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn test_config_valid_kubernetes_passes() {
    let config = CamelConfig {
        platform: PlatformCamelConfig::Kubernetes(KubernetesPlatformCamelConfig {
            namespace: Some("default".to_string()),
            lease_name_prefix: "camel-".to_string(),
            lease_duration_secs: 15,
            renew_deadline_secs: 10,
            retry_period_secs: 2,
            jitter_factor: 0.2,
        }),
        ..CamelConfig::default()
    };
    assert!(config.validate().is_ok());
}

#[test]
fn test_config_valid_supervision_passes() {
    let config = CamelConfig {
        supervision: Some(SupervisionCamelConfig {
            max_attempts: Some(5),
            initial_delay_ms: 1000,
            backoff_multiplier: 2.0,
            max_delay_ms: 60000,
        }),
        ..CamelConfig::default()
    };
    assert!(config.validate().is_ok());
}

#[test]
fn test_config_valid_journal_passes() {
    let config = CamelConfig {
        runtime_journal: Some(JournalConfig {
            path: std::path::PathBuf::from("/tmp/test.db"),
            durability: JournalDurability::default(),
            compaction_threshold_events: 10_000,
        }),
        ..CamelConfig::default()
    };
    assert!(config.validate().is_ok());
}

#[test]
fn bean_config_deserialises_with_limits() {
    let toml_str = r#"
        plugin = "my-plugin"
        [limits]
        timeout-secs = 600
        max-memory = 4294967296
        "#;
    let cfg: BeanConfig = toml::from_str(toml_str).expect("deserialize");
    assert_eq!(cfg.plugin, "my-plugin");
    assert_eq!(cfg.limits.timeout_secs, Some(600));
    assert_eq!(cfg.limits.max_memory, Some(4_294_967_296));
    assert_eq!(cfg.limits.max_concurrent_calls, None);
}

#[test]
fn bean_config_defaults_limits_to_none() {
    let toml_str = r#"
        plugin = "my-plugin"
        "#;
    let cfg: BeanConfig = toml::from_str(toml_str).expect("deserialize");
    assert_eq!(cfg.limits, crate::wasm_limits::WasmLimitsConfig::default());
}

#[test]
fn test_config_invalid_datasource_rejected() {
    let mut datasources = HashMap::new();
    datasources.insert(
        "bad".to_string(),
        DatasourceConfig {
            db_url: "  ".into(),
            provider: None,
            max_connections: None,
            min_connections: None,
            idle_timeout_secs: None,
            max_lifetime_secs: None,
            ssl_mode: None,
            ssl_root_cert: None,
            ssl_cert: None,
            ssl_key: None,
            extra: std::collections::HashMap::new(),
        },
    );
    let config = CamelConfig {
        datasources,
        ..CamelConfig::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn test_config_valid_datasources_pass() {
    let mut datasources = HashMap::new();
    datasources.insert(
        "orders".to_string(),
        DatasourceConfig {
            db_url: "postgres://localhost/orders".into(),
            provider: None,
            max_connections: Some(20),
            min_connections: None,
            idle_timeout_secs: None,
            max_lifetime_secs: None,
            ssl_mode: None,
            ssl_root_cert: None,
            ssl_cert: None,
            ssl_key: None,
            extra: std::collections::HashMap::new(),
        },
    );
    let config = CamelConfig {
        datasources,
        ..CamelConfig::default()
    };
    assert!(config.validate().is_ok());
}
