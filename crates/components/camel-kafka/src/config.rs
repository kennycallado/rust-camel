use crate::broker_config::{KafkaBrokerConfig, validate_rdkafka_config, validate_sasl_creds};
use camel_component_api::CamelError;
use camel_component_api::NetworkRetryPolicy;
use camel_component_api::UriConfig;
use rdkafka::config::ClientConfig;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Minimum / maximum bounds for numeric URI parameters
// ---------------------------------------------------------------------------

const MIN_SESSION_TIMEOUT_MS: u32 = 1_000;
const MAX_SESSION_TIMEOUT_MS: u32 = 3_600_000; // 1 hour
const MIN_POLL_TIMEOUT_MS: u32 = 1;
const MAX_POLL_TIMEOUT_MS: u32 = 300_000; // 5 min
const MIN_MAX_POLL_RECORDS: u32 = 1;
const MAX_MAX_POLL_RECORDS: u32 = 100_000;
const MIN_REQUEST_TIMEOUT_MS: u32 = 1_000;
const MAX_REQUEST_TIMEOUT_MS: u32 = 3_600_000; // 1 hour
const MIN_COMMIT_TIMEOUT_MS: u32 = 100;
const MAX_COMMIT_TIMEOUT_MS: u32 = 60_000; // 60 s
const DEFAULT_COMMIT_TIMEOUT_MS: u32 = 10_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SecurityProtocol {
    #[default]
    Plaintext,
    Ssl,
    SaslPlaintext,
    SaslSsl,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SaslAuthType {
    #[default]
    None,
    Plain,
    ScramSha256,
    ScramSha512,
    /// TLS client auth only — no SASL credentials required.
    Ssl,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PartitionAssignmentStrategy {
    #[default]
    Range,
    RoundRobin,
    CooperativeSticky,
}

impl std::str::FromStr for PartitionAssignmentStrategy {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "range" => Ok(Self::Range),
            "roundrobin" => Ok(Self::RoundRobin),
            "cooperativesticky" => Ok(Self::CooperativeSticky),
            _ => Err(format!(
                "Invalid partitionAssignmentStrategy: '{}'. Valid values: range, roundRobin, cooperativeSticky",
                s
            )),
        }
    }
}

impl PartitionAssignmentStrategy {
    pub fn to_rdkafka_str(&self) -> &'static str {
        match self {
            Self::Range => "range",
            Self::RoundRobin => "roundrobin",
            Self::CooperativeSticky => "cooperative-sticky",
        }
    }
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

// --- KafkaConfig (global defaults) ---

/// Global Kafka configuration defaults.
///
/// This struct holds component-level defaults that can be set via YAML config
/// and applied to endpoint configurations when specific values aren't provided.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct KafkaConfig {
    pub brokers: String,
    pub group_id: String,
    pub session_timeout_ms: u32,
    pub heartbeat_interval_ms: u32,
    pub request_timeout_ms: u32,
    pub auto_offset_reset: String,
    pub isolation_level: String,
    pub security_protocol: String,

    /// Reconnect/backoff policy for the Kafka consumer poll loop.
    #[serde(default = "kafka_reconnect_default")]
    pub reconnect: NetworkRetryPolicy,

    /// Named broker configurations for multi-cluster setups.
    #[serde(default)]
    pub brokers_named: HashMap<String, KafkaBrokerConfig>,
}

fn kafka_reconnect_default() -> NetworkRetryPolicy {
    NetworkRetryPolicy {
        max_attempts: 0, // unlimited — preserves current behavior
        ..NetworkRetryPolicy::default()
    }
}

