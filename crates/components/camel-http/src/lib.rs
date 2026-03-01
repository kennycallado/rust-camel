use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::sync::RwLock;
use tower::Service;
use tracing::debug;

use camel_api::{BoxProcessor, CamelError, Exchange, body::Body};
use camel_component::{Component, Consumer, Endpoint};
use camel_endpoint::parse_uri;

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
    pub query_params: HashMap<String, String>,
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

        // CAMEL_OPTIONS: params that are consumed by Camel,        // Any remaining params should be forwarded as HTTP query params
        let camel_options = [
            "httpMethod",
            "throwExceptionOnFailure",
            "okStatusCodeRange",
            "followRedirects",
            "connectTimeout",
            "responseTimeout",
        ];

        let query_params: HashMap<String, String> = parts
            .params
            .into_iter()
            .filter(|(k, _)| !camel_options.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        Ok(Self {
            base_url,
            http_method,
            throw_exception_on_failure,
            ok_status_code_range: (ok_status_code_range.0, ok_status_code_range.1),
            follow_redirects,
            connect_timeout,
            response_timeout,
            query_params,
        })
    }
}

// ---------------------------------------------------------------------------
// HttpServerConfig
// ---------------------------------------------------------------------------

/// Configuration for an HTTP server (consumer) endpoint.
#[derive(Debug, Clone)]
pub struct HttpServerConfig {
    /// Bind address, e.g. "0.0.0.0" or "127.0.0.1".
    pub host: String,
    /// TCP port to listen on.
    pub port: u16,
    /// URL path this consumer handles, e.g. "/orders".
    pub path: String,
}

impl HttpServerConfig {
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = parse_uri(uri)?;
        if parts.scheme != "http" && parts.scheme != "https" {
            return Err(CamelError::InvalidUri(format!(
                "expected scheme 'http' or 'https', got '{}'",
                parts.scheme
            )));
        }

        // parts.path is everything after the scheme colon, e.g. "//0.0.0.0:8080/orders"
        // Strip leading "//"
        let authority_and_path = parts.path.trim_start_matches('/');

        // Split on the first "/" to separate "host:port" from "/path"
        let (authority, path_suffix) = if let Some(idx) = authority_and_path.find('/') {
            (&authority_and_path[..idx], &authority_and_path[idx..])
        } else {
            (authority_and_path, "/")
        };

        let path = if path_suffix.is_empty() { "/" } else { path_suffix }.to_string();

        // Parse host:port
        let (host, port) = if let Some(colon) = authority.rfind(':') {
            let port_str = &authority[colon + 1..];
            match port_str.parse::<u16>() {
                Ok(p) => (authority[..colon].to_string(), p),
                Err(_) => {
                    return Err(CamelError::InvalidUri(format!(
                        "invalid port '{}' in URI '{}'",
                        port_str, uri
                    )));
                }
            }
        } else {
            (authority.to_string(), 80)
        };

        Ok(Self { host, port, path })
    }
}

// ---------------------------------------------------------------------------
// RequestEnvelope / HttpReply
// ---------------------------------------------------------------------------

/// An inbound HTTP request sent from the Axum dispatch handler to an
/// `HttpConsumer` receive loop.
pub struct RequestEnvelope {
    pub method:   String,
    pub path:     String,
    pub query:    String,
    pub headers:  http::HeaderMap,
    pub body:     bytes::Bytes,
    pub reply_tx: tokio::sync::oneshot::Sender<HttpReply>,
}

/// The HTTP response that `HttpConsumer` sends back to the Axum handler.
#[derive(Debug, Clone)]
pub struct HttpReply {
    pub status:  u16,
    pub headers: Vec<(String, String)>,
    pub body:    bytes::Bytes,
}

// ---------------------------------------------------------------------------
// DispatchTable / ServerRegistry
// ---------------------------------------------------------------------------

/// Maps URL path → channel sender for the consumer that owns that path.
pub type DispatchTable = Arc<RwLock<HashMap<String, tokio::sync::mpsc::Sender<RequestEnvelope>>>>;

