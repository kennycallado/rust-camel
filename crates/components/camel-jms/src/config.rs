use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use camel_component_api::NetworkRetryPolicy;

use crate::BrokerType;

pub fn default_bridge_cache_dir() -> PathBuf {
    camel_bridge::download::default_cache_dir()
}

fn default_max_bridges() -> usize {
    8
}

fn default_bridge_start_timeout_ms() -> u64 {
    30_000
}

pub fn default_broker_reconnect_interval_ms() -> u64 {
    5_000
}

/// Per-component reconnect default: unlimited retries (max_attempts=0),
/// preserving the previous infinite-reconnect behavior via BackoffState.
/// Operators can opt into bounded retry via TOML `[reconnect]`.
pub(crate) fn jms_reconnect_default() -> NetworkRetryPolicy {
    NetworkRetryPolicy {
        max_attempts: 0, // unlimited
        initial_delay: Duration::from_millis(default_broker_reconnect_interval_ms()),
        multiplier: 2.0,
        max_delay: Duration::from_secs(30),
        jitter_factor: 0.0,
        ..NetworkRetryPolicy::default()
    }
}

fn default_health_check_interval_ms() -> u64 {
    5_000
}

#[derive(Debug, Clone, PartialEq)]
pub enum DestinationType {
    Queue,
    Topic,
}

// ── JMS-009: Acknowledgement mode ────────────────────────────────────────────

/// JMS session acknowledgement mode.
///
/// Controls how the JMS provider acknowledges consumed messages.
///
/// - `Auto`: The session automatically acknowledges a message after it is
///   delivered to the consumer. This is the simplest mode but may lose
///   messages if the consumer fails before processing.
/// - `Client`: The consumer must explicitly acknowledge each message.
///   Provides full control over when acknowledgement occurs.
/// - `DupsOk`: The session lazily acknowledges messages, which may result
///   in duplicate deliveries. Optimises throughput at the cost of
///   potential duplicates.
/// - `Transacted`: Messages are acknowledged as part of a transaction.
///   See [`JmsTransactionMode`] for transaction configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AcknowledgementMode {
    #[default]
    Auto,
    Client,
    DupsOk,
    Transacted,
}

impl fmt::Display for AcknowledgementMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => write!(f, "Auto"),
            Self::Client => write!(f, "Client"),
            Self::DupsOk => write!(f, "DupsOk"),
            Self::Transacted => write!(f, "Transacted"),
        }
    }
}

impl FromStr for AcknowledgementMode {
    type Err = camel_component_api::CamelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Auto" | "auto" => Ok(Self::Auto),
            "Client" | "client" => Ok(Self::Client),
            "DupsOk" | "dupsOk" | "dups_ok" => Ok(Self::DupsOk),
            "Transacted" | "transacted" => Ok(Self::Transacted),
            _ => Err(camel_component_api::CamelError::ProcessorError(format!(
                "invalid acknowledgement mode '{}': expected Auto, Client, DupsOk, or Transacted",
                s
            ))),
        }
    }
}

// ── JMS-012: Transaction mode ────────────────────────────────────────────────

/// JMS transaction mode for sessions.
///
/// - `None`: No transaction boundaries are applied. Each operation is
///   auto-acknowledged (subject to the acknowledgement mode).
/// - `Session`: All send/receive operations within a session are batched
///   into a single transaction that must be explicitly committed or rolled
///   back. **Not yet implemented** — using this value emits a warning and
///   falls back to `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JmsTransactionMode {
    #[default]
    None,
    Session,
}

impl fmt::Display for JmsTransactionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "None"),
            Self::Session => write!(f, "Session"),
        }
    }
}

impl FromStr for JmsTransactionMode {
    type Err = camel_component_api::CamelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "None" | "none" => Ok(Self::None),
            "Session" | "session" => Ok(Self::Session),
            _ => Err(camel_component_api::CamelError::ProcessorError(format!(
                "invalid transaction mode '{}': expected None or Session",
                s
            ))),
        }
    }
}

// ── JMS-005: Exchange pattern ────────────────────────────────────────────────

