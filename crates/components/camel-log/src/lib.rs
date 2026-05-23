//! Log component for rust-camel — routes exchange body and headers to the
//! `tracing` subsystem at a configurable log level, primarily for debugging.
//!
//! Main types: `LogComponent`, `LogEndpoint`, `LogProducer`, `LogLevel`.

use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

use tower::Service;
use tracing::{debug, error, info, trace, warn};

use camel_component_api::UriConfig;
use camel_component_api::parse_uri;
use camel_component_api::{BoxProcessor, CamelError, Exchange};
use camel_component_api::{Component, Consumer, Endpoint, ProducerContext};

// ---------------------------------------------------------------------------
// LogLevel
// ---------------------------------------------------------------------------

/// Log level for the log component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogLevel {
    Trace,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

impl FromStr for LogLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_log_level(s).map_err(|e| e.to_string())
    }
}

fn parse_log_level(s: &str) -> Result<LogLevel, CamelError> {
    match s.to_uppercase().as_str() {
        "TRACE" => Ok(LogLevel::Trace),
        "DEBUG" => Ok(LogLevel::Debug),
        "INFO" => Ok(LogLevel::Info),
        "WARN" | "WARNING" => Ok(LogLevel::Warn),
        "ERROR" => Ok(LogLevel::Error),
        _ => Err(CamelError::Config(format!(
            "unknown log level: '{}'. Valid: TRACE, DEBUG, INFO, WARN, ERROR",
            s
        ))),
    }
}

// ---------------------------------------------------------------------------
// LogConfig
// ---------------------------------------------------------------------------

/// Configuration parsed from a log URI.
///
/// Format: `log:category?level=info&showHeaders=true&showBody=true&logMask=false&showStreamInfo=false&groupSize=10`
#[derive(Debug, Clone)]
pub struct LogConfig {
    /// Log category (the path portion of the URI).
    pub category: String,
    /// Log level. Default: Info.
    pub level: LogLevel,
    /// Whether to include headers in the log output.
    pub show_headers: bool,
    /// Whether to include the body in the log output.
    pub show_body: bool,
    /// Maximum number of characters for the body in log output.
    /// Bodies longer than this are truncated. `None` means no limit.
    pub max_chars: Option<usize>,
    /// When true, redact sensitive headers and body in log output.
    /// Headers matching `/(?i)(password|secret|token|key|auth|credential)/` → `[REDACTED]`.
    /// Body → `[Body redacted by logMask]`.
    pub log_mask: bool,
    /// When true, show stream origin metadata. When false (default), show `[Stream]`.
    pub show_stream_info: bool,
    /// When set, only emit a log every `n` exchanges (group logging).
    /// The log message includes the exchange count.
    pub group_size: Option<usize>,
}

impl UriConfig for LogConfig {
    fn scheme() -> &'static str {
        "log"
    }

    fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = parse_uri(uri)?;
        Self::from_components(parts)
    }

    fn from_components(parts: camel_component_api::UriComponents) -> Result<Self, CamelError> {
        if parts.scheme != Self::scheme() {
            return Err(CamelError::InvalidUri(format!(
                "expected scheme '{}' but got '{}'",
                Self::scheme(),
                parts.scheme
            )));
        }

        let level = match parts.params.get("level") {
            Some(raw) => parse_log_level(raw)?,
            None => LogLevel::Info,
        };

        let show_headers = match parts.params.get("showHeaders") {
            Some(raw) => raw.parse::<bool>().map_err(|_| {
                CamelError::InvalidUri(format!("invalid boolean value for showHeaders: {raw}"))
            })?,
            None => false,
        };

        let show_body = match parts.params.get("showBody") {
            Some(raw) => raw.parse::<bool>().map_err(|_| {
                CamelError::InvalidUri(format!("invalid boolean value for showBody: {raw}"))
            })?,
            None => true,
        };

        let max_chars = match parts.params.get("maxChars") {
            Some(raw) => Some(raw.parse::<usize>().map_err(|_| {
                CamelError::InvalidUri(format!("invalid integer value for maxChars: {raw}"))
            })?),
            None => None,
        };

        let log_mask = match parts.params.get("logMask") {
            Some(raw) => raw.parse::<bool>().map_err(|_| {
                CamelError::InvalidUri(format!("invalid boolean value for logMask: {raw}"))
            })?,
            None => false,
        };

        let show_stream_info = match parts.params.get("showStreamInfo") {
            Some(raw) => raw.parse::<bool>().map_err(|_| {
                CamelError::InvalidUri(format!("invalid boolean value for showStreamInfo: {raw}"))
            })?,
            None => false,
        };

        let group_size = match parts.params.get("groupSize") {
            Some(raw) => Some(raw.parse::<usize>().map_err(|_| {
                CamelError::InvalidUri(format!("invalid integer value for groupSize: {raw}"))
            })?),
            None => None,
        };

        Ok(Self {
            category: parts.path,
            level,
            show_headers,
            show_body,
            max_chars,
            log_mask,
            show_stream_info,
            group_size,
        })
    }
}

