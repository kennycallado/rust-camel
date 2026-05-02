use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::task::{Context, Poll};

use tower::Service;
use tracing::{debug, error, info, trace, warn};

use camel_component_api::UriConfig;
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
        match s.to_lowercase().as_str() {
            "trace" => Ok(LogLevel::Trace),
            "debug" => Ok(LogLevel::Debug),
            "info" => Ok(LogLevel::Info),
            "warn" | "warning" => Ok(LogLevel::Warn),
            "error" => Ok(LogLevel::Error),
            _ => Err(format!("Invalid log level: {}", s)),
        }
    }
}

// ---------------------------------------------------------------------------
// LogConfig
// ---------------------------------------------------------------------------

/// Configuration parsed from a log URI.
///
/// Format: `log:category?level=info&showHeaders=true&showBody=true`
#[derive(Debug, Clone, UriConfig)]
#[uri_scheme = "log"]
#[uri_config(crate = "camel_component_api")]
pub struct LogConfig {
    /// Log category (the path portion of the URI).
    pub category: String,
    /// Log level. Default: Info.
    #[uri_param(default = "Info")]
    pub level: LogLevel,
    /// Whether to include headers in the log output.
    #[uri_param(name = "showHeaders", default = "false")]
    pub show_headers: bool,
    /// Whether to include the body in the log output.
    #[uri_param(name = "showBody", default = "true")]
    pub show_body: bool,
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
        Ok(BoxProcessor::new(LogProducer {
            config: self.config.clone(),
        }))
    }
}

// ---------------------------------------------------------------------------
// LogProducer
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct LogProducer {
    config: LogConfig,
}

impl LogProducer {
    fn format_exchange(&self, exchange: &Exchange) -> String {
        let mut parts = Vec::new();

        if self.config.show_body {
            let body_str = match &exchange.input.body {
                camel_component_api::Body::Empty => "[empty]".to_string(),
                camel_component_api::Body::Text(s) => s.clone(),
                camel_component_api::Body::Json(v) => v.to_string(),
                camel_component_api::Body::Xml(s) => s.clone(),
                camel_component_api::Body::Bytes(b) => format!("[{} bytes]", b.len()),
                camel_component_api::Body::Stream(s) => {
                    format!("[Stream: origin={:?}]", s.metadata.origin)
                }
            };
            parts.push(format!("Body: {body_str}"));
        }

        if self.config.show_headers && !exchange.input.headers.is_empty() {
            let headers: Vec<String> = exchange
                .input
                .headers
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            parts.push(format!("Headers: {{{}}}", headers.join(", ")));
        }

        if parts.is_empty() {
            format!("[{}] Exchange received", self.config.category)
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
        let msg = self.format_exchange(&exchange);
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
        let component = LogComponent::default();
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
        assert_eq!(err, "Invalid log level: nope");
    }

    #[test]
    fn test_log_config_invalid_level_falls_back_to_default() {
        let config = LogConfig::from_uri("log:test?level=invalid").unwrap();
        assert_eq!(config.level, LogLevel::Info);
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
        let producer = LogProducer {
            config: LogConfig {
                category: "cat".to_string(),
                level: LogLevel::Info,
                show_headers: false,
                show_body: false,
            },
        };
        let exchange = Exchange::new(Message::new("ignored"));
        let formatted = producer.format_exchange(&exchange);
        assert_eq!(formatted, "[cat] Exchange received");
    }

    #[test]
    fn test_format_exchange_body_variants() {
        let base = LogProducer {
            config: LogConfig {
                category: "cat".to_string(),
                level: LogLevel::Info,
                show_headers: false,
                show_body: true,
            },
        };

        let empty = Exchange::new(Message::default());
        assert!(base.format_exchange(&empty).contains("Body: [empty]"));

        let mut json_msg = Message::new("");
        json_msg.body = Body::Json(serde_json::json!({"k":"v"}));
        let json_ex = Exchange::new(json_msg);
        assert!(base.format_exchange(&json_ex).contains("Body: {\"k\":\"v\"}"));

        let mut xml_msg = Message::new("");
        xml_msg.body = Body::Xml("<a/>".to_string());
        let xml_ex = Exchange::new(xml_msg);
        assert!(base.format_exchange(&xml_ex).contains("Body: <a/>"));

        let mut bytes_msg = Message::new("");
        bytes_msg.body = Body::Bytes(b"abc".to_vec().into());
        let bytes_ex = Exchange::new(bytes_msg);
        assert!(base.format_exchange(&bytes_ex).contains("Body: [3 bytes]"));
    }
}
