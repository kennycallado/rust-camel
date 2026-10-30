use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use camel_api::redact::redact_url;
use camel_component_api::{CamelError, NetworkRetryPolicy};
use serde::Deserialize;

/// Broker password wrapper that never renders its value.
#[derive(Clone, Deserialize)]
pub struct SecretString(String);

impl SecretString {
    /// Borrow the wrapped secret. Callers must not log the result.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<&str> for SecretString {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// One `[components.rabbitmq.brokers.<name>]` entry.
///
/// Supported keys are exactly `url`, `username`, `password`, `vhost`, `tls`.
/// The optional `tls` section carries the TLS overrides defined by
/// [`crate::tls::RabbitTlsConfig`]: `ca_cert` is a custom trust anchor (valid
/// only with `amqps://`) and `server_name` is rejected at boot. Boot
/// validation of every configured broker runs in `RabbitMqBundle::from_toml`.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RabbitBrokerConfig {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<SecretString>,
    pub vhost: Option<String>,
    /// Optional TLS overrides for this broker (`[brokers.<name>.tls]`).
    pub tls: Option<crate::tls::RabbitTlsConfig>,
}

impl fmt::Debug for RabbitBrokerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RabbitBrokerConfig")
            .field("url", &redact_url(&self.url))
            .field("username", &self.username)
            .field("password", &self.password)
            .field("vhost", &self.vhost)
            // A PEM path and a hostname carry no secret material.
            .field("tls", &self.tls)
            .finish()
    }
}

/// `[components.rabbitmq]` section.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RabbitComponentConfig {
    #[serde(default)]
    pub brokers: HashMap<String, RabbitBrokerConfig>,
    pub reconnect: Option<NetworkRetryPolicy>,
}

/// Per-component reconnect default, mirroring `jms_reconnect_default`:
/// unlimited retries (max_attempts = 0) with exponential backoff.
pub fn rabbitmq_reconnect_default() -> NetworkRetryPolicy {
    NetworkRetryPolicy {
        max_attempts: 0, // unlimited
        initial_delay: Duration::from_secs(5),
        multiplier: 2.0,
        max_delay: Duration::from_secs(30),
        jitter_factor: 0.0,
        ..NetworkRetryPolicy::default()
    }
}

fn no_brokers_error() -> CamelError {
    CamelError::ProcessorError(
        "No RabbitMQ brokers configured — declare at least one in \
         [components.rabbitmq.brokers] in Camel.toml"
            .to_string(),
    )
}

/// Resolve the broker name for a URI's optional `?broker=` parameter.
///
/// - `Some(name)` must name a configured broker.
/// - `None` selects the sole broker when exactly one is configured.
/// - `None` with no brokers errors, naming the Camel.toml section.
/// - `None` with several brokers errors with the count and sorted names.
pub fn resolve_broker_name(
    brokers: &HashMap<String, RabbitBrokerConfig>,
    requested: Option<&str>,
) -> Result<String, CamelError> {
    match requested {
        Some(name) => {
            if brokers.contains_key(name) {
                Ok(name.to_string())
            } else {
                Err(CamelError::ProcessorError(format!(
                    "Unknown RabbitMQ broker '{name}' — declare it in \
                     [components.rabbitmq.brokers] in Camel.toml"
                )))
            }
        }
        None => match brokers.len() {
            0 => Err(no_brokers_error()),
            1 => brokers.keys().next().cloned().ok_or_else(no_brokers_error),
            _ => {
                let mut names: Vec<&str> = brokers.keys().map(String::as_str).collect();
                names.sort_unstable();
                Err(CamelError::ProcessorError(format!(
                    "Multiple RabbitMQ brokers configured ({}: {}); specify one with ?broker=<name> in the URI",
                    names.len(),
                    names.join(", ")
                )))
            }
        },
    }
}

/// URI parameters implemented in Phase 1 (the producer option set).
///
/// The metadata parity test asserts the descriptor exposes exactly these names.
pub const P1_OPTIONS: &[&str] = &["broker", "queue", "routingKey", "persistent", "contentType"];

/// Phase-2 URI parameters implemented so far (the consumer option set).
///
/// Incremental by draft: task 2.2 adds `requeueOnFailure`; task 2.3 appends
/// `prefetch`/`concurrentConsumers`, and task 2.6 freezes the full P2 set. The
/// metadata parity test asserts the descriptor exposes `P1_OPTIONS ∪
/// P2_OPTIONS` exactly.
pub const P2_OPTIONS: &[&str] = &["requeueOnFailure", "prefetch", "concurrentConsumers"];