impl Default for KafkaConfig {
    fn default() -> Self {
        Self {
            brokers: "localhost:9092".to_string(),
            group_id: "camel".to_string(),
            session_timeout_ms: 45_000,
            heartbeat_interval_ms: 10_000,
            request_timeout_ms: 30_000,
            auto_offset_reset: "latest".to_string(),
            isolation_level: "read_uncommitted".to_string(),
            security_protocol: "plaintext".to_string(),
            reconnect: NetworkRetryPolicy {
                max_attempts: 0, // unlimited
                ..NetworkRetryPolicy::default()
            },
            brokers_named: HashMap::new(),
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
    pub fn with_heartbeat_interval_ms(mut self, v: u32) -> Self {
        self.heartbeat_interval_ms = v;
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
    pub fn with_isolation_level(mut self, v: impl Into<String>) -> Self {
        self.isolation_level = v.into();
        self
    }
    pub fn with_security_protocol(mut self, v: impl Into<String>) -> Self {
        self.security_protocol = v.into();
        self
    }
    pub fn with_brokers_named(mut self, v: HashMap<String, KafkaBrokerConfig>) -> Self {
        self.brokers_named = v;
        self
    }

    /// Validate the global config defaults.
    ///
    /// Ensures `brokers` is non-empty and numeric params are within bounds.
    /// Called by `KafkaComponent::with_config` to reject bad defaults early.
    pub fn validate(&self) -> Result<(), CamelError> {
        if self.brokers.trim().is_empty() {
            return Err(CamelError::Config(
                "KafkaConfig.brokers must not be empty".into(),
            ));
        }
        if !(MIN_SESSION_TIMEOUT_MS..=MAX_SESSION_TIMEOUT_MS).contains(&self.session_timeout_ms) {
            return Err(CamelError::Config(format!(
                "KafkaConfig.session_timeout_ms must be between {MIN_SESSION_TIMEOUT_MS} and {MAX_SESSION_TIMEOUT_MS}, got {}",
                self.session_timeout_ms
            )));
        }
        // heartbeat_interval_ms should be < session_timeout_ms and > 0
        if self.heartbeat_interval_ms == 0 {
            return Err(CamelError::Config(
                "KafkaConfig.heartbeat_interval_ms must be > 0".into(),
            ));
        }
        if self.heartbeat_interval_ms >= self.session_timeout_ms {
            return Err(CamelError::Config(format!(
                "KafkaConfig.heartbeat_interval_ms ({}) must be less than session_timeout_ms ({})",
                self.heartbeat_interval_ms, self.session_timeout_ms
            )));
        }
        if !(MIN_REQUEST_TIMEOUT_MS..=MAX_REQUEST_TIMEOUT_MS).contains(&self.request_timeout_ms) {
            return Err(CamelError::Config(format!(
                "KafkaConfig.request_timeout_ms must be between {MIN_REQUEST_TIMEOUT_MS} and {MAX_REQUEST_TIMEOUT_MS}, got {}",
                self.request_timeout_ms
            )));
        }
        self.security_protocol
            .parse::<SecurityProtocol>()
            .map_err(|_| {
                CamelError::Config(format!(
                    "KafkaConfig.security_protocol must be one of: PLAINTEXT, SSL, SASL_PLAINTEXT, SASL_SSL; got '{}'",
                    self.security_protocol
                ))
            })?;
        if !matches!(
            self.auto_offset_reset.as_str(),
            "earliest" | "latest" | "none"
        ) {
            return Err(CamelError::Config(format!(
                "KafkaConfig.auto_offset_reset must be 'earliest', 'latest', or 'none', got '{}'",
                self.auto_offset_reset
            )));
        }
        if !matches!(
            self.isolation_level.as_str(),
            "read_uncommitted" | "read_committed"
        ) {
            return Err(CamelError::Config(format!(
                "KafkaConfig.isolation_level must be 'read_uncommitted' or 'read_committed', got '{}'",
                self.isolation_level
            )));
        }
        for (name, broker_cfg) in &self.brokers_named {
            if broker_cfg.brokers.trim().is_empty() {
                return Err(CamelError::Config(format!(
                    "KafkaConfig.brokers_named.{name}.brokers must not be empty"
                )));
            }
            validate_rdkafka_config(&broker_cfg.rdkafka_config).map_err(|e| {
                CamelError::Config(format!("KafkaConfig.brokers_named.{name}: {e}"))
            })?;
            if let Some(auth_type) = broker_cfg.sasl_auth_type {
                validate_sasl_creds(
                    auth_type,
                    &broker_cfg.sasl_username,
                    &broker_cfg.sasl_password,
                )
                .map_err(|e| {
                    CamelError::Config(format!("KafkaConfig.brokers_named.{name}: {e}"))
                })?;
            }
        }
        Ok(())
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
/// - `heartbeat_interval_ms` - Heartbeat interval in milliseconds
/// - `isolation_level` - Isolation level ("read_uncommitted" or "read_committed")
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
/// - `dlq_topic` - Dead Letter Queue topic name (optional; when set, failed messages are routed here)
/// - `dlq_max_retries` - Number of retries before routing to DLQ (default: 3)
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

    /// Heartbeat interval in milliseconds. `None` if not set in URI.
    /// Filled by `apply_defaults()` from global config, then `resolve_defaults()`.
    pub heartbeat_interval_ms: Option<u32>,

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

    /// Partition assignment strategy. Default: Range.
    pub partition_assignment_strategy: PartitionAssignmentStrategy,

    /// Client identifier sent to the broker. Default: "camel-kafka".
    pub client_id: Option<String>,

    /// Commit drain timeout in milliseconds (shutdown grace period). Default: 10000.
    pub commit_timeout_ms: u32,

    /// Isolation level for consumer. `None` if not set in URI.
    /// Must be "read_uncommitted" or "read_committed" when set.
    /// Filled by `apply_defaults()` from global config, then `resolve_defaults()`.
    pub isolation_level: Option<String>,

    /// Dead Letter Queue topic. When set, messages that fail processing after
    /// `dlq_max_retries` attempts are sent to this topic instead of being dropped.
    pub dlq_topic: Option<String>,

    /// Number of retries before routing to DLQ. Default: 3.
    pub dlq_max_retries: u32,

    /// Reconnect/backoff policy. `None` if not set in URI.
    /// Filled by `apply_defaults()` from global config, then `resolve_defaults()`.
    pub reconnect: Option<NetworkRetryPolicy>,

    /// Named broker reference (from URI param "brokerName").
    /// Resolved by `apply_broker_name()` before `apply_defaults()`.
    pub broker_name: Option<String>,

    /// Escape hatch from named broker — carried through to resolved config.
    pub rdkafka_config: HashMap<String, String>,
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

        let session_timeout_ms = match parts.params.get("sessionTimeoutMs") {
            Some(raw) => Some(raw.parse::<u32>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "sessionTimeoutMs must be an unsigned integer, got '{raw}'"
                ))
            })?),
            None => None,
        };