// ---------------------------------------------------------------------------
// LogComponent
// ---------------------------------------------------------------------------

/// The Log component logs exchange information using `tracing`.
pub struct LogComponent;

impl LogComponent {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LogComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for LogComponent {
    fn scheme(&self) -> &str {
        "log"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let config = LogConfig::from_uri(uri)?;
        Ok(Box::new(LogEndpoint {
            uri: uri.to_string(),
            config,
        }))
    }
}

// ---------------------------------------------------------------------------
// LogEndpoint
// ---------------------------------------------------------------------------

struct LogEndpoint {
    uri: String,
    config: LogConfig,
}

impl Endpoint for LogEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Err(CamelError::EndpointCreationFailed(
            "log endpoint does not support consumers".to_string(),
        ))
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(LogProducer::new(self.config.clone())))
    }
}

// ---------------------------------------------------------------------------
// LogProducer
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct LogProducer {
    config: LogConfig,
    exchange_count: Arc<AtomicUsize>,
}

impl LogProducer {
    fn new(config: LogConfig) -> Self {
        Self {
            config,
            exchange_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Returns true if the header key matches sensitive patterns.
    fn is_sensitive_header(key: &str) -> bool {
        let lower = key.to_lowercase();
        let sensitive_keywords = [
            "password",
            "passwd",
            "secret",
            "token",
            "apikey",
            "api-key",
            "api_key",
            "authorization",
            "auth",
            "credential",
            "private",
            "signature",
        ];
        sensitive_keywords.iter().any(|kw| {
            lower == *kw
                || lower.ends_with(&format!("-{kw}"))
                || lower.ends_with(&format!("_{kw}"))
                || lower.starts_with(&format!("{kw}-"))
                || lower.starts_with(&format!("{kw}_"))
        })
    }

    fn format_exchange(&self, exchange: &Exchange, count: usize) -> String {
        let mut parts = Vec::new();

        if self.config.show_body {
            let body_str = if self.config.log_mask {
                "[Body redacted by logMask]".to_string()
            } else {
                match &exchange.input.body {
                    camel_component_api::Body::Empty => "[empty]".to_string(),
                    camel_component_api::Body::Text(s) => s.clone(),
                    camel_component_api::Body::Json(v) => v.to_string(),
                    camel_component_api::Body::Xml(s) => s.clone(),
                    camel_component_api::Body::Bytes(b) => format!("[{} bytes]", b.len()),
                    camel_component_api::Body::Stream(s) => {
                        if self.config.show_stream_info {
                            format!("[Stream: origin={:?}]", s.metadata.origin)
                        } else {
                            "[Stream]".to_string()
                        }
                    }
                }
            };

            let mut body_str = body_str;
            if let Some(limit) = self.config.max_chars
                && body_str.len() > limit
            {
                body_str.truncate(limit);
            }

            parts.push(format!("Body: {body_str}"));
        }

        if self.config.show_headers && !exchange.input.headers.is_empty() {
            let headers: Vec<String> = exchange
                .input
                .headers
                .iter()
                .map(|(k, v)| {
                    if self.config.log_mask && Self::is_sensitive_header(k) {
                        format!("{k}=[REDACTED]")
                    } else {
                        format!("{k}={v}")
                    }
                })
                .collect();
            parts.push(format!("Headers: {{{}}}", headers.join(", ")));
        }

        if parts.is_empty() {
            format!("[{}] Exchange received", self.config.category)
        } else if self.config.group_size.is_some() {
            format!(
                "[{}] Group of {count}: {}",
                self.config.category,
                parts.join(" | ")
            )
        } else {
            format!("[{}] {}", self.config.category, parts.join(" | "))
        }
    }
}

impl Service<Exchange> for LogProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let count = self.exchange_count.fetch_add(1, Ordering::Relaxed) + 1;

        // Group logging: only emit every group_size exchanges
        if let Some(group_size) = self.config.group_size
            && !count.is_multiple_of(group_size)
        {
            return Box::pin(async move { Ok(exchange) });
        }

        let msg = self.format_exchange(&exchange, count);
        let level = self.config.level;

        Box::pin(async move {
            match level {
                LogLevel::Trace => trace!("{msg}"),
                LogLevel::Debug => debug!("{msg}"),
                LogLevel::Info => info!("{msg}"),
                LogLevel::Warn => warn!("{msg}"),
                LogLevel::Error => error!("{msg}"),
            }

            Ok(exchange)
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::Body;
    use camel_component_api::Message;
    use camel_component_api::NoOpComponentContext;
    use tower::ServiceExt;

    fn test_producer_ctx() -> ProducerContext {
        ProducerContext::new()
    }

    #[test]
    fn test_log_config_defaults() {
        let config = LogConfig::from_uri("log:myCategory").unwrap();
        assert_eq!(config.category, "myCategory");
        assert_eq!(config.level, LogLevel::Info);
        assert!(!config.show_headers);
        assert!(config.show_body);
    }

    #[test]
    fn test_log_config_with_params() {
        let config =
            LogConfig::from_uri("log:app?level=debug&showHeaders=true&showBody=false").unwrap();
        assert_eq!(config.category, "app");
        assert_eq!(config.level, LogLevel::Debug);
        assert!(config.show_headers);
        assert!(!config.show_body);
    }

    #[test]
    fn test_log_config_wrong_scheme() {
        let result = LogConfig::from_uri("timer:tick");
        assert!(result.is_err());
    }

    #[test]
    fn test_log_component_scheme() {
        let component = LogComponent::new();
        assert_eq!(component.scheme(), "log");
    }

    #[test]
    fn test_log_component_default() {
        let component = LogComponent;
        assert_eq!(component.scheme(), "log");
    }

    #[test]
    fn test_log_level_from_str_variants() {
        assert_eq!("trace".parse::<LogLevel>().unwrap(), LogLevel::Trace);
        assert_eq!("DEBUG".parse::<LogLevel>().unwrap(), LogLevel::Debug);
        assert_eq!("Info".parse::<LogLevel>().unwrap(), LogLevel::Info);
        assert_eq!("warning".parse::<LogLevel>().unwrap(), LogLevel::Warn);
        assert_eq!("error".parse::<LogLevel>().unwrap(), LogLevel::Error);
    }

    #[test]
    fn test_log_level_from_str_invalid() {
        let err = "nope".parse::<LogLevel>().unwrap_err();
        assert_eq!(
            err,
            "Configuration error: unknown log level: 'nope'. Valid: TRACE, DEBUG, INFO, WARN, ERROR"
        );
    }

    #[test]
    fn test_log_config_invalid_level_rejected() {
        let err = LogConfig::from_uri("log:test?level=invalid").unwrap_err();
        assert!(
            err.to_string()
                .contains("unknown log level: 'invalid'. Valid: TRACE, DEBUG, INFO, WARN, ERROR")
        );
    }

    #[test]
    fn test_valid_log_levels_accepted() {
        assert!(parse_log_level("DEBUG").is_ok());
        assert!(parse_log_level("info").is_ok());
        assert!(parse_log_level("WARN").is_ok());
        assert!(parse_log_level("WARNING").is_ok());
    }

    #[test]
    fn test_invalid_log_level_rejected() {
        assert!(parse_log_level("VERBOSE").is_err());
        assert!(parse_log_level("").is_err());
        assert!(parse_log_level("log").is_err());
    }

    #[test]
    fn test_log_endpoint_uri() {
        let component = LogComponent::new();
        let endpoint = component
            .create_endpoint("log:uri-check", &NoOpComponentContext)
            .unwrap();
        assert_eq!(endpoint.uri(), "log:uri-check");
    }

    #[test]
    fn test_log_endpoint_no_consumer() {
        let component = LogComponent::new();
        let endpoint = component
            .create_endpoint("log:info", &NoOpComponentContext)
            .unwrap();
        assert!(endpoint.create_consumer().is_err());
    }

    #[test]
    fn test_log_endpoint_creates_producer() {
        let ctx = test_producer_ctx();
        let component = LogComponent::new();
        let endpoint = component
            .create_endpoint("log:info", &NoOpComponentContext)
            .unwrap();
        assert!(endpoint.create_producer(&ctx).is_ok());
    }

    #[tokio::test]
    async fn test_log_producer_processes_exchange() {
        let ctx = test_producer_ctx();
        let component = LogComponent::new();
        let endpoint = component
            .create_endpoint("log:test?showHeaders=true", &NoOpComponentContext)
            .unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

        let mut exchange = Exchange::new(Message::new("hello world"));
        exchange
            .input
            .set_header("source", serde_json::Value::String("test".into()));

        let result = producer.oneshot(exchange).await.unwrap();
        // Log producer passes exchange through unchanged
        assert_eq!(result.input.body.as_text(), Some("hello world"));
    }

    #[test]
    fn test_format_exchange_without_body_or_headers() {
        let producer = LogProducer::new(LogConfig {
            category: "cat".to_string(),
            level: LogLevel::Info,
            show_headers: false,
            show_body: false,
            max_chars: None,
            log_mask: false,
            show_stream_info: false,
            group_size: None,
        });
        let exchange = Exchange::new(Message::new("ignored"));
        let formatted = producer.format_exchange(&exchange, 1);
        assert_eq!(formatted, "[cat] Exchange received");
    }

    #[test]
    fn test_format_exchange_body_variants() {
        let base = LogProducer::new(LogConfig {
            category: "cat".to_string(),
            level: LogLevel::Info,
            show_headers: false,
            show_body: true,
            max_chars: None,
            log_mask: false,
            show_stream_info: true,
            group_size: None,
        });

        let empty = Exchange::new(Message::default());
        assert!(base.format_exchange(&empty, 1).contains("Body: [empty]"));

        let mut json_msg = Message::new("");
        json_msg.body = Body::Json(serde_json::json!({"k":"v"}));
        let json_ex = Exchange::new(json_msg);
        assert!(
            base.format_exchange(&json_ex, 2)
                .contains("Body: {\"k\":\"v\"}")
        );

        let mut xml_msg = Message::new("");
        xml_msg.body = Body::Xml("<a/>".to_string());
        let xml_ex = Exchange::new(xml_msg);
        assert!(base.format_exchange(&xml_ex, 3).contains("Body: <a/>"));

        let mut bytes_msg = Message::new("");
        bytes_msg.body = Body::Bytes(b"abc".to_vec().into());
        let bytes_ex = Exchange::new(bytes_msg);
        assert!(
            base.format_exchange(&bytes_ex, 4)
                .contains("Body: [3 bytes]")
        );
    }

    #[test]
    fn test_log_truncates_large_body() {
        let producer = LogProducer::new(LogConfig {
            category: "trunc".to_string(),
            level: LogLevel::Info,
            show_headers: false,
            show_body: true,
            max_chars: Some(10),
            log_mask: false,
            show_stream_info: false,
            group_size: None,
        });

        let long_body = "a".repeat(100);
        let exchange = Exchange::new(Message::new(long_body));
        let formatted = producer.format_exchange(&exchange, 1);

        // Extract the body part from "[trunc] Body: ..."
        let body_part = formatted.split_once("Body: ").unwrap().1;
        assert!(
            body_part.len() <= 10,
            "expected body <= 10 chars, got {} chars: {body_part:?}",
            body_part.len()
        );
    }

    #[test]
    fn test_log_no_truncation_when_max_chars_unset() {
        let producer = LogProducer::new(LogConfig {
            category: "notrunc".to_string(),
            level: LogLevel::Info,
            show_headers: false,
            show_body: true,
            max_chars: None,
            log_mask: false,
            show_stream_info: false,
            group_size: None,
        });

        let long_body = "b".repeat(200);
        let exchange = Exchange::new(Message::new(long_body));
        let formatted = producer.format_exchange(&exchange, 1);

        let body_part = formatted.split_once("Body: ").unwrap().1;
        assert_eq!(body_part.len(), 200);
    }

    #[test]
    fn test_log_config_max_chars_param() {
        let config = LogConfig::from_uri("log:test?maxChars=50").unwrap();
        assert_eq!(config.max_chars, Some(50));
    }

    #[test]
    fn test_log_config_max_chars_default_unset() {
        let config = LogConfig::from_uri("log:test").unwrap();
        assert_eq!(config.max_chars, None);
    }

    // --- LOG-007: logMask tests ---

    #[test]
    fn test_log_config_log_mask_param() {
        let config = LogConfig::from_uri("log:test?logMask=true").unwrap();
        assert!(config.log_mask);
    }

    #[test]
    fn test_log_config_log_mask_default_false() {
        let config = LogConfig::from_uri("log:test").unwrap();
        assert!(!config.log_mask);
    }

    #[test]
    fn test_log_mask_redacts_sensitive_headers() {
        let producer = LogProducer::new(LogConfig {
            category: "cat".to_string(),
            level: LogLevel::Info,
            show_headers: true,
            show_body: false,
            max_chars: None,
            log_mask: true,
            show_stream_info: false,
            group_size: None,
        });

        let mut exchange = Exchange::new(Message::new("body"));
        exchange.input.set_header(
            "X-Auth-Token",
            serde_json::Value::String("secret123".into()),
        );
        exchange
            .input
            .set_header("password", serde_json::Value::String("hunter2".into()));
        exchange
            .input
            .set_header("ApiKey", serde_json::Value::String("abc".into()));
        exchange
            .input
            .set_header("normal-header", serde_json::Value::String("visible".into()));
        exchange.input.set_header(
            "user-credential",
            serde_json::Value::String("sensitive".into()),
        );
        exchange
            .input
            .set_header("secret-value", serde_json::Value::String("hidden".into()));

        let formatted = producer.format_exchange(&exchange, 1);
        assert!(
            formatted.contains("X-Auth-Token=[REDACTED]"),
            "auth header must be redacted: {formatted}"
        );
        assert!(
            formatted.contains("password=[REDACTED]"),
            "password header must be redacted: {formatted}"
        );
        assert!(
            formatted.contains("ApiKey=[REDACTED]"),
            "key header must be redacted: {formatted}"
        );
        assert!(
            formatted.contains("normal-header=\"visible\""),
            "normal header must be visible: {formatted}"
        );
        assert!(
            formatted.contains("user-credential=[REDACTED]"),
            "credential header must be redacted: {formatted}"
        );
        assert!(
            formatted.contains("secret-value=[REDACTED]"),
            "secret header must be redacted: {formatted}"
        );
    }

    #[test]
    fn test_log_mask_redacts_body() {
        let producer = LogProducer::new(LogConfig {
            category: "cat".to_string(),
            level: LogLevel::Info,
            show_headers: false,
            show_body: true,
            max_chars: None,
            log_mask: true,
            show_stream_info: false,
            group_size: None,
        });

        let exchange = Exchange::new(Message::new("sensitive body content"));
        let formatted = producer.format_exchange(&exchange, 1);
        assert!(
            formatted.contains("[Body redacted by logMask]"),
            "body must be redacted: {formatted}"
        );
        assert!(
            !formatted.contains("sensitive body content"),
            "body content must not appear: {formatted}"
        );
    }

    #[test]
    fn test_log_mask_off_shows_data() {
        let producer = LogProducer::new(LogConfig {
            category: "cat".to_string(),
            level: LogLevel::Info,
            show_headers: true,
            show_body: true,
            max_chars: None,
            log_mask: false,
            show_stream_info: false,
            group_size: None,
        });

        let mut exchange = Exchange::new(Message::new("visible body"));
        exchange
            .input
            .set_header("password", serde_json::Value::String("hunter2".into()));

        let formatted = producer.format_exchange(&exchange, 1);
        assert!(
            formatted.contains("visible body"),
            "body must be visible when mask off: {formatted}"
        );
        assert!(
            formatted.contains("hunter2"),
            "header value must be visible when mask off: {formatted}"
        );
    }

    // --- LOG-002: showStreamInfo tests ---

    #[test]
    fn test_log_stream_show_info() {
        // show_stream_info = false → just [Stream]
        let producer_no_info = LogProducer::new(LogConfig {
            category: "cat".to_string(),
            level: LogLevel::Info,
            show_headers: false,
            show_body: true,
            max_chars: None,
            log_mask: false,
            show_stream_info: false,
            group_size: None,
        });

        let mut msg = Message::new("");
        msg.body = Body::Stream(camel_component_api::StreamBody {
            stream: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            metadata: camel_component_api::StreamMetadata {
                origin: Some("file:///data/test.txt".to_string()),
                ..Default::default()
            },
        });
        let exchange = Exchange::new(msg);
        let formatted = producer_no_info.format_exchange(&exchange, 1);
        assert!(
            formatted.contains("Body: [Stream]"),
            "must show [Stream] when show_stream_info=false: {formatted}"
        );

        // show_stream_info = true → [Stream: origin=...]
        let producer_with_info = LogProducer::new(LogConfig {
            category: "cat".to_string(),
            level: LogLevel::Info,
            show_headers: false,
            show_body: true,
            max_chars: None,
            log_mask: false,
            show_stream_info: true,
            group_size: None,
        });

        let formatted = producer_with_info.format_exchange(&exchange, 1);
        assert!(
            formatted.contains("Body: [Stream: origin=Some(\"file:///data/test.txt\")]"),
            "must show origin when show_stream_info=true: {formatted}"
        );
    }

    // --- LOG-008: groupSize tests ---

    #[test]
    fn test_log_group_size() {
        let producer = LogProducer::new(LogConfig {
            category: "cat".to_string(),
            level: LogLevel::Info,
            show_headers: false,
            show_body: true,
            max_chars: None,
            log_mask: false,
            show_stream_info: false,
            group_size: Some(3),
        });

        // Count 1 and 2 should not trigger logging, count 3 should
        let ex1 = Exchange::new(Message::new("first"));
        let formatted1 = producer.format_exchange(&ex1, 3);
        assert!(
            formatted1.contains("Group of 3:"),
            "group_size=3 must include count: {formatted1}"
        );
        assert!(
            formatted1.contains("Body: first"),
            "group log must include body: {formatted1}"
        );
    }

    #[test]
    fn test_log_config_group_size_param() {
        let config = LogConfig::from_uri("log:test?groupSize=10").unwrap();
        assert_eq!(config.group_size, Some(10));
    }

    #[test]
    fn test_log_config_group_size_default_unset() {
        let config = LogConfig::from_uri("log:test").unwrap();
        assert_eq!(config.group_size, None);
    }

    #[test]
    fn test_log_config_show_stream_info_param() {
        let config = LogConfig::from_uri("log:test?showStreamInfo=true").unwrap();
        assert!(config.show_stream_info);
    }

    #[test]
    fn test_log_config_show_stream_info_default_false() {
        let config = LogConfig::from_uri("log:test").unwrap();
        assert!(!config.show_stream_info);
    }
}