/// Frozen Phase-3 URI parameters.
///
/// Landed incrementally: task 3.1 added `confirmTimeout`; task 3.2 added
/// `mandatory`; task 3.4 added `autoDeclare`, `exchangeType`, `durableQueue`
/// and `queueArguments`. Task 3.5 freezes the complete set. The metadata parity
/// test asserts the descriptor exposes `P1_OPTIONS ∪ P2_OPTIONS ∪ P3_OPTIONS`
/// exactly, so this list is the frozen P3 contract (no P4 option is added).
pub const P3_OPTIONS: &[&str] = &[
    "confirmTimeout",
    "mandatory",
    "autoDeclare",
    "exchangeType",
    "durableQueue",
    "queueArguments",
];

/// Phase-4 URI parameters implemented so far (the InOut request/reply set).
///
/// Task 4.1 adds `replyTimeout`; task 4.4 freezes the complete P4 set. The
/// metadata parity test asserts `P1 ∪ P2 ∪ P3 ∪ P4` exactly.
pub const P4_OPTIONS: &[&str] = &["replyTimeout"];

/// Default confirm wait (URI `confirmTimeout`, milliseconds): 5 s.
///
/// The producer awaits the broker's publish confirm at most this long; the
/// option surfaces in P3 (task 3.1) to override it.
pub const DEFAULT_CONFIRM_TIMEOUT_MS: u64 = 5000;

/// Default InOut direct reply-to wait (URI `replyTimeout`, milliseconds): 30 s.
///
/// An InOut request awaits the broker reply at most this long; task 4.1 adds
/// the option and the `ReplyTimeout` failure.
pub const DEFAULT_REPLY_TIMEOUT_MS: u64 = 30000;

/// Default for the mandatory flag (URI `mandatory`): `false`.
///
/// The AMQP `mandatory` flag asks the broker to return (rather than silently
/// drop) an unroutable message; the producer maps that return onto the
/// exchange's error path (task 3.2).
pub const DEFAULT_MANDATORY: bool = false;

/// Default for opt-in active topology declaration (URI `autoDeclare`): `false`.
///
/// When `false` the consumer start only passively checks that the queue exists
/// (task 3.3). The Camel spring-rabbitmq consumer defaults to `true`; this
/// component requires the explicit opt-in because an active declare mutates
/// broker topology (documented divergence).
pub const DEFAULT_AUTO_DECLARE: bool = false;

/// Default exchange kind for an active declare (URI `exchangeType`): `direct`.
pub const DEFAULT_EXCHANGE_TYPE: &str = "direct";

/// Default durability of the actively declared queue (URI `durableQueue`):
/// `true`.
pub const DEFAULT_DURABLE_QUEUE: bool = true;

/// True when `key` is a URI parameter this component understands.
fn is_known_option(key: &str) -> bool {
    P1_OPTIONS.contains(&key)
        || P2_OPTIONS.contains(&key)
        || P3_OPTIONS.contains(&key)
        || P4_OPTIONS.contains(&key)
}

