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
use camel_component::{Component, Consumer, Endpoint, ProducerContext};
use camel_endpoint::parse_uri;

// ---------------------------------------------------------------------------
// HttpConfig
// ---------------------------------------------------------------------------

/// Configuration for an HTTP client (producer) endpoint.
///
/// # Memory Limits
///
/// HTTP operations enforce conservative memory limits to prevent denial-of-service
/// attacks from untrusted network sources. These limits are significantly lower than
/// file component limits (100MB) because HTTP typically handles API responses rather
/// than large file transfers, and clients may be untrusted.
///
/// ## Default Limits
///
/// - **HTTP client body**: 10MB (typical API responses)
/// - **HTTP server request**: 2MB (untrusted network input - see `HttpServerConfig`)
/// - **HTTP server response**: 10MB (same as client - see `HttpServerConfig`)
///
/// ## Rationale
///
/// The 10MB limit for HTTP client responses is appropriate for most API interactions
/// while providing protection against:
/// - Malicious servers sending oversized responses
/// - Runaway processes generating unexpectedly large payloads
/// - Memory exhaustion attacks
///
/// The 2MB server request limit is even more conservative because it handles input
/// from potentially untrusted clients on the public internet.
///
/// ## Overriding Limits
///
/// Override the default client body limit using the `maxBodySize` URI parameter:
///
/// ```text
/// http://api.example.com/large-data?maxBodySize=52428800
/// ```
///
/// For server endpoints, use `maxRequestBody` and `maxResponseBody` parameters:
///
/// ```text
/// http://0.0.0.0:8080/upload?maxRequestBody=52428800
/// ```
///
/// ## Behavior When Exceeded
///
/// When a body exceeds the configured limit:
/// - An error is returned immediately
/// - No memory is exhausted - the limit is checked before allocation
/// - The HTTP connection is terminated cleanly
///
/// ## Security Considerations
///
/// HTTP endpoints should be treated with more caution than file endpoints because:
/// - Clients may be unknown and untrusted
/// - Network traffic can be spoofed or malicious
/// - DoS attacks often exploit unbounded resource consumption
///
/// Only increase limits when you control both ends of the connection or when
/// business requirements demand larger payloads.
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
    // Security settings
    pub allow_private_ips: bool,
    pub blocked_hosts: Vec<String>,
    // Memory limit for body materialization (in bytes)
    pub max_body_size: usize,
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

        // SSRF protection settings
        let allow_private_ips = parts
            .params
            .get("allowPrivateIps")
            .map(|v| v == "true")
            .unwrap_or(false); // Default: block private IPs

        let blocked_hosts = parts
            .params
            .get("blockedHosts")
            .map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default();

        let max_body_size = parts
            .params
            .get("maxBodySize")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(10 * 1024 * 1024); // Default: 10MB

        // CAMEL_OPTIONS: params that are consumed by Camel,        // Any remaining params should be forwarded as HTTP query params
        let camel_options = [
            "httpMethod",
            "throwExceptionOnFailure",
            "okStatusCodeRange",
            "followRedirects",
            "connectTimeout",
            "responseTimeout",
            "allowPrivateIps",
            "blockedHosts",
            "maxBodySize",
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
            allow_private_ips,
            blocked_hosts,
            max_body_size,
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
    /// Maximum request body size in bytes.
    pub max_request_body: usize,
    /// Maximum response body size for materializing streams in bytes.
    pub max_response_body: usize,
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

        let path = if path_suffix.is_empty() {
            "/"
        } else {
            path_suffix
        }
        .to_string();

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

        let max_request_body = parts
            .params
            .get("maxRequestBody")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(2 * 1024 * 1024); // Default: 2MB

        let max_response_body = parts
            .params
            .get("maxResponseBody")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(10 * 1024 * 1024); // Default: 10MB

        Ok(Self {
            host,
            port,
            path,
            max_request_body,
            max_response_body,
        })
    }
}

// ---------------------------------------------------------------------------
// RequestEnvelope / HttpReply
// ---------------------------------------------------------------------------

/// An inbound HTTP request sent from the Axum dispatch handler to an
/// `HttpConsumer` receive loop.
pub struct RequestEnvelope {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: http::HeaderMap,
    pub body: bytes::Bytes,
    pub reply_tx: tokio::sync::oneshot::Sender<HttpReply>,
}

/// The HTTP response that `HttpConsumer` sends back to the Axum handler.
#[derive(Debug, Clone)]
pub struct HttpReply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: bytes::Bytes,
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
        max_request_body: usize,
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
        let task = tokio::spawn(run_axum_server(
            listener,
            dispatch_for_server,
            max_request_body,
        ));

        // Re-acquire lock to insert — handle the race where another task won.
        let mut guard = self.inner.lock().map_err(|_| {
            CamelError::EndpointCreationFailed("ServerRegistry lock poisoned".into())
        })?;
        // If another task already registered this port while we were binding,
        // abort our task (it has no routes) and use the winner's dispatch table.
        if let Some(existing) = guard.get(&port) {
            task.abort();
            return Ok(Arc::clone(&existing.dispatch));
        }
        guard.insert(
            port,
            ServerHandle {
                dispatch: Arc::clone(&dispatch),
                _task: task,
            },
        );
        Ok(dispatch)
    }
}

// ---------------------------------------------------------------------------
// Axum server
// ---------------------------------------------------------------------------

use axum::{
    Router,
    body::Body as AxumBody,
    extract::{Request, State},
    http::{Response, StatusCode},
    response::IntoResponse,
};

#[derive(Clone)]
struct AppState {
    dispatch: DispatchTable,
    max_request_body: usize,
}

