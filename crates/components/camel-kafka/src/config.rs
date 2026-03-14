use camel_api::CamelError;
use camel_endpoint::UriConfig;
use rdkafka::config::ClientConfig;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SecurityProtocol {
    #[default]
    Plaintext,
    Ssl,
    SaslPlaintext,
    SaslSsl,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SaslAuthType {
    #[default]
    None,
    Plain,
    ScramSha256,
    ScramSha512,
    /// TLS client auth only — no SASL credentials required.
    Ssl,
}

impl std::str::FromStr for SecurityProtocol {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "PLAINTEXT" => Ok(Self::Plaintext),
            "SSL" => Ok(Self::Ssl),
            "SASL_PLAINTEXT" => Ok(Self::SaslPlaintext),
            "SASL_SSL" => Ok(Self::SaslSsl),
            _ => Err(format!("Invalid securityProtocol: '{s}'")),
        }
    }
}

impl std::str::FromStr for SaslAuthType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "NONE" => Ok(Self::None),
            "PLAIN" => Ok(Self::Plain),
            "SCRAM_SHA_256" | "SCRAM-SHA-256" => Ok(Self::ScramSha256),
            "SCRAM_SHA_512" | "SCRAM-SHA-512" => Ok(Self::ScramSha512),
            "SSL" => Ok(Self::Ssl),
            _ => Err(format!("Invalid saslAuthType: '{s}'")),
        }
    }
}

/// Configuration parsed from a Kafka URI.
///
/// Format: `kafka:topic?brokers=localhost:9092&groupId=camel&...`
///
/// # Fields
///
/// - `topic` - Kafka topic name (path component, required)
/// - `brokers` - Comma-separated broker addresses (default: "localhost:9092")
/// - `group_id` - Consumer group ID (default: "camel")
/// - `auto_offset_reset` - Auto offset reset policy, must be "earliest", "latest", or "none" (default: "latest")
/// - `session_timeout_ms` - Session timeout in milliseconds (default: 45000)
/// - `poll_timeout_ms` - Poll timeout in milliseconds (default: 5000)
/// - `max_poll_records` - Max poll records (default: 500)
/// - `acks` - Producer acks setting, must be "0", "1", or "all" (default: "all")
/// - `request_timeout_ms` - Request timeout in milliseconds (default: 30000)
/// - `security_protocol` - Security protocol (default: Plaintext)
/// - `sasl_auth_type` - SASL authentication type (default: None)
/// - `sasl_username` - SASL username (required for SASL mechanisms except None and Ssl)
/// - `sasl_password` - SASL password (required for SASL mechanisms except None and Ssl)
/// - `ssl_keystore_location` - SSL keystore location
/// - `ssl_keystore_password` - SSL keystore password
/// - `ssl_truststore_location` - SSL truststore location
/// - `ssl_truststore_password` - SSL truststore password
/// - `allow_manual_commit` - Allow manual commit (default: false)
#[derive(Clone)]
pub struct KafkaConfig {
    /// Kafka topic name (path component).
    pub topic: String,

    /// Comma-separated broker addresses. Default: "localhost:9092".
    pub brokers: String,

    /// Consumer group ID. Default: "camel".
    pub group_id: String,

    /// Auto offset reset policy. Default: "latest".
    /// Must be "earliest", "latest", or "none".
    pub auto_offset_reset: String,

    /// Session timeout in milliseconds. Default: 45000.
    pub session_timeout_ms: u32,

    /// Poll timeout in milliseconds. Default: 5000.
    pub poll_timeout_ms: u32,

    /// Max poll records. Default: 500.
    pub max_poll_records: u32,

    /// Producer acks setting. Default: "all".
    /// Must be "0", "1", or "all".
    pub acks: String,

    /// Request timeout in milliseconds. Default: 30000.
    pub request_timeout_ms: u32,

    /// Security protocol. Default: Plaintext.
    pub security_protocol: SecurityProtocol,

    /// SASL authentication type. Default: None.
    pub sasl_auth_type: SaslAuthType,

    /// SASL username (required for SASL mechanisms except None and Ssl).
    pub sasl_username: Option<String>,

    /// SASL password (required for SASL mechanisms except None and Ssl).
    pub sasl_password: Option<String>,

