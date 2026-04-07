use camel_component_api::CamelError;
use camel_component_api::UriConfig;
use rdkafka::config::ClientConfig;
use tracing::warn;

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

// --- KafkaConfig (global defaults, no serde) ---

/// Global Kafka configuration defaults.
///
/// This struct holds component-level defaults that can be set via YAML config
/// and applied to endpoint configurations when specific values aren't provided.
#[derive(Debug, Clone, PartialEq)]
pub struct KafkaConfig {
    pub brokers: String,
    pub group_id: String,
    pub session_timeout_ms: u32,
    pub request_timeout_ms: u32,
    pub auto_offset_reset: String,
    pub security_protocol: String,
}

impl Default for KafkaConfig {
    fn default() -> Self {
        Self {
            brokers: "localhost:9092".to_string(),
            group_id: "camel".to_string(),
            session_timeout_ms: 45_000,
            request_timeout_ms: 30_000,
            auto_offset_reset: "latest".to_string(),
            security_protocol: "plaintext".to_string(),
        }
    }
}

impl KafkaConfig {
    pub fn with_brokers(mut self, v: impl Into<String>) -> Self {
        self.brokers = v.into();
        self
    }
    pub fn with_group_id(mut self, v: impl Into<String>) -> Self {
        self.group_id = v.into();
        self
    }
    pub fn with_session_timeout_ms(mut self, v: u32) -> Self {
        self.session_timeout_ms = v;
        self
    }
    pub fn with_request_timeout_ms(mut self, v: u32) -> Self {
        self.request_timeout_ms = v;
        self
    }
    pub fn with_auto_offset_reset(mut self, v: impl Into<String>) -> Self {
        self.auto_offset_reset = v.into();
        self
    }
    pub fn with_security_protocol(mut self, v: impl Into<String>) -> Self {
        self.security_protocol = v.into();
        self
    }
}

// --- KafkaEndpointConfig (parsed from URI) ---

/// Configuration parsed from a Kafka URI.
///
/// Format: `kafka:topic?brokers=localhost:9092&groupId=camel&...`
///
/// # Fields with Global Defaults (Option<T>)
///
/// These fields can be set via global defaults in `Camel.toml`. They are `Option<T>`
/// to distinguish between "not set by URI" (`None`) and "explicitly set by URI" (`Some(v)`).
/// After calling `apply_defaults()` + `resolve_defaults()`, all are guaranteed `Some`.
///
/// - `brokers` - Comma-separated broker addresses
/// - `group_id` - Consumer group ID
/// - `session_timeout_ms` - Session timeout in milliseconds
/// - `request_timeout_ms` - Request timeout in milliseconds
/// - `auto_offset_reset` - Auto offset reset policy ("earliest", "latest", or "none")
/// - `security_protocol` - Security protocol enum
///
/// # Fields Without Global Defaults
///
/// These fields are per-endpoint only and have no global defaults:
///
/// - `topic` - Kafka topic name (path component, required)
/// - `poll_timeout_ms` - Poll timeout in milliseconds (default: 5000)
/// - `max_poll_records` - Max poll records (default: 500)
/// - `acks` - Producer acks setting, must be "0", "1", or "all" (default: "all")
/// - `sasl_auth_type` - SASL authentication type (default: None)
/// - `sasl_username` - SASL username (required for SASL mechanisms except None and Ssl)
/// - `sasl_password` - SASL password (required for SASL mechanisms except None and Ssl)
/// - `ssl_keystore_location` - SSL keystore location
/// - `ssl_keystore_password` - SSL keystore password
/// - `ssl_truststore_location` - SSL truststore location
/// - `ssl_truststore_password` - SSL truststore password
/// - `allow_manual_commit` - Allow manual commit (default: false)
#[derive(Clone)]
pub struct KafkaEndpointConfig {
    /// Kafka topic name (path component).
    pub topic: String,