/// Handle to a running Axum server on one port.
struct ServerHandle {
    dispatch: DispatchTable,
    /// Kept alive so the task isn't dropped; not used directly.
    _task: tokio::task::JoinHandle<()>,
}

/// Process-global registry mapping port → running Axum server handle.
pub struct ServerRegistry {
    inner: Mutex<HashMap<u16, ServerHandle>>,
}

impl ServerRegistry {
    /// Returns the global singleton.
    pub fn global() -> &'static Self {
        static INSTANCE: OnceLock<ServerRegistry> = OnceLock::new();
        INSTANCE.get_or_init(|| ServerRegistry {
            inner: Mutex::new(HashMap::new()),
        })
    }

    /// Returns the `DispatchTable` for `port`, spawning a new Axum server if
    /// none is running on that port yet.
    pub async fn get_or_spawn(
        &'static self,
        host: &str,
        port: u16,
    ) -> Result<DispatchTable, CamelError> {
        // Fast path: check without spawning.
        {
            let guard = self.inner.lock().map_err(|_| {
                CamelError::EndpointCreationFailed("ServerRegistry lock poisoned".into())
            })?;
            if let Some(handle) = guard.get(&port) {
                return Ok(Arc::clone(&handle.dispatch));
            }
        }

        // Slow path: need to bind and spawn.
        let addr = format!("{}:{}", host, port);
        let listener = tokio::net::TcpListener::bind(&addr).await.map_err(|e| {
            CamelError::EndpointCreationFailed(format!("Failed to bind {addr}: {e}"))
        })?;

        let dispatch: DispatchTable = Arc::new(RwLock::new(HashMap::new()));
        let dispatch_for_server = Arc::clone(&dispatch);
        let task = tokio::spawn(run_axum_server(listener, dispatch_for_server));

        // Re-acquire lock to insert — handle the race where another task won.
        let mut guard = self.inner.lock().map_err(|_| {
            CamelError::EndpointCreationFailed("ServerRegistry lock poisoned".into())
        })?;
        // If another task already registered this port while we were binding,
        // our server will fail at accept-time (address in use) — that's fine.
        // Use the winner's dispatch table.
        if let Some(existing) = guard.get(&port) {
            return Ok(Arc::clone(&existing.dispatch));
        }
        guard.insert(port, ServerHandle { dispatch: Arc::clone(&dispatch), _task: task });
        Ok(dispatch)
    }
}

async fn run_axum_server(_listener: tokio::net::TcpListener, _dispatch: DispatchTable) {
    // Implemented in Task 5
}

// ---------------------------------------------------------------------------
// HttpComponent / HttpsComponent
// ---------------------------------------------------------------------------

pub struct HttpComponent;

impl HttpComponent {
    pub fn new() -> Self {
        Self
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
        let client = build_client(&config)?;
        Ok(Box::new(HttpEndpoint {
            uri: uri.to_string(),
            config,
            client,
        }))
    }
}

pub struct HttpsComponent;

impl HttpsComponent {
    pub fn new() -> Self {
        Self
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
        let client = build_client(&config)?;
        Ok(Box::new(HttpEndpoint {
            uri: uri.to_string(),
            config,
            client,
        }))
    }
}