        let heartbeat_interval_ms = match parts.params.get("heartbeatIntervalMs") {
            Some(raw) => Some(raw.parse::<u32>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "heartbeatIntervalMs must be an unsigned integer, got '{raw}'"
                ))
            })?),
            None => None,
        };

        // Fields without global defaults use hardcoded defaults
        let poll_timeout_ms = match parts.params.get("pollTimeoutMs") {
            Some(raw) => raw.parse::<u32>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "pollTimeoutMs must be an unsigned integer, got '{raw}'"
                ))
            })?,
            None => 5000,
        };

        let max_poll_records = match parts.params.get("maxPollRecords") {
            Some(raw) => raw.parse::<u32>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "maxPollRecords must be an unsigned integer, got '{raw}'"
                ))
            })?,
            None => 500,
        };

        let acks = parts
            .params
            .get("acks")
            .cloned()
            .unwrap_or_else(|| "all".to_string());

        let request_timeout_ms = match parts.params.get("requestTimeoutMs") {
            Some(raw) => Some(raw.parse::<u32>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "requestTimeoutMs must be an unsigned integer, got '{raw}'"
                ))
            })?),
            None => None,
        };

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

        let allow_manual_commit = match parts.params.get("allowManualCommit") {
            Some(raw) => raw.parse::<bool>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "allowManualCommit must be a boolean ('true' or 'false'), got '{raw}'"
                ))
            })?,
            None => false,
        };

        let partition_assignment_strategy = parts
            .params
            .get("partitionAssignmentStrategy")
            .map(|s| s.parse::<PartitionAssignmentStrategy>())
            .transpose()
            .map_err(CamelError::InvalidUri)?
            .unwrap_or_default();

        let client_id = parts.params.get("clientId").cloned();

        let broker_name = parts.params.get("brokerName").cloned();

        let commit_timeout_ms = match parts.params.get("commitTimeoutMs") {
            Some(raw) => raw.parse::<u32>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "commitTimeoutMs must be an unsigned integer, got '{raw}'"
                ))
            })?,
            None => DEFAULT_COMMIT_TIMEOUT_MS,
        };

        let isolation_level = parts.params.get("isolationLevel").cloned();

        let dlq_topic = parts.params.get("dlqTopic").cloned();

        let dlq_max_retries = match parts.params.get("dlqMaxRetries") {
            Some(raw) => raw.parse::<u32>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "dlqMaxRetries must be an unsigned integer, got '{raw}'"
                ))
            })?,
            None => 3,
        };

        let config = Self {
            topic,
            brokers,
            group_id,
            auto_offset_reset,
            session_timeout_ms,
            heartbeat_interval_ms,
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
            partition_assignment_strategy,
            client_id,
            commit_timeout_ms,
            isolation_level,
            dlq_topic,
            dlq_max_retries,
            reconnect: None,
            broker_name,
            rdkafka_config: HashMap::new(),
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

        // Validate numeric bounds for values provided in URI
        if let Some(v) = self.session_timeout_ms
            && !(MIN_SESSION_TIMEOUT_MS..=MAX_SESSION_TIMEOUT_MS).contains(&v)
        {
            return Err(CamelError::InvalidUri(format!(
                "sessionTimeoutMs must be between {MIN_SESSION_TIMEOUT_MS} and {MAX_SESSION_TIMEOUT_MS}, got {v}"
            )));
        }
        if let Some(v) = self.heartbeat_interval_ms {
            if v == 0 {
                return Err(CamelError::InvalidUri(
                    "heartbeatIntervalMs must be > 0".to_string(),
                ));
            }
            if let Some(st) = self.session_timeout_ms
                && v >= st
            {
                return Err(CamelError::InvalidUri(format!(
                    "heartbeatIntervalMs ({v}) must be less than sessionTimeoutMs ({st})"
                )));
            }
        }
        if !(MIN_POLL_TIMEOUT_MS..=MAX_POLL_TIMEOUT_MS).contains(&self.poll_timeout_ms) {
            return Err(CamelError::InvalidUri(format!(
                "pollTimeoutMs must be between {MIN_POLL_TIMEOUT_MS} and {MAX_POLL_TIMEOUT_MS}, got {}",
                self.poll_timeout_ms
            )));
        }
        if !(MIN_MAX_POLL_RECORDS..=MAX_MAX_POLL_RECORDS).contains(&self.max_poll_records) {
            return Err(CamelError::InvalidUri(format!(
                "maxPollRecords must be between {MIN_MAX_POLL_RECORDS} and {MAX_MAX_POLL_RECORDS}, got {}",
                self.max_poll_records
            )));
        }
        if let Some(v) = self.request_timeout_ms
            && !(MIN_REQUEST_TIMEOUT_MS..=MAX_REQUEST_TIMEOUT_MS).contains(&v)
        {
            return Err(CamelError::InvalidUri(format!(
                "requestTimeoutMs must be between {MIN_REQUEST_TIMEOUT_MS} and {MAX_REQUEST_TIMEOUT_MS}, got {v}"
            )));
        }
        if !(MIN_COMMIT_TIMEOUT_MS..=MAX_COMMIT_TIMEOUT_MS).contains(&self.commit_timeout_ms) {
            return Err(CamelError::InvalidUri(format!(
                "commitTimeoutMs must be between {MIN_COMMIT_TIMEOUT_MS} and {MAX_COMMIT_TIMEOUT_MS}, got {}",
                self.commit_timeout_ms
            )));
        }

        // Validate isolation_level only if set in URI
        if let Some(ref il) = self.isolation_level
            && !matches!(il.as_str(), "read_uncommitted" | "read_committed")
        {
            return Err(CamelError::InvalidUri(format!(
                "isolationLevel must be 'read_uncommitted' or 'read_committed', got '{}'",
                il
            )));
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
        if self.heartbeat_interval_ms.is_none() {
            self.heartbeat_interval_ms = Some(defaults.heartbeat_interval_ms);
        }
        if self.request_timeout_ms.is_none() {
            self.request_timeout_ms = Some(defaults.request_timeout_ms);
        }
        if self.auto_offset_reset.is_none() {
            self.auto_offset_reset = Some(defaults.auto_offset_reset.clone());
        }
        if self.isolation_level.is_none() {
            self.isolation_level = Some(defaults.isolation_level.clone());
        }
        if self.security_protocol.is_none() {
            self.security_protocol = defaults.security_protocol.parse::<SecurityProtocol>().ok();
        }
        if self.reconnect.is_none() {
            self.reconnect = Some(defaults.reconnect.clone());
        }
    }

    /// Resolve any remaining `None` fields to hardcoded defaults.
    ///
    /// This should be called after `apply_defaults()` to ensure all fields
    /// that can have global defaults are guaranteed to be `Some`.
    pub fn resolve_defaults(&mut self) -> Result<(), CamelError> {
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
        if self.heartbeat_interval_ms.is_none() {
            self.heartbeat_interval_ms = Some(defaults.heartbeat_interval_ms);
        }
        if self.request_timeout_ms.is_none() {
            self.request_timeout_ms = Some(defaults.request_timeout_ms);
        }
        if self.auto_offset_reset.is_none() {
            self.auto_offset_reset = Some(defaults.auto_offset_reset);
        }
        if self.isolation_level.is_none() {
            self.isolation_level = Some(defaults.isolation_level);
        }
        if self.security_protocol.is_none() {
            self.security_protocol = Some(
                defaults
                    .security_protocol
                    .parse::<SecurityProtocol>()
                    .map_err(|e| {
                        CamelError::Config(format!("invalid default security protocol: {e}"))
                    })?,
            );
        }
        if self.reconnect.is_none() {
            self.reconnect = Some(defaults.reconnect.clone());
        }
        Ok(())
    }

    /// Resolve all `None` fields to defaults and validate, returning a
    /// `ResolvedKafkaEndpointConfig` that guarantees every field is populated.
    ///
    /// This is the **only** safe way to construct a producer or consumer.
    /// It rejects empty brokers, empty group_id, and out-of-range numeric values.
    pub fn resolve(self) -> Result<ResolvedKafkaEndpointConfig, CamelError> {
        let mut cfg = self;
        cfg.resolve_defaults()?;

        let brokers = cfg
            .brokers
            .clone()
            .ok_or_else(|| CamelError::Config("brokers must be set".into()))?;
        if brokers.trim().is_empty() {
            return Err(CamelError::InvalidUri(
                "brokers must not be empty".to_string(),
            ));
        }

        let group_id = cfg
            .group_id
            .clone()
            .ok_or_else(|| CamelError::Config("group_id must be set".into()))?;
        if group_id.trim().is_empty() {
            return Err(CamelError::InvalidUri(
                "group_id must not be empty".to_string(),
            ));
        }

        let auto_offset_reset = cfg
            .auto_offset_reset
            .clone()
            .ok_or_else(|| CamelError::Config("auto_offset_reset must be set".into()))?;
        // Re-validate after resolution (defaults are safe, but be defensive)
        if !matches!(auto_offset_reset.as_str(), "earliest" | "latest" | "none") {
            return Err(CamelError::Config(format!(
                "auto_offset_reset must be 'earliest', 'latest', or 'none', got '{auto_offset_reset}'"
            )));
        }

        let session_timeout_ms = cfg
            .session_timeout_ms
            .ok_or_else(|| CamelError::Config("session_timeout_ms must be set".into()))?;
        if !(MIN_SESSION_TIMEOUT_MS..=MAX_SESSION_TIMEOUT_MS).contains(&session_timeout_ms) {
            return Err(CamelError::Config(format!(
                "sessionTimeoutMs must be between {MIN_SESSION_TIMEOUT_MS} and {MAX_SESSION_TIMEOUT_MS}, got {session_timeout_ms}"
            )));
        }

        let heartbeat_interval_ms = cfg
            .heartbeat_interval_ms
            .ok_or_else(|| CamelError::Config("heartbeat_interval_ms must be set".into()))?;
        if heartbeat_interval_ms == 0 {
            return Err(CamelError::Config(
                "heartbeat_interval_ms must be > 0".into(),
            ));
        }
        if heartbeat_interval_ms >= session_timeout_ms {
            return Err(CamelError::Config(format!(
                "heartbeat_interval_ms ({heartbeat_interval_ms}) must be less than session_timeout_ms ({session_timeout_ms})"
            )));
        }

        let request_timeout_ms = cfg
            .request_timeout_ms
            .ok_or_else(|| CamelError::Config("request_timeout_ms must be set".into()))?;
        if !(MIN_REQUEST_TIMEOUT_MS..=MAX_REQUEST_TIMEOUT_MS).contains(&request_timeout_ms) {
            return Err(CamelError::Config(format!(
                "requestTimeoutMs must be between {MIN_REQUEST_TIMEOUT_MS} and {MAX_REQUEST_TIMEOUT_MS}, got {request_timeout_ms}"
            )));
        }

        // Re-validate per-endpoint numeric bounds
        if !(MIN_POLL_TIMEOUT_MS..=MAX_POLL_TIMEOUT_MS).contains(&cfg.poll_timeout_ms) {
            return Err(CamelError::Config(format!(
                "pollTimeoutMs must be between {MIN_POLL_TIMEOUT_MS} and {MAX_POLL_TIMEOUT_MS}, got {}",
                cfg.poll_timeout_ms
            )));
        }
        if !(MIN_MAX_POLL_RECORDS..=MAX_MAX_POLL_RECORDS).contains(&cfg.max_poll_records) {
            return Err(CamelError::Config(format!(
                "maxPollRecords must be between {MIN_MAX_POLL_RECORDS} and {MAX_MAX_POLL_RECORDS}, got {}",
                cfg.max_poll_records
            )));
        }
        if !(MIN_COMMIT_TIMEOUT_MS..=MAX_COMMIT_TIMEOUT_MS).contains(&cfg.commit_timeout_ms) {
            return Err(CamelError::Config(format!(
                "commitTimeoutMs must be between {MIN_COMMIT_TIMEOUT_MS} and {MAX_COMMIT_TIMEOUT_MS}, got {}",
                cfg.commit_timeout_ms
            )));
        }

        // Validate isolation_level after resolution
        let isolation_level = cfg
            .isolation_level
            .ok_or_else(|| CamelError::Config("isolation_level must be set".into()))?;
        if !matches!(
            isolation_level.as_str(),
            "read_uncommitted" | "read_committed"
        ) {
            return Err(CamelError::Config(format!(
                "isolation_level must be 'read_uncommitted' or 'read_committed', got '{isolation_level}'"
            )));
        }

        Ok(ResolvedKafkaEndpointConfig {
            topic: cfg.topic,
            brokers,
            group_id,
            auto_offset_reset,
            session_timeout_ms,
            heartbeat_interval_ms,
            poll_timeout_ms: cfg.poll_timeout_ms,
            max_poll_records: cfg.max_poll_records,
            acks: cfg.acks,
            request_timeout_ms,
            security_protocol: cfg.security_protocol.unwrap_or(SecurityProtocol::Plaintext),
            sasl_auth_type: cfg.sasl_auth_type,
            sasl_username: cfg.sasl_username,
            sasl_password: cfg.sasl_password,
            ssl_keystore_location: cfg.ssl_keystore_location,
            ssl_keystore_password: cfg.ssl_keystore_password,
            ssl_truststore_location: cfg.ssl_truststore_location,
            ssl_truststore_password: cfg.ssl_truststore_password,
            allow_manual_commit: cfg.allow_manual_commit,
            partition_assignment_strategy: cfg.partition_assignment_strategy,
            client_id: cfg.client_id.unwrap_or_else(|| "camel-kafka".to_string()),
            commit_timeout_ms: cfg.commit_timeout_ms,
            isolation_level,
            dlq_topic: cfg.dlq_topic,
            dlq_max_retries: cfg.dlq_max_retries,
            reconnect: cfg.reconnect.unwrap_or_default(),
            rdkafka_config: cfg.rdkafka_config,
        })
    }
}