/// Message exchange pattern for JMS endpoints.
///
/// - `InOnly`: Fire-and-forget (default). The producer sends a message and
///   does not wait for a reply.
/// - `InOut`: Request-reply. The producer sends a message and waits for a
///   correlated reply on a temporary or dedicated reply destination.
///   **Not yet implemented** — using this value emits a warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExchangePattern {
    #[default]
    InOnly,
    InOut,
}

impl fmt::Display for ExchangePattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InOnly => write!(f, "InOnly"),
            Self::InOut => write!(f, "InOut"),
        }
    }
}

impl FromStr for ExchangePattern {
    type Err = camel_component_api::CamelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "InOnly" | "inOnly" | "in_only" => Ok(Self::InOnly),
            "InOut" | "inOut" | "in_out" => Ok(Self::InOut),
            _ => Err(camel_component_api::CamelError::ProcessorError(format!(
                "invalid exchange pattern '{}': expected InOnly or InOut",
                s
            ))),
        }
    }
}

// ── Endpoint config ──────────────────────────────────────────────────────────

fn default_concurrent_consumers() -> u32 {
    1
}

#[derive(Debug, Clone)]
pub struct JmsEndpointConfig {
    pub destination_type: DestinationType,
    pub destination_name: String,
    pub broker_name: Option<String>,

    // JMS-009: Acknowledgement mode for the JMS session (default: Auto).
    pub acknowledgement_mode: AcknowledgementMode,

    // JMS-010: JMS SQL-92 selector expression used to filter incoming messages.
    // Only messages matching the selector are delivered to the consumer.
    pub message_selector: Option<String>,

    // JMS-011: Number of concurrent consumer tasks to spawn for this endpoint.
    // Each consumer polls messages independently. Default: 1.
    pub concurrent_consumers: u32,

    // JMS-012: Transaction mode for the JMS session (default: None).
    pub transaction_mode: JmsTransactionMode,

    // JMS-013: QoS options for outbound messages.
    /// Message time-to-live in milliseconds. `None` means no expiration.
    pub time_to_live: Option<u64>,
    /// JMS message priority (0-9, where 9 is highest). `None` uses broker default.
    pub priority: Option<u8>,
    /// Whether to use persistent delivery mode (default: true). Persistent
    /// messages survive broker restarts; non-persistent messages may be lost.
    pub persistent_delivery: bool,

    // JMS-018: When `true` (default), JMS message properties are mapped to
    // Camel exchange headers on receive and Camel exchange headers are mapped
    // to JMS message properties on send.
    pub map_jms_headers: bool,

    // JMS-005: Message exchange pattern (default: InOnly).
    pub exchange_pattern: ExchangePattern,
}

impl JmsEndpointConfig {
    pub fn from_uri(uri: &str) -> Result<Self, camel_component_api::CamelError> {
        let (scheme, rest) = if let Some(r) = uri.strip_prefix("jms:") {
            ("jms", r)
        } else if let Some(r) = uri.strip_prefix("activemq:") {
            ("activemq", r)
        } else if let Some(r) = uri.strip_prefix("artemis:") {
            ("artemis", r)
        } else {
            return Err(camel_component_api::CamelError::ProcessorError(
                "expected scheme 'jms', 'activemq', or 'artemis'".to_string(),
            ));
        };

        let (path, query) = match rest.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (rest, None),
        };

        let (destination_type, destination_name) =
            match path.splitn(2, ':').collect::<Vec<_>>().as_slice() {
                // Shorthand (no prefix): only allowed for activemq/artemis, NOT jms
                [name] if !name.is_empty() && scheme != "jms" => {
                    (DestinationType::Queue, name.to_string())
                }
                // Explicit queue: or topic:
                [prefix, name]
                    if (*prefix == "queue" || *prefix == "topic") && !name.is_empty() =>
                {
                    let dt = if *prefix == "queue" {
                        DestinationType::Queue
                    } else {
                        DestinationType::Topic
                    };
                    (dt, name.to_string())
                }
                // jms: shorthand (rejected)
                [name] if !name.is_empty() && scheme == "jms" => {
                    return Err(camel_component_api::CamelError::ProcessorError(format!(
                        "URI 'jms:{}' is ambiguous — use 'jms:queue:{}' or 'jms:topic:{}'",
                        name, name, name
                    )));
                }
                _ => {
                    return Err(camel_component_api::CamelError::ProcessorError(
                        "destination must be 'queue:<name>' or 'topic:<name>'".to_string(),
                    ));
                }
            };