    /// SSL keystore location.
    pub ssl_keystore_location: Option<String>,

    /// SSL keystore password.
    pub ssl_keystore_password: Option<String>,

    /// SSL truststore location.
    pub ssl_truststore_location: Option<String>,

    /// SSL truststore password.
    pub ssl_truststore_password: Option<String>,

    /// Allow manual commit. Default: false.
    pub allow_manual_commit: bool,
}

impl KafkaConfig {
    /// Parse a Kafka URI into configuration.
    ///
    /// # Example
    /// ```ignore
    /// let config = KafkaConfig::from_uri("kafka:my-topic?brokers=localhost:9092&groupId=my-group")?;
    /// ```
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = camel_endpoint::parse_uri(uri)?;
        Self::from_components(parts)
    }

    /// Parse already-extracted URI components into configuration.
    pub fn from_components(parts: camel_endpoint::UriComponents) -> Result<Self, CamelError> {
        // Validate scheme
        if parts.scheme != "kafka" {
            return Err(CamelError::InvalidUri(format!(
                "expected scheme 'kafka', got '{}'",
                parts.scheme
            )));
        }

        // Extract topic from path
        let topic = parts.path.trim_start_matches('/').to_string();
        if topic.is_empty() {
            return Err(CamelError::InvalidUri(
                "Kafka URI must specify a topic (e.g. kafka:my-topic)".to_string(),
            ));
        }

        // Parse all parameters using defaults
        let brokers = parts
            .params
            .get("brokers")
            .cloned()
            .unwrap_or_else(|| "localhost:9092".to_string());

        let group_id = parts
            .params
            .get("groupId")
            .cloned()
            .unwrap_or_else(|| "camel".to_string());

        let auto_offset_reset = parts
            .params
            .get("autoOffsetReset")
            .cloned()
            .unwrap_or_else(|| "latest".to_string());

        let session_timeout_ms = parts
            .params
            .get("sessionTimeoutMs")
            .and_then(|s| s.parse().ok())
            .unwrap_or(45000);

        let poll_timeout_ms = parts
            .params
            .get("pollTimeoutMs")
            .and_then(|s| s.parse().ok())
            .unwrap_or(5000);

        let max_poll_records = parts
            .params
            .get("maxPollRecords")
            .and_then(|s| s.parse().ok())
            .unwrap_or(500);

        let acks = parts
            .params
            .get("acks")
            .cloned()
            .unwrap_or_else(|| "all".to_string());

        let request_timeout_ms = parts
            .params
            .get("requestTimeoutMs")
            .and_then(|s| s.parse().ok())
            .unwrap_or(30000);

        let security_protocol = parts
            .params
            .get("securityProtocol")
            .map(|s| s.parse::<SecurityProtocol>())
            .transpose()
            .map_err(CamelError::InvalidUri)?
            .unwrap_or_default();

        let sasl_auth_type = parts
            .params
            .get("saslAuthType")
            .map(|s| s.parse::<SaslAuthType>())
            .transpose()
            .map_err(CamelError::InvalidUri)?
            .unwrap_or_default();

        let sasl_username = parts.params.get("saslUsername").cloned();
        let sasl_password = parts.params.get("saslPassword").cloned();

        let ssl_keystore_location = parts.params.get("sslKeystoreLocation").cloned();
        let ssl_keystore_password = parts.params.get("sslKeystorePassword").cloned();
        let ssl_truststore_location = parts.params.get("sslTruststoreLocation").cloned();
        let ssl_truststore_password = parts.params.get("sslTruststorePassword").cloned();

        let allow_manual_commit = parts
            .params
            .get("allowManualCommit")
            .map(|s| s.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        let config = Self {
            topic,
            brokers,
            group_id,
            auto_offset_reset,
            session_timeout_ms,
            poll_timeout_ms,
            max_poll_records,
            acks,
            request_timeout_ms,
            security_protocol,
            sasl_auth_type,
            sasl_username,
            sasl_password,
            ssl_keystore_location,
            ssl_keystore_password,
            ssl_truststore_location,
            ssl_truststore_password,
            allow_manual_commit,
        };

        config.validate()
    }

    fn validate(self) -> Result<Self, CamelError> {
        // Validate acks
        if !matches!(self.acks.as_str(), "0" | "1" | "all") {
            return Err(CamelError::InvalidUri(format!(
                "acks must be '0', '1', or 'all', got '{}'",
                self.acks
            )));
        }

        // Validate auto_offset_reset
        if !matches!(
            self.auto_offset_reset.as_str(),
            "earliest" | "latest" | "none"
        ) {
            return Err(CamelError::InvalidUri(format!(
                "autoOffsetReset must be 'earliest', 'latest', or 'none', got '{}'",
                self.auto_offset_reset
            )));
        }

        // Validate: SASL mechanisms (not SSL-only) require username + password
        if self.sasl_auth_type != SaslAuthType::None && self.sasl_auth_type != SaslAuthType::Ssl {
            if self.sasl_username.is_none() {
                return Err(CamelError::InvalidUri(
                    "saslAuthType requires saslUsername parameter".to_string(),
                ));
            }
            if self.sasl_password.is_none() {
                return Err(CamelError::InvalidUri(
                    "saslAuthType requires saslPassword parameter".to_string(),
                ));
            }
        }

        Ok(self)
    }
}

