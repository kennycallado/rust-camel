use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;
use tracing::{debug, error, info, trace, warn};

use camel_api::{BoxProcessor, CamelError, Exchange};
use camel_component::{Component, Consumer, Endpoint};
use camel_endpoint::parse_uri;

// ---------------------------------------------------------------------------
// LogLevel
// ---------------------------------------------------------------------------

/// Log level for the log component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "trace" => LogLevel::Trace,
            "debug" => LogLevel::Debug,
            "info" => LogLevel::Info,
            "warn" | "warning" => LogLevel::Warn,
            "error" => LogLevel::Error,
            _ => LogLevel::Info,
        }
    }
}

// ---------------------------------------------------------------------------
// LogConfig
// ---------------------------------------------------------------------------

/// Configuration parsed from a log URI.
///
/// Format: `log:category?level=info&showHeaders=true&showBody=true`
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
}

impl LogConfig {
    /// Parse a log URI into a config.
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = parse_uri(uri)?;
        if parts.scheme != "log" {
            return Err(CamelError::InvalidUri(format!(
                "expected scheme 'log', got '{}'",
                parts.scheme
            )));
        }

        let level = parts
            .params
            .get("level")
            .map(|v| LogLevel::from_str(v))
            .unwrap_or(LogLevel::Info);

        let show_headers = parts
            .params
            .get("showHeaders")
            .map(|v| v == "true")
            .unwrap_or(false);

        let show_body = parts
            .params
            .get("showBody")
            .map(|v| v == "true")
            .unwrap_or(true);

        Ok(Self {
            category: parts.path,
            level,
            show_headers,
            show_body,
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

    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
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

    fn create_producer(&self) -> Result<BoxProcessor, CamelError> {
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
                camel_api::Body::Empty => "[empty]".to_string(),
                camel_api::Body::Text(s) => s.clone(),
                camel_api::Body::Json(v) => v.to_string(),
                camel_api::Body::Bytes(b) => format!("[{} bytes]", b.len()),
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
    use camel_api::Message;
    use tower::ServiceExt;

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
    fn test_log_endpoint_no_consumer() {
        let component = LogComponent::new();
        let endpoint = component.create_endpoint("log:info").unwrap();
        assert!(endpoint.create_consumer().is_err());
    }

    #[test]
    fn test_log_endpoint_creates_producer() {
        let component = LogComponent::new();
        let endpoint = component.create_endpoint("log:info").unwrap();
        assert!(endpoint.create_producer().is_ok());
    }

    #[tokio::test]
    async fn test_log_producer_processes_exchange() {
        let component = LogComponent::new();
        let endpoint = component
            .create_endpoint("log:test?showHeaders=true")
            .unwrap();
        let producer = endpoint.create_producer().unwrap();

        let mut exchange = Exchange::new(Message::new("hello world"));
        exchange
            .input
            .set_header("source", serde_json::Value::String("test".into()));

        let result = producer.oneshot(exchange).await.unwrap();
        // Log producer passes exchange through unchanged
        assert_eq!(result.input.body.as_text(), Some("hello world"));
    }
}