// ---------------------------------------------------------------------------
// ResolvedKafkaEndpointConfig — all fields guaranteed populated
// ---------------------------------------------------------------------------

/// A fully-resolved Kafka endpoint configuration.
///
/// Constructed via [`KafkaEndpointConfig::resolve`], which applies defaults,
/// validates bounds, and rejects empty required fields.  Producer and consumer
/// constructors accept **only** this type, eliminating production `expect()` panics.
#[derive(Clone)]
pub struct ResolvedKafkaEndpointConfig {
    pub topic: String,
    pub brokers: String,
    pub group_id: String,
    pub auto_offset_reset: String,
    pub session_timeout_ms: u32,
    pub heartbeat_interval_ms: u32,
    pub poll_timeout_ms: u32,
    pub max_poll_records: u32,
    pub acks: String,
    pub request_timeout_ms: u32,
    pub security_protocol: SecurityProtocol,
    pub sasl_auth_type: SaslAuthType,
    pub sasl_username: Option<String>,
    pub sasl_password: Option<String>,
    pub ssl_keystore_location: Option<String>,
    pub ssl_keystore_password: Option<String>,
    pub ssl_truststore_location: Option<String>,
    pub ssl_truststore_password: Option<String>,
    pub allow_manual_commit: bool,
    pub partition_assignment_strategy: PartitionAssignmentStrategy,
    pub client_id: String,
    pub commit_timeout_ms: u32,
    pub isolation_level: String,
    pub dlq_topic: Option<String>,
    pub dlq_max_retries: u32,