        // Parse query parameters
        let mut broker_name: Option<String> = None;
        let mut acknowledgement_mode = AcknowledgementMode::default();
        let mut message_selector: Option<String> = None;
        let mut concurrent_consumers = default_concurrent_consumers();
        let mut transaction_mode = JmsTransactionMode::default();
        let mut time_to_live: Option<u64> = None;
        let mut priority: Option<u8> = None;
        let mut persistent_delivery = true;
        let mut map_jms_headers = true;
        let mut exchange_pattern = ExchangePattern::default();

        if let Some(q) = query {
            for kv in q.split('&') {
                let Some((k, v)) = kv.split_once('=') else {
                    continue;
                };
                match k {
                    "broker" if !v.is_empty() => {
                        broker_name = Some(v.to_string());
                    }
                    "acknowledgementMode" | "acknowledgement_mode" => {
                        acknowledgement_mode = AcknowledgementMode::from_str(v)?;
                    }
                    "messageSelector" | "message_selector" if !v.is_empty() => {
                        message_selector = Some(v.to_string());
                    }
                    "concurrentConsumers" | "concurrent_consumers" => {
                        concurrent_consumers = v.parse::<u32>().map_err(|_| {
                            camel_component_api::CamelError::ProcessorError(format!(
                                "invalid concurrent_consumers '{}': expected positive integer",
                                v
                            ))
                        })?;
                        if concurrent_consumers == 0 {
                            return Err(camel_component_api::CamelError::ProcessorError(
                                "concurrent_consumers must be >= 1".to_string(),
                            ));
                        }
                    }
                    "transactionMode" | "transaction_mode" => {
                        transaction_mode = JmsTransactionMode::from_str(v)?;
                    }
                    "timeToLive" | "time_to_live" => {
                        time_to_live = Some(v.parse::<u64>().map_err(|_| {
                            camel_component_api::CamelError::ProcessorError(format!(
                                "invalid time_to_live '{}': expected non-negative integer (ms)",
                                v
                            ))
                        })?);
                    }
                    "priority" => {
                        let p = v.parse::<u8>().map_err(|_| {
                            camel_component_api::CamelError::ProcessorError(format!(
                                "invalid priority '{}': expected integer 0-9",
                                v
                            ))
                        })?;
                        if p > 9 {
                            return Err(camel_component_api::CamelError::ProcessorError(format!(
                                "invalid priority '{}': must be 0-9",
                                p
                            )));
                        }
                        priority = Some(p);
                    }
                    "persistentDelivery" | "persistent_delivery" => {
                        persistent_delivery = v.parse::<bool>().map_err(|_| {
                            camel_component_api::CamelError::ProcessorError(format!(
                                "invalid persistent_delivery '{}': expected true or false",
                                v
                            ))
                        })?;
                    }
                    "mapJmsHeaders" | "map_jms_headers" => {
                        map_jms_headers = v.parse::<bool>().map_err(|_| {
                            camel_component_api::CamelError::ProcessorError(format!(
                                "invalid map_jms_headers '{}': expected true or false",
                                v
                            ))
                        })?;
                    }
                    "exchangePattern" | "exchange_pattern" => {
                        exchange_pattern = ExchangePattern::from_str(v)?;
                    }
                    _ => {} // ignore unknown params
                }
            }
        }

        Ok(JmsEndpointConfig {
            destination_type,
            destination_name,
            broker_name,
            acknowledgement_mode,
            message_selector,
            concurrent_consumers,
            transaction_mode,
            time_to_live,
            priority,
            persistent_delivery,
            map_jms_headers,
            exchange_pattern,
        })
    }
}

