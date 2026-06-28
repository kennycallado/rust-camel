use crate::config::{KafkaConfig, SaslAuthType, SecurityProtocol};
use camel_component_api::CamelError;
use rdkafka::config::ClientConfig;
use std::collections::HashMap;

/// rdkafka config keys that have explicit typed fields on KafkaBrokerConfig.
/// Rejected from `rdkafka_config` to prevent config fragmentation.
const RESERVED_RDKAFKA_KEYS: &[&str] = &[
    "bootstrap.servers",
    "client.id",
    "security.protocol",
    "sasl.mechanism",
    "sasl.username",
    "sasl.password",
    "ssl.keystore.location",
    "ssl.keystore.password",
    "ssl.truststore.location",
    "ssl.truststore.password",
];

/// Pre-configured named Kafka cluster.
///
/// Define under `[components.kafka.brokers_named]` in Camel.toml,
/// reference via `?brokerName=name` in URIs.
#[derive(Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct KafkaBrokerConfig {
    /// Comma-separated broker addresses (required — empty rejected by upstream validate).
    pub brokers: String,
    pub security_protocol: Option<SecurityProtocol>,
    pub sasl_auth_type: Option<SaslAuthType>,
    pub sasl_username: Option<String>,
    pub sasl_password: Option<String>,
    pub ssl_keystore_location: Option<String>,
    pub ssl_keystore_password: Option<String>,
    pub ssl_truststore_location: Option<String>,
    pub ssl_truststore_password: Option<String>,
    pub client_id: Option<String>,

    /// Escape hatch for rdkafka config keys not modeled above.
    /// Keys with dedicated fields are REJECTED (see RESERVED_RDKAFKA_KEYS).
    pub rdkafka_config: HashMap<String, String>,
}

impl std::fmt::Debug for KafkaBrokerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KafkaBrokerConfig")
            .field("brokers", &self.brokers)
            .field("security_protocol", &self.security_protocol)
            .field("sasl_auth_type", &self.sasl_auth_type)
            .field("sasl_username", &self.sasl_username)
            .field(
                "sasl_password",
                &self.sasl_password.as_deref().map(|_| "[REDACTED]"),
            )
            .field("ssl_keystore_location", &self.ssl_keystore_location)
            .field(
                "ssl_keystore_password",
                &self.ssl_keystore_password.as_deref().map(|_| "[REDACTED]"),
            )
            .field("ssl_truststore_location", &self.ssl_truststore_location)
            .field(
                "ssl_truststore_password",
                &self
                    .ssl_truststore_password
                    .as_deref()
                    .map(|_| "[REDACTED]"),
            )
            .field("client_id", &self.client_id)
            .field(
                "rdkafka_config",
                &format!("{} keys [REDACTED]", self.rdkafka_config.len()),
            )
            .finish()
    }
}

/// Validate that no reserved keys are present in rdkafka_config.
pub fn validate_rdkafka_config(rdkafka_config: &HashMap<String, String>) -> Result<(), CamelError> {
    for key in rdkafka_config.keys() {
        if RESERVED_RDKAFKA_KEYS.contains(&key.as_str()) {
            return Err(CamelError::Config(format!(
                "rdkafka_config key '{key}' is reserved — use the dedicated field instead"
            )));
        }
    }
    Ok(())
}

/// Validate that SASL credentials are present when the auth type requires them.
pub fn validate_sasl_creds(
    sasl_auth_type: SaslAuthType,
    sasl_username: &Option<String>,
    sasl_password: &Option<String>,
) -> Result<(), CamelError> {
    match sasl_auth_type {
        SaslAuthType::None | SaslAuthType::Ssl => {}
        _ => {
            if sasl_username.is_none() || sasl_password.is_none() {
                return Err(CamelError::Config(format!(
                    "SASL auth type '{sasl_auth_type:?}' requires sasl_username and sasl_password"
                )));
            }
        }
    }
    Ok(())
}