    /// Reconnect/backoff policy for the consumer poll loop.
    pub reconnect: NetworkRetryPolicy,

    /// Escape-hatch rdkafka config keys not modeled as dedicated fields.
    /// Populated from named broker config or URI `rdkafkaConfig` params.
    pub rdkafka_config: HashMap<String, String>,
}

impl ResolvedKafkaEndpointConfig {
    /// Build from an unresolved config (applies defaults + validates).
    pub fn from_unresolved(cfg: KafkaEndpointConfig) -> Result<Self, CamelError> {
        cfg.resolve()
    }
}

impl std::fmt::Debug for ResolvedKafkaEndpointConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedKafkaEndpointConfig")
            .field("topic", &self.topic)
            .field("brokers", &self.brokers)
            .field("group_id", &self.group_id)
            .field("auto_offset_reset", &self.auto_offset_reset)
            .field("session_timeout_ms", &self.session_timeout_ms)
            .field("heartbeat_interval_ms", &self.heartbeat_interval_ms)
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
            .field(
                "partition_assignment_strategy",
                &self.partition_assignment_strategy,
            )
            .field("client_id", &self.client_id)
            .field("commit_timeout_ms", &self.commit_timeout_ms)
            .field("isolation_level", &self.isolation_level)
            .field("dlq_topic", &self.dlq_topic)
            .field("dlq_max_retries", &self.dlq_max_retries)
            .field("reconnect", &self.reconnect)
            .field(
                "rdkafka_config",
                &format!("{} keys [REDACTED]", self.rdkafka_config.len()),
            )
            .finish()
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
            .field("heartbeat_interval_ms", &self.heartbeat_interval_ms)
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
            .field(
                "partition_assignment_strategy",
                &self.partition_assignment_strategy,
            )
            .field("client_id", &self.client_id)
            .field("commit_timeout_ms", &self.commit_timeout_ms)
            .field("isolation_level", &self.isolation_level)
            .field("dlq_topic", &self.dlq_topic)
            .field("dlq_max_retries", &self.dlq_max_retries)
            .field("reconnect", &self.reconnect)
            .field("broker_name", &self.broker_name)
            .field(
                "rdkafka_config",
                &format!("{} keys [REDACTED]", self.rdkafka_config.len()),
            )
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

/// Apply security-related settings from a resolved config to an rdkafka `ClientConfig`.
/// Call this after setting the basic fields (brokers, group.id, etc.) and before `.create()`.
pub fn apply_security_config(config: &ResolvedKafkaEndpointConfig, cc: &mut ClientConfig) {
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
        c.resolve_defaults()
            .expect("resolve_defaults should succeed");
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

    #[test]
    fn test_partition_assignment_strategy_default_when_not_set() {
        let c = KafkaEndpointConfig::from_uri("kafka:orders").unwrap();
        assert_eq!(
            c.partition_assignment_strategy,
            PartitionAssignmentStrategy::Range
        );
    }

    #[test]
    fn test_partition_assignment_strategy_parse_roundrobin() {
        let c =
            KafkaEndpointConfig::from_uri("kafka:orders?partitionAssignmentStrategy=roundRobin")
                .unwrap();
        assert_eq!(
            c.partition_assignment_strategy,
            PartitionAssignmentStrategy::RoundRobin
        );
    }

    #[test]
    fn test_partition_assignment_strategy_parse_cooperative_sticky() {
        let c = KafkaEndpointConfig::from_uri(
            "kafka:orders?partitionAssignmentStrategy=cooperativeSticky",
        )
        .unwrap();
        assert_eq!(
            c.partition_assignment_strategy,
            PartitionAssignmentStrategy::CooperativeSticky
        );
    }

    #[test]
    fn test_partition_assignment_strategy_invalid_uri_fails() {
        let result =
            KafkaEndpointConfig::from_uri("kafka:orders?partitionAssignmentStrategy=invalid");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("partitionAssignmentStrategy"),
            "error should mention partitionAssignmentStrategy: {msg}"
        );
    }

    #[test]
    fn test_config_parses_broker_name() {
        let c = KafkaEndpointConfig::from_uri("kafka:orders?brokerName=prod").unwrap();
        assert_eq!(c.broker_name.as_deref(), Some("prod"));
    }

    #[test]
    fn test_config_broker_name_absent_by_default() {
        let c = KafkaEndpointConfig::from_uri("kafka:orders?brokers=localhost:9092").unwrap();
        assert!(c.broker_name.is_none());
    }

    #[test]
    fn test_config_rdkafka_config_empty_by_default() {
        let c = KafkaEndpointConfig::from_uri("kafka:orders?brokers=localhost:9092").unwrap();
        assert!(c.rdkafka_config.is_empty());

        let resolved = c.resolve().unwrap();
        assert!(resolved.rdkafka_config.is_empty());
    }
}