    /// Comma-separated broker addresses. `None` if not set in URI.
    /// Filled by `apply_defaults()` from global config, then `resolve_defaults()`.
    pub brokers: Option<String>,

    /// Consumer group ID. `None` if not set in URI.
    /// Filled by `apply_defaults()` from global config, then `resolve_defaults()`.
    pub group_id: Option<String>,

    /// Auto offset reset policy. `None` if not set in URI.
    /// Must be "earliest", "latest", or "none" when set.
    /// Filled by `apply_defaults()` from global config, then `resolve_defaults()`.
    pub auto_offset_reset: Option<String>,

    /// Session timeout in milliseconds. `None` if not set in URI.
    /// Filled by `apply_defaults()` from global config, then `resolve_defaults()`.
    pub session_timeout_ms: Option<u32>,

    /// Poll timeout in milliseconds. Default: 5000.
    pub poll_timeout_ms: u32,

    /// Max poll records. Default: 500.
    pub max_poll_records: u32,

    /// Producer acks setting. Default: "all".
    /// Must be "0", "1", or "all".
    pub acks: String,

    /// Request timeout in milliseconds. `None` if not set in URI.
    /// Filled by `apply_defaults()` from global config, then `resolve_defaults()`.
    pub request_timeout_ms: Option<u32>,

    /// Security protocol. `None` if not set in URI.
    /// Filled by `apply_defaults()` from global config, then `resolve_defaults()`.
    pub security_protocol: Option<SecurityProtocol>,

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

impl KafkaEndpointConfig {
    /// Parse a Kafka URI into configuration.
    ///
    /// # Example
    /// ```ignore
    /// let config = KafkaEndpointConfig::from_uri("kafka:my-topic?brokers=localhost:9092&groupId=my-group")?;
    /// ```
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = camel_component_api::parse_uri(uri)?;
        Self::from_components(parts)
    }