/// Apply `rdkafka_config` from resolved endpoint config to a `ClientConfig`.
/// Call after `apply_security_config()` in producer/consumer creation.
pub fn apply_rdkafka_config(
    config: &crate::config::ResolvedKafkaEndpointConfig,
    cc: &mut ClientConfig,
) {
    for (key, value) in &config.rdkafka_config {
        cc.set(key.as_str(), value);
    }
}

/// Resolve named broker config from `KafkaConfig.brokers_named`.
///
/// Only fills fields NOT already set in the URI (is_none() checks).
/// Call BEFORE `apply_defaults()` so named config takes priority.
pub fn apply_broker_name(
    config: &mut crate::config::KafkaEndpointConfig,
    global: &KafkaConfig,
) -> Result<(), CamelError> {
    let Some(ref name) = config.broker_name else {
        return Ok(());
    };

    let named = global.brokers_named.get(name).ok_or_else(|| {
        let available: Vec<String> = global.brokers_named.keys().cloned().collect();
        CamelError::Config(format!(
            "unknown broker '{name}', available brokers: [{}]",
            available.join(", ")
        ))
    })?;

    validate_rdkafka_config(&named.rdkafka_config)?;

    if config.brokers.is_none() {
        config.brokers = Some(named.brokers.clone());
    }
    if config.security_protocol.is_none() {
        config.security_protocol = named.security_protocol;
    }
    // sentinel: SaslAuthType is NOT Option<SaslAuthType> (pre-existing), so
    // == default() is used to detect "not set". If URI sets saslAuthType=None
    // the named broker overrides — known caveat documented in spec.
    if config.sasl_auth_type == SaslAuthType::default()
        && let Some(auth_type) = named.sasl_auth_type
    {
        config.sasl_auth_type = auth_type;
    }
    if config.sasl_username.is_none() {
        config.sasl_username = named.sasl_username.clone();
    }
    if config.sasl_password.is_none() {
        config.sasl_password = named.sasl_password.clone();
    }
    if config.ssl_keystore_location.is_none() {
        config.ssl_keystore_location = named.ssl_keystore_location.clone();
    }
    if config.ssl_keystore_password.is_none() {
        config.ssl_keystore_password = named.ssl_keystore_password.clone();
    }
    if config.ssl_truststore_location.is_none() {
        config.ssl_truststore_location = named.ssl_truststore_location.clone();
    }
    if config.ssl_truststore_password.is_none() {
        config.ssl_truststore_password = named.ssl_truststore_password.clone();
    }
    if config.client_id.is_none() {
        config.client_id = named.client_id.clone();
    }

    // Carry rdkafka_config from named config through resolution.
    // URI cannot set rdkafka_config today, safe to extend; merge is future-proof.
    config.rdkafka_config.extend(named.rdkafka_config.clone());

    // Re-validate SASL credentials after named broker may have set them
    validate_sasl_creds(
        config.sasl_auth_type,
        &config.sasl_username,
        &config.sasl_password,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_broker_config_toml_deserialize() {
        let toml_str = r#"
            brokers = "broker1:9092,broker2:9092"
            security_protocol = "SaslSsl"
            sasl_auth_type = "Plain"
            sasl_username = "user"
            sasl_password = "pass"
        "#;
        let cfg: KafkaBrokerConfig = toml::from_str(toml_str).expect("parse");
        assert_eq!(cfg.brokers, "broker1:9092,broker2:9092");
        assert_eq!(cfg.security_protocol, Some(SecurityProtocol::SaslSsl));
        assert_eq!(cfg.sasl_auth_type, Some(SaslAuthType::Plain));
    }

    #[test]
    fn test_broker_config_debug_redacts_secrets() {
        let cfg = KafkaBrokerConfig {
            sasl_password: Some("secret123".into()),
            ..KafkaBrokerConfig::default()
        };
        let s = format!("{:?}", cfg);
        assert!(!s.contains("secret123"));
        assert!(s.contains("[REDACTED]"));
    }

    #[test]
    fn test_broker_config_rdkafka_escape_hatch() {
        let toml_str = r#"
            brokers = "broker:9092"
            [rdkafka_config]
            "max.message.bytes" = "10485760"
        "#;
        let cfg: KafkaBrokerConfig = toml::from_str(toml_str).expect("parse");
        assert_eq!(
            cfg.rdkafka_config.get("max.message.bytes").unwrap(),
            "10485760"
        );
    }

    #[test]
    fn test_validate_rdkafka_config_rejects_reserved() {
        let mut map = HashMap::new();
        map.insert("bootstrap.servers".into(), "evil:9092".into());
        let err = validate_rdkafka_config(&map).unwrap_err();
        assert!(err.to_string().contains("reserved"));
    }

    #[test]
    fn test_validate_sasl_creds_missing_password() {
        let err = validate_sasl_creds(SaslAuthType::Plain, &Some("u".into()), &None).unwrap_err();
        assert!(err.to_string().contains("sasl_password"));
    }

    #[test]
    fn test_broker_config_default_brokers_empty() {
        let cfg = KafkaBrokerConfig::default();
        assert!(
            cfg.brokers.is_empty(),
            "default brokers should be empty so validate catches it"
        );
    }

    // -----------------------------------------------------------------------
    // apply_broker_name
    // -----------------------------------------------------------------------

    #[test]
    fn test_apply_broker_name_resolves_known_name() {
        let mut global = KafkaConfig::default();
        global.brokers_named.insert(
            "prod".into(),
            KafkaBrokerConfig {
                brokers: "prod1:9092,prod2:9092".into(),
                security_protocol: Some(SecurityProtocol::SaslSsl),
                sasl_auth_type: Some(SaslAuthType::Plain),
                sasl_username: Some("user".into()),
                sasl_password: Some("pass".into()),
                ..KafkaBrokerConfig::default()
            },
        );

        let mut ep =
            crate::config::KafkaEndpointConfig::from_uri("kafka:orders?brokerName=prod").unwrap();
        apply_broker_name(&mut ep, &global).expect("resolve");
        assert_eq!(ep.brokers.as_deref(), Some("prod1:9092,prod2:9092"));
        assert_eq!(ep.security_protocol, Some(SecurityProtocol::SaslSsl));
        assert_eq!(ep.sasl_auth_type, SaslAuthType::Plain);
    }

    #[test]
    fn test_apply_broker_name_none_is_noop() {
        let global = KafkaConfig::default();
        let mut ep =
            crate::config::KafkaEndpointConfig::from_uri("kafka:orders?brokers=uri:9092").unwrap();
        assert!(ep.broker_name.is_none());
        apply_broker_name(&mut ep, &global).expect("noop");
        assert_eq!(ep.brokers.as_deref(), Some("uri:9092"));
    }

    #[test]
    fn test_apply_broker_name_uri_wins() {
        let mut global = KafkaConfig::default();
        global.brokers_named.insert(
            "prod".into(),
            KafkaBrokerConfig {
                brokers: "named:9092".into(),
                security_protocol: Some(SecurityProtocol::SaslSsl),
                ..KafkaBrokerConfig::default()
            },
        );

        let mut ep = crate::config::KafkaEndpointConfig::from_uri(
            "kafka:orders?brokerName=prod&brokers=uri:9092&securityProtocol=SSL",
        )
        .unwrap();
        apply_broker_name(&mut ep, &global).expect("resolve");
        assert_eq!(ep.brokers.as_deref(), Some("uri:9092"));
        assert_eq!(ep.security_protocol, Some(SecurityProtocol::Ssl));
    }

    #[test]
    fn test_apply_broker_name_unknown_name_errors() {
        let global = KafkaConfig::default();
        let mut ep =
            crate::config::KafkaEndpointConfig::from_uri("kafka:orders?brokerName=nope").unwrap();
        let err = apply_broker_name(&mut ep, &global).unwrap_err();
        assert!(err.to_string().contains("unknown broker 'nope'"));
    }

    #[test]
    fn test_apply_broker_name_reserved_keys_rejected() {
        let mut global = KafkaConfig::default();
        global.brokers_named.insert(
            "bad".into(),
            KafkaBrokerConfig {
                brokers: "broker:9092".into(),
                rdkafka_config: [("bootstrap.servers".into(), "evil:9092".into())].into(),
                ..KafkaBrokerConfig::default()
            },
        );

        let mut ep =
            crate::config::KafkaEndpointConfig::from_uri("kafka:orders?brokerName=bad").unwrap();
        let err = apply_broker_name(&mut ep, &global).unwrap_err();
        assert!(err.to_string().contains("reserved"));
    }

    #[test]
    fn test_apply_broker_name_named_sasl_missing_password_errors() {
        let mut global = KafkaConfig::default();
        global.brokers_named.insert(
            "bad".into(),
            KafkaBrokerConfig {
                brokers: "broker:9092".into(),
                sasl_auth_type: Some(SaslAuthType::Plain),
                sasl_username: Some("user".into()),
                sasl_password: None,
                ..KafkaBrokerConfig::default()
            },
        );

        let mut ep =
            crate::config::KafkaEndpointConfig::from_uri("kafka:orders?brokerName=bad").unwrap();
        let err = apply_broker_name(&mut ep, &global).unwrap_err();
        assert!(err.to_string().contains("sasl_password"));
    }

    #[test]
    fn test_apply_broker_name_carries_rdkafka_config() {
        let mut global = KafkaConfig::default();
        let mut rdkafka = HashMap::new();
        rdkafka.insert("max.message.bytes".into(), "10485760".into());
        global.brokers_named.insert(
            "tuned".into(),
            KafkaBrokerConfig {
                brokers: "broker:9092".into(),
                rdkafka_config: rdkafka,
                ..KafkaBrokerConfig::default()
            },
        );

        let mut ep =
            crate::config::KafkaEndpointConfig::from_uri("kafka:orders?brokerName=tuned").unwrap();
        apply_broker_name(&mut ep, &global).expect("resolve");
        assert_eq!(
            ep.rdkafka_config.get("max.message.bytes").unwrap(),
            "10485760"
        );
    }

    #[test]
    fn test_apply_broker_name_error_includes_available() {
        let mut global = KafkaConfig::default();
        global
            .brokers_named
            .insert("prod".into(), KafkaBrokerConfig::default());
        global
            .brokers_named
            .insert("dev".into(), KafkaBrokerConfig::default());

        let mut ep =
            crate::config::KafkaEndpointConfig::from_uri("kafka:orders?brokerName=nope").unwrap();
        let err = apply_broker_name(&mut ep, &global).unwrap_err();
        assert!(err.to_string().contains("prod") && err.to_string().contains("dev"));
    }

    #[test]
    fn test_apply_broker_name_then_apply_defaults_preserves_named() {
        let mut global = KafkaConfig {
            brokers: "default-cluster:9092".into(),
            ..KafkaConfig::default()
        };
        global.brokers_named.insert(
            "prod".into(),
            KafkaBrokerConfig {
                brokers: "named:9092".into(),
                ..KafkaBrokerConfig::default()
            },
        );

        let mut ep =
            crate::config::KafkaEndpointConfig::from_uri("kafka:orders?brokerName=prod").unwrap();
        apply_broker_name(&mut ep, &global).expect("resolve");
        assert_eq!(ep.brokers.as_deref(), Some("named:9092"));

        // apply_defaults should NOT override the named value
        ep.apply_defaults(&global);
        assert_eq!(ep.brokers.as_deref(), Some("named:9092"));
    }
}
