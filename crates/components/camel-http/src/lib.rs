use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use tower::Service;
use tracing::debug;

use camel_api::{BoxProcessor, CamelError, Exchange, Message, body::Body};
use camel_component::{Component, Consumer, Endpoint};
use camel_endpoint::parse_uri;

const CAMEL_OPTIONS: &[&str] = &[
    "httpMethod",
    "throwExceptionOnFailure",
    "okStatusCodeRange",
    "followRedirects",
    "connectTimeout",
    "responseTimeout",
];

// ---------------------------------------------------------------------------
// HttpConfig
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct HttpConfig {
    pub base_url: String,
    pub http_method: Option<String>,
    pub throw_exception_on_failure: bool,
    pub ok_status_code_range: (u16, u16),
    pub follow_redirects: bool,
    pub connect_timeout: Duration,
    pub response_timeout: Option<Duration>,
}

impl HttpConfig {
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = parse_uri(uri)?;
        if parts.scheme != "http" && parts.scheme != "https" {
            return Err(CamelError::InvalidUri(format!(
                "expected scheme 'http' or 'https', got '{}'",
                parts.scheme
            )));
        }

        let base_url = format!("{}:{}", parts.scheme, parts.path);

        let http_method = parts.params.get("httpMethod").cloned();

        let throw_exception_on_failure = parts
            .params
            .get("throwExceptionOnFailure")
            .map(|v| v != "false")
            .unwrap_or(true);

        let ok_status_code_range = parts
            .params
            .get("okStatusCodeRange")
            .and_then(|v| {
                let (start, end) = v.split_once('-')?;
                Some((start.parse::<u16>().ok()?, end.parse::<u16>().ok()?))
            })
            .unwrap_or((200, 299));

        let follow_redirects = parts
            .params
            .get("followRedirects")
            .map(|v| v == "true")
            .unwrap_or(false);

        let connect_timeout = parts
            .params
            .get("connectTimeout")
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_millis(30000));

        let response_timeout = parts
            .params
            .get("responseTimeout")
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_millis);

        Ok(Self {
            base_url,
            http_method,
            throw_exception_on_failure,
            ok_status_code_range,
            follow_redirects,
            connect_timeout,
            response_timeout,
        })
    }
}

// ---------------------------------------------------------------------------
// HttpComponent / HttpsComponent
// ---------------------------------------------------------------------------

pub struct HttpComponent {
    client: reqwest::Client,
}

impl HttpComponent {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

impl Default for HttpComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for HttpComponent {
    fn scheme(&self) -> &str {
        "http"
    }

    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
        let config = HttpConfig::from_uri(uri)?;
        Ok(Box::new(HttpEndpoint {
            uri: uri.to_string(),
            config,
            client: self.client.clone(),
        }))
    }
}

pub struct HttpsComponent {
    client: reqwest::Client,
}

impl HttpsComponent {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

impl Default for HttpsComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for HttpsComponent {
    fn scheme(&self) -> &str {
        "https"
    }

    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
        let config = HttpConfig::from_uri(uri)?;
        Ok(Box::new(HttpEndpoint {
            uri: uri.to_string(),
            config,
            client: self.client.clone(),
        }))
    }
}

// ---------------------------------------------------------------------------
// HttpEndpoint
// ---------------------------------------------------------------------------

struct HttpEndpoint {
    uri: String,
    config: HttpConfig,
    client: reqwest::Client,
}

impl Endpoint for HttpEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Err(CamelError::EndpointCreationFailed(
            "HTTP endpoint does not support consumers (producer-only)".to_string(),
        ))
    }

    fn create_producer(&self) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(HttpProducer {
            config: self.config.clone(),
            client: self.client.clone(),
        }))
    }
}

// ---------------------------------------------------------------------------
// HttpProducer (stub)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct HttpProducer {
    config: HttpConfig,
    client: reqwest::Client,
}

impl Service<Exchange> for HttpProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _exchange: Exchange) -> Self::Future {
        Box::pin(async move {
            todo!("HTTP producer not yet implemented")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_http_config_defaults() {
        let config = HttpConfig::from_uri("http://localhost:8080/api").unwrap();
        assert_eq!(config.base_url, "http://localhost:8080/api");
        assert!(config.http_method.is_none());
        assert!(config.throw_exception_on_failure);
        assert_eq!(config.ok_status_code_range, (200, 299));
        assert!(!config.follow_redirects);
        assert_eq!(config.connect_timeout, Duration::from_millis(30000));
        assert!(config.response_timeout.is_none());
    }

    #[test]
    fn test_http_config_with_options() {
        let config = HttpConfig::from_uri(
            "https://api.example.com/v1?httpMethod=PUT&throwExceptionOnFailure=false&followRedirects=true&connectTimeout=5000&responseTimeout=10000"
        ).unwrap();
        assert_eq!(config.base_url, "https://api.example.com/v1");
        assert_eq!(config.http_method, Some("PUT".to_string()));
        assert!(!config.throw_exception_on_failure);
        assert!(config.follow_redirects);
        assert_eq!(config.connect_timeout, Duration::from_millis(5000));
        assert_eq!(config.response_timeout, Some(Duration::from_millis(10000)));
    }

    #[test]
    fn test_http_config_ok_status_range() {
        let config = HttpConfig::from_uri(
            "http://localhost/api?okStatusCodeRange=200-204"
        ).unwrap();
        assert_eq!(config.ok_status_code_range, (200, 204));
    }

    #[test]
    fn test_http_config_wrong_scheme() {
        let result = HttpConfig::from_uri("file:/tmp");
        assert!(result.is_err());
    }

    #[test]
    fn test_http_component_scheme() {
        let component = HttpComponent::new();
        assert_eq!(component.scheme(), "http");
    }

    #[test]
    fn test_https_component_scheme() {
        let component = HttpsComponent::new();
        assert_eq!(component.scheme(), "https");
    }

    #[test]
    fn test_http_endpoint_no_consumer() {
        let component = HttpComponent::new();
        let endpoint = component.create_endpoint("http://localhost/api").unwrap();
        assert!(endpoint.create_consumer().is_err());
    }

    #[test]
    fn test_http_endpoint_creates_producer() {
        let component = HttpComponent::new();
        let endpoint = component.create_endpoint("http://localhost/api").unwrap();
        assert!(endpoint.create_producer().is_ok());
    }
}