    /// Parse already-extracted URI components into configuration.
    pub fn from_components(parts: camel_component_api::UriComponents) -> Result<Self, CamelError> {
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

        // Parse parameters with global defaults as Option<T>
        // Fields that can have global defaults are None when not set in URI
        let brokers = parts.params.get("brokers").cloned();

        let group_id = parts.params.get("groupId").cloned();

        let auto_offset_reset = parts.params.get("autoOffsetReset").cloned();

        let session_timeout_ms = parts
            .params
            .get("sessionTimeoutMs")
            .and_then(|s| s.parse().ok());

        // Fields without global defaults use hardcoded defaults
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
            .and_then(|s| s.parse().ok());

        let security_protocol = parts
            .params
            .get("securityProtocol")
            .map(|s| s.parse::<SecurityProtocol>())
            .transpose()
            .map_err(CamelError::InvalidUri)?;

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

        // Validate auto_offset_reset only if set in URI
        // If None, it will be filled from defaults and validated later
        if let Some(ref aor) = self.auto_offset_reset
            && !matches!(aor.as_str(), "earliest" | "latest" | "none")
        {
            return Err(CamelError::InvalidUri(format!(
                "autoOffsetReset must be 'earliest', 'latest', or 'none', got '{}'",
                aor
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

    /// Apply global defaults to any `None` fields.
    ///
    /// This method fills in default values from the provided `KafkaConfig` for
    /// fields that are `None` (not set in URI). It's intended to be called after
    /// parsing a URI when global component defaults should be applied.
    pub fn apply_defaults(&mut self, defaults: &KafkaConfig) {
        if self.brokers.is_none() {
            self.brokers = Some(defaults.brokers.clone());
        }
        if self.group_id.is_none() {
            self.group_id = Some(defaults.group_id.clone());
        }
        if self.session_timeout_ms.is_none() {
            self.session_timeout_ms = Some(defaults.session_timeout_ms);
        }
        if self.request_timeout_ms.is_none() {
            self.request_timeout_ms = Some(defaults.request_timeout_ms);
        }
        if self.auto_offset_reset.is_none() {
            self.auto_offset_reset = Some(defaults.auto_offset_reset.clone());
        }
        if self.security_protocol.is_none() {
            self.security_protocol = Some(
                defaults
                    .security_protocol
                    .parse::<SecurityProtocol>()
                    .unwrap_or_else(|e| {
                        warn!(
                            "Invalid security_protocol '{}' in config ({}); using Plaintext",
                            defaults.security_protocol, e
                        );
                        SecurityProtocol::Plaintext
                    }),
            );
        }
    }

    /// Resolve any remaining `None` fields to hardcoded defaults.
    ///
    /// This should be called after `apply_defaults()` to ensure all fields
    /// that can have global defaults are guaranteed to be `Some`.
    pub fn resolve_defaults(&mut self) {
        let defaults = KafkaConfig::default();
        if self.brokers.is_none() {
            self.brokers = Some(defaults.brokers);
        }
        if self.group_id.is_none() {
            self.group_id = Some(defaults.group_id);
        }
        if self.session_timeout_ms.is_none() {
            self.session_timeout_ms = Some(defaults.session_timeout_ms);
        }
        if self.request_timeout_ms.is_none() {
            self.request_timeout_ms = Some(defaults.request_timeout_ms);
        }
        if self.auto_offset_reset.is_none() {
            self.auto_offset_reset = Some(defaults.auto_offset_reset);
        }
        if self.security_protocol.is_none() {
            self.security_protocol = Some(
                defaults
                    .security_protocol
                    .parse::<SecurityProtocol>()
                    .unwrap_or_else(|e| {
                        warn!(
                            "Invalid security_protocol '{}' in config ({}); using Plaintext",
                            defaults.security_protocol, e
                        );
                        SecurityProtocol::Plaintext
                    }),
            );
        }
    }
}

impl std::fmt::Debug for KafkaEndpointConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KafkaEndpointConfig")
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
impl UriConfig for KafkaEndpointConfig {
    fn scheme() -> &'static str {
        "kafka"
    }

    fn from_uri(uri: &str) -> Result<Self, CamelError> {
        // Call inherent method explicitly using the concrete type
        KafkaEndpointConfig::from_uri(uri)
    }

    fn from_components(parts: camel_component_api::UriComponents) -> Result<Self, CamelError> {
        // Call inherent method explicitly using the concrete type
        KafkaEndpointConfig::from_components(parts)
    }

    fn validate(self) -> Result<Self, CamelError> {
        // Call inherent method explicitly
        KafkaEndpointConfig::validate(self)
    }
}

/// Apply security-related settings from `KafkaEndpointConfig` to an rdkafka `ClientConfig`.
/// Call this after setting the basic fields (brokers, group.id, etc.) and before `.create()`.
pub fn apply_security_config(config: &KafkaEndpointConfig, cc: &mut ClientConfig) {
    // security_protocol is guaranteed Some after resolve_defaults()
    let security_protocol = config
        .security_protocol
        .expect("security_protocol must be Some after resolve_defaults()");
    cc.set(
        "security.protocol",
        match security_protocol {
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
        let mut c = KafkaEndpointConfig::from_uri("kafka:orders").unwrap();
        // After from_uri, global-default fields are None
        assert_eq!(c.topic, "orders");
        assert!(c.brokers.is_none());
        assert!(c.group_id.is_none());
        assert!(c.auto_offset_reset.is_none());
        assert!(c.session_timeout_ms.is_none());
        assert_eq!(c.poll_timeout_ms, 5000);
        assert_eq!(c.max_poll_records, 500);
        assert_eq!(c.acks, "all");
        assert!(c.request_timeout_ms.is_none());

        // After resolve_defaults, all are filled
        c.resolve_defaults();
        assert_eq!(c.brokers.as_deref(), Some("localhost:9092"));
        assert_eq!(c.group_id.as_deref(), Some("camel"));
        assert_eq!(c.auto_offset_reset.as_deref(), Some("latest"));
        assert_eq!(c.session_timeout_ms, Some(45000));
        assert_eq!(c.request_timeout_ms, Some(30000));
    }

    #[test]
    fn test_config_custom_params() {
        let c = KafkaEndpointConfig::from_uri(
            "kafka:events?brokers=kafka:9092&groupId=svc&autoOffsetReset=earliest\
             &sessionTimeoutMs=10000&pollTimeoutMs=1000&maxPollRecords=100\
             &acks=1&requestTimeoutMs=5000",
        )
        .unwrap();
        assert_eq!(c.topic, "events");
        assert_eq!(c.brokers.as_deref(), Some("kafka:9092"));
        assert_eq!(c.group_id.as_deref(), Some("svc"));
        assert_eq!(c.auto_offset_reset.as_deref(), Some("earliest"));
        assert_eq!(c.session_timeout_ms, Some(10000));
        assert_eq!(c.poll_timeout_ms, 1000);
        assert_eq!(c.max_poll_records, 100);
        assert_eq!(c.acks, "1");
        assert_eq!(c.request_timeout_ms, Some(5000));
    }

    #[test]
    fn test_config_missing_topic_fails() {
        // Empty topic in path should fail
        let result = KafkaEndpointConfig::from_uri("kafka:?brokers=localhost:9092");
        assert!(result.is_err());
    }

    #[test]
    fn test_config_invalid_scheme_fails() {
        let result = KafkaEndpointConfig::from_uri("redis:orders");
        assert!(result.is_err());
    }

    #[test]
    fn test_config_invalid_acks_fails() {
        let result = KafkaEndpointConfig::from_uri("kafka:orders?acks=invalid");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("acks"), "error should mention 'acks': {msg}");
    }

    #[test]
    fn test_config_invalid_auto_offset_reset_fails() {
        let result = KafkaEndpointConfig::from_uri("kafka:orders?autoOffsetReset=bad");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("autoOffsetReset"),
            "error should mention 'autoOffsetReset': {msg}"
        );
    }

    #[test]
    fn test_security_protocol_default() {
        let c = KafkaEndpointConfig::from_uri("kafka:orders").unwrap();
        // Not set in URI, so should be None
        assert!(c.security_protocol.is_none());
    }

    #[test]
    fn test_sasl_auth_type_default() {
        let c = KafkaEndpointConfig::from_uri("kafka:orders").unwrap();
        assert_eq!(c.sasl_auth_type, SaslAuthType::None);
    }

    #[test]
    fn test_allow_manual_commit_default_false() {
        let c = KafkaEndpointConfig::from_uri("kafka:orders").unwrap();
        assert!(!c.allow_manual_commit);
    }

    #[test]
    fn test_sasl_ssl_config_parsing() {
        let c = KafkaEndpointConfig::from_uri(
            "kafka:orders?securityProtocol=SASL_SSL&saslAuthType=SCRAM_SHA_512\
             &saslUsername=user&saslPassword=pass",
        )
        .unwrap();
        assert_eq!(c.security_protocol, Some(SecurityProtocol::SaslSsl));
        assert_eq!(c.sasl_auth_type, SaslAuthType::ScramSha512);
        assert_eq!(c.sasl_username, Some("user".to_string()));
        assert_eq!(c.sasl_password, Some("pass".to_string()));
    }

    #[test]
    fn test_ssl_only_config_parsing() {
        let c = KafkaEndpointConfig::from_uri(
            "kafka:orders?securityProtocol=SSL\
             &sslKeystoreLocation=/keystore.p12&sslKeystorePassword=ks\
             &sslTruststoreLocation=/truststore.jks&sslTruststorePassword=ts",
        )
        .unwrap();
        assert_eq!(c.security_protocol, Some(SecurityProtocol::Ssl));
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
        let c = KafkaEndpointConfig::from_uri("kafka:orders?allowManualCommit=true").unwrap();
        assert!(c.allow_manual_commit);
    }

    #[test]
    fn test_sasl_without_username_fails() {
        let result = KafkaEndpointConfig::from_uri(
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
        let result = KafkaEndpointConfig::from_uri(
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
        let c = KafkaEndpointConfig::from_uri("kafka:orders?securityProtocol=SSL&saslAuthType=SSL")
            .unwrap();
        assert_eq!(c.sasl_auth_type, SaslAuthType::Ssl);
        assert_eq!(c.sasl_username, None);
    }

    #[test]
    fn test_invalid_security_protocol_fails() {
        let result = KafkaEndpointConfig::from_uri("kafka:orders?securityProtocol=BOGUS");
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_sasl_auth_type_fails() {
        let result = KafkaEndpointConfig::from_uri("kafka:orders?saslAuthType=BOGUS");
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
        let mut config = KafkaEndpointConfig::from_uri(
            "kafka:t?securityProtocol=SASL_SSL&saslAuthType=SCRAM_SHA_512\
             &saslUsername=user&saslPassword=pass",
        )
        .unwrap();
        config.resolve_defaults();
        let mut cc = ClientConfig::new();
        apply_security_config(&config, &mut cc);
        // Does not panic — rdkafka stores values internally; just verify no crash
    }

    #[test]
    fn test_apply_ssl_only_does_not_set_sasl_mechanism() {
        let mut config = KafkaEndpointConfig::from_uri(
            "kafka:t?securityProtocol=SSL\
             &sslKeystoreLocation=/k&sslKeystorePassword=ks",
        )
        .unwrap();
        config.resolve_defaults();
        let mut cc = ClientConfig::new();
        apply_security_config(&config, &mut cc); // must not panic
    }

    #[test]
    fn test_apply_plaintext_is_noop() {
        let mut config = KafkaEndpointConfig::from_uri("kafka:t").unwrap();
        config.resolve_defaults();
        let mut cc = ClientConfig::new();
        apply_security_config(&config, &mut cc); // must not panic
    }

    #[test]
    fn test_debug_masks_passwords() {
        let config = KafkaEndpointConfig::from_uri(
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

#[cfg(test)]
mod kafka_config_tests {
    use super::*;

    #[test]
    fn test_kafka_config_defaults() {
        let cfg = KafkaConfig::default();
        assert_eq!(cfg.brokers, "localhost:9092");
        assert_eq!(cfg.group_id, "camel");
        assert_eq!(cfg.session_timeout_ms, 45_000);
        assert_eq!(cfg.security_protocol, "plaintext");
    }

    #[test]
    fn test_kafka_config_builder() {
        let cfg = KafkaConfig::default()
            .with_brokers("prod:9092")
            .with_group_id("my-group");
        assert_eq!(cfg.brokers, "prod:9092");
        assert_eq!(cfg.group_id, "my-group");
        assert_eq!(cfg.session_timeout_ms, 45_000); // unchanged
    }

    #[test]
    fn test_kafka_endpoint_config_apply_defaults() {
        // Create an endpoint config with None fields (simulating from_uri without params)
        let mut endpoint = KafkaEndpointConfig {
            topic: "test".to_string(),
            brokers: None,
            group_id: None,
            auto_offset_reset: None,
            session_timeout_ms: None,
            poll_timeout_ms: 5000,
            max_poll_records: 500,
            acks: "all".to_string(),
            request_timeout_ms: None,
            security_protocol: None,
            sasl_auth_type: SaslAuthType::None,
            sasl_username: None,
            sasl_password: None,
            ssl_keystore_location: None,
            ssl_keystore_password: None,
            ssl_truststore_location: None,
            ssl_truststore_password: None,
            allow_manual_commit: false,
        };

        let defaults = KafkaConfig::default()
            .with_brokers("kafka:9092")
            .with_group_id("my-group");

        endpoint.apply_defaults(&defaults);

        // Defaults should be applied to None fields
        assert_eq!(endpoint.brokers.as_deref(), Some("kafka:9092"));
        assert_eq!(endpoint.group_id.as_deref(), Some("my-group"));
        assert_eq!(endpoint.session_timeout_ms, Some(45_000));
        assert_eq!(endpoint.request_timeout_ms, Some(30_000));
        assert_eq!(endpoint.auto_offset_reset.as_deref(), Some("latest"));
        assert_eq!(
            endpoint.security_protocol,
            Some(SecurityProtocol::Plaintext)
        );
    }

    #[test]
    fn test_kafka_endpoint_config_apply_defaults_preserves_values() {
        // Create an endpoint config with values already set via URI
        let mut endpoint = KafkaEndpointConfig::from_uri(
            "kafka:orders?brokers=custom:9092&groupId=custom-group&sessionTimeoutMs=10000",
        )
        .unwrap();

        let defaults = KafkaConfig::default()
            .with_brokers("should-not-override:9092")
            .with_group_id("should-not-override");

        endpoint.apply_defaults(&defaults);

        // Existing values should be preserved
        assert_eq!(endpoint.brokers.as_deref(), Some("custom:9092"));
        assert_eq!(endpoint.group_id.as_deref(), Some("custom-group"));
        assert_eq!(endpoint.session_timeout_ms, Some(10_000));
    }

    /// Critical test: catches the original defect where `from_uri` filled absent
    /// params with hardcoded defaults, preventing `apply_defaults` from working.
    #[test]
    fn test_apply_defaults_with_from_uri_no_broker_param() {
        // Parse URI without brokers= param
        let mut ep = KafkaEndpointConfig::from_uri("kafka:orders").unwrap();
        // At this point, ep.brokers should be None (not "localhost:9092")
        assert!(
            ep.brokers.is_none(),
            "brokers should be None when not in URI, got {:?}",
            ep.brokers
        );

        let defaults = KafkaConfig::default().with_brokers("kafka-prod:9092");
        ep.apply_defaults(&defaults);
        // Custom default should be applied
        assert_eq!(
            ep.brokers.as_deref(),
            Some("kafka-prod:9092"),
            "custom default brokers should be applied"
        );
    }

    #[test]
    fn test_apply_defaults_security_protocol() {
        // Parse URI without securityProtocol param
        let mut ep = KafkaEndpointConfig::from_uri("kafka:orders").unwrap();
        assert!(
            ep.security_protocol.is_none(),
            "security_protocol should be None when not in URI"
        );

        let defaults = KafkaConfig::default().with_security_protocol("SSL");
        ep.apply_defaults(&defaults);
        assert_eq!(
            ep.security_protocol,
            Some(SecurityProtocol::Ssl),
            "custom security_protocol should be applied"
        );
    }

    #[test]
    fn test_resolve_defaults_fills_remaining_nones() {
        let mut ep = KafkaEndpointConfig::from_uri("kafka:orders").unwrap();
        // All global-default fields should be None
        assert!(ep.brokers.is_none());
        assert!(ep.group_id.is_none());
        assert!(ep.session_timeout_ms.is_none());
        assert!(ep.request_timeout_ms.is_none());
        assert!(ep.auto_offset_reset.is_none());
        assert!(ep.security_protocol.is_none());

        // resolve_defaults fills them from KafkaConfig::default()
        ep.resolve_defaults();
        assert_eq!(ep.brokers.as_deref(), Some("localhost:9092"));
        assert_eq!(ep.group_id.as_deref(), Some("camel"));
        assert_eq!(ep.session_timeout_ms, Some(45_000));
        assert_eq!(ep.request_timeout_ms, Some(30_000));
        assert_eq!(ep.auto_offset_reset.as_deref(), Some("latest"));
        assert_eq!(ep.security_protocol, Some(SecurityProtocol::Plaintext));
    }
}