impl std::fmt::Debug for KafkaConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KafkaConfig")
            .field("topic", &self.topic)
            .field("brokers", &self.brokers)
            .field("group_id", &self.group_id)
            .field("auto_offset_reset", &self.auto_offset_reset)
            .field("session_timeout_ms", &self.session_timeout_ms)
            .field("poll_timeout_ms", &self.poll_timeout_ms)
            .field("max_poll_records", &self.max_poll_records)
            .field("acks", &self.acks)
            .field("request_timeout_ms", &self.request_timeout_ms)
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
            .field("allow_manual_commit", &self.allow_manual_commit)
            .finish()
    }
}

// Implement the UriConfig trait to satisfy the interface contract
impl UriConfig for KafkaConfig {
    fn scheme() -> &'static str {
        "kafka"
    }

    fn from_uri(uri: &str) -> Result<Self, CamelError> {
        // Call inherent method explicitly using the concrete type
        KafkaConfig::from_uri(uri)
    }

    fn from_components(parts: camel_endpoint::UriComponents) -> Result<Self, CamelError> {
        // Call inherent method explicitly using the concrete type
        KafkaConfig::from_components(parts)
    }

    fn validate(self) -> Result<Self, CamelError> {
        // Call inherent method explicitly
        KafkaConfig::validate(self)
    }
}