/// Parsed `rabbitmq:<exchange>?queue=<queue>&routingKey=<key>` endpoint URI.
#[derive(Clone, Debug)]
pub struct RabbitEndpointConfig {
    /// Named broker from `[components.rabbitmq.brokers]` (URI `broker` param).
    pub broker: Option<String>,
    /// Exchange name; the empty string is the broker's default exchange.
    pub exchange: String,
    /// Queue name (URI `queue` param).
    pub queue: Option<String>,
    /// Routing key (URI `routingKey` param); falls back to `queue`.
    pub routing_key: Option<String>,
    /// Delivery mode 2 (`true`, default) or 1 (`false`).
    pub persistent: bool,
    /// Content type (URI `contentType` param).
    pub content_type: Option<String>,
    /// Whether a failed route nacks with `requeue=true` (URI
    /// `requeueOnFailure`, default `false`).
    ///
    /// WARNING: requeueing on failure without a broker-side delivery limit
    /// (a queue `x-delivery-limit` and/or a dead-letter exchange) forms a hot
    /// loop — the broker immediately redelivers the same poison message and
    /// the failing route requeues it forever. Leave this `false` (reject to
    /// the DLX) unless the queue bounds redeliveries.
    pub requeue_on_failure: bool,
    /// Broker-side prefetch (URI `prefetch`, default `10`): the maximum number
    /// of unacknowledged deliveries per channel (`basic.qos`). Zero is invalid.
    pub prefetch: u16,
    /// Number of channel + consumer engines (URI `concurrentConsumers`,
    /// default `1`). Each engine gets its own AMQP channel so prefetch applies
    /// per engine. Zero is invalid.
    pub concurrent_consumers: u32,
    /// Bound on the publisher-confirm wait (URI `confirmTimeout`,
    /// milliseconds, default `5000`). A confirm that does not arrive within
    /// this bound fails the exchange; the cached channel is dropped because
    /// its confirm accounting is then uncertain (task 3.1).
    pub confirm_timeout: Duration,
    /// Bound on the InOut direct reply-to wait (URI `replyTimeout`,
    /// milliseconds, default `30000`). On expiry the request fails
    /// `ReplyTimeout` and its correlation entry is removed, so a late reply is
    /// dropped (task 4.1).
    pub reply_timeout: Duration,
    /// AMQP `mandatory` flag (URI `mandatory`, default `false`). When `true`,
    /// an unroutable message the broker returns via `basic.return` fails the
    /// exchange naming the target; when `false` the broker drops it silently
    /// and the publish is `Ok` (task 3.2).
    pub mandatory: bool,
    /// Opt-in active topology declaration at consumer start (URI `autoDeclare`,
    /// default `false`, task 3.4). When `true` the start actively declares the
    /// exchange, queue, and binding; when `false` it only passively checks the
    /// queue exists (task 3.3).
    pub auto_declare: bool,
    /// Exchange kind for an active declare (URI `exchangeType`, default
    /// `"direct"`). Known AMQP kinds map to lapin's `ExchangeKind`; any other
    /// string is passed through as `ExchangeKind::Custom` and the broker
    /// decides (a rejected type fails the route start, fail closed).
    pub exchange_type: String,
    /// Durability of the actively declared queue (URI `durableQueue`, default
    /// `true`). Applies to the QUEUE only; the exchange is always declared
    /// durable (task 3.4).
    pub durable_queue: bool,
    /// Queue arguments (URI `queueArguments`, default empty): a JSON object
    /// whose values are scalars. A string maps to a long-string `FieldTable`
    /// entry, an integer to a long-long-int, a float to a double, and a boolean
    /// to a boolean (e.g. `{"x-message-ttl":30000}`). Null, arrays, and nested
    /// objects fail closed at parse time.
    pub queue_arguments: HashMap<String, serde_json::Value>,
}

impl RabbitEndpointConfig {
    /// Parse a `rabbitmq:` URI (same shape as
    /// `camel-kafka/src/config.rs::KafkaEndpointConfig::from_uri`).
    ///
    /// Unknown query parameters fail closed, naming the offending parameter.
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = camel_component_api::parse_uri(uri)?;

        if parts.scheme != "rabbitmq" {
            return Err(CamelError::InvalidUri(format!(
                "expected scheme 'rabbitmq', got '{}'",
                parts.scheme
            )));
        }

        for key in parts.params.keys() {
            if !is_known_option(key.as_str()) {
                return Err(CamelError::InvalidUri(format!(
                    "unknown query parameter '{key}' for scheme 'rabbitmq'"
                )));
            }
        }

        // Path is the exchange name; empty or `default` means the default exchange.
        let path = parts.path.trim_start_matches('/');
        let exchange = if path.is_empty() || path == "default" {
            String::new()
        } else {
            path.to_string()
        };

        let broker = parts.params.get("broker").cloned();
        let queue = parts.params.get("queue").cloned();
        let routing_key = parts.params.get("routingKey").cloned();

        // The default exchange routes by queue name, so it needs a target.
        // A named exchange may publish without one (fanout/headers ignore the
        // routing key); `target()` then defaults the key to "".
        if exchange.is_empty() && queue.is_none() && routing_key.is_none() {
            return Err(CamelError::InvalidUri(
                "rabbitmq URI requires the 'queue' or 'routingKey' parameter".to_string(),
            ));
        }