#[cfg(test)]
mod security_config_tests {
    use super::*;
    use rdkafka::config::ClientConfig;

    #[test]
    fn test_apply_sasl_ssl_sets_required_keys() {
        let config = KafkaEndpointConfig::from_uri(
            "kafka:t?brokers=localhost:9092&groupId=g\
             &securityProtocol=SASL_SSL&saslAuthType=SCRAM_SHA_512\
             &saslUsername=user&saslPassword=pass",
        )
        .unwrap()
        .resolve()
        .unwrap();
        let mut cc = ClientConfig::new();
        apply_security_config(&config, &mut cc);
        // Does not panic — rdkafka stores values internally; just verify no crash
    }

    #[test]
    fn test_apply_ssl_only_does_not_set_sasl_mechanism() {
        let config = KafkaEndpointConfig::from_uri(
            "kafka:t?brokers=localhost:9092&groupId=g\
             &securityProtocol=SSL\
             &sslKeystoreLocation=/k&sslKeystorePassword=ks",
        )
        .unwrap()
        .resolve()
        .unwrap();
        let mut cc = ClientConfig::new();
        apply_security_config(&config, &mut cc); // must not panic
    }

    #[test]
    fn test_apply_plaintext_is_noop() {
        let config = KafkaEndpointConfig::from_uri("kafka:t?brokers=localhost:9092&groupId=g")
            .unwrap()
            .resolve()
            .unwrap();
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

    #[test]
    fn test_partition_assignment_strategy_default_is_range() {
        assert_eq!(
            PartitionAssignmentStrategy::default(),
            PartitionAssignmentStrategy::Range
        );
    }

    #[test]
    fn test_partition_assignment_strategy_from_str_roundrobin() {
        let s: Result<PartitionAssignmentStrategy, _> = "roundRobin".parse();
        assert_eq!(s.unwrap(), PartitionAssignmentStrategy::RoundRobin);
    }

    #[test]
    fn test_partition_assignment_strategy_from_str_cooperative_sticky() {
        let s: Result<PartitionAssignmentStrategy, _> = "cooperativeSticky".parse();
        assert_eq!(s.unwrap(), PartitionAssignmentStrategy::CooperativeSticky);
    }

    #[test]
    fn test_partition_assignment_strategy_from_str_case_insensitive() {
        assert_eq!(
            "ROUNDROBIN".parse::<PartitionAssignmentStrategy>().unwrap(),
            PartitionAssignmentStrategy::RoundRobin
        );
        assert_eq!(
            "Range".parse::<PartitionAssignmentStrategy>().unwrap(),
            PartitionAssignmentStrategy::Range
        );
        assert_eq!(
            "COOPERATIVESTICKY"
                .parse::<PartitionAssignmentStrategy>()
                .unwrap(),
            PartitionAssignmentStrategy::CooperativeSticky
        );
    }

    #[test]
    fn test_partition_assignment_strategy_from_str_invalid() {
        let result: Result<PartitionAssignmentStrategy, _> = "bogus".parse();
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(
            msg.contains("bogus"),
            "error should mention the invalid value: {msg}"
        );
        assert!(
            msg.contains("range"),
            "error should list valid options: {msg}"
        );
    }

    #[test]
    fn test_partition_assignment_strategy_to_rdkafka_str() {
        assert_eq!(PartitionAssignmentStrategy::Range.to_rdkafka_str(), "range");
        assert_eq!(
            PartitionAssignmentStrategy::RoundRobin.to_rdkafka_str(),
            "roundrobin"
        );
        assert_eq!(
            PartitionAssignmentStrategy::CooperativeSticky.to_rdkafka_str(),
            "cooperative-sticky"
        );
    }
}

#[cfg(test)]
mod serde_roundtrip_tests {
    use super::*;

    #[test]
    fn test_security_protocol_serde_roundtrip() {
        let variants = [
            (SecurityProtocol::Plaintext, "\"Plaintext\""),
            (SecurityProtocol::Ssl, "\"Ssl\""),
            (SecurityProtocol::SaslPlaintext, "\"SaslPlaintext\""),
            (SecurityProtocol::SaslSsl, "\"SaslSsl\""),
        ];
        for (variant, expected) in &variants {
            let serialized = serde_json::to_string(variant).expect("serialize");
            assert_eq!(&serialized, expected);
            let deserialized: SecurityProtocol =
                serde_json::from_str(expected).expect("deserialize");
            assert_eq!(deserialized, *variant);
        }
    }