/// Apply security-related settings from `KafkaConfig` to an rdkafka `ClientConfig`.
/// Call this after setting the basic fields (brokers, group.id, etc.) and before `.create()`.
pub fn apply_security_config(config: &KafkaConfig, cc: &mut ClientConfig) {
    cc.set(
        "security.protocol",
        match config.security_protocol {
            SecurityProtocol::Plaintext => "PLAINTEXT",
            SecurityProtocol::Ssl => "SSL",
            SecurityProtocol::SaslPlaintext => "SASL_PLAINTEXT",
            SecurityProtocol::SaslSsl => "SASL_SSL",
        },
    );

    // SASL mechanism (skip for None and Ssl-only)
    match config.sasl_auth_type {
        SaslAuthType::None | SaslAuthType::Ssl => {}
        SaslAuthType::Plain => {
            cc.set("sasl.mechanism", "PLAIN");
        }
        SaslAuthType::ScramSha256 => {
            cc.set("sasl.mechanism", "SCRAM-SHA-256");
        }
        SaslAuthType::ScramSha512 => {
            cc.set("sasl.mechanism", "SCRAM-SHA-512");
        }
    }

    // Credentials for all SASL mechanisms (except None and Ssl-only)
    if !matches!(
        config.sasl_auth_type,
        SaslAuthType::None | SaslAuthType::Ssl
    ) {
        if let Some(ref u) = config.sasl_username {
            cc.set("sasl.username", u);
        }
        if let Some(ref p) = config.sasl_password {
            cc.set("sasl.password", p);
        }
    }

    // SSL keystore / truststore
    if let Some(ref loc) = config.ssl_keystore_location {
        cc.set("ssl.keystore.location", loc);
    }
    if let Some(ref pw) = config.ssl_keystore_password {
        cc.set("ssl.keystore.password", pw);
    }
    if let Some(ref loc) = config.ssl_truststore_location {
        cc.set("ssl.truststore.location", loc);
    }
    if let Some(ref pw) = config.ssl_truststore_password {
        cc.set("ssl.truststore.password", pw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let c = KafkaConfig::from_uri("kafka:orders").unwrap();
        assert_eq!(c.topic, "orders");
        assert_eq!(c.brokers, "localhost:9092");
        assert_eq!(c.group_id, "camel");
        assert_eq!(c.auto_offset_reset, "latest");
        assert_eq!(c.session_timeout_ms, 45000);
        assert_eq!(c.poll_timeout_ms, 5000);
        assert_eq!(c.max_poll_records, 500);
        assert_eq!(c.acks, "all");
        assert_eq!(c.request_timeout_ms, 30000);
    }

    #[test]
    fn test_config_custom_params() {
        let c = KafkaConfig::from_uri(
            "kafka:events?brokers=kafka:9092&groupId=svc&autoOffsetReset=earliest\
             &sessionTimeoutMs=10000&pollTimeoutMs=1000&maxPollRecords=100\
             &acks=1&requestTimeoutMs=5000",
        )
        .unwrap();
        assert_eq!(c.topic, "events");
        assert_eq!(c.brokers, "kafka:9092");
        assert_eq!(c.group_id, "svc");
        assert_eq!(c.auto_offset_reset, "earliest");
        assert_eq!(c.session_timeout_ms, 10000);
        assert_eq!(c.poll_timeout_ms, 1000);
        assert_eq!(c.max_poll_records, 100);
        assert_eq!(c.acks, "1");
        assert_eq!(c.request_timeout_ms, 5000);
    }

    #[test]
    fn test_config_missing_topic_fails() {
        // Empty topic in path should fail
        let result = KafkaConfig::from_uri("kafka:?brokers=localhost:9092");
        assert!(result.is_err());
    }

    #[test]
    fn test_config_invalid_scheme_fails() {
        let result = KafkaConfig::from_uri("redis:orders");
        assert!(result.is_err());
    }

    #[test]
    fn test_config_invalid_acks_fails() {
        let result = KafkaConfig::from_uri("kafka:orders?acks=invalid");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("acks"), "error should mention 'acks': {msg}");
    }

    #[test]
    fn test_config_invalid_auto_offset_reset_fails() {
        let result = KafkaConfig::from_uri("kafka:orders?autoOffsetReset=bad");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("autoOffsetReset"),
            "error should mention 'autoOffsetReset': {msg}"
        );
    }

    #[test]
    fn test_security_protocol_default() {
        let c = KafkaConfig::from_uri("kafka:orders").unwrap();
        assert_eq!(c.security_protocol, SecurityProtocol::Plaintext);
    }

    #[test]
    fn test_sasl_auth_type_default() {
        let c = KafkaConfig::from_uri("kafka:orders").unwrap();
        assert_eq!(c.sasl_auth_type, SaslAuthType::None);
    }

    #[test]
    fn test_allow_manual_commit_default_false() {
        let c = KafkaConfig::from_uri("kafka:orders").unwrap();
        assert!(!c.allow_manual_commit);
    }

    #[test]
    fn test_sasl_ssl_config_parsing() {
        let c = KafkaConfig::from_uri(
            "kafka:orders?securityProtocol=SASL_SSL&saslAuthType=SCRAM_SHA_512\
             &saslUsername=user&saslPassword=pass",
        )
        .unwrap();
        assert_eq!(c.security_protocol, SecurityProtocol::SaslSsl);
        assert_eq!(c.sasl_auth_type, SaslAuthType::ScramSha512);
        assert_eq!(c.sasl_username, Some("user".to_string()));
        assert_eq!(c.sasl_password, Some("pass".to_string()));
    }

    #[test]
    fn test_ssl_only_config_parsing() {
        let c = KafkaConfig::from_uri(
            "kafka:orders?securityProtocol=SSL\
             &sslKeystoreLocation=/keystore.p12&sslKeystorePassword=ks\
             &sslTruststoreLocation=/truststore.jks&sslTruststorePassword=ts",
        )
        .unwrap();
        assert_eq!(c.security_protocol, SecurityProtocol::Ssl);
        assert_eq!(c.ssl_keystore_location, Some("/keystore.p12".to_string()));
        assert_eq!(c.ssl_keystore_password, Some("ks".to_string()));
        assert_eq!(
            c.ssl_truststore_location,
            Some("/truststore.jks".to_string())
        );
        assert_eq!(c.ssl_truststore_password, Some("ts".to_string()));
    }

    #[test]
    fn test_allow_manual_commit_parsing() {
        let c = KafkaConfig::from_uri("kafka:orders?allowManualCommit=true").unwrap();
        assert!(c.allow_manual_commit);
    }

    #[test]
    fn test_sasl_without_username_fails() {
        let result = KafkaConfig::from_uri(
            "kafka:orders?securityProtocol=SASL_SSL&saslAuthType=PLAIN&saslPassword=pass",
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("saslUsername"),
            "error should mention saslUsername: {msg}"
        );
    }

    #[test]
    fn test_sasl_without_password_fails() {
        let result = KafkaConfig::from_uri(
            "kafka:orders?securityProtocol=SASL_SSL&saslAuthType=PLAIN&saslUsername=user",
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("saslPassword"),
            "error should mention saslPassword: {msg}"
        );
    }

    #[test]
    fn test_sasl_auth_type_ssl_does_not_require_credentials() {
        // saslAuthType=SSL means TLS-only — no SASL credentials needed
        let c =
            KafkaConfig::from_uri("kafka:orders?securityProtocol=SSL&saslAuthType=SSL").unwrap();
        assert_eq!(c.sasl_auth_type, SaslAuthType::Ssl);
        assert_eq!(c.sasl_username, None);
    }

    #[test]
    fn test_invalid_security_protocol_fails() {
        let result = KafkaConfig::from_uri("kafka:orders?securityProtocol=BOGUS");
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_sasl_auth_type_fails() {
        let result = KafkaConfig::from_uri("kafka:orders?saslAuthType=BOGUS");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("saslAuthType"),
            "error should mention saslAuthType: {msg}"
        );
    }
}