        let persistent = match parts.params.get("persistent") {
            Some(raw) => raw.parse::<bool>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "persistent must be a boolean ('true' or 'false'), got '{raw}'"
                ))
            })?,
            None => true,
        };

        let content_type = parts.params.get("contentType").cloned();

        let requeue_on_failure = match parts.params.get("requeueOnFailure") {
            Some(raw) => raw.parse::<bool>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "requeueOnFailure must be a boolean ('true' or 'false'), got '{raw}'"
                ))
            })?,
            None => false,
        };

        let prefetch = match parts.params.get("prefetch") {
            Some(raw) => {
                let parsed = raw.parse::<u16>().map_err(|_| {
                    CamelError::InvalidUri(format!(
                        "prefetch must be a positive integer (<= 65535), got '{raw}'"
                    ))
                })?;
                if parsed == 0 {
                    return Err(CamelError::InvalidUri(
                        "prefetch must be greater than 0".to_string(),
                    ));
                }
                parsed
            }
            None => 10,
        };

        let concurrent_consumers = match parts.params.get("concurrentConsumers") {
            Some(raw) => {
                let parsed = raw.parse::<u32>().map_err(|_| {
                    CamelError::InvalidUri(format!(
                        "concurrentConsumers must be a positive integer, got '{raw}'"
                    ))
                })?;
                if parsed == 0 {
                    return Err(CamelError::InvalidUri(
                        "concurrentConsumers must be greater than 0".to_string(),
                    ));
                }
                parsed
            }
            None => 1,
        };

        let confirm_timeout = match parts.params.get("confirmTimeout") {
            Some(raw) => {
                let millis = raw.parse::<u64>().map_err(|_| {
                    CamelError::InvalidUri(format!(
                        "confirmTimeout must be a non-negative integer of milliseconds, got '{raw}'"
                    ))
                })?;
                Duration::from_millis(millis)
            }
            None => Duration::from_millis(DEFAULT_CONFIRM_TIMEOUT_MS),
        };

        let reply_timeout = match parts.params.get("replyTimeout") {
            Some(raw) => {
                let millis = raw.parse::<u64>().map_err(|_| {
                    CamelError::InvalidUri(format!(
                        "replyTimeout must be a non-negative integer of milliseconds, got '{raw}'"
                    ))
                })?;
                Duration::from_millis(millis)
            }
            None => Duration::from_millis(DEFAULT_REPLY_TIMEOUT_MS),
        };

        let mandatory = match parts.params.get("mandatory") {
            Some(raw) => raw.parse::<bool>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "mandatory must be a boolean ('true' or 'false'), got '{raw}'"
                ))
            })?,
            None => DEFAULT_MANDATORY,
        };

        let auto_declare = match parts.params.get("autoDeclare") {
            Some(raw) => raw.parse::<bool>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "autoDeclare must be a boolean ('true' or 'false'), got '{raw}'"
                ))
            })?,
            None => DEFAULT_AUTO_DECLARE,
        };

        let exchange_type = match parts.params.get("exchangeType") {
            Some(raw) => raw.clone(),
            None => DEFAULT_EXCHANGE_TYPE.to_string(),
        };

        let durable_queue = match parts.params.get("durableQueue") {
            Some(raw) => raw.parse::<bool>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "durableQueue must be a boolean ('true' or 'false'), got '{raw}'"
                ))
            })?,
            None => DEFAULT_DURABLE_QUEUE,
        };

        // `queueArguments` is a JSON object whose values are scalars. No
        // existing component parses map params from URIs, so this JSON-object
        // encoding is this change's decision: a non-object, or a value that
        // cannot map to an AMQP scalar, fails closed naming the option.
        let queue_arguments = match parts.params.get("queueArguments") {
            Some(raw) => {
                // Parse into raw tokens first: serde_json silently reclassifies
                // integer tokens that fit neither `i64` nor `u64` as `f64`, so
                // classifying the parsed `Value` lets out-of-range integers
                // slip through as doubles. The raw token text is authoritative.
                let parsed =
                    serde_json::from_str::<HashMap<String, Box<serde_json::value::RawValue>>>(raw)
                        .map_err(|_| {
                            CamelError::InvalidUri(format!(
                                "queueArguments must be a JSON object, got '{raw}'"
                            ))
                        })?;
                let mut values = HashMap::with_capacity(parsed.len());
                for (key, raw_value) in parsed {
                    values.insert(key.clone(), parse_queue_argument(&key, raw_value.get())?);
                }
                values
            }
            None => HashMap::new(),
        };

        Ok(Self {
            broker,
            exchange,
            queue,
            routing_key,
            persistent,
            content_type,
            requeue_on_failure,
            prefetch,
            concurrent_consumers,
            confirm_timeout,
            reply_timeout,
            mandatory,
            auto_declare,
            exchange_type,
            durable_queue,
            queue_arguments,
        })
    }

    /// Publish target `(exchange, routing_key)`.
    ///
    /// An explicit `routingKey` wins; otherwise the queue name is used.
    /// A named exchange may carry neither (fanout/headers ignore the routing
    /// key), in which case the key is `""`. Only the default exchange requires
    /// a target, guaranteed by `from_uri`.
    pub fn target(&self) -> (String, String) {
        let routing_key = self
            .routing_key
            .clone()
            .or_else(|| self.queue.clone())
            .unwrap_or_default();
        (self.exchange.clone(), routing_key)
    }
}