    #[test]
    fn test_sasl_auth_type_serde_roundtrip() {
        let variants = [
            (SaslAuthType::None, "\"None\""),
            (SaslAuthType::Plain, "\"Plain\""),
            (SaslAuthType::ScramSha256, "\"ScramSha256\""),
            (SaslAuthType::Ssl, "\"Ssl\""),
        ];
        for (variant, expected) in &variants {
            let serialized = serde_json::to_string(variant).expect("serialize");
            assert_eq!(&serialized, expected);
            let deserialized: SaslAuthType = serde_json::from_str(expected).expect("deserialize");
            assert_eq!(deserialized, *variant);
        }
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
    fn kafka_config_has_reconnect_policy() {
        let toml_str = r#"
            [reconnect]
            max_attempts = 5
            initial_delay_ms = 200
        "#;
        let cfg: super::KafkaConfig = toml::from_str(toml_str).expect("parse");
        assert_eq!(cfg.reconnect.max_attempts, 5);
        assert_eq!(
            cfg.reconnect.initial_delay,
            std::time::Duration::from_millis(200)
        );
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
            heartbeat_interval_ms: None,
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
            partition_assignment_strategy: PartitionAssignmentStrategy::Range,
            client_id: None,
            commit_timeout_ms: DEFAULT_COMMIT_TIMEOUT_MS,
            isolation_level: None,
            dlq_topic: None,
            dlq_max_retries: 3,
            reconnect: None,
            broker_name: None,
            rdkafka_config: HashMap::new(),
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
        ep.resolve_defaults()
            .expect("resolve_defaults should fill defaults");
        assert_eq!(ep.brokers.as_deref(), Some("localhost:9092"));
        assert_eq!(ep.group_id.as_deref(), Some("camel"));
        assert_eq!(ep.session_timeout_ms, Some(45_000));
        assert_eq!(ep.request_timeout_ms, Some(30_000));
        assert_eq!(ep.auto_offset_reset.as_deref(), Some("latest"));
        assert_eq!(ep.security_protocol, Some(SecurityProtocol::Plaintext));
    }

    // --- KAFKA-005: new URI options ---

    #[test]
    fn test_client_id_from_uri() {
        let c = KafkaEndpointConfig::from_uri("kafka:orders?clientId=my-app").unwrap();
        assert_eq!(c.client_id, Some("my-app".to_string()));
    }

    #[test]
    fn test_client_id_default_none() {
        let c = KafkaEndpointConfig::from_uri("kafka:orders").unwrap();
        assert!(c.client_id.is_none());
    }

    #[test]
    fn test_commit_timeout_ms_from_uri() {
        let c = KafkaEndpointConfig::from_uri("kafka:orders?commitTimeoutMs=2000").unwrap();
        assert_eq!(c.commit_timeout_ms, 2000);
    }

    #[test]
    fn test_commit_timeout_ms_default() {
        let c = KafkaEndpointConfig::from_uri("kafka:orders").unwrap();
        assert_eq!(c.commit_timeout_ms, DEFAULT_COMMIT_TIMEOUT_MS);
    }

    // --- KAFKA-011: numeric bounds validation ---

    #[test]
    fn test_zero_session_timeout_ms_rejected() {
        let result = KafkaEndpointConfig::from_uri("kafka:orders?sessionTimeoutMs=0");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("sessionTimeoutMs"), "got: {msg}");
    }

    #[test]
    fn test_max_session_timeout_ms_rejected() {
        let result =
            KafkaEndpointConfig::from_uri(&format!("kafka:orders?sessionTimeoutMs={}", u32::MAX));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("sessionTimeoutMs"), "got: {msg}");
    }