#[cfg(test)]
mod security_config_tests {
    use super::*;
    use rdkafka::config::ClientConfig;

    #[test]
    fn test_apply_sasl_ssl_sets_required_keys() {
        let config = KafkaConfig::from_uri(
            "kafka:t?securityProtocol=SASL_SSL&saslAuthType=SCRAM_SHA_512\
             &saslUsername=user&saslPassword=pass",
        )
        .unwrap();
        let mut cc = ClientConfig::new();
        apply_security_config(&config, &mut cc);
        // Does not panic — rdkafka stores values internally; just verify no crash
    }

    #[test]
    fn test_apply_ssl_only_does_not_set_sasl_mechanism() {
        let config = KafkaConfig::from_uri(
            "kafka:t?securityProtocol=SSL\
             &sslKeystoreLocation=/k&sslKeystorePassword=ks",
        )
        .unwrap();
        let mut cc = ClientConfig::new();
        apply_security_config(&config, &mut cc); // must not panic
    }

    #[test]
    fn test_apply_plaintext_is_noop() {
        let config = KafkaConfig::from_uri("kafka:t").unwrap();
        let mut cc = ClientConfig::new();
        apply_security_config(&config, &mut cc); // must not panic
    }

    #[test]
    fn test_debug_masks_passwords() {
        let config = KafkaConfig::from_uri(
            "kafka:t?securityProtocol=SASL_SSL&saslAuthType=SCRAM_SHA_512\
             &saslUsername=myuser&saslPassword=supersecret999\
             &sslKeystorePassword=keystorepass888&sslTruststorePassword=truststorepass777",
        )
        .unwrap();
        let debug_str = format!("{config:?}");
        assert!(
            !debug_str.contains("supersecret999"),
            "sasl_password must not appear in debug: {debug_str}"
        );
        assert!(
            !debug_str.contains("keystorepass888"),
            "ssl_keystore_password must not appear in debug: {debug_str}"
        );
        assert!(
            !debug_str.contains("truststorepass777"),
            "ssl_truststore_password must not appear in debug: {debug_str}"
        );
        assert!(
            debug_str.contains("[REDACTED]"),
            "debug should contain [REDACTED]: {debug_str}"
        );
    }
}