/// Classify one `queueArguments` entry from its raw JSON token.
///
/// The raw token is authoritative: `serde_json` parses integer tokens that fit
/// neither `i64` nor `u64` as `f64`, so a parsed `Value` cannot distinguish
/// `-9223372036854775809` from a genuine float. Integer tokens must fit a
/// signed 64-bit value; strings, booleans, and genuine float syntax are
/// accepted. Everything else fails closed naming the option and key.
fn parse_queue_argument(key: &str, token: &str) -> Result<serde_json::Value, CamelError> {
    let token = token.trim();
    if let Ok(integer) = token.parse::<i64>() {
        return Ok(serde_json::Value::from(integer));
    }
    if token.parse::<u64>().is_ok() {
        return Err(queue_argument_error(
            key,
            "an integer that exceeds the signed 64-bit (i64) range",
        ));
    }
    // A pure integer token (e.g. `-9223372036854775809`) that fits neither
    // `i64` nor `u64` must not be mistaken for float syntax below: serde_json
    // parses it as `f64`, but it is an out-of-range integer, not a float.
    if is_integer_token(token) {
        return Err(queue_argument_error(
            key,
            "an integer that exceeds the signed 64-bit (i64) range",
        ));
    }
    match serde_json::from_str::<serde_json::Value>(token) {
        Ok(serde_json::Value::Bool(flag)) => Ok(serde_json::Value::Bool(flag)),
        Ok(serde_json::Value::String(text)) => Ok(serde_json::Value::String(text)),
        // `i64`/`u64` tokens are consumed above, so any remaining number is a
        // genuine float such as `1.5` or `1e20`.
        Ok(serde_json::Value::Number(number)) => Ok(serde_json::Value::Number(number)),
        Ok(serde_json::Value::Null) => Err(queue_argument_error(key, "a null value")),
        Ok(serde_json::Value::Array(_)) => Err(queue_argument_error(key, "an array")),
        Ok(serde_json::Value::Object(_)) => Err(queue_argument_error(key, "a nested object")),
        Err(_) => Err(queue_argument_error(key, "a non-scalar token")),
    }
}

fn queue_argument_error(key: &str, kind: &str) -> CamelError {
    CamelError::InvalidUri(format!(
        "queueArguments value for '{key}' must be a string, integer fitting a \
         signed 64-bit value, float, or boolean, got {kind}"
    ))
}