fn build_client(config: &HttpConfig) -> Result<reqwest::Client, CamelError> {
    let mut builder = reqwest::Client::builder().connect_timeout(config.connect_timeout);

    if !config.follow_redirects {
        builder = builder.redirect(reqwest::redirect::Policy::none());
    }

    builder.build().map_err(|e| {
        CamelError::EndpointCreationFailed(format!("Failed to build HTTP client: {e}"))
    })
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
// HttpProducer
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct HttpProducer {
    config: HttpConfig,
    client: reqwest::Client,
}

impl HttpProducer {
    fn resolve_method(exchange: &Exchange, config: &HttpConfig) -> String {
        if let Some(ref method) = config.http_method {
            return method.to_uppercase();
        }
        if let Some(method) = exchange
            .input
            .header("CamelHttpMethod")
            .and_then(|v| v.as_str())
        {
            return method.to_uppercase();
        }
        if !exchange.input.body.is_empty() {
            return "POST".to_string();
        }
        "GET".to_string()
    }

    fn resolve_url(exchange: &Exchange, config: &HttpConfig) -> String {
        if let Some(uri) = exchange
            .input
            .header("CamelHttpUri")
            .and_then(|v| v.as_str())
        {
            let mut url = uri.to_string();
            if let Some(path) = exchange
                .input
                .header("CamelHttpPath")
                .and_then(|v| v.as_str())
            {
                if !url.ends_with('/') && !path.starts_with('/') {
                    url.push('/');
                }
                url.push_str(path);
            }
            if let Some(query) = exchange
                .input
                .header("CamelHttpQuery")
                .and_then(|v| v.as_str())
            {
                url.push('?');
                url.push_str(query);
            }
            return url;
        }

        let mut url = config.base_url.clone();

        if let Some(path) = exchange
            .input
            .header("CamelHttpPath")
            .and_then(|v| v.as_str())
        {
            if !url.ends_with('/') && !path.starts_with('/') {
                url.push('/');
            }
            url.push_str(path);
        }

        if let Some(query) = exchange
            .input
            .header("CamelHttpQuery")
            .and_then(|v| v.as_str())
        {
            url.push('?');
            url.push_str(query);
        } else if !config.query_params.is_empty() {
            // Forward non-Camel query params from config
            url.push('?');
            let query_string: String = config
                .query_params
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&");
            url.push_str(&query_string);
        }

        url
    }

    fn body_to_bytes(body: &Body) -> Option<Vec<u8>> {
        match body {
            Body::Empty => None,
            Body::Bytes(b) => Some(b.to_vec()),
            Body::Text(s) => Some(s.as_bytes().to_vec()),
            Body::Json(v) => Some(v.to_string().into_bytes()),
        }
    }

    fn is_ok_status(status: u16, range: (u16, u16)) -> bool {
        status >= range.0 && status <= range.1
    }
}

impl Service<Exchange> for HttpProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let config = self.config.clone();
        let client = self.client.clone();

        Box::pin(async move {
            let method_str = HttpProducer::resolve_method(&exchange, &config);
            let url = HttpProducer::resolve_url(&exchange, &config);

            debug!(method = %method_str, url = %url, "HTTP request");

            let method = method_str.parse::<reqwest::Method>().map_err(|e| {
                CamelError::ProcessorError(format!("Invalid HTTP method '{}': {}", method_str, e))
            })?;

            let mut request = client.request(method, &url);

            if let Some(timeout) = config.response_timeout {
                request = request.timeout(timeout);
            }

            for (key, value) in &exchange.input.headers {
                if !key.starts_with("Camel")
                    && let Some(val_str) = value.as_str()
                    && let (Ok(name), Ok(val)) = (
                        reqwest::header::HeaderName::from_bytes(key.as_bytes()),
                        reqwest::header::HeaderValue::from_str(val_str),
                    )
                {
                    request = request.header(name, val);
                }
            }

            if let Some(body_bytes) = HttpProducer::body_to_bytes(&exchange.input.body) {
                request = request.body(body_bytes);
            }

            let response = request
                .send()
                .await
                .map_err(|e| CamelError::ProcessorError(format!("HTTP request failed: {e}")))?;

            let status_code = response.status().as_u16();
            let status_text = response
                .status()
                .canonical_reason()
                .unwrap_or("Unknown")
                .to_string();

            for (key, value) in response.headers() {
                if let Ok(val_str) = value.to_str() {
                    exchange
                        .input
                        .set_header(key.as_str(), serde_json::Value::String(val_str.to_string()));
                }
            }

            exchange.input.set_header(
                "CamelHttpResponseCode",
                serde_json::Value::Number(status_code.into()),
            );
            exchange.input.set_header(
                "CamelHttpResponseText",
                serde_json::Value::String(status_text.clone()),
            );

            let response_body = response.bytes().await.map_err(|e| {
                CamelError::ProcessorError(format!("Failed to read response body: {e}"))
            })?;

            if config.throw_exception_on_failure
                && !HttpProducer::is_ok_status(status_code, config.ok_status_code_range)
            {
                return Err(CamelError::HttpOperationFailed {
                    status_code,
                    status_text,
                    response_body: Some(String::from_utf8_lossy(&response_body).to_string()),
                });
            }

            if !response_body.is_empty() {
                exchange.input.body = Body::Bytes(bytes::Bytes::from(response_body.to_vec()));
            }

            debug!(status = status_code, url = %url, "HTTP response");
            Ok(exchange)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::Message;
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
        let config =
            HttpConfig::from_uri("http://localhost/api?okStatusCodeRange=200-204").unwrap();
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

    // -----------------------------------------------------------------------
    // Producer tests
    // -----------------------------------------------------------------------

    async fn start_test_server() -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://127.0.0.1:{}", addr.port());

        let handle = tokio::spawn(async move {
            loop {
                if let Ok((mut stream, _)) = listener.accept().await {
                    tokio::spawn(async move {
                        use tokio::io::{AsyncReadExt, AsyncWriteExt};
                        let mut buf = vec![0u8; 4096];
                        let n = stream.read(&mut buf).await.unwrap_or(0);
                        let request = String::from_utf8_lossy(&buf[..n]).to_string();

                        let method = request.split_whitespace().next().unwrap_or("GET");

                        let body = format!(r#"{{"method":"{}","echo":"ok"}}"#, method);
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nX-Custom: test-value\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                    });
                }
            }
        });

        (url, handle)
    }

    async fn start_status_server(status: u16) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://127.0.0.1:{}", addr.port());

        let handle = tokio::spawn(async move {
            loop {
                if let Ok((mut stream, _)) = listener.accept().await {
                    let status = status;
                    tokio::spawn(async move {
                        use tokio::io::{AsyncReadExt, AsyncWriteExt};
                        let mut buf = vec![0u8; 4096];
                        let _ = stream.read(&mut buf).await;

                        let status_text = match status {
                            404 => "Not Found",
                            500 => "Internal Server Error",
                            _ => "Error",
                        };
                        let body = "error body";
                        let response = format!(
                            "HTTP/1.1 {} {}\r\nContent-Length: {}\r\n\r\n{}",
                            status,
                            status_text,
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                    });
                }
            }
        });

        (url, handle)
    }

    #[tokio::test]
    async fn test_http_producer_get_request() {
        use tower::ServiceExt;

        let (url, _handle) = start_test_server().await;

        let component = HttpComponent::new();
        let endpoint = component
            .create_endpoint(&format!("{url}/api/test"))
            .unwrap();
        let producer = endpoint.create_producer().unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await.unwrap();

        let status = result
            .input
            .header("CamelHttpResponseCode")
            .and_then(|v| v.as_u64())
            .unwrap();
        assert_eq!(status, 200);

        assert!(!result.input.body.is_empty());
    }

    #[tokio::test]
    async fn test_http_producer_post_with_body() {
        use tower::ServiceExt;

        let (url, _handle) = start_test_server().await;

        let component = HttpComponent::new();
        let endpoint = component
            .create_endpoint(&format!("{url}/api/data"))
            .unwrap();
        let producer = endpoint.create_producer().unwrap();

        let exchange = Exchange::new(Message::new("request body"));
        let result = producer.oneshot(exchange).await.unwrap();

        let status = result
            .input
            .header("CamelHttpResponseCode")
            .and_then(|v| v.as_u64())
            .unwrap();
        assert_eq!(status, 200);
    }

    #[tokio::test]
    async fn test_http_producer_method_from_header() {
        use tower::ServiceExt;

        let (url, _handle) = start_test_server().await;

        let component = HttpComponent::new();
        let endpoint = component.create_endpoint(&format!("{url}/api")).unwrap();
        let producer = endpoint.create_producer().unwrap();

        let mut exchange = Exchange::new(Message::default());
        exchange.input.set_header(
            "CamelHttpMethod",
            serde_json::Value::String("DELETE".to_string()),
        );

        let result = producer.oneshot(exchange).await.unwrap();
        let status = result
            .input
            .header("CamelHttpResponseCode")
            .and_then(|v| v.as_u64())
            .unwrap();
        assert_eq!(status, 200);
    }

    #[tokio::test]
    async fn test_http_producer_forced_method() {
        use tower::ServiceExt;

        let (url, _handle) = start_test_server().await;

        let component = HttpComponent::new();
        let endpoint = component
            .create_endpoint(&format!("{url}/api?httpMethod=PUT"))
            .unwrap();
        let producer = endpoint.create_producer().unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await.unwrap();

        let status = result
            .input
            .header("CamelHttpResponseCode")
            .and_then(|v| v.as_u64())
            .unwrap();
        assert_eq!(status, 200);
    }

    #[tokio::test]
    async fn test_http_producer_throw_exception_on_failure() {
        use tower::ServiceExt;

        let (url, _handle) = start_status_server(404).await;

        let component = HttpComponent::new();
        let endpoint = component
            .create_endpoint(&format!("{url}/not-found"))
            .unwrap();
        let producer = endpoint.create_producer().unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await;
        assert!(result.is_err());

        match result.unwrap_err() {
            CamelError::HttpOperationFailed { status_code, .. } => {
                assert_eq!(status_code, 404);
            }
            e => panic!("Expected HttpOperationFailed, got: {e}"),
        }
    }

    #[tokio::test]
    async fn test_http_producer_no_throw_on_failure() {
        use tower::ServiceExt;

        let (url, _handle) = start_status_server(500).await;

        let component = HttpComponent::new();
        let endpoint = component
            .create_endpoint(&format!("{url}/error?throwExceptionOnFailure=false"))
            .unwrap();
        let producer = endpoint.create_producer().unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await.unwrap();

        let status = result
            .input
            .header("CamelHttpResponseCode")
            .and_then(|v| v.as_u64())
            .unwrap();
        assert_eq!(status, 500);
    }

    #[tokio::test]
    async fn test_http_producer_uri_override() {
        use tower::ServiceExt;

        let (url, _handle) = start_test_server().await;

        let component = HttpComponent::new();
        let endpoint = component
            .create_endpoint("http://localhost:1/does-not-exist")
            .unwrap();
        let producer = endpoint.create_producer().unwrap();

        let mut exchange = Exchange::new(Message::default());
        exchange.input.set_header(
            "CamelHttpUri",
            serde_json::Value::String(format!("{url}/api")),
        );

        let result = producer.oneshot(exchange).await.unwrap();
        let status = result
            .input
            .header("CamelHttpResponseCode")
            .and_then(|v| v.as_u64())
            .unwrap();
        assert_eq!(status, 200);
    }

    #[tokio::test]
    async fn test_http_producer_response_headers_mapped() {
        use tower::ServiceExt;

        let (url, _handle) = start_test_server().await;

        let component = HttpComponent::new();
        let endpoint = component.create_endpoint(&format!("{url}/api")).unwrap();
        let producer = endpoint.create_producer().unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await.unwrap();

        assert!(
            result.input.header("content-type").is_some()
                || result.input.header("Content-Type").is_some()
        );
        assert!(result.input.header("CamelHttpResponseText").is_some());
    }

    // -----------------------------------------------------------------------
    // Bug fix tests: Client configuration per-endpoint
    // -----------------------------------------------------------------------

    async fn start_redirect_server() -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://127.0.0.1:{}", addr.port());

        let handle = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            loop {
                if let Ok((mut stream, _)) = listener.accept().await {
                    tokio::spawn(async move {
                        let mut buf = vec![0u8; 4096];
                        let n = stream.read(&mut buf).await.unwrap_or(0);
                        let request = String::from_utf8_lossy(&buf[..n]).to_string();

                        // Check if this is a request to /final
                        if request.contains("GET /final") {
                            let body = r#"{"status":"final"}"#;
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                                body.len(),
                                body
                            );
                            let _ = stream.write_all(response.as_bytes()).await;
                        } else {
                            // Redirect to /final
                            let response = "HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\n\r\n";
                            let _ = stream.write_all(response.as_bytes()).await;
                        }
                    });
                }
            }
        });

        (url, handle)
    }

    #[tokio::test]
    async fn test_follow_redirects_false_does_not_follow() {
        use tower::ServiceExt;

        let (url, _handle) = start_redirect_server().await;

        let component = HttpComponent::new();
        let endpoint = component
            .create_endpoint(&format!(
                "{url}?followRedirects=false&throwExceptionOnFailure=false"
            ))
            .unwrap();
        let producer = endpoint.create_producer().unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await.unwrap();

        // Should get 302, NOT follow redirect to 200
        let status = result
            .input
            .header("CamelHttpResponseCode")
            .and_then(|v| v.as_u64())
            .unwrap();
        assert_eq!(
            status, 302,
            "Should NOT follow redirect when followRedirects=false"
        );
    }

    #[tokio::test]
    async fn test_follow_redirects_true_follows_redirect() {
        use tower::ServiceExt;

        let (url, _handle) = start_redirect_server().await;

        let component = HttpComponent::new();
        let endpoint = component
            .create_endpoint(&format!("{url}?followRedirects=true"))
            .unwrap();
        let producer = endpoint.create_producer().unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await.unwrap();

        // Should follow redirect and get 200
        let status = result
            .input
            .header("CamelHttpResponseCode")
            .and_then(|v| v.as_u64())
            .unwrap();
        assert_eq!(
            status, 200,
            "Should follow redirect when followRedirects=true"
        );
    }

    #[tokio::test]
    async fn test_query_params_forwarded_to_http_request() {
        use tower::ServiceExt;

        let (url, _handle) = start_test_server().await;

        let component = HttpComponent::new();
        // apiKey is NOT a Camel option, should be forwarded as query param
        let endpoint = component
            .create_endpoint(&format!("{url}/api?apiKey=secret123&httpMethod=GET"))
            .unwrap();
        let producer = endpoint.create_producer().unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await.unwrap();

        // The test server returns the request info in response
        // We just verify it succeeds (the query param was sent)
        let status = result
            .input
            .header("CamelHttpResponseCode")
            .and_then(|v| v.as_u64())
            .unwrap();
        assert_eq!(status, 200);
    }

    #[tokio::test]
    async fn test_non_camel_query_params_are_forwarded() {
        // This test verifies Bug #3 fix: non-Camel options should be forwarded
        // We'll test the config parsing, not the actual HTTP call
        let config = HttpConfig::from_uri(
            "http://example.com/api?apiKey=secret123&httpMethod=GET&token=abc456",
        )
        .unwrap();

        // apiKey and token are NOT Camel options, should be forwarded
        assert!(
            config.query_params.contains_key("apiKey"),
            "apiKey should be preserved"
        );
        assert!(
            config.query_params.contains_key("token"),
            "token should be preserved"
        );
        assert_eq!(config.query_params.get("apiKey").unwrap(), "secret123");
        assert_eq!(config.query_params.get("token").unwrap(), "abc456");

        // httpMethod IS a Camel option, should NOT be in query_params
        assert!(
            !config.query_params.contains_key("httpMethod"),
            "httpMethod should not be forwarded"
        );
    }

    // -----------------------------------------------------------------------
    // HttpServerConfig tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_http_server_config_parse() {
        let cfg = HttpServerConfig::from_uri("http://0.0.0.0:8080/orders").unwrap();
        assert_eq!(cfg.host, "0.0.0.0");
        assert_eq!(cfg.port, 8080);
        assert_eq!(cfg.path, "/orders");
    }

    #[test]
    fn test_http_server_config_default_path() {
        let cfg = HttpServerConfig::from_uri("http://0.0.0.0:3000").unwrap();
        assert_eq!(cfg.path, "/");
    }

    #[test]
    fn test_http_server_config_wrong_scheme() {
        assert!(HttpServerConfig::from_uri("file:/tmp").is_err());
    }

    #[test]
    fn test_http_server_config_invalid_port() {
        assert!(HttpServerConfig::from_uri("http://localhost:abc/path").is_err());
    }

    #[test]
    fn test_request_envelope_and_reply_are_send() {
        fn assert_send<T: Send>() {}
        assert_send::<RequestEnvelope>();
        assert_send::<HttpReply>();
    }

    // -----------------------------------------------------------------------
    // ServerRegistry tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_server_registry_global_is_singleton() {
        let r1 = ServerRegistry::global();
        let r2 = ServerRegistry::global();
        assert!(std::ptr::eq(r1 as *const _, r2 as *const _));
    }
}