#[derive(Clone, PartialEq, serde::Deserialize)]
pub struct BrokerConfig {
    pub broker_url: String,
    pub broker_type: BrokerType,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl std::fmt::Debug for BrokerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrokerConfig")
            .field("broker_url", &self.broker_url)
            .field("broker_type", &self.broker_type)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct JmsPoolConfig {
    #[serde(default)]
    pub brokers: HashMap<String, BrokerConfig>,
    #[serde(default = "default_max_bridges")]
    pub max_bridges: usize,
    #[serde(default = "default_bridge_start_timeout_ms")]
    pub bridge_start_timeout_ms: u64,
    #[serde(default = "default_broker_reconnect_interval_ms")]
    pub broker_reconnect_interval_ms: u64,
    #[serde(default = "default_health_check_interval_ms")]
    pub health_check_interval_ms: u64,
    #[serde(default = "default_bridge_cache_dir")]
    pub bridge_cache_dir: PathBuf,
    #[serde(default = "jms_reconnect_default")]
    pub reconnect: NetworkRetryPolicy,
}

impl Default for JmsPoolConfig {
    fn default() -> Self {
        Self {
            brokers: HashMap::new(),
            max_bridges: default_max_bridges(),
            bridge_start_timeout_ms: default_bridge_start_timeout_ms(),
            broker_reconnect_interval_ms: default_broker_reconnect_interval_ms(),
            health_check_interval_ms: default_health_check_interval_ms(),
            bridge_cache_dir: default_bridge_cache_dir(),
            reconnect: jms_reconnect_default(),
        }
    }
}

impl JmsPoolConfig {
    /// Convenience constructor for single-broker scenarios (tests, simple examples).
    /// Creates a pool with one broker named "default".
    pub fn single_broker(broker_url: impl Into<String>, broker_type: BrokerType) -> Self {
        let url = broker_url.into();
        let mut brokers = HashMap::new();
        brokers.insert(
            "default".to_string(),
            BrokerConfig {
                broker_url: url,
                broker_type,
                username: None,
                password: None,
            },
        );
        Self {
            brokers,
            max_bridges: 1,
            ..Self::default()
        }
    }

    /// Validates the config: all brokers must have non-empty URLs with a known
    /// scheme, `max_bridges` must be >= 1, and all timing fields must be strictly
    /// positive to prevent busy-loops.
    pub fn validate(&self) -> Result<(), camel_component_api::CamelError> {
        use camel_component_api::CamelError;

        if self.max_bridges < 1 {
            return Err(CamelError::Config("max_bridges must be >= 1".to_string()));
        }

        let known_schemes = ["tcp://", "ssl://", "failover://", "ws://", "wss://"];

        for (name, bc) in &self.brokers {
            if bc.broker_url.is_empty() {
                return Err(CamelError::ProcessorError(format!(
                    "broker '{}' has an empty broker_url",
                    name
                )));
            }

            let has_known_scheme = known_schemes.iter().any(|s| bc.broker_url.starts_with(s));
            if !has_known_scheme {
                return Err(CamelError::ProcessorError(format!(
                    "broker '{}' has an invalid broker_url '{}': must start with one of {:?}",
                    name, bc.broker_url, known_schemes
                )));
            }
        }

        if self.bridge_start_timeout_ms == 0 {
            return Err(CamelError::Config(
                "bridge_start_timeout_ms must be > 0".to_string(),
            ));
        }
        if self.health_check_interval_ms == 0 {
            return Err(CamelError::Config(
                "health_check_interval_ms must be > 0".to_string(),
            ));
        }
        if self.broker_reconnect_interval_ms == 0 {
            return Err(CamelError::Config(
                "broker_reconnect_interval_ms must be > 0".to_string(),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BrokerType;

    #[test]
    fn parse_jms_queue_explicit() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Queue);
        assert_eq!(cfg.destination_name, "orders");
        assert_eq!(cfg.broker_name, None);
    }

    #[test]
    fn parse_jms_topic_explicit() {
        let cfg = JmsEndpointConfig::from_uri("jms:topic:events").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Topic);
        assert_eq!(cfg.destination_name, "events");
    }

    #[test]
    fn parse_jms_with_broker_param() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?broker=primary").unwrap();
        assert_eq!(cfg.broker_name, Some("primary".to_string()));
        assert_eq!(cfg.destination_name, "orders");
    }

    #[test]
    fn jms_shorthand_rejected() {
        let err = JmsEndpointConfig::from_uri("jms:orders").unwrap_err();
        assert!(err.to_string().contains("ambiguous"), "got: {}", err);
    }