/// Whether `token` is pure JSON integer syntax (optional sign plus digits).
fn is_integer_token(token: &str) -> bool {
    let digits = token
        .strip_prefix('-')
        .or_else(|| token.strip_prefix('+'))
        .unwrap_or(token);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use super::*;

    fn broker(url: &str) -> RabbitBrokerConfig {
        RabbitBrokerConfig {
            url: url.to_string(),
            username: None,
            password: None,
            vhost: None,
            tls: None,
        }
    }

    fn err_text(result: Result<String, CamelError>) -> String {
        match result {
            Ok(value) => panic!("expected error, got Ok({value})"),
            Err(error) => error.to_string(),
        }
    }

    fn err_message<T>(result: Result<T, CamelError>) -> String {
        match result {
            Ok(_) => panic!("expected error, got Ok"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn endpoint_config_parses_default_exchange() {
        let config = RabbitEndpointConfig::from_uri("rabbitmq:default?queue=orders")
            .expect("default-exchange URI must parse");
        assert_eq!(config.exchange, "");
        assert_eq!(config.target(), ("".to_string(), "orders".to_string()));
        // Task 3.4 defaults: no active declare, direct exchange, durable queue,
        // no queue arguments.
        assert!(!config.auto_declare, "autoDeclare must default to false");
        assert_eq!(
            config.exchange_type, "direct",
            "exchangeType must default to direct"
        );
        assert!(config.durable_queue, "durableQueue must default to true");
        assert!(
            config.queue_arguments.is_empty(),
            "queueArguments must default to an empty map"
        );
    }

    #[test]
    fn endpoint_config_rejects_non_object_queue_arguments() {
        for raw in ["[1,2]", "5", r#""plain""#] {
            let message = err_message(RabbitEndpointConfig::from_uri(&format!(
                "rabbitmq:default?queue=q&queueArguments={raw}"
            )));
            assert!(message.contains("queueArguments"), "got: {message}");
            assert!(
                message.contains("JSON object"),
                "a non-object queueArguments must be rejected with the \
                 JSON-object diagnostic, got: {message}"
            );
        }
    }

    #[test]
    fn queue_arguments_accepts_integer_value() {
        let config = RabbitEndpointConfig::from_uri(
            r#"rabbitmq:default?queue=q&queueArguments={"x-message-ttl":30000}"#,
        )
        .expect("integer queueArguments must parse");
        match config.queue_arguments.get("x-message-ttl") {
            Some(serde_json::Value::Number(number)) => {
                assert_eq!(number.as_i64(), Some(30000), "got: {number}");
            }
            other => panic!("expected an integer value, got {other:?}"),
        }
    }

    #[test]
    fn queue_arguments_accepts_float_value() {
        let config = RabbitEndpointConfig::from_uri(
            r#"rabbitmq:default?queue=q&queueArguments={"x-ratio":1.5}"#,
        )
        .expect("float queueArguments must parse");
        match config.queue_arguments.get("x-ratio") {
            Some(serde_json::Value::Number(number)) => {
                assert_eq!(number.as_f64(), Some(1.5), "got: {number}");
            }
            other => panic!("expected a float value, got {other:?}"),
        }
    }

    #[test]
    fn queue_arguments_accepts_boolean_value() {
        let config = RabbitEndpointConfig::from_uri(
            r#"rabbitmq:default?queue=q&queueArguments={"x-single-active-consumer":true}"#,
        )
        .expect("boolean queueArguments must parse");
        assert_eq!(
            config.queue_arguments.get("x-single-active-consumer"),
            Some(&serde_json::Value::Bool(true))
        );
    }

    #[test]
    fn queue_arguments_accepts_string_value() {
        let config = RabbitEndpointConfig::from_uri(
            r#"rabbitmq:default?queue=q&queueArguments={"x-dead-letter-exchange":"dlx"}"#,
        )
        .expect("string queueArguments must parse");
        assert_eq!(
            config.queue_arguments.get("x-dead-letter-exchange"),
            Some(&serde_json::Value::String("dlx".to_string()))
        );
    }

    #[test]
    fn queue_arguments_rejects_array_value() {
        let message = err_message(RabbitEndpointConfig::from_uri(
            r#"rabbitmq:default?queue=q&queueArguments={"x-bad":[1]}"#,
        ));
        assert!(message.contains("queueArguments"), "got: {message}");
        assert!(message.contains("x-bad"), "got: {message}");
        assert!(message.contains("array"), "got: {message}");
    }

    #[test]
    fn queue_arguments_rejects_object_value() {
        let message = err_message(RabbitEndpointConfig::from_uri(
            r#"rabbitmq:default?queue=q&queueArguments={"x-bad":{"a":1}}"#,
        ));
        assert!(message.contains("queueArguments"), "got: {message}");
        assert!(message.contains("x-bad"), "got: {message}");
        assert!(message.contains("object"), "got: {message}");
    }

    #[test]
    fn queue_arguments_rejects_null_value() {
        let message = err_message(RabbitEndpointConfig::from_uri(
            r#"rabbitmq:default?queue=q&queueArguments={"x-bad":null}"#,
        ));
        assert!(message.contains("queueArguments"), "got: {message}");
        assert!(message.contains("x-bad"), "got: {message}");
        assert!(message.contains("null"), "got: {message}");
    }

    #[test]
    fn queue_arguments_rejects_out_of_range_integer() {
        let message = err_message(RabbitEndpointConfig::from_uri(
            r#"rabbitmq:default?queue=q&queueArguments={"x-big":18446744073709551615}"#,
        ));
        assert!(message.contains("queueArguments"), "got: {message}");
        assert!(message.contains("x-big"), "got: {message}");
        assert!(message.contains("i64"), "got: {message}");
    }

    #[test]
    fn queue_arguments_rejects_negative_integer_below_i64_min() {
        let message = err_message(RabbitEndpointConfig::from_uri(
            r#"rabbitmq:default?queue=q&queueArguments={"x-big":-9223372036854775809}"#,
        ));
        assert!(message.contains("queueArguments"), "got: {message}");
        assert!(message.contains("x-big"), "got: {message}");
    }

    #[test]
    fn queue_arguments_rejects_positive_integer_above_u64_range() {
        let message = err_message(RabbitEndpointConfig::from_uri(
            r#"rabbitmq:default?queue=q&queueArguments={"x-big":18446744073709551616}"#,
        ));
        assert!(message.contains("queueArguments"), "got: {message}");
        assert!(message.contains("x-big"), "got: {message}");
    }

    #[test]
    fn queue_arguments_accepts_genuine_float_syntax_beyond_integer_ranges() {
        let config = RabbitEndpointConfig::from_uri(
            r#"rabbitmq:default?queue=q&queueArguments={"x-float":1e20}"#,
        )
        .expect("genuine float syntax must parse as a float");
        match config.queue_arguments.get("x-float") {
            Some(serde_json::Value::Number(number)) => {
                assert_eq!(number.as_f64(), Some(1e20), "got: {number}");
                assert!(number.is_f64(), "1e20 must be a float, got: {number}");
            }
            other => panic!("expected a float value, got {other:?}"),
        }
    }

    #[test]
    fn endpoint_config_explicit_routing_key_wins() {
        let config = RabbitEndpointConfig::from_uri("rabbitmq:ex?queue=q&routingKey=rk")
            .expect("explicit routingKey URI must parse");
        assert_eq!(config.target(), ("ex".to_string(), "rk".to_string()));
    }

    #[test]
    fn endpoint_config_routing_key_falls_back_to_queue() {
        let config = RabbitEndpointConfig::from_uri("rabbitmq:ex?queue=q")
            .expect("queue-only URI must parse");
        assert_eq!(config.target(), ("ex".to_string(), "q".to_string()));
    }

    #[test]
    fn endpoint_config_fanout_exchange_without_target_parses() {
        let config = RabbitEndpointConfig::from_uri("rabbitmq:fans?exchangeType=fanout")
            .expect("a fanout exchange must not require queue or routingKey");
        assert_eq!(config.exchange, "fans");
        assert_eq!(config.target(), ("fans".to_string(), String::new()));
    }

    #[test]
    fn endpoint_config_headers_exchange_without_target_parses() {
        let config = RabbitEndpointConfig::from_uri("rabbitmq:hdr?exchangeType=headers")
            .expect("a headers exchange must not require queue or routingKey");
        assert_eq!(config.exchange, "hdr");
        assert_eq!(config.target(), ("hdr".to_string(), String::new()));
    }

    #[test]
    fn endpoint_config_fanout_explicit_routing_key_unchanged() {
        let config =
            RabbitEndpointConfig::from_uri("rabbitmq:fans?exchangeType=fanout&routingKey=rk")
                .expect("an explicit routingKey on a fanout exchange must parse");
        assert_eq!(config.target(), ("fans".to_string(), "rk".to_string()));
    }

    #[test]
    fn endpoint_config_requires_queue_or_routing_key_for_default_exchange() {
        let message = err_message(RabbitEndpointConfig::from_uri("rabbitmq:"));
        assert!(message.contains("queue"), "got: {message}");
        assert!(message.contains("routingKey"), "got: {message}");
    }

    #[test]
    fn endpoint_config_rejects_unknown_param() {
        let message = err_message(RabbitEndpointConfig::from_uri(
            "rabbitmq:ex?queue=q&bogus=1",
        ));
        assert!(message.contains("bogus"), "got: {message}");
    }

    #[test]
    fn endpoint_config_rejects_wrong_scheme() {
        assert!(
            RabbitEndpointConfig::from_uri("amqp:ex").is_err(),
            "amqp: is reserved for a future component and must be rejected"
        );
    }

    #[test]
    fn endpoint_config_parses_content_type() {
        let config =
            RabbitEndpointConfig::from_uri("rabbitmq:ex?queue=q&contentType=application/json")
                .expect("contentType URI must parse");
        assert_eq!(config.content_type.as_deref(), Some("application/json"));
    }

    #[test]
    fn config_rejects_zero_prefetch() {
        let message = err_message(RabbitEndpointConfig::from_uri(
            "rabbitmq:default?queue=q&prefetch=0",
        ));
        assert!(message.contains("prefetch"), "got: {message}");
        // Must be the invalid-value diagnostic, not the unknown-option one.
        assert!(message.contains("must be greater than 0"), "got: {message}");
    }

    #[test]
    fn config_rejects_zero_concurrent_consumers() {
        let message = err_message(RabbitEndpointConfig::from_uri(
            "rabbitmq:default?queue=q&concurrentConsumers=0",
        ));
        assert!(message.contains("concurrentConsumers"), "got: {message}");
        assert!(message.contains("must be greater than 0"), "got: {message}");
    }

    #[test]
    fn resolve_broker_name_single_broker_selects_implicitly() {
        let mut brokers = HashMap::new();
        brokers.insert("main".to_string(), broker("amqp://localhost:5672"));
        let resolved = resolve_broker_name(&brokers, None);
        assert_eq!(resolved.ok().as_deref(), Some("main"));
    }

    #[test]
    fn resolve_broker_name_explicit_unknown_errors() {
        let mut brokers = HashMap::new();
        brokers.insert("main".to_string(), broker("amqp://localhost:5672"));
        let message = err_text(resolve_broker_name(&brokers, Some("ghost")));
        assert!(
            message.contains("components.rabbitmq.brokers"),
            "got: {message}"
        );
    }

    #[test]
    fn resolve_broker_name_ambiguous_errors() {
        let mut brokers = HashMap::new();
        brokers.insert("main".to_string(), broker("amqp://main:5672"));
        brokers.insert("backup".to_string(), broker("amqp://backup:5672"));
        let message = err_text(resolve_broker_name(&brokers, None));
        assert!(message.contains("broker="), "got: {message}");
        assert!(message.contains('2'), "got: {message}");
        assert!(message.contains("main"), "got: {message}");
        assert!(message.contains("backup"), "got: {message}");
    }

    #[test]
    fn resolve_broker_name_empty_errors() {
        let brokers = HashMap::new();
        let message = err_text(resolve_broker_name(&brokers, None));
        assert!(
            message.contains("components.rabbitmq.brokers"),
            "got: {message}"
        );
    }

    #[test]
    fn broker_config_debug_redacts_password() {
        let config = RabbitBrokerConfig {
            url: "amqp://user:credentialURL@host:5672/".to_string(),
            username: Some("user".to_string()),
            password: Some(SecretString::from("hunter2")),
            vhost: None,
            tls: None,
        };
        let rendered = format!("{config:?}");
        assert!(rendered.contains("<redacted>"), "got: {rendered}");
        assert!(!rendered.contains("hunter2"), "got: {rendered}");
        assert!(
            rendered.contains("amqp://***@host:5672/"),
            "got: {rendered}"
        );
        assert!(!rendered.contains("credentialURL"), "got: {rendered}");
    }

    #[test]
    fn broker_config_rejects_unknown_field() {
        let raw = "url = \"amqp://localhost:5672\"\nbogus = 1\n";
        let parsed = toml::from_str::<RabbitBrokerConfig>(raw);
        assert!(parsed.is_err(), "expected deny_unknown_fields rejection");
    }

    #[test]
    fn redact_url_canonical_masks_password() {
        assert_eq!(
            camel_api::redact::redact_url("amqp://u:p@h:5672/"),
            "amqp://***@h:5672/"
        );
    }

    #[test]
    fn reconnect_default_matches_jms_shape() {
        let policy = rabbitmq_reconnect_default();
        assert!(policy.enabled, "expected enabled");
        assert_eq!(policy.max_attempts, 0);
        assert_eq!(policy.initial_delay, Duration::from_secs(5));
        assert_eq!(policy.multiplier, 2.0);
        assert_eq!(policy.max_delay, Duration::from_secs(30));
        assert_eq!(policy.jitter_factor, 0.0);
    }
}