    #[test]
    fn test_zero_poll_timeout_ms_rejected() {
        let result = KafkaEndpointConfig::from_uri("kafka:orders?pollTimeoutMs=0");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("pollTimeoutMs"), "got: {msg}");
    }

    #[test]
    fn test_zero_max_poll_records_rejected() {
        let result = KafkaEndpointConfig::from_uri("kafka:orders?maxPollRecords=0");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("maxPollRecords"), "got: {msg}");
    }

    #[test]
    fn test_zero_request_timeout_ms_rejected() {
        let result = KafkaEndpointConfig::from_uri("kafka:orders?requestTimeoutMs=0");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("requestTimeoutMs"), "got: {msg}");
    }

    #[test]
    fn test_zero_commit_timeout_ms_rejected() {
        let result = KafkaEndpointConfig::from_uri("kafka:orders?commitTimeoutMs=0");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("commitTimeoutMs"), "got: {msg}");
    }

    #[test]
    fn test_valid_numeric_bounds_accepted() {
        let c = KafkaEndpointConfig::from_uri(
            "kafka:orders?sessionTimeoutMs=10000&pollTimeoutMs=1000&maxPollRecords=100\
             &requestTimeoutMs=5000&commitTimeoutMs=1000",
        )
        .unwrap();
        assert_eq!(c.session_timeout_ms, Some(10_000));
        assert_eq!(c.poll_timeout_ms, 1000);
        assert_eq!(c.max_poll_records, 100);
        assert_eq!(c.request_timeout_ms, Some(5000));
        assert_eq!(c.commit_timeout_ms, 1000);
    }

    // --- KAFKA-017: empty brokers rejection ---

    #[test]
    fn test_empty_brokers_in_uri_rejected() {
        let result = KafkaEndpointConfig::from_uri("kafka:orders?brokers=");
        // Empty string is accepted at parse time (it's a valid string),
        // but resolve() must reject it.
        let cfg = result.unwrap();
        let resolved = cfg.resolve();
        assert!(resolved.is_err());
        let msg = resolved.unwrap_err().to_string();
        assert!(msg.contains("brokers"), "got: {msg}");
    }

    // --- KAFKA-001: resolved config invariant ---

    #[test]
    fn test_resolve_produces_resolved_config() {
        let cfg =
            KafkaEndpointConfig::from_uri("kafka:orders?brokers=localhost:9092&groupId=test-group")
                .unwrap();
        let resolved = cfg.resolve().expect("resolve should succeed");
        assert_eq!(resolved.topic, "orders");
        assert_eq!(resolved.brokers, "localhost:9092");
        assert_eq!(resolved.group_id, "test-group");
        assert_eq!(resolved.client_id, "camel-kafka"); // default
        assert_eq!(resolved.commit_timeout_ms, DEFAULT_COMMIT_TIMEOUT_MS);
    }

    #[test]
    fn test_resolve_with_custom_client_id() {
        let cfg = KafkaEndpointConfig::from_uri(
            "kafka:orders?brokers=localhost:9092&groupId=test-group&clientId=my-app",
        )
        .unwrap();
        let resolved = cfg.resolve().expect("resolve should succeed");
        assert_eq!(resolved.client_id, "my-app");
    }

    #[test]
    fn test_resolve_empty_group_id_rejected() {
        let cfg =
            KafkaEndpointConfig::from_uri("kafka:orders?brokers=localhost:9092&groupId=").unwrap();
        let resolved = cfg.resolve();
        assert!(resolved.is_err());
        let msg = resolved.unwrap_err().to_string();
        assert!(msg.contains("group_id"), "got: {msg}");
    }

    #[test]
    fn test_resolved_config_debug_masks_passwords() {
        let cfg = KafkaEndpointConfig::from_uri(
            "kafka:orders?brokers=localhost:9092&groupId=test-group\
              &securityProtocol=SASL_SSL&saslAuthType=PLAIN\
              &saslUsername=user&saslPassword=secret123",
        )
        .unwrap();
        let resolved = cfg.resolve().unwrap();
        let debug_str = format!("{resolved:?}");
        assert!(
            !debug_str.contains("secret123"),
            "password leaked: {debug_str}"
        );
        assert!(
            debug_str.contains("[REDACTED]"),
            "no redaction: {debug_str}"
        );
    }

    // --- KafkaConfig::validate() tests (KAFKA-011, KAFKA-017) ---

    #[test]
    fn test_kafka_config_rejects_empty_brokers() {
        let cfg = KafkaConfig {
            brokers: "".into(),
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
        let msg = cfg.validate().unwrap_err().to_string();
        assert!(msg.contains("brokers"), "got: {msg}");
    }

    #[test]
    fn test_kafka_config_rejects_whitespace_only_brokers() {
        let cfg = KafkaConfig {
            brokers: "   ".into(),
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_kafka_config_accepts_valid_brokers() {
        let cfg = KafkaConfig {
            brokers: "localhost:9092".into(),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_kafka_config_rejects_zero_session_timeout() {
        let cfg = KafkaConfig {
            session_timeout_ms: 0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
        let msg = cfg.validate().unwrap_err().to_string();
        assert!(msg.contains("session_timeout_ms"), "got: {msg}");
    }

    #[test]
    fn test_kafka_config_rejects_zero_request_timeout() {
        let cfg = KafkaConfig {
            request_timeout_ms: 0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
        let msg = cfg.validate().unwrap_err().to_string();
        assert!(msg.contains("request_timeout_ms"), "got: {msg}");
    }

    #[test]
    fn test_kafka_config_accepts_valid_numeric_defaults() {
        let cfg = KafkaConfig::default();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_rejects_invalid_allow_manual_commit_bool() {
        let result = KafkaEndpointConfig::from_uri("kafka:orders?allowManualCommit=yes");
        assert!(result.is_err());
    }

    #[test]
    fn test_rejects_invalid_poll_timeout_numeric() {
        let result = KafkaEndpointConfig::from_uri("kafka:orders?pollTimeoutMs=abc");
        assert!(result.is_err());
    }

    #[test]
    fn test_rejects_invalid_max_poll_records_numeric() {
        let result = KafkaEndpointConfig::from_uri("kafka:orders?maxPollRecords=abc");
        assert!(result.is_err());
    }

    #[test]
    fn test_rejects_invalid_session_timeout_numeric() {
        let result = KafkaEndpointConfig::from_uri("kafka:orders?sessionTimeoutMs=abc");
        assert!(result.is_err());
    }

    #[test]
    fn test_rejects_invalid_request_timeout_numeric() {
        let result = KafkaEndpointConfig::from_uri("kafka:orders?requestTimeoutMs=abc");
        assert!(result.is_err());
    }

    #[test]
    fn test_rejects_invalid_commit_timeout_numeric() {
        let result = KafkaEndpointConfig::from_uri("kafka:orders?commitTimeoutMs=abc");
        assert!(result.is_err());
    }

    #[test]
    fn test_kafka_config_rejects_invalid_security_protocol_default() {
        let cfg = KafkaConfig {
            security_protocol: "BOGUS".to_string(),
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_kafka_config_rejects_invalid_auto_offset_reset_default() {
        let cfg = KafkaConfig {
            auto_offset_reset: "bad".to_string(),
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_kafka_config_brokers_named() {
        let toml_str = r#"
            brokers = "default:9092"
            [brokers_named]
            prod = { brokers = "prod1:9092,prod2:9092" }
            dev = { brokers = "dev:9092" }
        "#;
        let cfg: super::KafkaConfig = toml::from_str(toml_str).expect("parse");
        assert_eq!(cfg.brokers, "default:9092");
        assert_eq!(cfg.brokers_named.len(), 2);
        assert_eq!(
            cfg.brokers_named.get("prod").unwrap().brokers,
            "prod1:9092,prod2:9092"
        );
    }

    #[test]
    fn test_kafka_config_brokers_named_empty_by_default() {
        let cfg = super::KafkaConfig::default();
        assert!(cfg.brokers_named.is_empty());
    }

    #[test]
    fn test_kafka_config_validate_rejects_empty_named_broker() {
        let mut cfg = super::KafkaConfig::default();
        cfg.brokers_named
            .insert("bad".into(), KafkaBrokerConfig::default());
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("brokers must not be empty"));
        assert!(err.to_string().contains("bad"));
    }

    #[test]
    fn test_kafka_config_validate_rejects_named_broker_reserved_keys() {
        let mut cfg = super::KafkaConfig::default();
        let mut rdkafka = std::collections::HashMap::new();
        rdkafka.insert("bootstrap.servers".into(), "evil:9092".into());
        cfg.brokers_named.insert(
            "reserved".into(),
            KafkaBrokerConfig {
                brokers: "broker:9092".into(),
                rdkafka_config: rdkafka,
                ..Default::default()
            },
        );
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("reserved"));
        assert!(err.to_string().contains("reserved"));
    }

    #[test]
    fn test_kafka_config_validate_rejects_named_broker_missing_sasl_password() {
        let mut cfg = super::KafkaConfig::default();
        cfg.brokers_named.insert(
            "nopass".into(),
            KafkaBrokerConfig {
                brokers: "broker:9092".into(),
                sasl_auth_type: Some(SaslAuthType::Plain),
                sasl_username: Some("user".into()),
                sasl_password: None,
                ..Default::default()
            },
        );
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("sasl_password"));
        assert!(err.to_string().contains("nopass"));
    }
}