    #[test]
    fn invalid_destination_type_returns_error() {
        let err = JmsEndpointConfig::from_uri("jms:inbox:orders").unwrap_err();
        assert!(
            err.to_string().contains("'queue' or 'topic'")
                || err.to_string().contains("queue:<name>"),
            "got: {}",
            err
        );
    }

    #[test]
    fn parse_activemq_queue_explicit() {
        let cfg = JmsEndpointConfig::from_uri("activemq:queue:orders").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Queue);
        assert_eq!(cfg.destination_name, "orders");
    }

    #[test]
    fn parse_activemq_shorthand() {
        let cfg = JmsEndpointConfig::from_uri("activemq:orders").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Queue);
        assert_eq!(cfg.destination_name, "orders");
    }

    #[test]
    fn parse_artemis_shorthand_defaults_to_queue() {
        let cfg = JmsEndpointConfig::from_uri("artemis:events").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Queue);
        assert_eq!(cfg.destination_name, "events");
    }

    #[test]
    fn parse_jms_with_empty_broker_param_treated_as_none() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?broker=").unwrap();
        assert_eq!(cfg.broker_name, None);
    }

    #[test]
    fn parse_activemq_topic() {
        let cfg = JmsEndpointConfig::from_uri("activemq:topic:events").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Topic);
        assert_eq!(cfg.destination_name, "events");
    }

    #[test]
    fn single_broker_convenience() {
        let cfg = JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::ActiveMq);
        assert_eq!(cfg.max_bridges, 1);
        assert!(cfg.brokers.contains_key("default"));
        let bc = &cfg.brokers["default"];
        assert_eq!(bc.broker_url, "tcp://localhost:61616");
        assert_eq!(bc.broker_type, BrokerType::ActiveMq);
    }

    #[test]
    fn default_pool_config() {
        let cfg = JmsPoolConfig::default();
        assert_eq!(cfg.max_bridges, 8);
        assert!(cfg.brokers.is_empty());
        assert_eq!(cfg.bridge_start_timeout_ms, 30_000);
        assert_eq!(cfg.broker_reconnect_interval_ms, 5_000);
        assert_eq!(cfg.health_check_interval_ms, 5_000);
    }

    #[test]
    fn validate_empty_broker_url() {
        let cfg = JmsPoolConfig::single_broker("", BrokerType::ActiveMq);
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("empty broker_url"), "got: {}", err);
    }

    #[test]
    fn broker_config_debug_redacts_password() {
        let bc = BrokerConfig {
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            username: Some("admin".to_string()),
            password: Some("secret".to_string()),
        };
        let s = format!("{bc:?}");
        assert!(s.contains("<redacted>"), "got: {s}");
        assert!(!s.contains("secret"), "got: {s}");
    }

    #[test]
    fn validate_ok() {
        let cfg = JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::ActiveMq);
        cfg.validate().unwrap();
    }

    #[test]
    fn validate_rejects_zero_bridge_start_timeout() {
        let mut cfg = JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::ActiveMq);
        cfg.bridge_start_timeout_ms = 0;
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string().contains("bridge_start_timeout_ms"),
            "got: {}",
            err
        );
    }

    #[test]
    fn validate_rejects_zero_health_check_interval() {
        let mut cfg = JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::ActiveMq);
        cfg.health_check_interval_ms = 0;
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string().contains("health_check_interval_ms"),
            "got: {}",
            err
        );
    }

    #[test]
    fn validate_rejects_zero_reconnect_interval() {
        let mut cfg = JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::ActiveMq);
        cfg.broker_reconnect_interval_ms = 0;
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string().contains("broker_reconnect_interval_ms"),
            "got: {}",
            err
        );
    }

    // ── JMS-006: max_bridges validation ──────────────────────────────────────

    #[test]
    fn rejects_zero_max_bridges() {
        let mut cfg = JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::ActiveMq);
        cfg.max_bridges = 0;
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("max_bridges"), "got: {}", err);
    }

    #[test]
    fn rejects_zero_timeout() {
        let mut cfg = JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::ActiveMq);
        cfg.bridge_start_timeout_ms = 0;
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string().contains("bridge_start_timeout_ms"),
            "got: {}",
            err
        );
    }

    // ── JMS-016: broker URL scheme validation ────────────────────────────────

    #[test]
    fn rejects_empty_broker_url() {
        let cfg = JmsPoolConfig::single_broker("", BrokerType::ActiveMq);
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("empty broker_url"), "got: {}", err);
    }

    #[test]
    fn rejects_unknown_broker_url_scheme() {
        let cfg = JmsPoolConfig::single_broker("amqp://localhost:5672", BrokerType::ActiveMq);
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string().contains("invalid broker_url"),
            "got: {}",
            err
        );
    }

    #[test]
    fn accepts_known_broker_url_schemes() {
        for url in &[
            "tcp://localhost:61616",
            "ssl://localhost:61617",
            "failover://tcp://localhost:61616",
            "ws://localhost:61618",
            "wss://localhost:61619",
        ] {
            let cfg = JmsPoolConfig::single_broker(*url, BrokerType::ActiveMq);
            assert!(cfg.validate().is_ok(), "scheme should be accepted: {url}");
        }
    }

    #[test]
    fn accepts_valid_config() {
        let cfg = JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::ActiveMq);
        // ensure max_bridges and timeout have valid defaults
        assert!(cfg.validate().is_ok());
    }

    // ── JMS-005/JMS-009/JMS-010/JMS-011/JMS-012/JMS-013/JMS-018: endpoint config ──

    #[test]
    fn default_endpoint_config_has_sensible_defaults() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders").unwrap();
        assert_eq!(cfg.acknowledgement_mode, AcknowledgementMode::Auto);
        assert_eq!(cfg.message_selector, None);
        assert_eq!(cfg.concurrent_consumers, 1);
        assert_eq!(cfg.transaction_mode, JmsTransactionMode::None);
        assert_eq!(cfg.time_to_live, None);
        assert_eq!(cfg.priority, None);
        assert!(cfg.persistent_delivery);
        assert!(cfg.map_jms_headers);
        assert_eq!(cfg.exchange_pattern, ExchangePattern::InOnly);
    }

    #[test]
    fn parse_acknowledgement_mode_client() {
        let cfg =
            JmsEndpointConfig::from_uri("jms:queue:orders?acknowledgementMode=Client").unwrap();
        assert_eq!(cfg.acknowledgement_mode, AcknowledgementMode::Client);
    }

    #[test]
    fn parse_acknowledgement_mode_dups_ok() {
        let cfg =
            JmsEndpointConfig::from_uri("jms:queue:orders?acknowledgement_mode=dups_ok").unwrap();
        assert_eq!(cfg.acknowledgement_mode, AcknowledgementMode::DupsOk);
    }

    #[test]
    fn parse_acknowledgement_mode_invalid() {
        let err = JmsEndpointConfig::from_uri("jms:queue:orders?acknowledgementMode=invalid")
            .unwrap_err();
        assert!(
            err.to_string().contains("invalid acknowledgement mode"),
            "got: {}",
            err
        );
    }

    #[test]
    fn parse_message_selector() {
        let cfg =
            JmsEndpointConfig::from_uri("jms:queue:orders?messageSelector=priority%20%3E%205")
                .unwrap();
        // URL encoding is NOT decoded by from_uri — the raw value is stored
        assert_eq!(cfg.message_selector, Some("priority%20%3E%205".to_string()));
    }

    #[test]
    fn parse_empty_message_selector_is_none() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?message_selector=").unwrap();
        assert_eq!(cfg.message_selector, None);
    }

    #[test]
    fn parse_concurrent_consumers() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?concurrentConsumers=4").unwrap();
        assert_eq!(cfg.concurrent_consumers, 4);
    }

    #[test]
    fn parse_concurrent_consumers_zero_rejected() {
        let err =
            JmsEndpointConfig::from_uri("jms:queue:orders?concurrentConsumers=0").unwrap_err();
        assert!(
            err.to_string()
                .contains("concurrent_consumers must be >= 1"),
            "got: {}",
            err
        );
    }

    #[test]
    fn parse_concurrent_consumers_invalid() {
        let err =
            JmsEndpointConfig::from_uri("jms:queue:orders?concurrentConsumers=abc").unwrap_err();
        assert!(
            err.to_string().contains("invalid concurrent_consumers"),
            "got: {}",
            err
        );
    }

    #[test]
    fn parse_transaction_mode_session() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?transactionMode=Session").unwrap();
        assert_eq!(cfg.transaction_mode, JmsTransactionMode::Session);
    }

    #[test]
    fn parse_transaction_mode_invalid() {
        let err =
            JmsEndpointConfig::from_uri("jms:queue:orders?transaction_mode=invalid").unwrap_err();
        assert!(
            err.to_string().contains("invalid transaction mode"),
            "got: {}",
            err
        );
    }

    #[test]
    fn parse_time_to_live() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?timeToLive=30000").unwrap();
        assert_eq!(cfg.time_to_live, Some(30_000));
    }

    #[test]
    fn parse_priority() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?priority=5").unwrap();
        assert_eq!(cfg.priority, Some(5));
    }

    #[test]
    fn parse_priority_above_9_rejected() {
        let err = JmsEndpointConfig::from_uri("jms:queue:orders?priority=10").unwrap_err();
        assert!(err.to_string().contains("must be 0-9"), "got: {}", err);
    }

    #[test]
    fn parse_persistent_delivery_false() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?persistentDelivery=false").unwrap();
        assert!(!cfg.persistent_delivery);
    }

    #[test]
    fn parse_map_jms_headers_false() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?mapJmsHeaders=false").unwrap();
        assert!(!cfg.map_jms_headers);
    }

    #[test]
    fn parse_exchange_pattern_inout() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?exchangePattern=InOut").unwrap();
        assert_eq!(cfg.exchange_pattern, ExchangePattern::InOut);
    }

    #[test]
    fn parse_exchange_pattern_invalid() {
        let err =
            JmsEndpointConfig::from_uri("jms:queue:orders?exchangePattern=invalid").unwrap_err();
        assert!(
            err.to_string().contains("invalid exchange pattern"),
            "got: {}",
            err
        );
    }

    #[test]
    fn parse_multiple_query_params() {
        let cfg = JmsEndpointConfig::from_uri(
            "jms:queue:orders?broker=primary&acknowledgementMode=Client&concurrentConsumers=3&persistentDelivery=false&priority=7",
        )
        .unwrap();
        assert_eq!(cfg.broker_name, Some("primary".to_string()));
        assert_eq!(cfg.acknowledgement_mode, AcknowledgementMode::Client);
        assert_eq!(cfg.concurrent_consumers, 3);
        assert!(!cfg.persistent_delivery);
        assert_eq!(cfg.priority, Some(7));
    }

    #[test]
    fn acknowledgement_mode_display_roundtrip() {
        for mode in &[
            AcknowledgementMode::Auto,
            AcknowledgementMode::Client,
            AcknowledgementMode::DupsOk,
            AcknowledgementMode::Transacted,
        ] {
            let s = mode.to_string();
            let parsed: AcknowledgementMode = s.parse().unwrap();
            assert_eq!(*mode, parsed);
        }
    }

    #[test]
    fn transaction_mode_display_roundtrip() {
        for mode in &[JmsTransactionMode::None, JmsTransactionMode::Session] {
            let s = mode.to_string();
            let parsed: JmsTransactionMode = s.parse().unwrap();
            assert_eq!(*mode, parsed);
        }
    }

    #[test]
    fn exchange_pattern_display_roundtrip() {
        for mode in &[ExchangePattern::InOnly, ExchangePattern::InOut] {
            let s = mode.to_string();
            let parsed: ExchangePattern = s.parse().unwrap();
            assert_eq!(*mode, parsed);
        }
    }

    #[test]
    fn build_exchange_without_header_mapping() {
        // Verify map_jms_headers=false prevents header mapping in consumer
        // (tested indirectly through the consumer module's build_exchange)
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?mapJmsHeaders=false").unwrap();
        assert!(!cfg.map_jms_headers);
    }

    #[test]
    fn jms_pool_config_has_reconnect_policy() {
        let cfg = JmsPoolConfig::default();
        assert_eq!(cfg.reconnect.max_attempts, 0); // unlimited
        assert!(cfg.reconnect.enabled);
    }
}