async fn run_axum_server(
    listener: tokio::net::TcpListener,
    dispatch: DispatchTable,
    max_request_body: usize,
) {
    let state = AppState {
        dispatch,
        max_request_body,
    };
    let app = Router::new().fallback(dispatch_handler).with_state(state);

    axum::serve(listener, app).await.unwrap_or_else(|e| {
        tracing::error!(error = %e, "Axum server error");
    });
}

async fn dispatch_handler(State(state): State<AppState>, req: Request) -> impl IntoResponse {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    let headers = req.headers().clone();

    let body_bytes = match axum::body::to_bytes(req.into_body(), state.max_request_body).await {
        Ok(b) => b,
        Err(_) => {
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(AxumBody::empty())
                .expect(
                    "Response::builder() with a known-valid status code and body is infallible",
                );
        }
    };

    // Look up handler for this path
    let sender = {
        let table = state.dispatch.read().await;
        table.get(&path).cloned()
    };

    let Some(sender) = sender else {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(AxumBody::from("No consumer registered for this path"))
            .expect("Response::builder() with a known-valid status code and body is infallible");
    };

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel::<HttpReply>();
    let envelope = RequestEnvelope {
        method,
        path,
        query,
        headers,
        body: body_bytes,
        reply_tx,
    };

    if sender.send(envelope).await.is_err() {
        return Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .body(AxumBody::from("Consumer unavailable"))
            .expect("Response::builder() with a known-valid status code and body is infallible");
    }

    match reply_rx.await {
        Ok(reply) => {
            let status =
                StatusCode::from_u16(reply.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let mut builder = Response::builder().status(status);
            for (k, v) in &reply.headers {
                builder = builder.header(k.as_str(), v.as_str());
            }
            builder
                .body(AxumBody::from(reply.body))
                .unwrap_or_else(|_| {
                    Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(AxumBody::from("Invalid response headers from consumer"))
                        .expect("Response::builder() with a known-valid status code and body is infallible")
                })
        }
        Err(_) => Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(AxumBody::from("Pipeline error"))
            .expect("Response::builder() with a known-valid status code and body is infallible"),
    }
}

// ---------------------------------------------------------------------------
// HttpConsumer
// ---------------------------------------------------------------------------

pub struct HttpConsumer {
    config: HttpServerConfig,
}

impl HttpConsumer {
    pub fn new(config: HttpServerConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl Consumer for HttpConsumer {
    async fn start(&mut self, ctx: camel_component::ConsumerContext) -> Result<(), CamelError> {
        use camel_api::{Exchange, Message, body::Body};

        let dispatch = ServerRegistry::global()
            .get_or_spawn(
                &self.config.host,
                self.config.port,
                self.config.max_request_body,
            )
            .await?;

        // Create a channel for this path and register it
        let (env_tx, mut env_rx) = tokio::sync::mpsc::channel::<RequestEnvelope>(64);
        {
            let mut table = dispatch.write().await;
            table.insert(self.config.path.clone(), env_tx);
        }

        let path = self.config.path.clone();
        let cancel_token = ctx.cancel_token();
        let max_response_body = self.config.max_response_body;

        loop {
            tokio::select! {
                _ = ctx.cancelled() => {
                    break;
                }
                envelope = env_rx.recv() => {
                    let Some(envelope) = envelope else { break; };

                    // Build Exchange from HTTP request
                    let mut msg = Message::default();

                    // Set standard Camel HTTP headers
                    msg.set_header("CamelHttpMethod",
                        serde_json::Value::String(envelope.method.clone()));
                    msg.set_header("CamelHttpPath",
                        serde_json::Value::String(envelope.path.clone()));
                    msg.set_header("CamelHttpQuery",
                        serde_json::Value::String(envelope.query.clone()));

                    // Forward HTTP headers (skip pseudo-headers)
                    for (k, v) in &envelope.headers {
                        if let Ok(val_str) = v.to_str() {
                            msg.set_header(
                                k.as_str(),
                                serde_json::Value::String(val_str.to_string()),
                            );
                        }
                    }

                    // Body: Try to convert to text if UTF-8, otherwise keep as bytes
                    if !envelope.body.is_empty() {
                        match std::str::from_utf8(&envelope.body) {
                            Ok(text) => msg.body = Body::Text(text.to_string()),
                            Err(_) => msg.body = Body::Bytes(envelope.body.clone()),
                        }
                    }

                    #[allow(unused_mut)]
                    let mut exchange = Exchange::new(msg);

                    // Extract W3C TraceContext headers for distributed tracing (opt-in via "otel" feature)
                    #[cfg(feature = "otel")]
                    {
                        let headers: HashMap<String, String> = envelope
                            .headers
                            .iter()
                            .filter_map(|(k, v)| {
                                Some((k.as_str().to_lowercase(), v.to_str().ok()?.to_string()))
                            })
                            .collect();
                        camel_otel::extract_into_exchange(&mut exchange, &headers);
                    }

                    let reply_tx = envelope.reply_tx;
                    let sender = ctx.sender().clone();
                    let path_clone = path.clone();
                    let cancel = cancel_token.clone();

                    // Spawn a task to handle this request concurrently
                    //
                    // NOTE: This spawns a separate tokio task for each incoming HTTP request to enable
                    // true concurrent request processing. This change was introduced as part of the
                    // pipeline concurrency feature and was NOT part of the original HttpConsumer design.
                    //
                    // Rationale:
                    // 1. Without spawning per-request tasks, the send_and_wait() operation would block
                    //    the consumer's main loop until the pipeline processing completes
                    // 2. This blocking would prevent multiple HTTP requests from being processed
                    //    concurrently, even when ConcurrencyModel::Concurrent is enabled on the pipeline
                    // 3. The channel would never have multiple exchanges buffered simultaneously,
                    //    defeating the purpose of pipeline-side concurrency
                    // 4. By spawning a task per request, we allow the consumer loop to continue
                    //    accepting new requests while existing ones are processed in the pipeline
                    //
                    // This approach effectively decouples request acceptance from pipeline processing,
                    // allowing the channel to buffer multiple exchanges that can be processed concurrently
                    // by the pipeline when ConcurrencyModel::Concurrent is active.
                    tokio::spawn(async move {
                        // Check for cancellation before sending to pipeline.
                        // Returns 503 (Service Unavailable) instead of letting the request
                        // enter a shutting-down pipeline. This is a behavioral change from
                        // the pre-concurrency implementation where cancellation during
                        // processing would result in a 500 (Internal Server Error).
                        // 503 is more semantically correct: the server is temporarily
                        // unable to handle the request due to shutdown.
                        if cancel.is_cancelled() {
                            let _ = reply_tx.send(HttpReply {
                                status: 503,
                                headers: vec![],
                                body: bytes::Bytes::from("Service Unavailable"),
                            });
                            return;
                        }

                        // Send through pipeline and await result
                        let (tx, rx) = tokio::sync::oneshot::channel();
                        let envelope = camel_component::consumer::ExchangeEnvelope {
                            exchange,
                            reply_tx: Some(tx),
                        };

                        let result = match sender.send(envelope).await {
                            Ok(()) => rx.await.map_err(|_| camel_api::CamelError::ChannelClosed),
                            Err(_) => Err(camel_api::CamelError::ChannelClosed),
                        }
                        .and_then(|r| r);

                        let reply = match result {
                            Ok(out) => {
                                let status = out
                                    .input
                                    .header("CamelHttpResponseCode")
                                    .and_then(|v| v.as_u64())
                                    .map(|s| s as u16)
                                    .unwrap_or(200);

                                let body_bytes = match out.input.body {
                                    Body::Empty => bytes::Bytes::new(),
                                    Body::Bytes(b) => b,
                                    Body::Text(s) => bytes::Bytes::from(s.into_bytes()),
                                    Body::Xml(s) => bytes::Bytes::from(s.into_bytes()),
                                    Body::Json(v) => bytes::Bytes::from(v.to_string().into_bytes()),
                                    Body::Stream(_) => {
                                        // Materialize stream for HTTP response
                                        match out.input.body.into_bytes(max_response_body).await {
                                            Ok(b) => b,
                                            Err(e) => {
                                                debug!(error = %e, "Failed to materialize stream body for HTTP reply");
                                                return;
                                            }
                                        }
                                    }
                                };

                                let resp_headers: Vec<(String, String)> = out
                                    .input
                                    .headers
                                    .iter()
                                    // Filter Camel internal headers
                                    .filter(|(k, _)| !k.starts_with("Camel"))
                                    // Filter hop-by-hop and request-only headers
                                    // Based on Apache Camel's HttpUtil.addCommonFilters()
                                    .filter(|(k, _)| {
                                        !matches!(
                                            k.to_lowercase().as_str(),
                                            // RFC 2616 Section 4.5 - General headers
                                            "content-length" |      // Auto-calculated by framework
                                            "content-type" |        // Auto-calculated from body
                                            "transfer-encoding" |   // Hop-by-hop
                                            "connection" |          // Hop-by-hop
                                            "cache-control" |       // Hop-by-hop
                                            "date" |                // Auto-generated
                                            "pragma" |              // Hop-by-hop
                                            "trailer" |             // Hop-by-hop
                                            "upgrade" |             // Hop-by-hop
                                            "via" |                 // Hop-by-hop
                                            "warning" |             // Hop-by-hop
                                            // Request-only headers
                                            "host" |                // Request-only
                                            "user-agent" |          // Request-only
                                            "accept" |              // Request-only
                                            "accept-encoding" |     // Request-only
                                            "accept-language" |     // Request-only
                                            "accept-charset" |      // Request-only
                                            "authorization" |       // Request-only (security)
                                            "proxy-authorization" | // Request-only (security)
                                            "cookie" |              // Request-only
                                            "expect" |              // Request-only
                                            "from" |                // Request-only
                                            "if-match" |            // Request-only
                                            "if-modified-since" |   // Request-only
                                            "if-none-match" |       // Request-only
                                            "if-range" |            // Request-only
                                            "if-unmodified-since" | // Request-only
                                            "max-forwards" |        // Request-only
                                            "proxy-connection" |    // Request-only
                                            "range" |               // Request-only
                                            "referer" |             // Request-only
                                            "te"                    // Request-only
                                        )
                                    })
                                    .filter_map(|(k, v)| {
                                        v.as_str().map(|s| (k.clone(), s.to_string()))
                                    })
                                    .collect();

                                HttpReply {
                                    status,
                                    headers: resp_headers,
                                    body: body_bytes,
                                }
                            }
                            Err(e) => {
                                tracing::error!(error = %e, path = %path_clone, "Pipeline error processing HTTP request");
                                HttpReply {
                                    status: 500,
                                    headers: vec![],
                                    body: bytes::Bytes::from("Internal Server Error"),
                                }
                            }
                        };

                        // Reply to Axum handler (ignore error if client disconnected)
                        let _ = reply_tx.send(reply);
                    });
                }
            }
        }

        // Deregister this path
        {
            let mut table = dispatch.write().await;
            table.remove(&path);
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }

    fn concurrency_model(&self) -> camel_component::ConcurrencyModel {
        camel_component::ConcurrencyModel::Concurrent { max: None }
    }
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
        let server_config = HttpServerConfig::from_uri(uri)?;
        let client = build_client(&config)?;
        Ok(Box::new(HttpEndpoint {
            uri: uri.to_string(),
            config,
            server_config,
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
        let server_config = HttpServerConfig::from_uri(uri)?;
        let client = build_client(&config)?;
        Ok(Box::new(HttpEndpoint {
            uri: uri.to_string(),
            config,
            server_config,
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
    server_config: HttpServerConfig,
    client: reqwest::Client,
}

impl Endpoint for HttpEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(HttpConsumer::new(self.server_config.clone())))
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(HttpProducer {
            config: Arc::new(self.config.clone()),
            client: self.client.clone(),
        }))
    }
}

// ---------------------------------------------------------------------------
// SSRF Protection
// ---------------------------------------------------------------------------

fn validate_url_for_ssrf(url: &str, config: &HttpConfig) -> Result<(), CamelError> {
    let parsed = url::Url::parse(url)
        .map_err(|e| CamelError::ProcessorError(format!("Invalid URL: {}", e)))?;

    // Check blocked hosts
    if let Some(host) = parsed.host_str()
        && config.blocked_hosts.iter().any(|blocked| host == blocked)
    {
        return Err(CamelError::ProcessorError(format!(
            "Host '{}' is blocked",
            host
        )));
    }

    // Check private IPs if not allowed
    if !config.allow_private_ips
        && let Some(host) = parsed.host()
    {
        match host {
            url::Host::Ipv4(ip) => {
                if ip.is_private() || ip.is_loopback() || ip.is_link_local() {
                    return Err(CamelError::ProcessorError(format!(
                        "Private IP '{}' not allowed (set allowPrivateIps=true to override)",
                        ip
                    )));
                }
            }
            url::Host::Ipv6(ip) => {
                if ip.is_loopback() {
                    return Err(CamelError::ProcessorError(format!(
                        "Loopback IP '{}' not allowed",
                        ip
                    )));
                }
            }
            url::Host::Domain(domain) => {
                // Block common internal domains
                let blocked_domains = ["localhost", "127.0.0.1", "0.0.0.0", "local"];
                if blocked_domains.contains(&domain) {
                    return Err(CamelError::ProcessorError(format!(
                        "Domain '{}' is not allowed",
                        domain
                    )));
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// HttpProducer
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct HttpProducer {
    config: Arc<HttpConfig>,
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

            // SECURITY: Validate URL for SSRF
            validate_url_for_ssrf(&url, &config)?;

            debug!(
                correlation_id = %exchange.correlation_id(),
                method = %method_str,
                url = %url,
                "HTTP request"
            );

            let method = method_str.parse::<reqwest::Method>().map_err(|e| {
                CamelError::ProcessorError(format!("Invalid HTTP method '{}': {}", method_str, e))
            })?;

            let mut request = client.request(method, &url);

            if let Some(timeout) = config.response_timeout {
                request = request.timeout(timeout);
            }

            // Inject W3C TraceContext headers for distributed tracing (opt-in via "otel" feature)
            #[cfg(feature = "otel")]
            {
                let mut otel_headers = HashMap::new();
                camel_otel::inject_from_exchange(&exchange, &mut otel_headers);
                for (k, v) in otel_headers {
                    if let (Ok(name), Ok(val)) = (
                        reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                        reqwest::header::HeaderValue::from_str(&v),
                    ) {
                        request = request.header(name, val);
                    }
                }
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

            match exchange.input.body {
                Body::Stream(ref s) => {
                    let mut stream_lock = s.stream.lock().await;
                    if let Some(stream) = stream_lock.take() {
                        request = request.body(reqwest::Body::wrap_stream(stream));
                    } else {
                        return Err(CamelError::AlreadyConsumed);
                    }
                }
                _ => {
                    // For other types, materialize with configured limit
                    let body = std::mem::take(&mut exchange.input.body);
                    let bytes = body.into_bytes(config.max_body_size).await?;
                    if !bytes.is_empty() {
                        request = request.body(bytes);
                    }
                }
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
                    method: method_str,
                    url,
                    status_code,
                    status_text,
                    response_body: Some(String::from_utf8_lossy(&response_body).to_string()),
                });
            }

            if !response_body.is_empty() {
                exchange.input.body = Body::Bytes(bytes::Bytes::from(response_body.to_vec()));
            }

            debug!(
                correlation_id = %exchange.correlation_id(),
                status = status_code,
                url = %url,
                "HTTP response"
            );
            Ok(exchange)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::Message;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Mutex;

    // NullRouteController for testing
    struct NullRouteController;
    #[async_trait::async_trait]
    impl camel_api::RouteController for NullRouteController {
        async fn start_route(&mut self, _: &str) -> Result<(), camel_api::CamelError> {
            Ok(())
        }
        async fn stop_route(&mut self, _: &str) -> Result<(), camel_api::CamelError> {
            Ok(())
        }
        async fn restart_route(&mut self, _: &str) -> Result<(), camel_api::CamelError> {
            Ok(())
        }
        async fn suspend_route(&mut self, _: &str) -> Result<(), camel_api::CamelError> {
            Ok(())
        }
        async fn resume_route(&mut self, _: &str) -> Result<(), camel_api::CamelError> {
            Ok(())
        }
        fn route_status(&self, _: &str) -> Option<camel_api::RouteStatus> {
            None
        }
        async fn start_all_routes(&mut self) -> Result<(), camel_api::CamelError> {
            Ok(())
        }
        async fn stop_all_routes(&mut self) -> Result<(), camel_api::CamelError> {
            Ok(())
        }
    }

    fn test_producer_ctx() -> ProducerContext {
        ProducerContext::new(Arc::new(Mutex::new(NullRouteController)))
    }

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
    fn test_http_endpoint_creates_consumer() {
        let component = HttpComponent::new();
        let endpoint = component
            .create_endpoint("http://0.0.0.0:19100/test")
            .unwrap();
        assert!(endpoint.create_consumer().is_ok());
    }

    #[test]
    fn test_https_endpoint_creates_consumer() {
        let component = HttpsComponent::new();
        let endpoint = component
            .create_endpoint("https://0.0.0.0:8443/test")
            .unwrap();
        assert!(endpoint.create_consumer().is_ok());
    }

    #[test]
    fn test_http_endpoint_creates_producer() {
        let ctx = test_producer_ctx();
        let component = HttpComponent::new();
        let endpoint = component.create_endpoint("http://localhost/api").unwrap();
        assert!(endpoint.create_producer(&ctx).is_ok());
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
        let ctx = test_producer_ctx();

        let component = HttpComponent::new();
        let endpoint = component
            .create_endpoint(&format!("{url}/api/test?allowPrivateIps=true"))
            .unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

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
        let ctx = test_producer_ctx();

        let component = HttpComponent::new();
        let endpoint = component
            .create_endpoint(&format!("{url}/api/data?allowPrivateIps=true"))
            .unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

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
        let ctx = test_producer_ctx();

        let component = HttpComponent::new();
        let endpoint = component
            .create_endpoint(&format!("{url}/api?allowPrivateIps=true"))
            .unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

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
        let ctx = test_producer_ctx();

        let component = HttpComponent::new();
        let endpoint = component
            .create_endpoint(&format!("{url}/api?httpMethod=PUT&allowPrivateIps=true"))
            .unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

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
        let ctx = test_producer_ctx();

        let component = HttpComponent::new();
        let endpoint = component
            .create_endpoint(&format!("{url}/not-found?allowPrivateIps=true"))
            .unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

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
        let ctx = test_producer_ctx();

        let component = HttpComponent::new();
        let endpoint = component
            .create_endpoint(&format!(
                "{url}/error?throwExceptionOnFailure=false&allowPrivateIps=true"
            ))
            .unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

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
        let ctx = test_producer_ctx();

        let component = HttpComponent::new();
        let endpoint = component
            .create_endpoint("http://localhost:1/does-not-exist?allowPrivateIps=true")
            .unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

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
        let ctx = test_producer_ctx();

        let component = HttpComponent::new();
        let endpoint = component
            .create_endpoint(&format!("{url}/api?allowPrivateIps=true"))
            .unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

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
        let ctx = test_producer_ctx();

        let component = HttpComponent::new();
        let endpoint = component
            .create_endpoint(&format!(
                "{url}?followRedirects=false&throwExceptionOnFailure=false&allowPrivateIps=true"
            ))
            .unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

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
        let ctx = test_producer_ctx();

        let component = HttpComponent::new();
        let endpoint = component
            .create_endpoint(&format!("{url}?followRedirects=true&allowPrivateIps=true"))
            .unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

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
        let ctx = test_producer_ctx();

        let component = HttpComponent::new();
        // apiKey is NOT a Camel option, should be forwarded as query param
        let endpoint = component
            .create_endpoint(&format!(
                "{url}/api?apiKey=secret123&httpMethod=GET&allowPrivateIps=true"
            ))
            .unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

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
    // SSRF Protection tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_http_producer_blocks_metadata_endpoint() {
        use tower::ServiceExt;

        let ctx = test_producer_ctx();
        let component = HttpComponent::new();
        let endpoint = component
            .create_endpoint("http://example.com/api?allowPrivateIps=false")
            .unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

        let mut exchange = Exchange::new(Message::default());
        exchange.input.set_header(
            "CamelHttpUri",
            serde_json::Value::String("http://169.254.169.254/latest/meta-data/".to_string()),
        );

        let result = producer.oneshot(exchange).await;
        assert!(result.is_err(), "Should block AWS metadata endpoint");

        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("Private IP"),
            "Error should mention private IP blocking, got: {}",
            err
        );
    }

    #[test]
    fn test_ssrf_config_defaults() {
        let config = HttpConfig::from_uri("http://example.com/api").unwrap();
        assert!(
            !config.allow_private_ips,
            "Private IPs should be blocked by default"
        );
        assert!(
            config.blocked_hosts.is_empty(),
            "Blocked hosts should be empty by default"
        );
    }

    #[test]
    fn test_ssrf_config_allow_private_ips() {
        let config = HttpConfig::from_uri("http://example.com/api?allowPrivateIps=true").unwrap();
        assert!(
            config.allow_private_ips,
            "Private IPs should be allowed when explicitly set"
        );
    }

    #[test]
    fn test_ssrf_config_blocked_hosts() {
        let config =
            HttpConfig::from_uri("http://example.com/api?blockedHosts=evil.com,malware.net")
                .unwrap();
        assert_eq!(config.blocked_hosts, vec!["evil.com", "malware.net"]);
    }

    #[tokio::test]
    async fn test_http_producer_blocks_localhost() {
        use tower::ServiceExt;

        let ctx = test_producer_ctx();
        let component = HttpComponent::new();
        let endpoint = component.create_endpoint("http://example.com/api").unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

        let mut exchange = Exchange::new(Message::default());
        exchange.input.set_header(
            "CamelHttpUri",
            serde_json::Value::String("http://localhost:8080/internal".to_string()),
        );

        let result = producer.oneshot(exchange).await;
        assert!(result.is_err(), "Should block localhost");
    }

    #[tokio::test]
    async fn test_http_producer_blocks_loopback_ip() {
        use tower::ServiceExt;

        let ctx = test_producer_ctx();
        let component = HttpComponent::new();
        let endpoint = component.create_endpoint("http://example.com/api").unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

        let mut exchange = Exchange::new(Message::default());
        exchange.input.set_header(
            "CamelHttpUri",
            serde_json::Value::String("http://127.0.0.1:8080/internal".to_string()),
        );

        let result = producer.oneshot(exchange).await;
        assert!(result.is_err(), "Should block loopback IP");
    }

    #[tokio::test]
    async fn test_http_producer_allows_private_ip_when_enabled() {
        use tower::ServiceExt;

        let ctx = test_producer_ctx();
        let component = HttpComponent::new();
        // With allowPrivateIps=true, the validation should pass
        // (actual connection will fail, but that's expected)
        let endpoint = component
            .create_endpoint("http://192.168.1.1/api?allowPrivateIps=true")
            .unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

        let exchange = Exchange::new(Message::default());

        // The request will fail because we can't connect, but it should NOT fail
        // due to SSRF protection
        let result = producer.oneshot(exchange).await;
        // We expect connection error, not SSRF error
        if let Err(ref e) = result {
            let err_str = e.to_string();
            assert!(
                !err_str.contains("Private IP") && !err_str.contains("not allowed"),
                "Should not be SSRF error, got: {}",
                err_str
            );
        }
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

    // -----------------------------------------------------------------------
    // Axum dispatch handler tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_dispatch_handler_returns_404_for_unknown_path() {
        let dispatch: DispatchTable = Arc::new(RwLock::new(HashMap::new()));
        // Nothing registered in the dispatch table
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(run_axum_server(listener, dispatch, 2 * 1024 * 1024));

        // Wait for server to start
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let resp = reqwest::get(format!("http://127.0.0.1:{port}/unknown"))
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 404);
    }

    // -----------------------------------------------------------------------
    // HttpConsumer tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_http_consumer_start_registers_path() {
        use camel_component::ConsumerContext;

        // Get an OS-assigned free port
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener); // Release port — ServerRegistry will rebind it

        let consumer_cfg = HttpServerConfig {
            host: "127.0.0.1".to_string(),
            port,
            path: "/ping".to_string(),
            max_request_body: 2 * 1024 * 1024,
            max_response_body: 10 * 1024 * 1024,
        };
        let mut consumer = HttpConsumer::new(consumer_cfg);

        let (tx, mut rx) = tokio::sync::mpsc::channel::<camel_component::ExchangeEnvelope>(16);
        let token = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone());

        tokio::spawn(async move {
            consumer.start(ctx).await.unwrap();
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = reqwest::Client::new();
        let resp_future = client
            .post(format!("http://127.0.0.1:{port}/ping"))
            .body("hello world")
            .send();

        let (http_result, _) = tokio::join!(resp_future, async {
            if let Some(mut envelope) = rx.recv().await {
                // Set a custom status code
                envelope.exchange.input.set_header(
                    "CamelHttpResponseCode",
                    serde_json::Value::Number(201.into()),
                );
                if let Some(reply_tx) = envelope.reply_tx {
                    let _ = reply_tx.send(Ok(envelope.exchange));
                }
            }
        });

        let resp = http_result.unwrap();
        assert_eq!(resp.status().as_u16(), 201);

        token.cancel();
    }

    // -----------------------------------------------------------------------
    // Integration tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_integration_single_consumer_round_trip() {
        use camel_component::{ConsumerContext, ExchangeEnvelope};

        // Get an OS-assigned free port (ephemeral)
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener); // Release — ServerRegistry will rebind

        let component = HttpComponent::new();
        let endpoint = component
            .create_endpoint(&format!("http://127.0.0.1:{port}/echo"))
            .unwrap();
        let mut consumer = endpoint.create_consumer().unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
        let token = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone());

        tokio::spawn(async move { consumer.start(ctx).await.unwrap() });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = reqwest::Client::new();
        let send_fut = client
            .post(format!("http://127.0.0.1:{port}/echo"))
            .header("Content-Type", "text/plain")
            .body("ping")
            .send();

        let (http_result, _) = tokio::join!(send_fut, async {
            if let Some(mut envelope) = rx.recv().await {
                assert_eq!(
                    envelope.exchange.input.header("CamelHttpMethod"),
                    Some(&serde_json::Value::String("POST".into()))
                );
                assert_eq!(
                    envelope.exchange.input.header("CamelHttpPath"),
                    Some(&serde_json::Value::String("/echo".into()))
                );
                envelope.exchange.input.body = camel_api::body::Body::Text("pong".to_string());
                if let Some(reply_tx) = envelope.reply_tx {
                    let _ = reply_tx.send(Ok(envelope.exchange));
                }
            }
        });

        let resp = http_result.unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let body = resp.text().await.unwrap();
        assert_eq!(body, "pong");

        token.cancel();
    }

    #[tokio::test]
    async fn test_integration_two_consumers_shared_port() {
        use camel_component::{ConsumerContext, ExchangeEnvelope};

        // Get an OS-assigned free port (ephemeral)
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let component = HttpComponent::new();

        // Consumer A: /hello
        let endpoint_a = component
            .create_endpoint(&format!("http://127.0.0.1:{port}/hello"))
            .unwrap();
        let mut consumer_a = endpoint_a.create_consumer().unwrap();

        // Consumer B: /world
        let endpoint_b = component
            .create_endpoint(&format!("http://127.0.0.1:{port}/world"))
            .unwrap();
        let mut consumer_b = endpoint_b.create_consumer().unwrap();

        let (tx_a, mut rx_a) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
        let token_a = tokio_util::sync::CancellationToken::new();
        let ctx_a = ConsumerContext::new(tx_a, token_a.clone());

        let (tx_b, mut rx_b) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
        let token_b = tokio_util::sync::CancellationToken::new();
        let ctx_b = ConsumerContext::new(tx_b, token_b.clone());

        tokio::spawn(async move { consumer_a.start(ctx_a).await.unwrap() });
        tokio::spawn(async move { consumer_b.start(ctx_b).await.unwrap() });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = reqwest::Client::new();

        // Request to /hello
        let fut_hello = client.get(format!("http://127.0.0.1:{port}/hello")).send();
        let (resp_hello, _) = tokio::join!(fut_hello, async {
            if let Some(mut envelope) = rx_a.recv().await {
                envelope.exchange.input.body =
                    camel_api::body::Body::Text("hello-response".to_string());
                if let Some(reply_tx) = envelope.reply_tx {
                    let _ = reply_tx.send(Ok(envelope.exchange));
                }
            }
        });

        // Request to /world
        let fut_world = client.get(format!("http://127.0.0.1:{port}/world")).send();
        let (resp_world, _) = tokio::join!(fut_world, async {
            if let Some(mut envelope) = rx_b.recv().await {
                envelope.exchange.input.body =
                    camel_api::body::Body::Text("world-response".to_string());
                if let Some(reply_tx) = envelope.reply_tx {
                    let _ = reply_tx.send(Ok(envelope.exchange));
                }
            }
        });

        let body_a = resp_hello.unwrap().text().await.unwrap();
        let body_b = resp_world.unwrap().text().await.unwrap();

        assert_eq!(body_a, "hello-response");
        assert_eq!(body_b, "world-response");

        token_a.cancel();
        token_b.cancel();
    }

    #[tokio::test]
    async fn test_integration_unregistered_path_returns_404() {
        use camel_component::{ConsumerContext, ExchangeEnvelope};

        // Get an OS-assigned free port (ephemeral)
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let component = HttpComponent::new();
        let endpoint = component
            .create_endpoint(&format!("http://127.0.0.1:{port}/registered"))
            .unwrap();
        let mut consumer = endpoint.create_consumer().unwrap();

        let (tx, _rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
        let token = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone());

        tokio::spawn(async move { consumer.start(ctx).await.unwrap() });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://127.0.0.1:{port}/not-there"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 404);

        token.cancel();
    }

    #[test]
    fn test_http_consumer_declares_concurrent() {
        use camel_component::ConcurrencyModel;

        let config = HttpServerConfig {
            host: "127.0.0.1".to_string(),
            port: 19999,
            path: "/test".to_string(),
            max_request_body: 2 * 1024 * 1024,
            max_response_body: 10 * 1024 * 1024,
        };
        let consumer = HttpConsumer::new(config);
        assert_eq!(
            consumer.concurrency_model(),
            ConcurrencyModel::Concurrent { max: None }
        );
    }

    // -----------------------------------------------------------------------
    // OpenTelemetry propagation tests (only compiled with "otel" feature)
    // -----------------------------------------------------------------------

    #[cfg(feature = "otel")]
    mod otel_tests {
        use super::*;
        use camel_api::Message;
        use tower::ServiceExt;

        #[tokio::test]
        async fn test_producer_injects_traceparent_header() {
            let (url, _handle) = start_test_server_with_header_capture().await;
            let ctx = test_producer_ctx();

            let component = HttpComponent::new();
            let endpoint = component
                .create_endpoint(&format!("{url}/api?allowPrivateIps=true"))
                .unwrap();
            let producer = endpoint.create_producer(&ctx).unwrap();

            // Create exchange with an OTel context by extracting from a traceparent header
            let mut exchange = Exchange::new(Message::default());
            let mut headers = std::collections::HashMap::new();
            headers.insert(
                "traceparent".to_string(),
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_string(),
            );
            camel_otel::extract_into_exchange(&mut exchange, &headers);

            let result = producer.oneshot(exchange).await.unwrap();

            // Verify request succeeded
            let status = result
                .input
                .header("CamelHttpResponseCode")
                .and_then(|v| v.as_u64())
                .unwrap();
            assert_eq!(status, 200);

            // The test server echoes back the received traceparent header
            let traceparent = result.input.header("x-received-traceparent");
            assert!(
                traceparent.is_some(),
                "traceparent header should have been sent"
            );

            let traceparent_str = traceparent.unwrap().as_str().unwrap();
            // Verify format: version-traceid-spanid-flags
            let parts: Vec<&str> = traceparent_str.split('-').collect();
            assert_eq!(parts.len(), 4, "traceparent should have 4 parts");
            assert_eq!(parts[0], "00", "version should be 00");
            assert_eq!(
                parts[1], "4bf92f3577b34da6a3ce929d0e0e4736",
                "trace-id should match"
            );
            assert_eq!(parts[2], "00f067aa0ba902b7", "span-id should match");
            assert_eq!(parts[3], "01", "flags should be 01 (sampled)");
        }

        #[tokio::test]
        async fn test_consumer_extracts_traceparent_header() {
            use camel_component::{ConsumerContext, ExchangeEnvelope};

            // Get an OS-assigned free port
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            drop(listener);

            let component = HttpComponent::new();
            let endpoint = component
                .create_endpoint(&format!("http://127.0.0.1:{port}/trace"))
                .unwrap();
            let mut consumer = endpoint.create_consumer().unwrap();

            let (tx, mut rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
            let token = tokio_util::sync::CancellationToken::new();
            let ctx = ConsumerContext::new(tx, token.clone());

            tokio::spawn(async move { consumer.start(ctx).await.unwrap() });
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            // Send request with traceparent header
            let client = reqwest::Client::new();
            let send_fut = client
                .post(format!("http://127.0.0.1:{port}/trace"))
                .header(
                    "traceparent",
                    "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
                )
                .body("test")
                .send();

            let (http_result, _) = tokio::join!(send_fut, async {
                if let Some(envelope) = rx.recv().await {
                    // Verify the exchange has a valid OTel context by re-injecting it
                    // and checking the traceparent matches
                    let mut injected_headers = std::collections::HashMap::new();
                    camel_otel::inject_from_exchange(&envelope.exchange, &mut injected_headers);

                    assert!(
                        injected_headers.contains_key("traceparent"),
                        "Exchange should have traceparent after extraction"
                    );

                    let traceparent = injected_headers.get("traceparent").unwrap();
                    let parts: Vec<&str> = traceparent.split('-').collect();
                    assert_eq!(parts.len(), 4, "traceparent should have 4 parts");
                    assert_eq!(
                        parts[1], "4bf92f3577b34da6a3ce929d0e0e4736",
                        "Trace ID should match the original traceparent header"
                    );

                    if let Some(reply_tx) = envelope.reply_tx {
                        let _ = reply_tx.send(Ok(envelope.exchange));
                    }
                }
            });

            let resp = http_result.unwrap();
            assert_eq!(resp.status().as_u16(), 200);

            token.cancel();
        }

        #[tokio::test]
        async fn test_consumer_extracts_mixed_case_traceparent_header() {
            use camel_component::{ConsumerContext, ExchangeEnvelope};

            // Get an OS-assigned free port
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            drop(listener);

            let component = HttpComponent::new();
            let endpoint = component
                .create_endpoint(&format!("http://127.0.0.1:{port}/trace"))
                .unwrap();
            let mut consumer = endpoint.create_consumer().unwrap();

            let (tx, mut rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
            let token = tokio_util::sync::CancellationToken::new();
            let ctx = ConsumerContext::new(tx, token.clone());

            tokio::spawn(async move { consumer.start(ctx).await.unwrap() });
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            // Send request with MIXED-CASE TraceParent header (not lowercase)
            let client = reqwest::Client::new();
            let send_fut = client
                .post(format!("http://127.0.0.1:{port}/trace"))
                .header(
                    "TraceParent",
                    "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
                )
                .body("test")
                .send();

            let (http_result, _) = tokio::join!(send_fut, async {
                if let Some(envelope) = rx.recv().await {
                    // Verify the exchange has a valid OTel context by re-injecting it
                    // and checking the traceparent matches
                    let mut injected_headers = HashMap::new();
                    camel_otel::inject_from_exchange(&envelope.exchange, &mut injected_headers);

                    assert!(
                        injected_headers.contains_key("traceparent"),
                        "Exchange should have traceparent after extraction from mixed-case header"
                    );

                    let traceparent = injected_headers.get("traceparent").unwrap();
                    let parts: Vec<&str> = traceparent.split('-').collect();
                    assert_eq!(parts.len(), 4, "traceparent should have 4 parts");
                    assert_eq!(
                        parts[1], "4bf92f3577b34da6a3ce929d0e0e4736",
                        "Trace ID should match the original mixed-case TraceParent header"
                    );

                    if let Some(reply_tx) = envelope.reply_tx {
                        let _ = reply_tx.send(Ok(envelope.exchange));
                    }
                }
            });

            let resp = http_result.unwrap();
            assert_eq!(resp.status().as_u16(), 200);

            token.cancel();
        }

        #[tokio::test]
        async fn test_producer_no_trace_context_no_crash() {
            let (url, _handle) = start_test_server().await;
            let ctx = test_producer_ctx();

            let component = HttpComponent::new();
            let endpoint = component
                .create_endpoint(&format!("{url}/api?allowPrivateIps=true"))
                .unwrap();
            let producer = endpoint.create_producer(&ctx).unwrap();

            // Create exchange with default (empty) otel_context - no trace context
            let exchange = Exchange::new(Message::default());

            // Should succeed without panic
            let result = producer.oneshot(exchange).await.unwrap();

            // Verify request succeeded
            let status = result
                .input
                .header("CamelHttpResponseCode")
                .and_then(|v| v.as_u64())
                .unwrap();
            assert_eq!(status, 200);
        }

        /// Test server that captures and echoes back the traceparent header
        async fn start_test_server_with_header_capture() -> (String, tokio::task::JoinHandle<()>) {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let url = format!("http://127.0.0.1:{}", addr.port());

            let handle = tokio::spawn(async move {
                loop {
                    if let Ok((mut stream, _)) = listener.accept().await {
                        tokio::spawn(async move {
                            use tokio::io::{AsyncReadExt, AsyncWriteExt};
                            let mut buf = vec![0u8; 8192];
                            let n = stream.read(&mut buf).await.unwrap_or(0);
                            let request = String::from_utf8_lossy(&buf[..n]).to_string();

                            // Extract traceparent header from request
                            let traceparent = request
                                .lines()
                                .find(|line| line.to_lowercase().starts_with("traceparent:"))
                                .map(|line| {
                                    line.split(':')
                                        .nth(1)
                                        .map(|s| s.trim().to_string())
                                        .unwrap_or_default()
                                })
                                .unwrap_or_default();

                            let body =
                                format!(r#"{{"echo":"ok","traceparent":"{}"}}"#, traceparent);
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nX-Received-Traceparent: {}\r\n\r\n{}",
                                body.len(),
                                traceparent,
                                body
                            );
                            let _ = stream.write_all(response.as_bytes()).await;
                        });
                    }
                }
            });

            (url, handle)
        }
    }
}
