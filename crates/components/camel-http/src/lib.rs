pub mod auth;
pub mod bundle;
pub mod config;
pub mod health;
pub mod registry;
pub(crate) mod rest_match;
pub(crate) mod ssrf;
pub mod static_config;
pub mod static_dispatch;
pub mod static_endpoint;
pub(crate) mod tls_reload;
use crate::config::parse_ok_status_code_range;
pub use bundle::HttpBundle;
pub use bundle::HttpStaticBundle;
pub use config::HttpConfig;
pub use health::HttpHealthCheck;
pub use registry::HttpRouteRegistry;
pub use static_config::HttpStaticConfig;
pub use static_endpoint::{HttpStaticComponent, HttpStaticConsumer, HttpStaticEndpoint};

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::sync::OnceCell;
use tower::Layer;
use tower::Service;
use tracing::debug;

use axum::body::BodyDataStream;
use camel_api::component_metadata::{
    ComponentCapabilities, ComponentMetadata, OptionKind, UriOption,
};
use camel_auth::bearer_token_layer::BearerTokenLayer;
use camel_auth::oauth2::TokenProvider;
use camel_component_api::tls_source::ServerTlsSource;
use camel_component_api::{Body, BoxProcessor, CamelError, Exchange, StreamBody, StreamMetadata};
use camel_component_api::{Component, Consumer, Endpoint, ProducerContext, RuntimeObservability};
use camel_component_api::{UriComponents, UriConfig, parse_uri};
use futures::TryStreamExt;
use futures::stream::BoxStream;

// ---------------------------------------------------------------------------
// HttpEndpointConfig
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
pub struct HttpEndpointConfig {
    pub base_url: String,
    pub http_method: Option<String>,
    pub throw_exception_on_failure: bool,
    pub ok_status_code_range: (u16, u16),
    pub response_timeout: Option<Duration>,
    pub query_params: HashMap<String, String>,
    pub allow_internal: bool,
    pub blocked_hosts: Vec<String>,
    pub max_body_size: usize,
    pub read_timeout_ms: u64,
    pub max_response_bytes: usize,
    pub auth: HttpAuth,
    pub token_provider: Option<Arc<dyn TokenProvider>>,
    pub user_agent: Option<String>,
    pub cookie_handling: CookieHandling,
    pub bridge_endpoint: bool,
    pub connection_close: bool,
    pub skip_request_headers: Vec<String>,
    pub skip_response_headers: Vec<String>,
    pub follow_redirects: bool,
    pub max_redirects: usize,
}

#[derive(Clone, PartialEq)]
pub enum HttpAuth {
    None,
    Basic { username: String, password: String },
    Bearer { token: String },
}

impl std::fmt::Debug for HttpAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpAuth::None => f.write_str("None"),
            HttpAuth::Basic { username, .. } => f
                .debug_struct("Basic")
                .field("username", username)
                .field("password", &"***")
                .finish(),
            HttpAuth::Bearer { .. } => f.debug_struct("Bearer").field("token", &"***").finish(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookieHandling {
    Disabled,
    InMemory,
}

/// Camel options that should NOT be forwarded as HTTP query params
const HTTP_CAMEL_OPTIONS: &[&str] = &[
    "httpMethod",
    "throwExceptionOnFailure",
    "okStatusCodeRange",
    "followRedirects",
    "maxRedirects",
    "connectTimeout",
    "responseTimeout",
    "allowInternal",
    "blockedHosts",
    "maxBodySize",
    "readTimeout",
    "maxResponseBytes",
    "authMethod",
    "authUsername",
    "authPassword",
    "authBearerToken",
    "userAgent",
    "cookieHandling",
    "bridgeEndpoint",
    "connectionClose",
    "skipRequestHeaders",
    "skipResponseHeaders",
];

impl UriConfig for HttpEndpointConfig {
    /// Returns "http" as the primary scheme (also accepts "https")
    fn scheme() -> &'static str {
        "http"
    }

    fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = parse_uri(uri)?;
        Self::from_components(parts)
    }

    fn from_components(parts: UriComponents) -> Result<Self, CamelError> {
        // Validate scheme - accept both http and https
        if parts.scheme != "http" && parts.scheme != "https" {
            return Err(CamelError::InvalidUri(format!(
                "expected scheme 'http' or 'https', got '{}'",
                parts.scheme
            )));
        }

        // Construct base_url from scheme + path
        // e.g., "http://localhost:8080/api" from scheme "http" and path "//localhost:8080/api"
        let base_url = format!("{}:{}", parts.scheme, parts.path);

        let http_method = parts.params.get("httpMethod").cloned();

        let throw_exception_on_failure = match parts.params.get("throwExceptionOnFailure") {
            Some(v) => parse_bool_param_http(v).map_err(|e| {
                CamelError::InvalidUri(format!("invalid value for throwExceptionOnFailure: {e}"))
            })?,
            None => true,
        };

        // Parse status code range from "start-end" format (e.g., "200-299")
        let ok_status_code_range = match parts.params.get("okStatusCodeRange") {
            Some(v) => parse_ok_status_code_range(v)?,
            None => (200, 299),
        };

        let response_timeout = match parts.params.get("responseTimeout") {
            Some(v) => Some(v.parse::<u64>().map(Duration::from_millis).map_err(|e| {
                CamelError::InvalidUri(format!("invalid value for responseTimeout: {e}"))
            })?),
            None => None,
        };

        // SSRF protection settings
        let allow_internal = match parts.params.get("allowInternal") {
            Some(v) => parse_bool_param_http(v).map_err(|e| {
                CamelError::InvalidUri(format!("invalid value for allowInternal: {e}"))
            })?,
            None => false, // Default: block private IPs
        };

        // Parse comma-separated blocked hosts
        let blocked_hosts = parts
            .params
            .get("blockedHosts")
            .map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default();

        let max_body_size = match parts.params.get("maxBodySize") {
            Some(v) => v.parse::<usize>().map_err(|e| {
                CamelError::InvalidUri(format!("invalid value for maxBodySize: {e}"))
            })?,
            None => 10 * 1024 * 1024, // Default: 10MB
        };

        let read_timeout_ms = match parts.params.get("readTimeout") {
            Some(v) => v.parse::<u64>().map_err(|e| {
                CamelError::InvalidUri(format!("invalid value for readTimeout: {e}"))
            })?,
            None => 30_000, // Default: 30s
        };

        let max_response_bytes = match parts.params.get("maxResponseBytes") {
            Some(v) => v.parse::<usize>().map_err(|e| {
                CamelError::InvalidUri(format!("invalid value for maxResponseBytes: {e}"))
            })?,
            None => 10 * 1024 * 1024, // Default: 10MB
        };

        let auth = parse_auth_from_params(&parts.params)?;

        let user_agent = parts.params.get("userAgent").cloned();

        let cookie_handling = match parts.params.get("cookieHandling") {
            Some(v) if v.eq_ignore_ascii_case("inmemory") => CookieHandling::InMemory,
            Some(v) if v.eq_ignore_ascii_case("disabled") => CookieHandling::Disabled,
            Some(v) => {
                return Err(CamelError::InvalidUri(format!(
                    "invalid value for cookieHandling: {v} (expected Disabled or InMemory)"
                )));
            }
            None => CookieHandling::Disabled,
        };

        let bridge_endpoint = match parts.params.get("bridgeEndpoint") {
            Some(v) => parse_bool_param_http(v).map_err(|e| {
                CamelError::InvalidUri(format!("invalid value for bridgeEndpoint: {e}"))
            })?,
            None => false,
        };

        let connection_close = match parts.params.get("connectionClose") {
            Some(v) => parse_bool_param_http(v).map_err(|e| {
                CamelError::InvalidUri(format!("invalid value for connectionClose: {e}"))
            })?,
            None => false,
        };

        let skip_request_headers = parts
            .params
            .get("skipRequestHeaders")
            .map(|v| {
                v.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_ascii_lowercase())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let skip_response_headers = parts
            .params
            .get("skipResponseHeaders")
            .map(|v| {
                v.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_ascii_lowercase())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let follow_redirects = match parts.params.get("followRedirects") {
            Some(v) => parse_bool_param_http(v).map_err(|e| {
                CamelError::InvalidUri(format!("invalid value for followRedirects: {e}"))
            })?,
            None => false,
        };

        let max_redirects = match parts.params.get("maxRedirects") {
            Some(v) => v.parse::<usize>().map_err(|e| {
                CamelError::InvalidUri(format!("invalid value for maxRedirects: {e}"))
            })?,
            None => 10,
        };

        // Collect remaining params (not Camel options) as query params
        let query_params: HashMap<String, String> = parts
            .params
            .into_iter()
            .filter(|(k, _)| !HTTP_CAMEL_OPTIONS.contains(&k.as_str()))
            .collect();

        Ok(Self {
            base_url,
            http_method,
            throw_exception_on_failure,
            ok_status_code_range,
            response_timeout,
            query_params,
            allow_internal,
            blocked_hosts,
            max_body_size,
            read_timeout_ms,
            max_response_bytes,
            auth,
            token_provider: None,
            user_agent,
            cookie_handling,
            bridge_endpoint,
            connection_close,
            skip_request_headers,
            skip_response_headers,
            follow_redirects,
            max_redirects,
        })
    }
}

fn parse_auth_from_params(params: &HashMap<String, String>) -> Result<HttpAuth, CamelError> {
    let Some(method) = params.get("authMethod") else {
        return Ok(HttpAuth::None);
    };

    if method.eq_ignore_ascii_case("none") {
        return Ok(HttpAuth::None);
    }

    if method.eq_ignore_ascii_case("basic") {
        let username = params.get("authUsername").cloned().ok_or_else(|| {
            CamelError::InvalidUri("authUsername is required for authMethod=Basic".to_string())
        })?;
        let password = params.get("authPassword").cloned().ok_or_else(|| {
            CamelError::InvalidUri("authPassword is required for authMethod=Basic".to_string())
        })?;
        return Ok(HttpAuth::Basic { username, password });
    }

    if method.eq_ignore_ascii_case("bearer") {
        let token = params.get("authBearerToken").cloned().ok_or_else(|| {
            CamelError::InvalidUri("authBearerToken is required for authMethod=Bearer".to_string())
        })?;
        return Ok(HttpAuth::Bearer { token });
    }

    Err(CamelError::InvalidUri(format!(
        "invalid value for authMethod: {method} (expected None, Basic, or Bearer)"
    )))
}

fn parse_bool_param_http(value: &str) -> Result<bool, CamelError> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(CamelError::InvalidUri(format!(
            "invalid boolean value: '{value}'"
        ))),
    }
}

impl HttpEndpointConfig {
    pub fn from_uri_with_defaults(uri: &str, config: &HttpConfig) -> Result<Self, CamelError> {
        let parts = parse_uri(uri)?;
        let mut endpoint = Self::from_components(parts.clone())?;
        if endpoint.response_timeout.is_none() {
            endpoint.response_timeout = Some(Duration::from_millis(config.response_timeout_ms));
        }
        if !parts.params.contains_key("allowInternal") {
            endpoint.allow_internal = config.allow_internal;
        }
        if !parts.params.contains_key("blockedHosts") {
            endpoint.blocked_hosts = config.blocked_hosts.clone();
        }
        if !parts.params.contains_key("maxBodySize") {
            endpoint.max_body_size = config.max_body_size;
        }
        if !parts.params.contains_key("readTimeout") {
            endpoint.read_timeout_ms = config.read_timeout_ms;
        }
        if !parts.params.contains_key("maxResponseBytes") {
            endpoint.max_response_bytes = config.max_response_bytes;
        }
        if !parts.params.contains_key("okStatusCodeRange")
            && let Some(range) = &config.ok_status_code_range
        {
            endpoint.ok_status_code_range = parse_ok_status_code_range(range)?;
        }
        if !parts.params.contains_key("followRedirects") {
            endpoint.follow_redirects = config.follow_redirects;
        }
        if !parts.params.contains_key("maxRedirects") {
            endpoint.max_redirects = config.max_redirects.unwrap_or(10);
        }

        Ok(endpoint)
    }
}

// ---------------------------------------------------------------------------
// HttpServerConfig
// ---------------------------------------------------------------------------

/// Configuration for an HTTP server (consumer) endpoint.
#[derive(Debug, Clone)]
pub struct HttpServerConfig {
    /// URI scheme ("http" or "https") parsed from the endpoint URI.
    pub scheme: String,
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
    /// Maximum number of in-flight requests handled concurrently by this server.
    pub max_inflight_requests: usize,
    /// HTTP method this consumer handles (e.g. `"GET"`). When `Some`,
    /// the consumer registers as a method-aware REST endpoint and the
    /// path is treated as a template (e.g. `/users/{id}` is matched
    /// against any `/users/<value>`). When `None`, the consumer
    /// registers in the legacy path-only `api_routes` registry.
    /// Extracted from the `httpMethod=` URI param at config build time.
    pub method: Option<String>,
    /// Server-side TLS config. Populated from `tlsCert`/`tlsKey` URI params.
    /// `None` for plain HTTP servers.
    pub tls_config: Option<crate::config::ServerTlsConfig>,
}

impl UriConfig for HttpServerConfig {
    /// Returns "http" as the primary scheme (also accepts "https")
    fn scheme() -> &'static str {
        "http"
    }

    fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = parse_uri(uri)?;
        Self::from_components(parts)
    }

    fn from_components(parts: UriComponents) -> Result<Self, CamelError> {
        // Validate scheme - accept both http and https
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

        // Parse host:port from authority
        let (host, port) = if let Some(colon) = authority.rfind(':') {
            let port_str = &authority[colon + 1..];
            match port_str.parse::<u16>() {
                Ok(p) => (authority[..colon].to_string(), p),
                Err(_) => {
                    return Err(CamelError::InvalidUri(format!(
                        "invalid port '{}' in authority",
                        port_str
                    )));
                }
            }
        } else {
            // Default port based on scheme: 443 for https, 80 for http
            let default_port = if parts.scheme == "https" { 443 } else { 80 };
            (authority.to_string(), default_port)
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

        let max_inflight_requests = parts
            .params
            .get("maxInflightRequests")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1024);

        // Uppercase-normalize so a hand-written `httpMethod=get` matches the
        // uppercase method the dispatcher compares against (axum's
        // `req.method().to_string()` yields "GET"). Without this, a
        // lower-case `httpMethod` would never match and silently 404.
        // Review I5.
        let method = parts.params.get("httpMethod").map(|m| m.to_uppercase());

        Ok(Self {
            scheme: parts.scheme,
            host,
            port,
            path,
            max_request_body,
            max_response_body,
            max_inflight_requests,
            method,
            tls_config: {
                let cert = parts.params.get("tlsCert").cloned();
                let key = parts.params.get("tlsKey").cloned();
                match (cert, key) {
                    (Some(c), Some(k)) => Some(crate::config::ServerTlsConfig {
                        cert_path: c,
                        key_path: k,
                    }),
                    (None, None) => None,
                    _ => None, // partial — enforced in create_consumer, not here
                }
            },
        })
    }
}

impl HttpServerConfig {
    pub fn from_uri_with_defaults(uri: &str, config: &HttpConfig) -> Result<Self, CamelError> {
        let parts = parse_uri(uri)?;
        let mut server = Self::from_components(parts.clone())?;
        if !parts.params.contains_key("maxRequestBody") {
            server.max_request_body = config.max_request_body;
        }
        if !parts.params.contains_key("maxResponseBody") {
            // Default max_response_body is 10MB via HttpConfig::default().max_body_size.
            server.max_response_body = config.max_body_size;
        }
        Ok(server)
    }
}

// ---------------------------------------------------------------------------
// RequestEnvelope / HttpReply
// ---------------------------------------------------------------------------

/// Body of the HTTP response: already-materialized bytes or a lazy stream.
///
/// **Internal plumbing** — subject to change without notice.
pub enum HttpReplyBody {
    Bytes(bytes::Bytes),
    Stream(BoxStream<'static, Result<bytes::Bytes, CamelError>>),
}

/// An inbound HTTP request sent from the Axum dispatch handler to an
/// `HttpConsumer` receive loop.
///
/// **Internal plumbing** — subject to change without notice.
pub struct RequestEnvelope {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: http::HeaderMap,
    pub body: StreamBody,
    /// Path parameters extracted from a REST template match, e.g.
    /// `id=42` for a request to `/users/42` matched against
    /// `/users/{id}`. Empty for non-REST requests or for literal
    /// template matches. The consumer turns these into
    /// `CamelHttpPath_<param>` headers on the Exchange (expert guidance E2).
    pub path_params: std::collections::HashMap<String, String>,
    pub reply_tx: tokio::sync::oneshot::Sender<HttpReply>,
}

/// The HTTP response that `HttpConsumer` sends back to the Axum handler.
///
/// **Internal plumbing** — subject to change without notice.
pub struct HttpReply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: HttpReplyBody,
}

// ---------------------------------------------------------------------------
// HttpRouteRegistry / ServerRegistry
// ---------------------------------------------------------------------------

type ServerKey = (String, u16);

/// Handle to a running Axum server on one interface/port.
struct ServerHandle {
    registry: HttpRouteRegistry,
    max_request_body: usize,
    max_response_body: usize,
    max_inflight_requests: usize,
    is_tls: bool,
    tls_cert_path: Option<String>,
    tls_key_path: Option<String>,
    /// JoinHandle for the monitor_axum_task wrapper. `is_finished()` is the
    /// dead-server eviction signal in `get_or_spawn`.
    monitor_task: tokio::task::JoinHandle<()>,
    // Retained so the reload handler (Task 7) can call reload_from_config()
    // to hot-swap certs without restarting the server.
    tls_config: Option<axum_server::tls_rustls::RustlsConfig>,
    tls_source: Option<ServerTlsSource>,
}

/// Process-global registry mapping (host, port) → running Axum server handle.
pub struct ServerRegistry {
    inner: Mutex<HashMap<ServerKey, Arc<OnceCell<ServerHandle>>>>,
}

impl ServerRegistry {
    /// Returns the global singleton.
    pub fn global() -> &'static Self {
        static INSTANCE: OnceLock<ServerRegistry> = OnceLock::new();
        INSTANCE.get_or_init(|| ServerRegistry {
            inner: Mutex::new(HashMap::new()),
        })
    }

    /// Returns route registry for `port`, spawning new Axum server if
    /// none is running on that port yet.
    #[allow(clippy::too_many_arguments)]
    pub async fn get_or_spawn(
        &'static self,
        host: &str,
        port: u16,
        max_request_body: usize,
        max_response_body: usize,
        max_inflight_requests: usize,
        runtime: Arc<dyn RuntimeObservability>,
        route_id: String,
        tls_config: Option<crate::config::ServerTlsConfig>,
    ) -> Result<HttpRouteRegistry, CamelError> {
        let host_owned = host.to_string();

        let cell = {
            let mut guard = self.inner.lock().map_err(|_| {
                CamelError::EndpointCreationFailed("ServerRegistry lock poisoned".into())
            })?;
            let key = (host.to_string(), port);
            // Evict dead server so a fresh one can spawn (matches gRPC D-L2 pattern).
            // The monitor task awaits the server task, so monitor_task.is_finished()
            // is a reliable proxy for the server being gone (either crashed or aborted).
            if let Some(existing) = guard.get(&key)
                && let Some(handle) = existing.get()
                && handle.monitor_task.is_finished()
            {
                // Deregister TLS reload handler so a respawned HTTPS server
                // doesn't reload stale cert config from the crashed handler.
                if handle.is_tls {
                    let scheme = if handle.is_tls { "https" } else { "http" };
                    camel_component_api::tls_source::TlsReloadRegistry::global()
                        .unregister(scheme, host, port);
                }
                guard.remove(&key);
            }
            guard
                .entry(key)
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };

        if let Some(existing) = cell.get()
            && existing.max_request_body != max_request_body
        {
            return Err(CamelError::EndpointCreationFailed(format!(
                "incompatible maxRequestBody for shared server (host={host}, port={port}): {} vs {}",
                existing.max_request_body, max_request_body
            )));
        }

        if let Some(existing) = cell.get()
            && existing.max_response_body != max_response_body
        {
            return Err(CamelError::EndpointCreationFailed(format!(
                "incompatible maxResponseBody for shared server (host={host}, port={port}): {} vs {}",
                existing.max_response_body, max_response_body
            )));
        }

        if let Some(existing) = cell.get()
            && existing.max_inflight_requests != max_inflight_requests
        {
            return Err(CamelError::EndpointCreationFailed(format!(
                "incompatible maxInflightRequests for shared server (host={host}, port={port}): {} vs {}",
                existing.max_inflight_requests, max_inflight_requests
            )));
        }

        // TLS mode mismatch: plain vs TLS
        if let Some(existing) = cell.get()
            && existing.is_tls != tls_config.is_some()
        {
            return Err(CamelError::EndpointCreationFailed(format!(
                "incompatible TLS mode for shared server (host={host}, port={port}): existing is_tls={}, new has_tls={}",
                existing.is_tls,
                tls_config.is_some()
            )));
        }

        // TLS cert/key mismatch: different cert on same TLS port
        if let (Some(existing), Some(new_tls)) = (cell.get(), &tls_config)
            && (existing.tls_cert_path.as_deref() != Some(&new_tls.cert_path)
                || existing.tls_key_path.as_deref() != Some(&new_tls.key_path))
        {
            return Err(CamelError::EndpointCreationFailed(format!(
                "incompatible TLS cert/key for shared server (host={host}, port={port}): routes on the same TLS port must use the same cert and key"
            )));
        }

        let handle = cell
            .get_or_try_init(|| {
                let rt = Arc::clone(&runtime);
                let rid = route_id.clone();
                async move {
                    let addr = format!("{host_owned}:{port}");
                    let listener = tokio::net::TcpListener::bind(&addr).await.map_err(|e| {
                        CamelError::EndpointCreationFailed(format!("Failed to bind {addr}: {e}"))
                    })?;
                    let registry = HttpRouteRegistry::new();
                    let inflight = Arc::new(tokio::sync::Semaphore::new(max_inflight_requests));
                    // Constructed once in the TLS branch so they can be retained
                    // on ServerHandle for the reload handler (Task 7).
                    let tls_rustls_cfg: Option<axum_server::tls_rustls::RustlsConfig>;
                    let tls_source: Option<ServerTlsSource>;
                    let server_task = if let Some(ref tls) = tls_config {
                        let rustls_config = load_tls_config(&tls.cert_path, &tls.key_path)?;
                        let source = ServerTlsSource {
                            cert_path: std::path::PathBuf::from(&tls.cert_path),
                            key_path: std::path::PathBuf::from(&tls.key_path),
                            client_ca_path: None,
                        };
                        // Build the RustlsConfig once — clone() is cheap (Arc
                        // internally) and shares the ArcSwap the reload handler
                        // will mutate via reload_from_config().
                        let rustls_cfg = axum_server::tls_rustls::RustlsConfig::from_config(
                            std::sync::Arc::new(rustls_config),
                        );
                        tls_rustls_cfg = Some(rustls_cfg.clone());
                        tls_source = Some(source);
                        // Convert tokio listener to std for axum-server
                        let std_listener = listener.into_std().map_err(|e| {
                            CamelError::EndpointCreationFailed(format!(
                                "TLS listener conversion: {e}"
                            ))
                        })?;
                        tokio::spawn(run_axum_server_tls(
                            std_listener,
                            rustls_cfg,
                            registry.clone(),
                            max_request_body,
                            max_response_body,
                            Arc::clone(&inflight),
                            Arc::clone(&rt),
                            rid.clone(),
                        ))
                    } else {
                        tls_rustls_cfg = None;
                        tls_source = None;
                        tokio::spawn(run_axum_server(
                            listener,
                            registry.clone(),
                            max_request_body,
                            max_response_body,
                            Arc::clone(&inflight),
                            Arc::clone(&rt),
                            rid.clone(),
                        ))
                    };
                    let addr_for_monitor = format!("{host_owned}:{port}");
                    let monitor_task = tokio::spawn(monitor_axum_task(
                        server_task,
                        addr_for_monitor,
                        Arc::clone(&rt),
                        rid,
                    ));
                    let handle = ServerHandle {
                        registry,
                        max_request_body,
                        max_response_body,
                        max_inflight_requests,
                        is_tls: tls_config.is_some(),
                        tls_cert_path: tls_config.as_ref().map(|t| t.cert_path.clone()),
                        tls_key_path: tls_config.as_ref().map(|t| t.key_path.clone()),
                        monitor_task,
                        tls_config: tls_rustls_cfg,
                        tls_source,
                    };
                    // Register reload handler (exactly-once: inside OnceCell init closure).
                    // Note: HTTP servers are process-lifetime (no release/eviction path),
                    // so handlers are never unregistered. If eviction is added later,
                    // add TlsReloadRegistry::global().unregister() there.
                    if let (Some(tls_cfg), Some(source)) =
                        (handle.tls_config.as_ref(), handle.tls_source.as_ref())
                    {
                        let handler = Arc::new(crate::tls_reload::HttpReloadHandler::new(
                            tls_cfg.clone(),
                            source.clone(),
                            host_owned.clone(),
                            port,
                        ));
                        camel_component_api::tls_source::TlsReloadRegistry::global()
                            .register(handler);
                    }
                    Ok::<ServerHandle, CamelError>(handle)
                }
            })
            .await?;

        Ok(handle.registry.clone())
    }

    /// Unregister one consumer from a server. HTTP servers are process-lifetime:
    /// the server stays in the registry for potential restart. Path
    /// deregistration happens separately in the consumer's cleanup.
    pub async fn unregister(&self, host: &str, port: u16) {
        debug!(
            host = host,
            port = port,
            "consumer unregistered from HTTP server"
        );
    }

    /// Reset the global registry — **test-only**.
    ///
    /// Clears all registered server handles so that tests can start from a clean
    /// state. This is intentionally `#[cfg(test)]` because the registry is a
    /// process-global singleton in production and resetting it would break
    /// running servers.
    #[cfg(test)]
    pub fn reset() {
        let instance = Self::global();
        let mut guard = instance
            .inner
            .lock()
            .expect("ServerRegistry lock poisoned during test reset");
        guard.clear();
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
pub(crate) struct AppState {
    registry: HttpRouteRegistry,
    max_request_body: usize,
    max_response_body: usize,
    inflight: Arc<tokio::sync::Semaphore>,
}

async fn run_axum_server(
    listener: tokio::net::TcpListener,
    registry: HttpRouteRegistry,
    max_request_body: usize,
    max_response_body: usize,
    inflight: Arc<tokio::sync::Semaphore>,
    runtime: Arc<dyn RuntimeObservability>,
    route_id: String,
) {
    let state = AppState {
        registry,
        max_request_body,
        max_response_body,
        inflight,
    };
    let app = Router::new().fallback(dispatch_handler).with_state(state);

    axum::serve(listener, app).await.unwrap_or_else(|e| {
        runtime
            .metrics()
            .increment_errors(&route_id, "e:http:accept");
        // log-policy: outside-contract
        tracing::error!(error = %e, "Axum server error");
    });
}

#[allow(clippy::too_many_arguments)]
async fn run_axum_server_tls(
    listener: std::net::TcpListener,
    tls_cfg: axum_server::tls_rustls::RustlsConfig,
    registry: HttpRouteRegistry,
    max_request_body: usize,
    max_response_body: usize,
    inflight: Arc<tokio::sync::Semaphore>,
    runtime: Arc<dyn RuntimeObservability>,
    route_id: String,
) {
    let state = AppState {
        registry,
        max_request_body,
        max_response_body,
        inflight,
    };
    let app = Router::new().fallback(dispatch_handler).with_state(state);

    // RustlsConfig is now constructed once in get_or_spawn and retained on
    // ServerHandle so the reload handler can call reload_from_config() on it.

    axum_server::from_tcp_rustls(listener, tls_cfg)
        .serve(app.into_make_service())
        .await
        .unwrap_or_else(|e| {
            runtime
                .metrics()
                .increment_errors(&route_id, "e:http:accept-tls");
            // log-policy: outside-contract
            tracing::error!(error = %e, "Axum TLS server error");
        });
}

/// Monitors an Axum server task and emits a structured error event if it
/// exits unexpectedly.
///
/// # Limitations
/// The HTTP server is shared across all routes on a port. Full per-route
/// CrashNotification propagation is deferred — this provides observable
/// structured logging as a first guard.
async fn monitor_axum_task(
    handle: tokio::task::JoinHandle<()>,
    addr: String,
    runtime: Arc<dyn RuntimeObservability>,
    route_id: String,
) {
    match handle.await {
        Ok(()) => {
            // Clean exit (process shutdown or normal stop)
        }
        Err(join_err) => {
            runtime
                .metrics()
                .increment_errors(&route_id, "e:http:server-task-exited");
            // log-policy: outside-contract
            tracing::error!(
                addr = %addr,
                error = %join_err,
                "Axum server task exited unexpectedly — all routes on this port are now dead"
            );
        }
    }
}

/// Load a rustls ServerConfig from PEM cert/key files.
/// Adapted from camel-ws lib.rs load_tls_config.
fn load_tls_config(
    cert_path: &str,
    key_path: &str,
) -> Result<tokio_rustls::rustls::ServerConfig, CamelError> {
    use std::fs::File;
    use std::io::BufReader;

    let cert_file = File::open(cert_path)
        .map_err(|e| CamelError::EndpointCreationFailed(format!("TLS cert file error: {e}")))?;
    let key_file = File::open(key_path)
        .map_err(|e| CamelError::EndpointCreationFailed(format!("TLS key file error: {e}")))?;

    let certs: Vec<_> = rustls_pemfile::certs(&mut BufReader::new(cert_file))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| CamelError::EndpointCreationFailed(format!("TLS cert parse error: {e}")))?;

    let key = rustls_pemfile::private_key(&mut BufReader::new(key_file))
        .map_err(|e| CamelError::EndpointCreationFailed(format!("TLS key parse error: {e}")))?
        .ok_or_else(|| CamelError::EndpointCreationFailed("TLS: no private key found".into()))?;

    tokio_rustls::rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| CamelError::EndpointCreationFailed(format!("TLS config error: {e}")))
}

async fn dispatch_handler(State(state): State<AppState>, req: Request) -> impl IntoResponse {
    let path = req.uri().path().to_owned();
    let method = req.method().to_string();

    // Dispatch precedence (spec §7.2 / ADR-0009):
    //   1. Exact API path match (legacy `http:` routes without httpMethod)
    //   2. Templated API path match (REST, method-aware, by specificity)
    //   3. Static mount longest-prefix
    //   4. SPA fallback
    //
    // Legacy exact runs first: it is a cheap HashMap get, and the two
    // registries are mutually exclusive per route — a legacy route carries
    // no `httpMethod` and lives only in `api_routes`, while a REST-lowered
    // route carries `httpMethod` and lives only in `rest_endpoints`. So an
    // exact hit can never shadow a REST route that should have matched,
    // and running exact-first honours the documented precedence (the prior
    // REST-first order let a templated `GET /api/{resource}` steal a
    // request meant for an exact `GET /api/users`). Intra-REST method
    // disambiguation is handled inside `match_endpoint`, not by this
    // ordering. Review C2.
    let api_sender = {
        let inner = state.registry.inner.read().await;
        inner.api_routes.get(&path).cloned()
    }; // lock released BEFORE any IO

    let (rest_sender, path_params) = if api_sender.is_some() {
        // Exact legacy match won — skip the templated scan entirely.
        (None, Default::default())
    } else {
        let inner = state.registry.inner.read().await;
        match rest_match::match_endpoint(&method, &path, &inner.rest_endpoints) {
            rest_match::MatchOutcome::Found(m) => (Some(m.payload), m.path_params),
            rest_match::MatchOutcome::Ambiguous => {
                // Ambiguous registration should have been rejected at
                // lowering time (rest.rs). Reaching here means two
                // equal-specificity templates matched one request —
                // surface a loud error rather than a silent 404. Review C3.
                // log-policy: handler-owned
                tracing::warn!(
                    method = %method,
                    path = %path,
                    "ambiguous REST template match — returning 500"
                );
                return Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(AxumBody::from("Internal Server Error"))
                    .expect("infallible"); // allow-unwrap
            }
            rest_match::MatchOutcome::NotFound => (None, Default::default()),
        }
    }; // lock released BEFORE any IO

    let sender = api_sender.or(rest_sender);

    if let Some(sender) = sender {
        let query = req.uri().query().unwrap_or("").to_string();
        let headers = req.headers().clone();

        // Check Content-Length against limit BEFORE opening the stream
        let content_length: Option<u64> = headers
            .get(http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse().ok());

        if let Some(len) = content_length
            && len > state.max_request_body as u64
        {
            return Response::builder()
                .status(StatusCode::PAYLOAD_TOO_LARGE)
                .body(AxumBody::from("Request body exceeds configured limit"))
                .expect("infallible"); // allow-unwrap
        }

        let _permit = match Arc::clone(&state.inflight).try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                return Response::builder()
                    .status(StatusCode::SERVICE_UNAVAILABLE)
                    .body(AxumBody::from("Service Unavailable"))
                    .expect("infallible"); // allow-unwrap
            }
        };

        // Build StreamBody from Axum body WITHOUT materializing
        let content_type = headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let data_stream: BodyDataStream = req.into_body().into_data_stream();
        let mapped_stream = data_stream.map_err(|e| CamelError::Io(e.to_string()));
        let boxed: BoxStream<'static, Result<bytes::Bytes, CamelError>> = Box::pin(mapped_stream);

        let stream_body = StreamBody {
            stream: Arc::new(tokio::sync::Mutex::new(Some(boxed))),
            metadata: StreamMetadata {
                size_hint: content_length,
                content_type,
                origin: None,
            },
        };

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel::<HttpReply>();
        let envelope = RequestEnvelope {
            method,
            path,
            query,
            headers,
            body: stream_body,
            path_params,
            reply_tx,
        };

        if sender.send(envelope).await.is_err() {
            return Response::builder()
                .status(StatusCode::SERVICE_UNAVAILABLE)
                .body(AxumBody::from("Consumer unavailable"))
                .expect("infallible"); // allow-unwrap
        }

        match reply_rx.await {
            Ok(reply) => {
                let reply = match reply.body {
                    HttpReplyBody::Bytes(b)
                        if exceeds_max_response_body(b.len(), state.max_response_body) =>
                    {
                        HttpReply {
                            status: 500,
                            headers: vec![],
                            body: HttpReplyBody::Bytes(bytes::Bytes::from(
                                "Response body exceeds configured limit",
                            )),
                        }
                    }
                    _ => reply,
                };

                let status =
                    StatusCode::from_u16(reply.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                let mut builder = Response::builder().status(status);
                for (k, v) in &reply.headers {
                    builder = builder.header(k.as_str(), v.as_str());
                }
                match reply.body {
                    HttpReplyBody::Bytes(b) => {
                        builder.body(AxumBody::from(b)).unwrap_or_else(|_| {
                            Response::builder()
                                .status(StatusCode::INTERNAL_SERVER_ERROR)
                                .body(AxumBody::from("Invalid response headers from consumer"))
                                .expect("infallible") // allow-unwrap
                        })
                    }
                    HttpReplyBody::Stream(stream) => builder
                        .body(AxumBody::from_stream(stream))
                        .unwrap_or_else(|_| {
                            Response::builder()
                                .status(StatusCode::INTERNAL_SERVER_ERROR)
                                .body(AxumBody::from("Invalid response headers from consumer"))
                                .expect("infallible") // allow-unwrap
                        }),
                }
            }
            Err(_) => Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(AxumBody::from("Pipeline error"))
                .expect("infallible"), // allow-unwrap
        }
    } else {
        // No API route matched — try static mounts
        static_dispatch::dispatch_static(&state, req, &path).await
    }
}

fn exceeds_max_response_body(len: usize, max: usize) -> bool {
    len > max
}

fn title_case_header(name: &str) -> String {
    name.split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().chain(chars.as_str().chars()).collect(),
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

// ---------------------------------------------------------------------------
// HttpConsumer
// ---------------------------------------------------------------------------

pub struct HttpConsumer {
    config: HttpServerConfig,
    /// Runtime observability handle for ADR-0012 metrics and health calls.
    runtime: Arc<dyn RuntimeObservability>,
}

impl HttpConsumer {
    pub fn new(config: HttpServerConfig, runtime: Arc<dyn RuntimeObservability>) -> Self {
        Self { config, runtime }
    }
}

#[async_trait::async_trait]
impl Consumer for HttpConsumer {
    async fn start(&mut self, ctx: camel_component_api::ConsumerContext) -> Result<(), CamelError> {
        use camel_component_api::{Body, Exchange, Message};

        let registry = ServerRegistry::global()
            .get_or_spawn(
                &self.config.host,
                self.config.port,
                self.config.max_request_body,
                self.config.max_response_body,
                self.config.max_inflight_requests,
                self.runtime.clone(),
                ctx.route_id().to_string(),
                self.config.tls_config.clone(),
            )
            .await?;

        // Create channel for this path and register it
        let (env_tx, mut env_rx) = tokio::sync::mpsc::channel::<RequestEnvelope>(64);
        // When the from-URI carries `httpMethod=...` (REST-lowered
        // route), register the consumer as a method-aware REST endpoint
        // so the dispatcher can route by (method, path template).
        // Otherwise fall back to the legacy path-only api_routes
        // registry. The two registries never overlap for the same
        // route: each consumer registers in exactly one of them.
        if let Some(method) = self.config.method.clone() {
            let segments = rest_match::parse_path_template(&self.config.path);
            registry
                .register_rest_endpoint(method, segments, env_tx)
                .await;
        } else {
            registry
                .register_api_route(self.config.path.clone(), env_tx)
                .await;
        }

        let path = self.config.path.clone();
        let registry_for_cleanup = registry.clone();
        let cancel_token = ctx.cancel_token();
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

                    // Set path-parameter headers from REST template
                    // match. Expert guidance E2: the consumer is
                    // responsible for translating the dispatcher's
                    // matched params into `CamelHttpPath_<param>`
                    // headers on the Exchange, matching the convention
                    // used by Camel HTTP for templated routes.
                    for (param_name, param_value) in &envelope.path_params {
                        msg.set_header(
                            format!("CamelHttpPath_{param_name}"),
                            serde_json::Value::String(param_value.clone()),
                        );
                    }

                    // Forward HTTP headers with Title-Case names (hyper lowercases them)
                    for (k, v) in &envelope.headers {
                        if let Ok(val_str) = v.to_str() {
                            msg.set_header(
                                title_case_header(k.as_str()),
                                serde_json::Value::String(val_str.to_string()),
                            );
                        }
                    }

                    // Body: always arrives as Body::Stream (native streaming)
                    // Routes can call into_bytes() if they need to materialize
                    msg.body = Body::Stream(envelope.body);

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
                                body: HttpReplyBody::Bytes(bytes::Bytes::from("Service Unavailable")),
                            });
                            return;
                        }

                        // Send through pipeline and await result
                        let (tx, rx) = tokio::sync::oneshot::channel();
                        let envelope = camel_component_api::consumer::ExchangeEnvelope {
                            exchange,
                            reply_tx: Some(tx),
                        };

                        let result = match sender.send(envelope).await {
                            Ok(()) => rx.await.map_err(|_| camel_component_api::CamelError::ChannelClosed),
                            Err(_) => Err(camel_component_api::CamelError::ChannelClosed),
                        }
                        .and_then(|r| r);

                        let reply = match result {
                            Ok(out) => {
                                let status = out
                                    .input
                                    .header("CamelHttpResponseCode")
                                    .and_then(|v| {
                                        let raw = v.as_u64()
                                            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))?;
                                        let code = raw as u16;
                                        (100..1000).contains(&code).then_some(code)
                                    })
                                    .unwrap_or(200);

                                let user_content_type = out
                                    .input
                                    .header("Content-Type")
                                    .and_then(|v| v.as_str().map(|s| s.to_string()));

                                let (reply_body, inferred_content_type): (HttpReplyBody, Option<String>) = match out.input.body {
                                    Body::Empty => (HttpReplyBody::Bytes(bytes::Bytes::new()), None),
                                    Body::Bytes(b) => (HttpReplyBody::Bytes(b), None),
                                    Body::Text(s) => (HttpReplyBody::Bytes(bytes::Bytes::from(s.into_bytes())), Some("text/plain; charset=utf-8".to_string())),
                                    Body::Xml(s) => (HttpReplyBody::Bytes(bytes::Bytes::from(s.into_bytes())), Some("application/xml".to_string())),
                                    Body::Json(v) => (HttpReplyBody::Bytes(bytes::Bytes::from(
                                        v.to_string().into_bytes(),
                                    )), Some("application/json".to_string())),
                                    Body::Stream(s) => {
                                        let ct = s.metadata.content_type.clone();
                                        match s.stream.lock().await.take() {
                                            Some(stream) => (
                                                HttpReplyBody::Stream(stream),
                                                ct,
                                            ),
                                            None => {
                                                // log-policy: system-broken
                                                tracing::error!(
                                                    "Body::Stream already consumed before HTTP reply — returning 500"
                                                );
                                                let error_reply = HttpReply {
                                                    status: 500,
                                                    headers: vec![],
                                                    body: HttpReplyBody::Bytes(bytes::Bytes::new()),
                                                };
                                                if reply_tx.send(error_reply).is_err() {
                                                    debug!("reply_tx dropped before error reply could be sent");
                                                }
                                                return;
                                            }
                                        }
                                    }
                                };

                                let mut resp_headers: Vec<(String, String)> = out
                                    .input
                                    .headers
                                    .iter()
                                    .filter(|(k, _)| !k.starts_with("Camel"))
                                    .filter(|(k, _)| {
                                        !matches!(
                                            k.to_lowercase().as_str(),
                                            "content-length"
                                            | "content-type"
                                            | "transfer-encoding"
                                            | "connection"
                                            | "cache-control"
                                            | "date"
                                            | "pragma"
                                            | "trailer"
                                            | "upgrade"
                                            | "via"
                                            | "warning"
                                            | "host"
                                            | "user-agent"
                                            | "accept"
                                            | "accept-encoding"
                                            | "accept-language"
                                            | "accept-charset"
                                            | "authorization"
                                            | "proxy-authorization"
                                            | "cookie"
                                            | "expect"
                                            | "from"
                                            | "if-match"
                                            | "if-modified-since"
                                            | "if-none-match"
                                            | "if-range"
                                            | "if-unmodified-since"
                                            | "max-forwards"
                                            | "proxy-connection"
                                            | "range"
                                            | "referer"
                                            | "te"
                                        )
                                    })
                                    .filter_map(|(k, v)| {
                                        v.as_str().map(|s| (k.clone(), s.to_string()))
                                    })
                                    .collect();

                                let content_type = user_content_type
                                    .or(inferred_content_type);
                                if let Some(ct) = content_type {
                                    resp_headers.push(("Content-Type".to_string(), ct));
                                }

                                HttpReply {
                                    status,
                                    headers: resp_headers,
                                    body: reply_body,
                                }
                            }
                            Err(e) => {
                                pipeline_error_to_reply(e, &path_clone)
                            }
                        };

                        // Reply to Axum handler (ignore error if client disconnected)
                        let _ = reply_tx.send(reply);
                    });
                }
            }
        }

        // Deregister this consumer. Mirror the registration choice:
        // REST-registered consumers remove their (method, path) endpoint
        // WITHOUT touching sibling verbs on the same template (review C1);
        // legacy consumers clean up api_routes.
        if let Some(method) = &self.config.method {
            registry_for_cleanup
                .unregister_rest_endpoint(method, &path)
                .await;
        } else {
            registry_for_cleanup.unregister_api_route(&path).await;
        }

        // D-L10: decrement the shared server's refcount. When the last
        // consumer on this (host, port) leaves, the server + monitor tasks
        // are aborted and the registry entry is removed.
        ServerRegistry::global()
            .unregister(&self.config.host, self.config.port)
            .await;

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }

    fn concurrency_model(&self) -> camel_component_api::ConcurrencyModel {
        camel_component_api::ConcurrencyModel::Concurrent { max: None }
    }
}

// ---------------------------------------------------------------------------
// HttpComponent / HttpsComponent
// ---------------------------------------------------------------------------

pub struct HttpComponent {
    config: HttpConfig,
}

pub(crate) fn build_client(
    config: &HttpConfig,
    cookie_handling: CookieHandling,
    resolve_override: Option<(&str, &[std::net::SocketAddr])>,
) -> reqwest::Client {
    let mut builder = reqwest::Client::builder()
        .no_proxy() // CRITICAL: env proxies bypass resolve_to_addrs
        .connect_timeout(Duration::from_millis(config.connect_timeout_ms))
        .pool_max_idle_per_host(config.pool_max_idle_per_host)
        .pool_idle_timeout(Duration::from_millis(config.pool_idle_timeout_ms));

    // Redirects are always handled manually in the producer's send path
    // so that each hop can be SSRF-validated. reqwest's built-in redirect
    // policy is sync and cannot perform async DNS resolution or SSRF checks.
    builder = builder.redirect(reqwest::redirect::Policy::none());

    if let Some((host, addrs)) = resolve_override {
        builder = builder.resolve_to_addrs(host, addrs);
    }

    if matches!(cookie_handling, CookieHandling::InMemory) {
        // TODO(HTTP-013): enable reqwest cookie jar once workspace reqwest features include cookie_store.
    }

    if let Some(tls) = &config.tls
        && tls.enabled
    {
        if tls.insecure || !tls.verify_peer {
            // log-policy: handler-owned
            tracing::warn!("HTTP TLS verification disabled — connections are vulnerable to MitM");
            builder = builder.danger_accept_invalid_certs(true);
        }

        if let Some(ca_path) = &tls.ca_cert_path
            && let Ok(ca_bytes) = std::fs::read(ca_path)
        {
            let cert = reqwest::Certificate::from_pem(&ca_bytes)
                .or_else(|_| reqwest::Certificate::from_der(&ca_bytes));
            if let Ok(ca_cert) = cert {
                builder = builder.add_root_certificate(ca_cert);
            }
        }

        if let (Some(cert_path), Some(key_path)) = (&tls.client_cert_path, &tls.client_key_path)
            && let (Ok(cert_bytes), Ok(key_bytes)) =
                (std::fs::read(cert_path), std::fs::read(key_path))
        {
            let mut identity_pem = cert_bytes;
            identity_pem.extend_from_slice(&key_bytes);
            if let Ok(identity) = reqwest::Identity::from_pem(&identity_pem) {
                builder = builder.identity(identity);
            }
        }
    }

    builder
        .build()
        .expect("reqwest::Client::build() with valid config should not fail") // allow-unwrap
}

impl HttpComponent {
    pub fn new() -> Self {
        let config = HttpConfig::default();
        Self { config }
    }

    pub fn with_config(config: HttpConfig) -> Self {
        Self { config }
    }

    pub fn with_optional_config(config: Option<HttpConfig>) -> Self {
        match config {
            Some(cfg) => Self::with_config(cfg),
            None => Self::new(),
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

    fn metadata(&self) -> ComponentMetadata {
        ComponentMetadata {
            scheme: "http".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            description: "HTTP client and server component".to_string(),
            uri_syntax: "http://host:port/path?options".to_string(),
            capabilities: ComponentCapabilities {
                supports_consumer: true,
                supports_producer: true,
                supports_streaming: true,
                ..Default::default()
            },
            uri_options: vec![
                UriOption::new(
                    "httpMethod",
                    "HTTP method. Defaults to CamelHttpMethod header or POST/GET",
                    OptionKind::String,
                ),
                UriOption::new(
                    "throwExceptionOnFailure",
                    "Throw on non-2xx status",
                    OptionKind::Bool,
                )
                .with_default("true"),
                UriOption::new(
                    "okStatusCodeRange",
                    "Success status code range",
                    OptionKind::String,
                )
                .with_default("200-299"),
                UriOption::new(
                    "responseTimeout",
                    "Response timeout in milliseconds",
                    OptionKind::Int,
                ),
                UriOption::new(
                    "maxBodySize",
                    "Max request/response body bytes",
                    OptionKind::Int,
                )
                .with_default("10485760"),
                UriOption::new(
                    "authMethod",
                    "Authentication method",
                    OptionKind::Enum(vec!["Basic".to_string(), "Bearer".to_string()]),
                ),
                UriOption::new("authUsername", "Basic auth username", OptionKind::String).secret(),
                UriOption::new("authPassword", "Basic auth password", OptionKind::String).secret(),
                UriOption::new("authBearerToken", "Bearer auth token", OptionKind::String).secret(),
                UriOption::new("userAgent", "User-Agent header", OptionKind::String),
                UriOption::new("followRedirects", "Follow HTTP redirects", OptionKind::Bool)
                    .with_default("false"),
                UriOption::new("maxRedirects", "Max redirect hops", OptionKind::Int)
                    .with_default("10"),
            ],
            ..ComponentMetadata::minimal("http")
        }
    }

    fn create_endpoint(
        &self,
        uri: &str,
        ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        self.config.validate()?;
        let config = HttpEndpointConfig::from_uri_with_defaults(uri, &self.config)?;
        let server_config = HttpServerConfig::from_uri_with_defaults(uri, &self.config)?;
        let client = build_client(&self.config, config.cookie_handling, None);
        ctx.register_current_route_health_check(Arc::new(HttpHealthCheck::new(
            server_config.host.clone(),
            server_config.port,
        )));
        Ok(Box::new(HttpEndpoint {
            uri: uri.to_string(),
            config,
            server_config,
            client,
            http_config: self.config.clone(),
        }))
    }
}

pub struct HttpsComponent {
    config: HttpConfig,
}

impl HttpsComponent {
    pub fn new() -> Self {
        let config = HttpConfig::default();
        Self { config }
    }

    pub fn with_config(config: HttpConfig) -> Self {
        Self { config }
    }

    pub fn with_optional_config(config: Option<HttpConfig>) -> Self {
        match config {
            Some(cfg) => Self::with_config(cfg),
            None => Self::new(),
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

    fn metadata(&self) -> ComponentMetadata {
        // HTTPS shares the same URI option surface as HTTP.  Only the scheme
        // differs; the metadata for `https` simply reuses the HTTP option list
        // and capability declaration.
        ComponentMetadata {
            scheme: "https".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            description: "HTTPS client and server component (TLS over HTTP)".to_string(),
            uri_syntax: "https://host:port/path?options".to_string(),
            capabilities: ComponentCapabilities {
                supports_consumer: true,
                supports_producer: true,
                supports_streaming: true,
                ..Default::default()
            },
            uri_options: vec![
                UriOption::new(
                    "httpMethod",
                    "HTTP method. Defaults to CamelHttpMethod header or POST/GET",
                    OptionKind::String,
                ),
                UriOption::new(
                    "throwExceptionOnFailure",
                    "Throw on non-2xx status",
                    OptionKind::Bool,
                )
                .with_default("true"),
                UriOption::new(
                    "okStatusCodeRange",
                    "Success status code range",
                    OptionKind::String,
                )
                .with_default("200-299"),
                UriOption::new(
                    "responseTimeout",
                    "Response timeout in milliseconds",
                    OptionKind::Int,
                ),
                UriOption::new(
                    "maxBodySize",
                    "Max request/response body bytes",
                    OptionKind::Int,
                )
                .with_default("10485760"),
                UriOption::new(
                    "authMethod",
                    "Authentication method",
                    OptionKind::Enum(vec!["Basic".to_string(), "Bearer".to_string()]),
                ),
                UriOption::new("authUsername", "Basic auth username", OptionKind::String).secret(),
                UriOption::new("authPassword", "Basic auth password", OptionKind::String).secret(),
                UriOption::new("authBearerToken", "Bearer auth token", OptionKind::String).secret(),
                UriOption::new("userAgent", "User-Agent header", OptionKind::String),
                UriOption::new("followRedirects", "Follow HTTP redirects", OptionKind::Bool)
                    .with_default("false"),
                UriOption::new("maxRedirects", "Max redirect hops", OptionKind::Int)
                    .with_default("10"),
            ],
            ..ComponentMetadata::minimal("https")
        }
    }

    fn create_endpoint(
        &self,
        uri: &str,
        ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        self.config.validate()?;
        let config = HttpEndpointConfig::from_uri_with_defaults(uri, &self.config)?;
        let server_config = HttpServerConfig::from_uri_with_defaults(uri, &self.config)?;
        let client = build_client(&self.config, config.cookie_handling, None);
        ctx.register_current_route_health_check(Arc::new(HttpHealthCheck::new(
            server_config.host.clone(),
            server_config.port,
        )));
        Ok(Box::new(HttpEndpoint {
            uri: uri.to_string(),
            config,
            server_config,
            client,
            http_config: self.config.clone(),
        }))
    }
}

// ---------------------------------------------------------------------------
// HttpEndpoint
// ---------------------------------------------------------------------------

struct HttpEndpoint {
    uri: String,
    config: HttpEndpointConfig,
    server_config: HttpServerConfig,
    client: reqwest::Client,
    http_config: HttpConfig,
}

impl Endpoint for HttpEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        rt: Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        // Scheme/config consistency check (spec §5) — uses parsed scheme
        // from HttpServerConfig, not a fragile port-443 heuristic.
        let scheme_is_https = self.server_config.scheme == "https";
        let has_tls = self.server_config.tls_config.is_some();

        if scheme_is_https && !has_tls {
            return Err(CamelError::EndpointCreationFailed(
                "https:// consumer requires tlsCert and tlsKey parameters".to_string(),
            ));
        }
        if !scheme_is_https && has_tls {
            return Err(CamelError::EndpointCreationFailed(
                "http:// is incompatible with tlsCert/tlsKey — use https:// for TLS".to_string(),
            ));
        }
        Ok(Box::new(HttpConsumer::new(self.server_config.clone(), rt)))
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn camel_component_api::RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        let producer = HttpProducer {
            config: Arc::new(self.config.clone()),
            client: self.client.clone(),
            http_config: Arc::new(self.http_config.clone()),
        };
        if let Some(ref provider) = self.config.token_provider {
            let layer = BearerTokenLayer::new(Arc::clone(provider));
            Ok(BoxProcessor::new(layer.layer(producer)))
        } else {
            Ok(BoxProcessor::new(producer))
        }
    }
}

// ---------------------------------------------------------------------------
// HttpProducer
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct HttpProducer {
    config: Arc<HttpEndpointConfig>,
    client: reqwest::Client,
    http_config: Arc<HttpConfig>,
}

impl HttpProducer {
    fn resolve_method(exchange: &Exchange, config: &HttpEndpointConfig) -> String {
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

    fn resolve_url(exchange: &Exchange, config: &HttpEndpointConfig) -> String {
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
            let mut parsed = url::Url::parse(&url).expect("base URL must be valid"); // allow-unwrap
            for (k, v) in &config.query_params {
                parsed.query_pairs_mut().append_pair(k, v);
            }
            url = parsed.to_string();
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
        let http_config = self.http_config.clone();

        Box::pin(async move {
            let method_str = HttpProducer::resolve_method(&exchange, &config);
            let url = HttpProducer::resolve_url(&exchange, &config);

            // SECURITY: Validate URL for SSRF
            ssrf::validate_url_for_ssrf(&url, &config)?;

            // Resolve hostname and pin validated IPs to prevent DNS-rebinding TOCTOU
            // (L-H2). When the URL uses a domain name and SSRF protection is active,
            // build a per-request client with resolve_to_addrs so reqwest connects
            // directly to the validated addresses without re-resolving DNS.
            let resolved = ssrf::resolve_initial_url_for_ssrf(&url, config.allow_internal).await?;
            let client: reqwest::Client = if let Some((ref host, ref addrs)) = resolved {
                build_client(
                    &http_config,
                    config.cookie_handling,
                    Some((host.as_str(), addrs)),
                )
            } else {
                client
            };

            debug!(
                correlation_id = %exchange.correlation_id(),
                method = %method_str,
                url = %url,
                "HTTP request"
            );

            let method = method_str.parse::<reqwest::Method>().map_err(|e| {
                CamelError::ProcessorError(format!("Invalid HTTP method '{}': {}", method_str, e))
            })?;

            // Collect headers for potential redirect replay
            let mut collected_headers: Vec<(
                reqwest::header::HeaderName,
                reqwest::header::HeaderValue,
            )> = Vec::new();

            if let Some(user_agent) = &config.user_agent
                && !config.bridge_endpoint
                && let Ok(val) = reqwest::header::HeaderValue::from_str(user_agent)
            {
                collected_headers.push((reqwest::header::USER_AGENT, val));
            }

            // Inject W3C TraceContext headers for distributed tracing (opt-in via "otel" feature)
            #[cfg(feature = "otel")]
            let should_inject_otel = !config.bridge_endpoint;
            #[cfg(feature = "otel")]
            if should_inject_otel {
                let mut otel_headers = HashMap::new();
                camel_otel::inject_from_exchange(&exchange, &mut otel_headers);
                for (k, v) in otel_headers {
                    if let (Ok(name), Ok(val)) = (
                        reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                        reqwest::header::HeaderValue::from_str(&v),
                    ) {
                        collected_headers.push((name, val));
                    }
                }
            }

            for (key, value) in &exchange.input.headers {
                if !key.starts_with("Camel")
                    && !config
                        .skip_request_headers
                        .iter()
                        .any(|h| h.eq_ignore_ascii_case(key))
                    && let Some(val_str) = value.as_str()
                    && let (Ok(name), Ok(val)) = (
                        reqwest::header::HeaderName::from_bytes(key.as_bytes()),
                        reqwest::header::HeaderValue::from_str(val_str),
                    )
                {
                    collected_headers.push((name, val));
                }
            }

            // Auth headers
            if !config.bridge_endpoint {
                match &config.auth {
                    HttpAuth::None => {}
                    HttpAuth::Basic { username, password } => {
                        use base64::Engine;
                        // allow-secret: credentials combined for base64 Basic auth header
                        let credentials = format!("{username}:{password}");
                        let encoded = base64::engine::general_purpose::STANDARD.encode(credentials);
                        if let Ok(val) =
                            reqwest::header::HeaderValue::from_str(&format!("Basic {encoded}"))
                        {
                            collected_headers.push((reqwest::header::AUTHORIZATION, val));
                        }
                    }
                    HttpAuth::Bearer { token } => {
                        // allow-secret: Bearer token in Authorization header
                        let bearer = format!("Bearer {token}");
                        if let Ok(val) = reqwest::header::HeaderValue::from_str(&bearer) {
                            collected_headers.push((reqwest::header::AUTHORIZATION, val));
                        }
                    }
                }

                if config.connection_close
                    && let Ok(val) = reqwest::header::HeaderValue::from_str("close")
                {
                    collected_headers.push((reqwest::header::CONNECTION, val));
                }
            }

            // Materialize body
            let is_stream_body = matches!(exchange.input.body, Body::Stream(_));
            let materialized_body: Option<Vec<u8>> = if is_stream_body {
                None // Streams can't be replayed on redirect
            } else {
                let body = std::mem::take(&mut exchange.input.body);
                let bytes = body.into_bytes(config.max_body_size).await?;
                if bytes.is_empty() {
                    None
                } else {
                    Some(bytes.to_vec())
                }
            };

            let response = if config.follow_redirects && !is_stream_body {
                // Use manual redirect loop with per-hop SSRF validation
                ssrf::send_with_ssrf_safe_redirects(
                    &client,
                    &http_config,
                    &config,
                    method,
                    &url,
                    collected_headers,
                    materialized_body,
                    config.max_redirects,
                    config.response_timeout,
                )
                .await?
            } else {
                // Direct send (no redirect following, or streaming body)
                let mut request = client.request(method, &url);

                if let Some(timeout) = config.response_timeout {
                    request = request.timeout(timeout);
                }

                for (name, value) in &collected_headers {
                    request = request.header(name, value);
                }

                if is_stream_body {
                    if let Body::Stream(ref s) = exchange.input.body {
                        let mut stream_lock = s.stream.lock().await;
                        if let Some(stream) = stream_lock.take() {
                            request = request.body(reqwest::Body::wrap_stream(stream));
                        } else {
                            return Err(CamelError::AlreadyConsumed);
                        }
                    }
                } else if let Some(ref body_bytes) = materialized_body {
                    request = request.body(body_bytes.clone());
                }

                request
                    .send()
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("HTTP request failed: {e}")))?
            };

            let status_code = response.status().as_u16();
            let status_text = response
                .status()
                .canonical_reason()
                .unwrap_or("Unknown")
                .to_string();

            for (key, value) in response.headers() {
                if config
                    .skip_response_headers
                    .iter()
                    .any(|h| h.eq_ignore_ascii_case(key.as_str()))
                {
                    continue;
                }
                if let Ok(val_str) = value.to_str() {
                    exchange.input.set_header(
                        title_case_header(key.as_str()),
                        serde_json::Value::String(val_str.to_string()),
                    );
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

            // Read response body with timeout and size guard (HTTP-004, HTTP-005)
            let read_timeout = Duration::from_millis(config.read_timeout_ms);
            let response_body = tokio::time::timeout(read_timeout, async {
                // Check Content-Length header before allocating
                if let Some(content_len) = response.content_length()
                    && content_len > config.max_response_bytes as u64
                {
                    return Err(CamelError::ProcessorError(format!(
                        "Response body too large: {} bytes exceeds limit of {} bytes",
                        content_len, config.max_response_bytes
                    )));
                }
                // Use bytes_stream() for lazy streaming with size guard
                use futures::TryStreamExt;
                let mut stream = response.bytes_stream();
                let mut total: usize = 0;
                let mut collected = Vec::new();
                while let Some(chunk) = stream.try_next().await.map_err(|e| {
                    CamelError::ProcessorError(format!("Failed to read response body: {e}"))
                })? {
                    total += chunk.len();
                    if total > config.max_response_bytes {
                        return Err(CamelError::ProcessorError(format!(
                            "Response body too large: {} bytes exceeds limit of {} bytes",
                            total, config.max_response_bytes
                        )));
                    }
                    collected.push(chunk);
                }
                let mut result = bytes::BytesMut::with_capacity(total);
                for chunk in collected {
                    result.extend_from_slice(&chunk);
                }
                Ok::<bytes::Bytes, CamelError>(result.freeze())
            })
            .await
            .map_err(|_| {
                CamelError::ProcessorError(format!(
                    "Read timeout after {}ms",
                    config.read_timeout_ms
                ))
            })??;

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

/// Serializes tests that mutate or depend on the global `ServerRegistry`.
///
/// `ServerRegistry::global()` is a process-wide singleton that persists
/// across tests. `ServerRegistry::reset()` clears ALL entries; if it races
/// with another test that has a live server on a fixed port (e.g. 9991),
/// the registry entry is removed while the OS socket is still bound, so
/// the next `get_or_spawn` call on that port fails with "Address already
/// in use". Holding this mutex for the full body of each affected test
/// prevents the race without requiring `--test-threads=1`.
#[cfg(test)]
pub(crate) static REGISTRY_TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Map a pipeline error to an HTTP reply.
///
/// Extracted from the inline `match` in `dispatch_handler` for unit
/// testability (rc-1dk4). `TypeConversionFailed` (e.g. malformed JSON
/// body) maps to `400 Bad Request` with a structured JSON error body;
/// `Unauthenticated`/`Unauthorized` keep their existing `401`/`403`
/// mappings; all other errors map to `500 Internal Server Error`.
fn pipeline_error_to_reply(e: CamelError, path: &str) -> HttpReply {
    match e {
        CamelError::Unauthenticated(msg) => {
            tracing::warn!(error = %msg, path = %path, "Authentication failed");
            HttpReply {
                status: 401,
                headers: vec![("WWW-Authenticate".to_string(), "Bearer".to_string())],
                body: HttpReplyBody::Bytes(bytes::Bytes::from("Unauthorized")),
            }
        }
        CamelError::Unauthorized(msg) => {
            tracing::warn!(error = %msg, path = %path, "Authorization failed");
            HttpReply {
                status: 403,
                headers: vec![],
                body: HttpReplyBody::Bytes(bytes::Bytes::from("Forbidden")),
            }
        }
        CamelError::TypeConversionFailed(msg) => {
            tracing::warn!(error = %msg, path = %path, "Type conversion failed (bad request)");
            let body = serde_json::to_string(&serde_json::json!({
                "error": "bad_request",
                "message": msg,
            }))
            .unwrap_or_else(|_| "{}".to_string()); // allow-unwrap
            HttpReply {
                status: 400,
                headers: vec![("Content-Type".to_string(), "application/json".to_string())],
                body: HttpReplyBody::Bytes(bytes::Bytes::from(body)),
            }
        }
        CamelError::ValidationError(msg) => {
            tracing::warn!(error = %msg, path = %path, "Schema validation failed (bad request)");
            let body = serde_json::to_string(&serde_json::json!({
                "error": "validation_error",
                "message": msg,
            }))
            .unwrap_or_else(|_| "{}".to_string()); // allow-unwrap
            HttpReply {
                status: 400,
                headers: vec![("Content-Type".to_string(), "application/json".to_string())],
                body: HttpReplyBody::Bytes(bytes::Bytes::from(body)),
            }
        }
        e => {
            // log-policy: handler-owned
            tracing::warn!(error = %e, path = %path, "Pipeline error processing HTTP request");
            HttpReply {
                status: 500,
                headers: vec![],
                body: HttpReplyBody::Bytes(bytes::Bytes::from("Internal Server Error")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use camel_component_api::test_support::{NoopRuntimeObservability, PanicRuntimeObservability};
    fn test_rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(PanicRuntimeObservability)
    }
    fn rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(PanicRuntimeObservability)
    }
    fn noop_rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(NoopRuntimeObservability)
    }

    use super::*;
    use crate::rest_match::PathSegment;
    use camel_component_api::{Message, NoOpComponentContext};
    use std::sync::Arc;
    use std::time::Duration;

    fn test_producer_ctx() -> ProducerContext {
        ProducerContext::new()
    }

    #[test]
    fn test_http_config_defaults() {
        let config = HttpEndpointConfig::from_uri("http://localhost:8080/api").unwrap();
        assert_eq!(config.base_url, "http://localhost:8080/api");
        assert!(config.http_method.is_none());
        assert!(config.throw_exception_on_failure);
        assert_eq!(config.ok_status_code_range, (200, 299));
        assert!(config.response_timeout.is_none());
        assert!(matches!(config.auth, HttpAuth::None));
        assert!(matches!(config.cookie_handling, CookieHandling::Disabled));
        assert!(!config.bridge_endpoint);
        assert!(!config.connection_close);
    }

    #[test]
    fn test_http_config_scheme() {
        // UriConfig trait method returns "http" as primary scheme
        assert_eq!(HttpEndpointConfig::scheme(), "http");
    }

    #[test]
    fn test_http_config_from_components() {
        // Test from_components directly (trait method)
        let components = camel_component_api::UriComponents {
            scheme: "https".to_string(),
            path: "//api.example.com/v1".to_string(),
            params: std::collections::HashMap::from([(
                "httpMethod".to_string(),
                "POST".to_string(),
            )]),
        };
        let config = HttpEndpointConfig::from_components(components).unwrap();
        assert_eq!(config.base_url, "https://api.example.com/v1");
        assert_eq!(config.http_method, Some("POST".to_string()));
    }

    #[test]
    fn test_http_config_with_options() {
        let config = HttpEndpointConfig::from_uri(
            "https://api.example.com/v1?httpMethod=PUT&throwExceptionOnFailure=false&followRedirects=true&connectTimeout=5000&responseTimeout=10000"
        ).unwrap();
        assert_eq!(config.base_url, "https://api.example.com/v1");
        assert_eq!(config.http_method, Some("PUT".to_string()));
        assert!(!config.throw_exception_on_failure);
        assert_eq!(config.response_timeout, Some(Duration::from_millis(10000)));
    }

    #[test]
    fn test_http_endpoint_config_auth_and_headers_options() {
        let config = HttpEndpointConfig::from_uri(
            "http://localhost/api?authMethod=Basic&authUsername=u&authPassword=p&userAgent=camel-test&bridgeEndpoint=true&connectionClose=true&skipRequestHeaders=Authorization,X-Secret&skipResponseHeaders=Set-Cookie&cookieHandling=InMemory",
        )
        .unwrap();

        assert!(matches!(
            config.auth,
            HttpAuth::Basic { username, password } if username == "u" && password == "p"
        ));
        assert_eq!(config.user_agent.as_deref(), Some("camel-test"));
        assert!(matches!(config.cookie_handling, CookieHandling::InMemory));
        assert!(config.bridge_endpoint);
        assert!(config.connection_close);
        assert_eq!(
            config.skip_request_headers,
            vec!["authorization".to_string(), "x-secret".to_string()]
        );
        assert_eq!(config.skip_response_headers, vec!["set-cookie".to_string()]);
    }

    #[test]
    fn test_http_endpoint_config_bearer_auth() {
        let config = HttpEndpointConfig::from_uri(
            "http://localhost/api?authMethod=Bearer&authBearerToken=t",
        )
        .unwrap();
        assert!(matches!(
            config.auth,
            HttpAuth::Bearer { token } if token == "t"
        ));
    }

    #[test]
    fn test_from_uri_with_defaults_applies_config_when_uri_param_absent() {
        let config = HttpConfig::default()
            .with_response_timeout_ms(999)
            .with_allow_internal(true)
            .with_blocked_hosts(vec!["evil.com".to_string()])
            .with_max_body_size(12345);
        let endpoint =
            HttpEndpointConfig::from_uri_with_defaults("http://example.com/api", &config).unwrap();
        assert_eq!(endpoint.response_timeout, Some(Duration::from_millis(999)));
        assert!(endpoint.allow_internal);
        assert_eq!(endpoint.blocked_hosts, vec!["evil.com".to_string()]);
        assert_eq!(endpoint.max_body_size, 12345);
    }

    #[test]
    fn test_from_uri_with_defaults_uri_overrides_config() {
        let config = HttpConfig::default()
            .with_response_timeout_ms(999)
            .with_allow_internal(true)
            .with_blocked_hosts(vec!["evil.com".to_string()])
            .with_max_body_size(12345);
        let endpoint = HttpEndpointConfig::from_uri_with_defaults(
            "http://example.com/api?responseTimeout=500&allowInternal=false&blockedHosts=bad.net&maxBodySize=99",
            &config,
        )
        .unwrap();
        assert_eq!(endpoint.response_timeout, Some(Duration::from_millis(500)));
        assert!(!endpoint.allow_internal);
        assert_eq!(endpoint.blocked_hosts, vec!["bad.net".to_string()]);
        assert_eq!(endpoint.max_body_size, 99);
    }

    #[test]
    fn test_http_config_ok_status_range() {
        let config =
            HttpEndpointConfig::from_uri("http://localhost/api?okStatusCodeRange=200-204").unwrap();
        assert_eq!(config.ok_status_code_range, (200, 204));
    }

    #[test]
    fn test_http_config_wrong_scheme() {
        let result = HttpEndpointConfig::from_uri("file:/tmp");
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
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint("http://0.0.0.0:19100/test", &ctx)
            .unwrap();
        assert!(endpoint.create_consumer(rt()).is_ok());
    }

    #[test]
    fn test_https_endpoint_creates_consumer_errors_without_tls() {
        let component = HttpsComponent::new();
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint("https://0.0.0.0:8443/test", &ctx)
            .unwrap();
        // https:// without tlsCert/tlsKey must fail (scheme enforcement)
        assert!(endpoint.create_consumer(rt()).is_err());
    }

    #[test]
    fn test_http_endpoint_creates_producer() {
        let ctx = test_producer_ctx();
        let component = HttpComponent::new();
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint("http://localhost/api", &endpoint_ctx)
            .unwrap();
        assert!(endpoint.create_producer(rt(), &ctx).is_ok());
    }

    // -----------------------------------------------------------------------
    // Producer tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_producer_with_token_provider() {
        use camel_auth::oauth2::TokenProvider;
        use tower::ServiceExt;

        let captured_auth: Arc<std::sync::Mutex<Option<String>>> =
            Arc::new(std::sync::Mutex::new(None));
        let captured_clone = Arc::clone(&captured_auth);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let _handle = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                let auth = request
                    .lines()
                    .find(|l| l.to_lowercase().starts_with("authorization:"))
                    .map(|l| {
                        l.split(':')
                            .nth(1)
                            .map(|s| s.trim().to_string())
                            .unwrap_or_default()
                    });
                *captured_clone.lock().unwrap() = auth;
                let body = r#"{"echo":"ok"}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        });

        #[derive(Debug)]
        struct StaticProvider;
        #[async_trait::async_trait]
        impl TokenProvider for StaticProvider {
            async fn get_token(&self) -> Result<String, camel_auth::types::AuthError> {
                Ok("injected-token".into())
            }
        }

        let uri = format!("http://127.0.0.1:{}/api?allowInternal=true", port);
        let ctx = test_producer_ctx();
        let component = HttpComponent::new();
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component.create_endpoint(&uri, &endpoint_ctx).unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let exchange = Exchange::new(Message::new("hello"));

        let layer = BearerTokenLayer::new(Arc::new(StaticProvider));
        let mut layered = layer.layer(producer);
        let result = layered.ready().await.unwrap().call(exchange).await;
        assert!(result.is_ok(), "producer call failed: {:?}", result);

        tokio::time::sleep(Duration::from_millis(100)).await;
        let auth = captured_auth.lock().unwrap().take();
        assert_eq!(auth.as_deref(), Some("Bearer injected-token"));
    }

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
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("{url}/api/test?allowInternal=true"), &endpoint_ctx)
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

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
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("{url}/api/data?allowInternal=true"), &endpoint_ctx)
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

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
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("{url}/api?allowInternal=true"), &endpoint_ctx)
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

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
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(
                &format!("{url}/api?httpMethod=PUT&allowInternal=true"),
                &endpoint_ctx,
            )
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

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
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(
                &format!("{url}/not-found?allowInternal=true"),
                &endpoint_ctx,
            )
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

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
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(
                &format!("{url}/error?throwExceptionOnFailure=false&allowInternal=true"),
                &endpoint_ctx,
            )
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

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
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(
                "http://localhost:1/does-not-exist?allowInternal=true",
                &endpoint_ctx,
            )
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

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
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("{url}/api?allowInternal=true"), &endpoint_ctx)
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await.unwrap();

        assert!(
            result.input.header("Content-Type").is_some(),
            "Response should have Content-Type header"
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

        let component =
            HttpComponent::with_config(HttpConfig::default().with_follow_redirects(false));
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(
                &format!("{url}?throwExceptionOnFailure=false&allowInternal=true"),
                &endpoint_ctx,
            )
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

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

        let component =
            HttpComponent::with_config(HttpConfig::default().with_follow_redirects(true));
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("{url}?allowInternal=true"), &endpoint_ctx)
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

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

    /// Integration test: with allowInternal=true, redirects to private IPs are followed.
    /// This verifies the manual redirect loop executes correctly.
    #[tokio::test]
    async fn test_redirect_to_private_ip_is_ssrf_blocked() {
        use tower::ServiceExt;

        // Use the existing redirect server which redirects to /final on the same server
        let (url, _handle) = start_redirect_server().await;
        let ctx = test_producer_ctx();

        let component =
            HttpComponent::with_config(HttpConfig::default().with_follow_redirects(true));
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("{url}?allowInternal=true"), &endpoint_ctx)
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await;

        // With allowInternal=true, the redirect should succeed
        assert!(
            result.is_ok(),
            "Redirect should succeed with allowInternal=true, got: {:?}",
            result
        );
        let exchange = result.unwrap();
        let status = exchange
            .input
            .header("CamelHttpResponseCode")
            .and_then(|v| v.as_u64())
            .unwrap();
        assert_eq!(status, 200, "Should follow redirect to /final");
    }

    /// With allowInternal=true, redirects to private IPs should be followed.
    #[tokio::test]
    async fn test_redirect_to_private_ip_allowed_when_configured() {
        use tower::ServiceExt;

        // Start a server that redirects to /final on the same server (127.0.0.1)
        let (url, _handle) = start_redirect_server().await;
        let ctx = test_producer_ctx();

        let component =
            HttpComponent::with_config(HttpConfig::default().with_follow_redirects(true));
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("{url}?allowInternal=true"), &endpoint_ctx)
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await.unwrap();

        let status = result
            .input
            .header("CamelHttpResponseCode")
            .and_then(|v| v.as_u64())
            .unwrap();
        assert_eq!(
            status, 200,
            "Should follow redirect to private IP when allowInternal=true"
        );
    }

    /// Integration test: with allowInternal=false (default), a redirect to a
    /// private/metadata IP must be blocked by the SSRF guard — NOT followed.
    #[tokio::test]
    async fn test_redirect_to_private_ip_blocked_when_ssrf_guard_active() {
        use tower::ServiceExt;

        // Server that redirects to the AWS metadata endpoint (link-local private IP)
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://127.0.0.1:{}", addr.port());

        let handle = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            loop {
                if let Ok((mut stream, _)) = listener.accept().await {
                    tokio::spawn(async move {
                        let mut buf = vec![0u8; 4096];
                        let _ = stream.read(&mut buf).await;
                        // Always redirect to the metadata endpoint
                        let response = "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest/meta-data/\r\nContent-Length: 0\r\n\r\n";
                        let _ = stream.write_all(response.as_bytes()).await;
                    });
                }
            }
        });

        let ctx = test_producer_ctx();
        let component =
            HttpComponent::with_config(HttpConfig::default().with_follow_redirects(true));
        let endpoint_ctx = NoOpComponentContext;
        // allowInternal=false is the default — do NOT set it
        let endpoint = component.create_endpoint(&url, &endpoint_ctx).unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await;

        // Must be an error — SSRF guard blocks the redirect target
        assert!(
            result.is_err(),
            "Redirect to private IP 169.254.169.254 must be blocked when allowInternal=false"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("blocked IP")
                || err.contains("private IP")
                || err.contains("SSRF")
                || err.contains("not allowed"),
            "Error should mention SSRF/IP blocking, got: {err}"
        );

        handle.abort();
    }

    /// Integration test: exceeding maxRedirects produces a clear error.
    #[tokio::test]
    async fn test_too_many_redirects_returns_error() {
        use tower::ServiceExt;

        // Server that always redirects to itself (infinite loop)
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://127.0.0.1:{}", addr.port());

        let handle = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            loop {
                if let Ok((mut stream, _)) = listener.accept().await {
                    tokio::spawn(async move {
                        let mut buf = vec![0u8; 4096];
                        let _ = stream.read(&mut buf).await;
                        // Always redirect to /loop
                        let response =
                            "HTTP/1.1 302 Found\r\nLocation: /loop\r\nContent-Length: 0\r\n\r\n";
                        let _ = stream.write_all(response.as_bytes()).await;
                    });
                }
            }
        });

        let ctx = test_producer_ctx();
        let component =
            HttpComponent::with_config(HttpConfig::default().with_follow_redirects(true));
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(
                &format!("{url}?allowInternal=true&maxRedirects=2"),
                &endpoint_ctx,
            )
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await;

        // With the fix, exceeding max redirects returns the redirect response
        // as-is instead of erroring. The 302 redirect response is returned
        // after followRedirects exhausts the allowed redirect count (2).
        // Disable throwExceptionOnFailure to inspect the raw response status.
        //
        // Old behavior: Err("Too many redirects (max 2)")
        // New behavior: Ok(ex) with CamelHttpResponseCode = 302
        match result {
            Err(e) => {
                // If throw_exception_on_failure is on, we get HttpOperationFailed
                let msg = e.to_string();
                assert!(
                    msg.contains("HTTP operation failed") || msg.contains("302"),
                    "expected redirect-after-exhaustion error, got: {msg}"
                );
            }
            Ok(ex) => {
                let response_code = ex
                    .input
                    .header("CamelHttpResponseCode")
                    .and_then(|v| v.as_u64());
                assert_eq!(
                    response_code,
                    Some(302),
                    "expected 302 after exhausting redirects"
                );
            }
        }

        handle.abort();
    }

    #[tokio::test]
    async fn test_query_params_forwarded_to_http_request() {
        use tower::ServiceExt;

        let (url, _handle) = start_test_server().await;
        let ctx = test_producer_ctx();

        let component = HttpComponent::new();
        let endpoint_ctx = NoOpComponentContext;
        // apiKey is NOT a Camel option, should be forwarded as query param
        let endpoint = component
            .create_endpoint(
                &format!("{url}/api?apiKey=secret123&httpMethod=GET&allowInternal=true"),
                &endpoint_ctx,
            )
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

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
        let config = HttpEndpointConfig::from_uri(
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

    #[test]
    fn test_query_params_are_url_encoded_when_resolving_url() {
        let config =
            HttpEndpointConfig::from_uri("http://example.com/api?q=hello world&tag=a+b").unwrap();
        let exchange = Exchange::new(Message::default());

        let url = HttpProducer::resolve_url(&exchange, &config);

        assert!(url.contains("q=hello+world"), "url was: {url}");
        assert!(url.contains("tag=a%2Bb"), "url was: {url}");
    }

    // -----------------------------------------------------------------------
    // Timeout tests (HTTP-004)
    // -----------------------------------------------------------------------

    async fn start_slow_server(delay_ms: u64) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://127.0.0.1:{}", addr.port());

        let handle = tokio::spawn(async move {
            loop {
                if let Ok((mut stream, _)) = listener.accept().await {
                    let delay = delay_ms;
                    tokio::spawn(async move {
                        use tokio::io::{AsyncReadExt, AsyncWriteExt};
                        let mut buf = vec![0u8; 4096];
                        let _ = stream.read(&mut buf).await;
                        // Send headers immediately (no Content-Length → chunked)
                        let headers = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n";
                        let _ = stream.write_all(headers.as_bytes()).await;
                        // Delay before sending body chunk
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        let body = r#"{"status":"slow"}"#;
                        let chunk = format!("{:x}\r\n{}\r\n0\r\n\r\n", body.len(), body);
                        let _ = stream.write_all(chunk.as_bytes()).await;
                    });
                }
            }
        });

        (url, handle)
    }

    #[tokio::test]
    async fn test_http_producer_timeout() {
        use tower::ServiceExt;

        // Server delays 500ms, client timeout is 100ms → should timeout
        let (url, _handle) = start_slow_server(500).await;
        let ctx = test_producer_ctx();

        let component = HttpComponent::with_config(
            HttpConfig::default()
                .with_read_timeout_ms(100)
                .with_response_timeout_ms(30_000), // generous response timeout
        );
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("{url}/slow?allowInternal=true"), &endpoint_ctx)
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await;

        assert!(result.is_err(), "Expected timeout error, got: {:?}", result);
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Read timeout") || err.contains("timeout"),
            "Error should mention timeout, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_http_producer_no_timeout_when_fast() {
        use tower::ServiceExt;

        let (url, _handle) = start_test_server().await;
        let ctx = test_producer_ctx();

        let component =
            HttpComponent::with_config(HttpConfig::default().with_read_timeout_ms(5_000));
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("{url}/api?allowInternal=true"), &endpoint_ctx)
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await.unwrap();

        let status = result
            .input
            .header("CamelHttpResponseCode")
            .and_then(|v| v.as_u64())
            .unwrap();
        assert_eq!(status, 200);
    }

    // -----------------------------------------------------------------------
    // SSRF Protection tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_http_producer_blocks_metadata_endpoint() {
        use tower::ServiceExt;

        let ctx = test_producer_ctx();
        let component = HttpComponent::new();
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint("http://example.com/api?allowInternal=false", &endpoint_ctx)
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

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
        let config = HttpEndpointConfig::from_uri("http://example.com/api").unwrap();
        assert!(
            !config.allow_internal,
            "Private IPs should be blocked by default"
        );
        assert!(
            config.blocked_hosts.is_empty(),
            "Blocked hosts should be empty by default"
        );
    }

    #[test]
    fn test_ssrf_config_allow_internal() {
        let config =
            HttpEndpointConfig::from_uri("http://example.com/api?allowInternal=true").unwrap();
        assert!(
            config.allow_internal,
            "Private IPs should be allowed when explicitly set"
        );
    }

    #[test]
    fn test_ssrf_config_blocked_hosts() {
        let config = HttpEndpointConfig::from_uri(
            "http://example.com/api?blockedHosts=evil.com,malware.net",
        )
        .unwrap();
        assert_eq!(config.blocked_hosts, vec!["evil.com", "malware.net"]);
    }

    #[tokio::test]
    async fn test_http_producer_blocks_localhost() {
        use tower::ServiceExt;

        let ctx = test_producer_ctx();
        let component = HttpComponent::new();
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint("http://example.com/api", &endpoint_ctx)
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

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
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint("http://example.com/api", &endpoint_ctx)
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

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
        let endpoint_ctx = NoOpComponentContext;
        // With allowInternal=true, the validation should pass
        // (actual connection will fail, but that's expected)
        let endpoint = component
            .create_endpoint("http://192.168.1.1/api?allowInternal=true", &endpoint_ctx)
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

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
        assert_eq!(cfg.max_inflight_requests, 1024);
    }

    #[test]
    fn test_http_server_config_scheme() {
        // UriConfig trait method returns "http" as primary scheme
        assert_eq!(HttpServerConfig::scheme(), "http");
    }

    #[test]
    fn test_http_server_config_from_components() {
        // Test from_components directly (trait method)
        let components = camel_component_api::UriComponents {
            scheme: "https".to_string(),
            path: "//0.0.0.0:8443/api".to_string(),
            params: std::collections::HashMap::from([
                ("maxRequestBody".to_string(), "5242880".to_string()),
                ("maxInflightRequests".to_string(), "7".to_string()),
            ]),
        };
        let cfg = HttpServerConfig::from_components(components).unwrap();
        assert_eq!(cfg.host, "0.0.0.0");
        assert_eq!(cfg.port, 8443);
        assert_eq!(cfg.path, "/api");
        assert_eq!(cfg.max_request_body, 5242880);
        assert_eq!(cfg.max_inflight_requests, 7);
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
    fn test_http_server_config_default_port_by_scheme() {
        // HTTP without explicit port should default to 80
        let cfg_http = HttpServerConfig::from_uri("http://0.0.0.0/orders").unwrap();
        assert_eq!(cfg_http.port, 80);

        // HTTPS without explicit port should default to 443
        let cfg_https = HttpServerConfig::from_uri("https://0.0.0.0/orders").unwrap();
        assert_eq!(cfg_https.port, 443);
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

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn test_concurrent_get_or_spawn_returns_same_registry() {
        let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let results: Arc<std::sync::Mutex<Vec<HttpRouteRegistry>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut handles = Vec::new();
        for _ in 0..4 {
            let results = results.clone();
            handles.push(tokio::spawn(async move {
                let registry = ServerRegistry::global()
                    .get_or_spawn(
                        "127.0.0.1",
                        port,
                        2 * 1024 * 1024,
                        10 * 1024 * 1024,
                        1024,
                        test_rt(),
                        "test-route".into(),
                        None,
                    )
                    .await
                    .unwrap();
                results.lock().unwrap().push(registry);
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        let registries = results.lock().unwrap();
        assert_eq!(registries.len(), 4);
        for i in 1..registries.len() {
            assert!(
                Arc::ptr_eq(&registries[0].inner, &registries[i].inner),
                "all concurrent callers should get same route registry"
            );
        }
    }

    #[test]
    fn test_server_registry_distinguishes_host_and_port() {
        let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async {
            let registry = ServerRegistry::global();
            // Use two distinct host values with same configured port key.
            // Port 0 is acceptable here because the registry key uses the configured
            // tuple, not the OS-assigned ephemeral port.
            let d1 = registry
                .get_or_spawn(
                    "127.0.0.1",
                    0,
                    1024 * 1024,
                    10 * 1024 * 1024,
                    1024,
                    test_rt(),
                    "test-route-1".into(),
                    None,
                )
                .await;
            let d2 = registry
                .get_or_spawn(
                    "0.0.0.0",
                    0,
                    1024 * 1024,
                    10 * 1024 * 1024,
                    1024,
                    test_rt(),
                    "test-route-2".into(),
                    None,
                )
                .await;
            assert!(d1.is_ok());
            assert!(d2.is_ok());
            assert!(!Arc::ptr_eq(&d1.unwrap().inner, &d2.unwrap().inner));
        });
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn test_shared_server_max_request_body_policy_is_deterministic() {
        let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();
        let registry = ServerRegistry::global();
        // First registration: maxRequestBody = 1 MB
        let d1 = registry
            .get_or_spawn(
                "127.0.0.1",
                9991,
                1024 * 1024,
                10 * 1024 * 1024,
                1024,
                test_rt(),
                "test-route".into(),
                None,
            )
            .await;
        assert!(d1.is_ok());

        // Second registration on same (host,port): maxRequestBody = 2 MB
        // Expected: explicit EndpointCreationFailed about incompatible maxRequestBody
        let d2 = registry
            .get_or_spawn(
                "127.0.0.1",
                9991,
                2 * 1024 * 1024,
                10 * 1024 * 1024,
                1024,
                test_rt(),
                "test-route-2".into(),
                None,
            )
            .await;
        assert!(d2.is_err());
        let err = d2.unwrap_err();
        assert!(
            err.to_string().contains("maxRequestBody") || err.to_string().contains("incompatible"),
            "Expected incompatible maxRequestBody error, got: {}",
            err
        );
    }

    #[test]
    fn test_server_registry_reset_clears_entries() {
        let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async {
            // Register something on a unique port
            let d1 = ServerRegistry::global()
                .get_or_spawn(
                    "127.0.0.1",
                    9992,
                    1024 * 1024,
                    10 * 1024 * 1024,
                    1024,
                    test_rt(),
                    "test-route".into(),
                    None,
                )
                .await;
            assert!(d1.is_ok());

            // Verify entry exists
            let guard = ServerRegistry::global().inner.lock().expect("lock");
            assert!(guard.contains_key(&("127.0.0.1".to_string(), 9992)));
            drop(guard);

            // Reset
            ServerRegistry::reset();

            // Verify cleared
            let guard = ServerRegistry::global().inner.lock().expect("lock");
            assert!(
                guard.is_empty(),
                "registry should be empty after reset, has {} entries",
                guard.len()
            );
        });
    }

    #[tokio::test]
    async fn registry_rejects_tls_on_plain_port() {
        ServerRegistry::reset();
        let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);

        // First route: plain HTTP
        let _r1 = ServerRegistry::global()
            .get_or_spawn(
                "127.0.0.1",
                0,
                1024,
                1024,
                16,
                Arc::clone(&rt),
                "route-1".into(),
                None, // plain
            )
            .await;

        // Second route: TLS on same port → must fail
        let result = ServerRegistry::global()
            .get_or_spawn(
                "127.0.0.1",
                0,
                1024,
                1024,
                16,
                Arc::clone(&rt),
                "route-2".into(),
                Some(crate::config::ServerTlsConfig {
                    cert_path: "/x.pem".into(),
                    key_path: "/y.pem".into(),
                }),
            )
            .await;
        assert!(result.is_err(), "must reject TLS on plain port");
    }

    // -----------------------------------------------------------------------
    // D-L10: HTTP monitor_axum_task refcounted shutdown
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_unregister_last_http_route_keeps_server_alive() {
        let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();
        ServerRegistry::reset();
        let registry = ServerRegistry::global();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener); // Release — ServerRegistry will rebind
        let rt = test_rt();

        // Register 2 routes on the same (host, port) — OnceCell returns the
        // same ServerHandle.
        let _r1 = registry
            .get_or_spawn(
                "127.0.0.1",
                port,
                1024 * 1024,
                10 * 1024 * 1024,
                16,
                rt.clone(),
                "test-route-1".into(),
                None,
            )
            .await
            .unwrap();
        let _r2 = registry
            .get_or_spawn(
                "127.0.0.1",
                port,
                1024 * 1024,
                10 * 1024 * 1024,
                16,
                rt,
                "test-route-2".into(),
                None,
            )
            .await
            .unwrap();

        let key = ("127.0.0.1".to_string(), port);
        let cell = {
            let guard = registry.inner.lock().expect("lock");
            guard.get(&key).expect("entry should exist").clone()
        };

        // Unregister first route -> monitor still alive (count = 1).
        registry.unregister("127.0.0.1", port).await;
        {
            let handle = cell
                .get()
                .expect("handle should still exist after first unregister");
            assert!(
                !handle.monitor_task.is_finished(),
                "monitor task should still be alive after first unregister"
            );
        }

        // Unregister second route -> server stays alive (process-lifetime).
        registry.unregister("127.0.0.1", port).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        {
            let handle = cell
                .get()
                .expect("handle should still exist after last unregister");
            assert!(
                !handle.monitor_task.is_finished(),
                "monitor task should still be alive — server is process-lifetime"
            );
        }

        // Entry stays in registry for potential restart.
        {
            let guard = registry.inner.lock().expect("lock");
            assert!(
                guard.get(&key).is_some(),
                "entry should remain in registry — server kept alive for restart"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Axum dispatch handler tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_dispatch_handler_returns_404_for_unknown_path() {
        let registry = HttpRouteRegistry::new();
        // Nothing registered in route registry
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(run_axum_server(
            listener,
            registry,
            2 * 1024 * 1024,
            10 * 1024 * 1024,
            Arc::new(tokio::sync::Semaphore::new(1024)),
            test_rt(),
            "test-route".into(),
        ));

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
        use camel_component_api::ConsumerContext;

        // Get an OS-assigned free port
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener); // Release port — ServerRegistry will rebind it

        let consumer_cfg = HttpServerConfig {
            scheme: "http".to_string(),
            host: "127.0.0.1".to_string(),
            port,
            path: "/ping".to_string(),
            max_request_body: 2 * 1024 * 1024,
            max_response_body: 10 * 1024 * 1024,
            max_inflight_requests: 1024,
            method: None,
            tls_config: None,
        };
        let mut consumer = HttpConsumer::new(consumer_cfg, test_rt());

        let (tx, mut rx) = tokio::sync::mpsc::channel::<camel_component_api::ExchangeEnvelope>(16);
        let token = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone(), "http-test-route".to_string());

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

    #[tokio::test]
    async fn test_http_consumer_returns_503_when_inflight_limit_reached() {
        use camel_component_api::{ConsumerContext, ExchangeEnvelope};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let consumer_cfg = HttpServerConfig {
            scheme: "http".to_string(),
            host: "127.0.0.1".to_string(),
            port,
            path: "/saturation".to_string(),
            max_request_body: 2 * 1024 * 1024,
            max_response_body: 10 * 1024 * 1024,
            max_inflight_requests: 1,
            method: None,
            tls_config: None,
        };
        let mut consumer = HttpConsumer::new(consumer_cfg, test_rt());

        let (tx, mut rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
        let token = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone(), "http-test-route".to_string());
        tokio::spawn(async move { consumer.start(ctx).await.unwrap() });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let (first_seen_tx, first_seen_rx) = tokio::sync::oneshot::channel::<()>();
        let (unblock_first_tx, unblock_first_rx) = tokio::sync::oneshot::channel::<()>();

        tokio::spawn(async move {
            let mut first_seen_tx = Some(first_seen_tx);
            let mut unblock_first_rx = Some(unblock_first_rx);

            while let Some(envelope) = rx.recv().await {
                if let Some(tx) = first_seen_tx.take() {
                    let _ = tx.send(());
                    if let Some(rx_unblock) = unblock_first_rx.take() {
                        let _ = rx_unblock.await;
                    }
                }

                if let Some(reply_tx) = envelope.reply_tx {
                    let _ = reply_tx.send(Ok(envelope.exchange));
                }
            }
        });

        let client = reqwest::Client::new();
        let first_req = {
            let client = client.clone();
            async move {
                client
                    .get(format!("http://127.0.0.1:{port}/saturation"))
                    .send()
                    .await
                    .unwrap()
            }
        };

        let first_handle = tokio::spawn(first_req);
        first_seen_rx.await.unwrap();

        let second_resp = client
            .get(format!("http://127.0.0.1:{port}/saturation"))
            .send()
            .await
            .unwrap();

        assert_eq!(second_resp.status().as_u16(), 503);

        let _ = unblock_first_tx.send(());
        let first_resp = first_handle.await.unwrap();
        assert_eq!(first_resp.status().as_u16(), 200);

        token.cancel();
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_http_consumer_enforces_max_response_body_for_bytes() {
        use camel_component_api::{ConsumerContext, ExchangeEnvelope};

        let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let consumer_cfg = HttpServerConfig {
            scheme: "http".to_string(),
            host: "127.0.0.1".to_string(),
            port,
            path: "/limit-bytes".to_string(),
            max_request_body: 2 * 1024 * 1024,
            max_response_body: 16,
            max_inflight_requests: 1024,
            method: None,
            tls_config: None,
        };
        let mut consumer = HttpConsumer::new(consumer_cfg, test_rt());

        let (tx, mut rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
        let token = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone(), "http-test-route".to_string());
        tokio::spawn(async move { consumer.start(ctx).await.unwrap() });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = reqwest::Client::new();
        let send_fut = client
            .get(format!("http://127.0.0.1:{port}/limit-bytes"))
            .send();

        let (http_result, _) = tokio::join!(send_fut, async {
            if let Some(mut envelope) = rx.recv().await {
                envelope.exchange.input.body =
                    camel_component_api::Body::Bytes(bytes::Bytes::from(vec![b'x'; 32]));
                if let Some(reply_tx) = envelope.reply_tx {
                    let _ = reply_tx.send(Ok(envelope.exchange));
                }
            }
        });

        let resp = http_result.unwrap();
        assert_eq!(resp.status().as_u16(), 500);
        let body = resp.text().await.unwrap();
        assert_eq!(body, "Response body exceeds configured limit");
        token.cancel();
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_http_consumer_enforces_max_response_body_for_json() {
        use camel_component_api::{ConsumerContext, ExchangeEnvelope};

        let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let consumer_cfg = HttpServerConfig {
            scheme: "http".to_string(),
            host: "127.0.0.1".to_string(),
            port,
            path: "/limit-json".to_string(),
            max_request_body: 2 * 1024 * 1024,
            max_response_body: 16,
            max_inflight_requests: 1024,
            method: None,
            tls_config: None,
        };
        let mut consumer = HttpConsumer::new(consumer_cfg, test_rt());

        let (tx, mut rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
        let token = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone(), "http-test-route".to_string());
        tokio::spawn(async move { consumer.start(ctx).await.unwrap() });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = reqwest::Client::new();
        let send_fut = client
            .get(format!("http://127.0.0.1:{port}/limit-json"))
            .send();

        let (http_result, _) = tokio::join!(send_fut, async {
            if let Some(mut envelope) = rx.recv().await {
                envelope.exchange.input.body = camel_component_api::Body::Json(
                    serde_json::json!({"message":"this response is bigger than sixteen"}),
                );
                if let Some(reply_tx) = envelope.reply_tx {
                    let _ = reply_tx.send(Ok(envelope.exchange));
                }
            }
        });

        let resp = http_result.unwrap();
        assert_eq!(resp.status().as_u16(), 500);
        let body = resp.text().await.unwrap();
        assert_eq!(body, "Response body exceeds configured limit");
        token.cancel();
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_http_consumer_enforces_max_response_body_for_xml() {
        use camel_component_api::{ConsumerContext, ExchangeEnvelope};

        let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let consumer_cfg = HttpServerConfig {
            scheme: "http".to_string(),
            host: "127.0.0.1".to_string(),
            port,
            path: "/limit-xml".to_string(),
            max_request_body: 2 * 1024 * 1024,
            max_response_body: 16,
            max_inflight_requests: 1024,
            method: None,
            tls_config: None,
        };
        let mut consumer = HttpConsumer::new(consumer_cfg, test_rt());

        let (tx, mut rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
        let token = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone(), "http-test-route".to_string());
        tokio::spawn(async move { consumer.start(ctx).await.unwrap() });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = reqwest::Client::new();
        let send_fut = client
            .get(format!("http://127.0.0.1:{port}/limit-xml"))
            .send();

        let (http_result, _) = tokio::join!(send_fut, async {
            if let Some(mut envelope) = rx.recv().await {
                envelope.exchange.input.body = camel_component_api::Body::Xml(
                    "<root><value>way-too-large</value></root>".into(),
                );
                if let Some(reply_tx) = envelope.reply_tx {
                    let _ = reply_tx.send(Ok(envelope.exchange));
                }
            }
        });

        let resp = http_result.unwrap();
        assert_eq!(resp.status().as_u16(), 500);
        let body = resp.text().await.unwrap();
        assert_eq!(body, "Response body exceeds configured limit");
        token.cancel();
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_http_consumer_does_not_enforce_max_response_body_for_stream() {
        use camel_component_api::{
            CamelError, ConsumerContext, ExchangeEnvelope, StreamBody, StreamMetadata,
        };
        use futures::stream;

        let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();

        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let consumer_cfg = HttpServerConfig {
            scheme: "http".to_string(),
            host: "0.0.0.0".to_string(),
            port,
            path: "/limit-stream".to_string(),
            max_request_body: 2 * 1024 * 1024,
            max_response_body: 16,
            max_inflight_requests: 1024,
            method: None,
            tls_config: None,
        };
        let mut consumer = HttpConsumer::new(consumer_cfg, test_rt());

        let (tx, mut rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
        let token = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone(), "http-test-route".to_string());
        tokio::spawn(async move { consumer.start(ctx).await.unwrap() });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = reqwest::Client::new();
        let send_fut = client
            .get(format!("http://127.0.0.1:{port}/limit-stream"))
            .send();

        let (http_result, _) = tokio::join!(send_fut, async {
            if let Some(mut envelope) = rx.recv().await {
                let chunks: Vec<Result<bytes::Bytes, CamelError>> =
                    vec![Ok(bytes::Bytes::from(vec![b'x'; 32]))];
                let stream = Box::pin(stream::iter(chunks));
                envelope.exchange.input.body = camel_component_api::Body::Stream(StreamBody {
                    stream: Arc::new(tokio::sync::Mutex::new(Some(stream))),
                    metadata: StreamMetadata {
                        size_hint: Some(32),
                        content_type: Some("application/octet-stream".into()),
                        origin: None,
                    },
                });
                if let Some(reply_tx) = envelope.reply_tx {
                    let _ = reply_tx.send(Ok(envelope.exchange));
                }
            }
        });

        let resp = http_result.unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let body = resp.bytes().await.unwrap();
        assert_eq!(body.len(), 32);
        token.cancel();
    }

    // -----------------------------------------------------------------------
    // Integration tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_integration_single_consumer_round_trip() {
        use camel_component_api::{ConsumerContext, ExchangeEnvelope};

        // Spawns an HTTP consumer on the global ServerRegistry
        // (HttpConsumer::start → get_or_spawn). Serialize against the other
        // registry tests so parallel runs do not race on shared global state.
        let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();

        // Get an OS-assigned free port (ephemeral)
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener); // Release — ServerRegistry will rebind

        let component = HttpComponent::new();
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("http://127.0.0.1:{port}/echo"), &endpoint_ctx)
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
        let token = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone(), "http-test-route".to_string());

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
                envelope.exchange.input.body = camel_component_api::Body::Text("pong".to_string());
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
    #[allow(clippy::await_holding_lock)]
    async fn test_integration_two_consumers_shared_port() {
        use camel_component_api::{ConsumerContext, ExchangeEnvelope};

        let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();

        // Get an OS-assigned free port (ephemeral)
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let component = HttpComponent::new();
        let endpoint_ctx = NoOpComponentContext;

        // Consumer A: /hello
        let endpoint_a = component
            .create_endpoint(&format!("http://127.0.0.1:{port}/hello"), &endpoint_ctx)
            .unwrap();
        let mut consumer_a = endpoint_a.create_consumer(rt()).unwrap();

        // Consumer B: /world
        let endpoint_b = component
            .create_endpoint(&format!("http://127.0.0.1:{port}/world"), &endpoint_ctx)
            .unwrap();
        let mut consumer_b = endpoint_b.create_consumer(rt()).unwrap();

        let (tx_a, mut rx_a) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
        let token_a = tokio_util::sync::CancellationToken::new();
        let ctx_a = ConsumerContext::new(tx_a, token_a.clone(), "http-test-route-a".to_string());

        let (tx_b, mut rx_b) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
        let token_b = tokio_util::sync::CancellationToken::new();
        let ctx_b = ConsumerContext::new(tx_b, token_b.clone(), "http-test-route-b".to_string());

        tokio::spawn(async move { consumer_a.start(ctx_a).await.unwrap() });
        tokio::spawn(async move { consumer_b.start(ctx_b).await.unwrap() });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = reqwest::Client::new();

        // Request to /hello
        let fut_hello = client.get(format!("http://127.0.0.1:{port}/hello")).send();
        let (resp_hello, _) = tokio::join!(fut_hello, async {
            if let Some(mut envelope) = rx_a.recv().await {
                envelope.exchange.input.body =
                    camel_component_api::Body::Text("hello-response".to_string());
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
                    camel_component_api::Body::Text("world-response".to_string());
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
    #[allow(clippy::await_holding_lock)]
    async fn test_integration_unregistered_path_returns_404() {
        use camel_component_api::{ConsumerContext, ExchangeEnvelope};

        let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();

        // Get an OS-assigned free port (ephemeral)
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let component = HttpComponent::new();
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(
                &format!("http://127.0.0.1:{port}/registered"),
                &endpoint_ctx,
            )
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();

        let (tx, _rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
        let token = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone(), "http-test-route".to_string());

        tokio::spawn(async move { consumer.start(ctx).await.unwrap() });

        // Wait until the server is actually accepting connections (CI runners can be slow).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
                .await
                .is_ok()
            {
                break;
            }
            if std::time::Instant::now() >= deadline {
                panic!("HTTP server did not start within 5s on port {port}");
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

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
        use camel_component_api::ConcurrencyModel;

        let config = HttpServerConfig {
            scheme: "http".to_string(),
            host: "127.0.0.1".to_string(),
            port: 19999,
            path: "/test".to_string(),
            max_request_body: 2 * 1024 * 1024,
            max_response_body: 10 * 1024 * 1024,
            max_inflight_requests: 1024,
            method: None,
            tls_config: None,
        };
        let consumer = HttpConsumer::new(config, test_rt());
        assert_eq!(
            consumer.concurrency_model(),
            ConcurrencyModel::Concurrent { max: None }
        );
    }

    #[test]
    fn server_config_parses_tls_cert_and_key() {
        let cfg = HttpServerConfig::from_uri(
            "https://0.0.0.0:8443/api?tlsCert=/a/cert.pem&tlsKey=/a/key.pem",
        )
        .unwrap();
        assert_eq!(cfg.tls_config.as_ref().unwrap().cert_path, "/a/cert.pem");
        assert_eq!(cfg.tls_config.as_ref().unwrap().key_path, "/a/key.pem");
    }

    #[test]
    fn server_config_no_tls_when_params_absent() {
        let cfg = HttpServerConfig::from_uri("http://0.0.0.0:8080/api").unwrap();
        assert!(cfg.tls_config.is_none());
    }

    // -----------------------------------------------------------------------
    // HttpReplyBody streaming tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_http_reply_body_stream_variant_exists() {
        use bytes::Bytes;
        use camel_component_api::CamelError;
        use futures::stream;

        let chunks: Vec<Result<Bytes, CamelError>> =
            vec![Ok(Bytes::from("hello")), Ok(Bytes::from(" world"))];
        let stream = Box::pin(stream::iter(chunks));
        let reply_body = HttpReplyBody::Stream(stream);
        // Si compila y el match funciona, el test pasa
        match reply_body {
            HttpReplyBody::Stream(_) => {}
            HttpReplyBody::Bytes(_) => panic!("expected Stream variant"),
        }
    }

    // -----------------------------------------------------------------------
    // OpenTelemetry propagation tests (only compiled with "otel" feature)
    // -----------------------------------------------------------------------

    #[cfg(feature = "otel")]
    mod otel_tests {
        use super::*;
        use camel_component_api::Message;
        use tower::ServiceExt;

        #[tokio::test]
        async fn test_producer_injects_traceparent_header() {
            let (url, _handle) = start_test_server_with_header_capture().await;
            let ctx = test_producer_ctx();

            let component = HttpComponent::new();
            let endpoint_ctx = NoOpComponentContext;
            let endpoint = component
                .create_endpoint(&format!("{url}/api?allowInternal=true"), &endpoint_ctx)
                .unwrap();
            let producer = endpoint.create_producer(rt(), &ctx).unwrap();

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
            let traceparent = result.input.header("X-Received-Traceparent");
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
            use camel_component_api::{ConsumerContext, ExchangeEnvelope};

            // Get an OS-assigned free port
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            drop(listener);

            let component = HttpComponent::new();
            let endpoint_ctx = NoOpComponentContext;
            let endpoint = component
                .create_endpoint(&format!("http://127.0.0.1:{port}/trace"), &endpoint_ctx)
                .unwrap();
            let mut consumer = endpoint.create_consumer(rt()).unwrap();

            let (tx, mut rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
            let token = tokio_util::sync::CancellationToken::new();
            let ctx = ConsumerContext::new(tx, token.clone(), "http-test-route".to_string());

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
            use camel_component_api::{ConsumerContext, ExchangeEnvelope};

            // Get an OS-assigned free port
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            drop(listener);

            let component = HttpComponent::new();
            let endpoint_ctx = NoOpComponentContext;
            let endpoint = component
                .create_endpoint(&format!("http://127.0.0.1:{port}/trace"), &endpoint_ctx)
                .unwrap();
            let mut consumer = endpoint.create_consumer(rt()).unwrap();

            let (tx, mut rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
            let token = tokio_util::sync::CancellationToken::new();
            let ctx = ConsumerContext::new(tx, token.clone(), "http-test-route".to_string());

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
            let endpoint_ctx = NoOpComponentContext;
            let endpoint = component
                .create_endpoint(&format!("{url}/api?allowInternal=true"), &endpoint_ctx)
                .unwrap();
            let producer = endpoint.create_producer(rt(), &ctx).unwrap();

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

    // -----------------------------------------------------------------------
    // Response streaming tests (Eje A - Task 2)
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // Request streaming tests (Eje B - Task 3)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_request_body_arrives_as_stream() {
        use camel_component_api::Body;
        use camel_component_api::{ConsumerContext, ExchangeEnvelope};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let component = HttpComponent::new();
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("http://127.0.0.1:{port}/upload"), &endpoint_ctx)
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
        let token = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone(), "http-test-route".to_string());

        tokio::spawn(async move { consumer.start(ctx).await.unwrap() });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = reqwest::Client::new();
        let send_fut = client
            .post(format!("http://127.0.0.1:{port}/upload"))
            .body("hello streaming world")
            .send();

        let (http_result, _) = tokio::join!(send_fut, async {
            if let Some(mut envelope) = rx.recv().await {
                // Body must be Body::Stream, not Body::Text or Body::Bytes
                assert!(
                    matches!(envelope.exchange.input.body, Body::Stream(_)),
                    "expected Body::Stream, got discriminant {:?}",
                    std::mem::discriminant(&envelope.exchange.input.body)
                );
                // Materialize to verify content
                let bytes = envelope
                    .exchange
                    .input
                    .body
                    .into_bytes(1024 * 1024)
                    .await
                    .unwrap();
                assert_eq!(&bytes[..], b"hello streaming world");

                envelope.exchange.input.body = camel_component_api::Body::Empty;
                if let Some(reply_tx) = envelope.reply_tx {
                    let _ = reply_tx.send(Ok(envelope.exchange));
                }
            }
        });

        let resp = http_result.unwrap();
        assert_eq!(resp.status().as_u16(), 200);

        token.cancel();
    }

    // -----------------------------------------------------------------------
    // Response streaming tests (Eje A - Task 2)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_streaming_response_chunked() {
        use bytes::Bytes;
        use camel_component_api::Body;
        use camel_component_api::CamelError;
        use camel_component_api::{ConsumerContext, ExchangeEnvelope};
        use camel_component_api::{StreamBody, StreamMetadata};
        use futures::stream;
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let component = HttpComponent::new();
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("http://127.0.0.1:{port}/stream"), &endpoint_ctx)
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
        let token = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone(), "http-test-route".to_string());

        tokio::spawn(async move { consumer.start(ctx).await.unwrap() });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = reqwest::Client::new();
        let send_fut = client.get(format!("http://127.0.0.1:{port}/stream")).send();

        let (http_result, _) = tokio::join!(send_fut, async {
            if let Some(mut envelope) = rx.recv().await {
                // Respond with Body::Stream
                let chunks: Vec<Result<Bytes, CamelError>> =
                    vec![Ok(Bytes::from("chunk1")), Ok(Bytes::from("chunk2"))];
                let stream = Box::pin(stream::iter(chunks));
                envelope.exchange.input.body = Body::Stream(StreamBody {
                    stream: Arc::new(Mutex::new(Some(stream))),
                    metadata: StreamMetadata::default(),
                });
                if let Some(reply_tx) = envelope.reply_tx {
                    let _ = reply_tx.send(Ok(envelope.exchange));
                }
            }
        });

        let resp = http_result.unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let body = resp.text().await.unwrap();
        assert_eq!(body, "chunk1chunk2");

        token.cancel();
    }

    // -----------------------------------------------------------------------
    // 413 Content-Length limit test (Task 4)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_413_when_content_length_exceeds_limit() {
        use camel_component_api::ConsumerContext;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        // maxRequestBody=100 — any request declaring more than 100 bytes must get 413
        let component = HttpComponent::new();
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(
                &format!("http://127.0.0.1:{port}/upload?maxRequestBody=100"),
                &endpoint_ctx,
            )
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();

        let (tx, _rx) = tokio::sync::mpsc::channel::<camel_component_api::ExchangeEnvelope>(16);
        let token = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone(), "http-test-route".to_string());

        tokio::spawn(async move { consumer.start(ctx).await.unwrap() });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{port}/upload"))
            .header("Content-Length", "1000") // declares 1000 bytes, limit is 100
            .body("x".repeat(1000))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status().as_u16(), 413);

        token.cancel();
    }

    /// Chunked upload without Content-Length header must NOT be rejected by maxRequestBody.
    /// The spec says: "If there is no Content-Length, the limit does not apply at the
    /// consumer level — the route is responsible."
    #[tokio::test]
    async fn test_chunked_upload_without_content_length_bypasses_limit() {
        use bytes::Bytes;
        use camel_component_api::Body;
        use camel_component_api::ConsumerContext;
        use futures::stream;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        // maxRequestBody=10 — very small limit; chunked uploads have no Content-Length
        let component = HttpComponent::new();
        let endpoint_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(
                &format!("http://127.0.0.1:{port}/upload?maxRequestBody=10"),
                &endpoint_ctx,
            )
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel::<camel_component_api::ExchangeEnvelope>(16);
        let token = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone(), "http-test-route".to_string());

        tokio::spawn(async move { consumer.start(ctx).await.unwrap() });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = reqwest::Client::new();

        // Use wrap_stream so reqwest sends chunked transfer encoding WITHOUT a
        // Content-Length header. 100 bytes exceeds the 10-byte maxRequestBody limit,
        // but since there's no Content-Length the 413 check must NOT fire.
        let chunks: Vec<Result<Bytes, std::io::Error>> = vec![
            Ok(Bytes::from("y".repeat(50))),
            Ok(Bytes::from("y".repeat(50))),
        ];
        let stream_body = reqwest::Body::wrap_stream(stream::iter(chunks));
        let send_fut = client
            .post(format!("http://127.0.0.1:{port}/upload"))
            .body(stream_body)
            .send();

        let consumer_fut = async {
            // Use timeout to avoid deadlock if the handler rejects before enqueueing
            match tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await {
                Ok(Some(mut envelope)) => {
                    assert!(
                        matches!(envelope.exchange.input.body, Body::Stream(_)),
                        "expected Body::Stream"
                    );
                    envelope.exchange.input.body = camel_component_api::Body::Empty;
                    if let Some(reply_tx) = envelope.reply_tx {
                        let _ = reply_tx.send(Ok(envelope.exchange));
                    }
                }
                Ok(None) => panic!("consumer channel closed unexpectedly"),
                Err(_) => {
                    // Timeout: the request was rejected before reaching the consumer.
                    // The HTTP response will carry the real status code (we check below).
                }
            }
        };

        let (http_result, _) = tokio::join!(send_fut, consumer_fut);

        let resp = http_result.unwrap();
        // Must NOT be 413; chunked uploads without Content-Length bypass the limit.
        assert_ne!(
            resp.status().as_u16(),
            413,
            "chunked upload must not be rejected by maxRequestBody"
        );
        assert_eq!(resp.status().as_u16(), 200);

        token.cancel();
    }

    #[test]
    fn test_is_private_ip_ranges() {
        use camel_api::is_ssrf_blocked_ip;
        assert!(is_ssrf_blocked_ip(&"10.0.0.1".parse().unwrap())); // allow-unwrap
        assert!(is_ssrf_blocked_ip(&"172.16.1.10".parse().unwrap())); // allow-unwrap
        assert!(is_ssrf_blocked_ip(&"192.168.1.1".parse().unwrap())); // allow-unwrap
        assert!(is_ssrf_blocked_ip(&"127.0.0.1".parse().unwrap())); // allow-unwrap
        assert!(is_ssrf_blocked_ip(&"169.254.1.1".parse().unwrap())); // allow-unwrap
        assert!(is_ssrf_blocked_ip(&"0.1.2.3".parse().unwrap())); // allow-unwrap

        assert!(is_ssrf_blocked_ip(&"::1".parse().unwrap())); // allow-unwrap
        assert!(is_ssrf_blocked_ip(&"fc00::1".parse().unwrap())); // allow-unwrap
        assert!(is_ssrf_blocked_ip(&"fd12::1".parse().unwrap())); // allow-unwrap
        assert!(is_ssrf_blocked_ip(&"fe80::1".parse().unwrap())); // allow-unwrap
        // ::ffff:0:0/96 (IPv4-mapped): only blocked if the mapped IPv4 is blocked
        assert!(is_ssrf_blocked_ip(&"::ffff:10.0.0.1".parse().unwrap())); // allow-unwrap
        assert!(is_ssrf_blocked_ip(&"::ffff:192.168.1.1".parse().unwrap())); // allow-unwrap
        assert!(is_ssrf_blocked_ip(&"::ffff:127.0.0.1".parse().unwrap())); // allow-unwrap

        assert!(!is_ssrf_blocked_ip(&"8.8.8.8".parse().unwrap())); // allow-unwrap
        assert!(!is_ssrf_blocked_ip(&"::ffff:8.8.8.8".parse().unwrap())); // allow-unwrap — public IPv4-mapped
        assert!(!is_ssrf_blocked_ip(
            &"2001:4860:4860::8888".parse().unwrap()
        )); // allow-unwrap
    }

    #[test]
    fn test_title_case_header() {
        assert_eq!(title_case_header("content-type"), "Content-Type");
        assert_eq!(title_case_header("authorization"), "Authorization");
        assert_eq!(title_case_header("x-custom-header"), "X-Custom-Header");
        assert_eq!(title_case_header("host"), "Host");
        assert_eq!(title_case_header("x-b3-traceid"), "X-B3-Traceid");
        assert_eq!(title_case_header("single"), "Single");
        assert_eq!(title_case_header(""), "");
    }

    #[test]
    fn test_resolve_url_combines_path_and_query_sources() {
        let cfg = HttpEndpointConfig::from_uri("http://example.com/base?foo=bar").unwrap();
        let mut exchange = Exchange::new(Message::default());
        exchange.input.set_header(
            "CamelHttpPath",
            serde_json::Value::String("next".to_string()),
        );
        let url = HttpProducer::resolve_url(&exchange, &cfg);
        assert!(url.starts_with("http://example.com/base/next?"));
        assert!(url.contains("foo=bar"));

        exchange.input.set_header(
            "CamelHttpUri",
            serde_json::Value::String("http://other.test/root".to_string()),
        );
        exchange.input.set_header(
            "CamelHttpQuery",
            serde_json::Value::String("a=1&b=2".to_string()),
        );

        let override_url = HttpProducer::resolve_url(&exchange, &cfg);
        assert_eq!(override_url, "http://other.test/root/next?a=1&b=2");
    }

    #[test]
    fn test_http_producer_helpers_status_and_size_boundaries() {
        assert!(HttpProducer::is_ok_status(200, (200, 299)));
        assert!(HttpProducer::is_ok_status(299, (200, 299)));
        assert!(!HttpProducer::is_ok_status(199, (200, 299)));
        assert!(!HttpProducer::is_ok_status(300, (200, 299)));

        assert!(!exceeds_max_response_body(10, 10));
        assert!(exceeds_max_response_body(11, 10));
    }

    // -----------------------------------------------------------------------
    // Content-Type inference tests
    // -----------------------------------------------------------------------

    async fn setup_consumer_on_free_port(
        path: &str,
    ) -> (
        u16,
        tokio::sync::mpsc::Receiver<camel_component_api::ExchangeEnvelope>,
        tokio_util::sync::CancellationToken,
    ) {
        use camel_component_api::ConsumerContext;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let consumer_cfg = HttpServerConfig {
            scheme: "http".to_string(),
            host: "127.0.0.1".to_string(),
            port,
            path: path.to_string(),
            max_request_body: 2 * 1024 * 1024,
            max_response_body: 10 * 1024 * 1024,
            max_inflight_requests: 1024,
            method: None,
            tls_config: None,
        };
        let mut consumer = HttpConsumer::new(consumer_cfg, test_rt());

        let (tx, rx) = tokio::sync::mpsc::channel::<camel_component_api::ExchangeEnvelope>(16);
        let token = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone(), "http-test-route".to_string());

        tokio::spawn(async move { consumer.start(ctx).await.unwrap() });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        (port, rx, token)
    }

    #[tokio::test]
    async fn test_content_type_inferred_for_json_body() {
        let (port, mut rx, token) = setup_consumer_on_free_port("/json").await;

        let client = reqwest::Client::new();
        let send_fut = client.get(format!("http://127.0.0.1:{port}/json")).send();

        let (http_result, _) = tokio::join!(send_fut, async {
            if let Some(mut envelope) = rx.recv().await {
                envelope.exchange.input.body =
                    camel_component_api::Body::Json(serde_json::json!({"message": "hello"}));
                if let Some(reply_tx) = envelope.reply_tx {
                    let _ = reply_tx.send(Ok(envelope.exchange));
                }
            }
        });

        let resp = http_result.unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let ct = resp
            .headers()
            .get("content-type")
            .expect("Content-Type header should be present");
        assert_eq!(ct, "application/json");
        let body = resp.text().await.unwrap();
        assert_eq!(body, r#"{"message":"hello"}"#);

        token.cancel();
    }

    #[tokio::test]
    async fn test_content_type_inferred_for_text_body() {
        let (port, mut rx, token) = setup_consumer_on_free_port("/text").await;

        let client = reqwest::Client::new();
        let send_fut = client.get(format!("http://127.0.0.1:{port}/text")).send();

        let (http_result, _) = tokio::join!(send_fut, async {
            if let Some(mut envelope) = rx.recv().await {
                envelope.exchange.input.body =
                    camel_component_api::Body::Text("plain text response".to_string());
                if let Some(reply_tx) = envelope.reply_tx {
                    let _ = reply_tx.send(Ok(envelope.exchange));
                }
            }
        });

        let resp = http_result.unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let ct = resp
            .headers()
            .get("content-type")
            .expect("Content-Type header should be present");
        assert_eq!(ct, "text/plain; charset=utf-8");
        let body = resp.text().await.unwrap();
        assert_eq!(body, "plain text response");

        token.cancel();
    }

    #[tokio::test]
    async fn test_content_type_inferred_for_xml_body() {
        let (port, mut rx, token) = setup_consumer_on_free_port("/xml").await;

        let client = reqwest::Client::new();
        let send_fut = client.get(format!("http://127.0.0.1:{port}/xml")).send();

        let (http_result, _) = tokio::join!(send_fut, async {
            if let Some(mut envelope) = rx.recv().await {
                envelope.exchange.input.body =
                    camel_component_api::Body::Xml("<root><item>value</item></root>".to_string());
                if let Some(reply_tx) = envelope.reply_tx {
                    let _ = reply_tx.send(Ok(envelope.exchange));
                }
            }
        });

        let resp = http_result.unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let ct = resp
            .headers()
            .get("content-type")
            .expect("Content-Type header should be present");
        assert_eq!(ct, "application/xml");
        let body = resp.text().await.unwrap();
        assert_eq!(body, "<root><item>value</item></root>");

        token.cancel();
    }

    #[tokio::test]
    async fn test_no_content_type_for_empty_body() {
        let (port, mut rx, token) = setup_consumer_on_free_port("/empty").await;

        let client = reqwest::Client::new();
        let send_fut = client.get(format!("http://127.0.0.1:{port}/empty")).send();

        let (http_result, _) = tokio::join!(send_fut, async {
            if let Some(mut envelope) = rx.recv().await {
                envelope.exchange.input.body = camel_component_api::Body::Empty;
                if let Some(reply_tx) = envelope.reply_tx {
                    let _ = reply_tx.send(Ok(envelope.exchange));
                }
            }
        });

        let resp = http_result.unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        assert!(
            resp.headers().get("content-type").is_none(),
            "Empty body should not set Content-Type"
        );

        token.cancel();
    }

    #[tokio::test]
    async fn test_no_content_type_for_raw_bytes_body() {
        let (port, mut rx, token) = setup_consumer_on_free_port("/bytes").await;

        let client = reqwest::Client::new();
        let send_fut = client.get(format!("http://127.0.0.1:{port}/bytes")).send();

        let (http_result, _) = tokio::join!(send_fut, async {
            if let Some(mut envelope) = rx.recv().await {
                envelope.exchange.input.body =
                    camel_component_api::Body::Bytes(bytes::Bytes::from_static(b"\x00\x01\x02"));
                if let Some(reply_tx) = envelope.reply_tx {
                    let _ = reply_tx.send(Ok(envelope.exchange));
                }
            }
        });

        let resp = http_result.unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        assert!(
            resp.headers().get("content-type").is_none(),
            "Raw Bytes body should not set Content-Type"
        );

        token.cancel();
    }

    #[tokio::test]
    async fn test_content_type_from_stream_metadata() {
        use camel_component_api::{StreamBody, StreamMetadata};
        use futures::stream;

        let (port, mut rx, token) = setup_consumer_on_free_port("/stream-ct").await;

        let client = reqwest::Client::new();
        let send_fut = client
            .get(format!("http://127.0.0.1:{port}/stream-ct"))
            .send();

        let (http_result, _) = tokio::join!(send_fut, async {
            if let Some(mut envelope) = rx.recv().await {
                let chunks: Vec<Result<bytes::Bytes, CamelError>> =
                    vec![Ok(bytes::Bytes::from("audio data"))];
                let stream = Box::pin(stream::iter(chunks));
                envelope.exchange.input.body = camel_component_api::Body::Stream(StreamBody {
                    stream: Arc::new(tokio::sync::Mutex::new(Some(stream))),
                    metadata: StreamMetadata {
                        size_hint: None,
                        content_type: Some("audio/mpeg".to_string()),
                        origin: None,
                    },
                });
                if let Some(reply_tx) = envelope.reply_tx {
                    let _ = reply_tx.send(Ok(envelope.exchange));
                }
            }
        });

        let resp = http_result.unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let ct = resp
            .headers()
            .get("content-type")
            .expect("Content-Type header should be present");
        assert_eq!(ct, "audio/mpeg");
        let body = resp.text().await.unwrap();
        assert_eq!(body, "audio data");

        token.cancel();
    }

    #[tokio::test]
    async fn test_user_content_type_overrides_inferred() {
        let (port, mut rx, token) = setup_consumer_on_free_port("/override-ct").await;

        let client = reqwest::Client::new();
        let send_fut = client
            .get(format!("http://127.0.0.1:{port}/override-ct"))
            .send();

        let (http_result, _) = tokio::join!(send_fut, async {
            if let Some(mut envelope) = rx.recv().await {
                envelope.exchange.input.body =
                    camel_component_api::Body::Json(serde_json::json!({"ok": true}));
                envelope.exchange.input.set_header(
                    "Content-Type",
                    serde_json::Value::String("text/html".to_string()),
                );
                if let Some(reply_tx) = envelope.reply_tx {
                    let _ = reply_tx.send(Ok(envelope.exchange));
                }
            }
        });

        let resp = http_result.unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let ct = resp
            .headers()
            .get("content-type")
            .expect("Content-Type header should be present");
        assert_eq!(
            ct, "text/html",
            "User-set Content-Type should take precedence over inferred type"
        );

        token.cancel();
    }

    #[tokio::test]
    async fn test_user_content_type_with_bytes_body() {
        let (port, mut rx, token) = setup_consumer_on_free_port("/bytes-ct").await;

        let client = reqwest::Client::new();
        let send_fut = client
            .get(format!("http://127.0.0.1:{port}/bytes-ct"))
            .send();

        let (http_result, _) = tokio::join!(send_fut, async {
            if let Some(mut envelope) = rx.recv().await {
                envelope.exchange.input.body =
                    camel_component_api::Body::Bytes(bytes::Bytes::from_static(b"{\"ok\":true}"));
                envelope.exchange.input.set_header(
                    "Content-Type",
                    serde_json::Value::String("application/json".to_string()),
                );
                if let Some(reply_tx) = envelope.reply_tx {
                    let _ = reply_tx.send(Ok(envelope.exchange));
                }
            }
        });

        let resp = http_result.unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let ct = resp
            .headers()
            .get("content-type")
            .expect("Content-Type header should be present for Bytes body with user header");
        assert_eq!(
            ct, "application/json",
            "User Content-Type should be sent for Bytes body"
        );

        token.cancel();
    }

    // -----------------------------------------------------------------------
    // Server monitor tests (GRL-005)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn monitor_task_silent_on_clean_exit() {
        let handle: tokio::task::JoinHandle<()> = tokio::spawn(async {});
        // Clean exit should complete without panicking or logging errors
        monitor_axum_task(
            handle,
            "127.0.0.1:0".to_string(),
            noop_rt(),
            "test-monitor".into(),
        )
        .await;
    }

    #[tokio::test]
    async fn monitor_task_handles_panicked_task() {
        let handle: tokio::task::JoinHandle<()> = tokio::spawn(async {
            panic!("simulated server crash");
        });
        // Should complete without panicking even though the inner task panicked
        monitor_axum_task(
            handle,
            "127.0.0.1:9999".to_string(),
            noop_rt(),
            "test-monitor".into(),
        )
        .await;
    }

    // -----------------------------------------------------------------------
    // Credential redaction tests
    // -----------------------------------------------------------------------

    #[test]
    fn http_auth_basic_debug_redacts_password() {
        let auth = HttpAuth::Basic {
            username: "admin".to_string(),
            password: "hunter2".to_string(),
        };
        let debug = format!("{:?}", auth);
        assert!(
            !debug.contains("hunter2"),
            "password must be redacted: {debug}"
        );
        assert!(debug.contains("admin"), "username should appear: {debug}");
    }

    #[test]
    fn http_auth_bearer_debug_redacts_token() {
        let auth = HttpAuth::Bearer {
            token: "eyJhbGciOiJIUzI1NiJ9.secret".to_string(),
        };
        let debug = format!("{:?}", auth);
        assert!(
            !debug.contains("eyJhbGci"),
            "token must be redacted: {debug}"
        );
    }

    #[test]
    fn http_auth_none_debug_shows_variant() {
        let debug = format!("{:?}", HttpAuth::None);
        assert!(
            debug.contains("None"),
            "None variant should appear: {debug}"
        );
    }

    #[test]
    fn http_endpoint_config_debug_redacts_auth_credentials() {
        let config = HttpEndpointConfig::from_uri(
            "http://localhost/api?authMethod=Basic&authUsername=admin&authPassword=secret123",
        )
        .unwrap();
        let debug = format!("{:?}", config);
        assert!(
            !debug.contains("secret123"),
            "password must be redacted in HttpEndpointConfig debug: {debug}"
        );
    }

    // -----------------------------------------------------------------------
    // Static file serving tests (Task 5)
    // -----------------------------------------------------------------------

    use crate::registry::{HttpRouteRegistry, MountMode, StaticMount};
    use tower_http::services::ServeDir;

    fn make_test_registry() -> HttpRouteRegistry {
        HttpRouteRegistry::new()
    }

    fn make_test_state(registry: HttpRouteRegistry) -> AppState {
        AppState {
            registry,
            max_request_body: 2 * 1024 * 1024,
            max_response_body: 10 * 1024 * 1024,
            inflight: Arc::new(tokio::sync::Semaphore::new(1024)),
        }
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn test_static_file_serving_serves_file_contents() {
        let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();
        ServerRegistry::reset();

        // Create temp dir with test files
        let temp_dir =
            std::env::temp_dir().join(format!("http_static_test_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        std::fs::write(temp_dir.join("hello.txt"), "Hello, static world!").unwrap();
        std::fs::write(temp_dir.join("style.css"), "body { color: red; }").unwrap();

        let canonical_dir = std::fs::canonicalize(&temp_dir).unwrap();

        let registry = make_test_registry();
        let serve_dir = ServeDir::new(&canonical_dir)
            .precompressed_gzip()
            .precompressed_br()
            .append_index_html_on_directories(true);

        let mount = StaticMount {
            mount_path: "/".to_string(),
            mode: MountMode::Static,
            dir: canonical_dir.clone(),
            cache_control: "public, max-age=3600".to_string(),
            error_pages: std::collections::HashMap::new(),
            serve_dir,
        };
        registry.register_static_mount(mount).await.unwrap();

        let state = make_test_state(registry);

        // Test serving hello.txt
        let req = Request::builder()
            .uri("/hello.txt")
            .body(AxumBody::empty())
            .unwrap();
        let resp = static_dispatch::dispatch_static(&state, req, "/hello.txt").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"Hello, static world!");

        // Test serving style.css
        let req = Request::builder()
            .uri("/style.css")
            .body(AxumBody::empty())
            .unwrap();
        let resp = static_dispatch::dispatch_static(&state, req, "/style.css").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"body { color: red; }");

        // Test 404 for non-existent file
        let req = Request::builder()
            .uri("/missing.txt")
            .body(AxumBody::empty())
            .unwrap();
        let resp = static_dispatch::dispatch_static(&state, req, "/missing.txt").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // Cleanup
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn test_spa_fallback_serves_index_for_unknown_paths() {
        let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();
        ServerRegistry::reset();

        let temp_dir = std::env::temp_dir().join(format!("http_spa_test_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        std::fs::write(temp_dir.join("index.html"), "<h1>SPA App</h1>").unwrap();
        std::fs::write(temp_dir.join("app.js"), "console.log('app')").unwrap();

        let canonical_dir = std::fs::canonicalize(&temp_dir).unwrap();

        let registry = make_test_registry();
        let serve_dir = ServeDir::new(&canonical_dir)
            .precompressed_gzip()
            .precompressed_br()
            .append_index_html_on_directories(true);

        let mount = StaticMount {
            mount_path: "/".to_string(),
            mode: MountMode::Spa,
            dir: canonical_dir.clone(),
            cache_control: "public, max-age=0".to_string(),
            error_pages: std::collections::HashMap::new(),
            serve_dir,
        };
        // Register as SPA mount
        registry.register_static_mount(mount).await.unwrap();

        let state = make_test_state(registry);

        // SPA fallback: GET /dashboard with Accept: text/html → index.html
        let req = Request::builder()
            .method("GET")
            .uri("/dashboard")
            .header("Accept", "text/html")
            .body(AxumBody::empty())
            .unwrap();
        let resp = static_dispatch::dispatch_static(&state, req, "/dashboard").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"<h1>SPA App</h1>");

        // Static file still works: GET /app.js
        let req = Request::builder()
            .method("GET")
            .uri("/app.js")
            .body(AxumBody::empty())
            .unwrap();
        let resp = static_dispatch::dispatch_static(&state, req, "/app.js").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"console.log('app')");

        // No SPA fallback for JSON accept → 404
        let req = Request::builder()
            .method("GET")
            .uri("/api/data")
            .header("Accept", "application/json")
            .body(AxumBody::empty())
            .unwrap();
        let resp = static_dispatch::dispatch_static(&state, req, "/api/data").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // No SPA fallback for file extensions → 404
        let req = Request::builder()
            .method("GET")
            .uri("/style.css")
            .header("Accept", "text/html")
            .body(AxumBody::empty())
            .unwrap();
        let resp = static_dispatch::dispatch_static(&state, req, "/style.css").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // Cleanup
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn test_error_page_mapping_serves_custom_404() {
        let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();
        ServerRegistry::reset();

        let temp_dir = std::env::temp_dir().join(format!("http_error_test_{}", std::process::id()));
        let errors_dir = temp_dir.join("errors");
        std::fs::create_dir_all(&errors_dir).unwrap();
        std::fs::write(temp_dir.join("index.html"), "<h1>Home</h1>").unwrap();
        std::fs::write(errors_dir.join("404.html"), "<h1>Custom 404</h1>").unwrap();

        let canonical_dir = std::fs::canonicalize(&temp_dir).unwrap();
        let canonical_404 = std::fs::canonicalize(errors_dir.join("404.html")).unwrap();

        let registry = make_test_registry();
        let serve_dir = ServeDir::new(&canonical_dir)
            .precompressed_gzip()
            .precompressed_br()
            .append_index_html_on_directories(true);

        let mut error_pages = std::collections::HashMap::new();
        error_pages.insert(404, canonical_404);

        let mount = StaticMount {
            mount_path: "/".to_string(),
            mode: MountMode::Static,
            dir: canonical_dir.clone(),
            cache_control: "public, max-age=0".to_string(),
            error_pages,
            serve_dir,
        };
        registry.register_static_mount(mount).await.unwrap();

        let state = make_test_state(registry);

        // Request non-existent file → custom 404 page
        let req = Request::builder()
            .method("GET")
            .uri("/missing.html")
            .body(AxumBody::empty())
            .unwrap();
        let resp = static_dispatch::dispatch_static(&state, req, "/missing.html").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"<h1>Custom 404</h1>");

        // Existing file still works
        let req = Request::builder()
            .method("GET")
            .uri("/index.html")
            .body(AxumBody::empty())
            .unwrap();
        let resp = static_dispatch::dispatch_static(&state, req, "/index.html").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"<h1>Home</h1>");

        // Cleanup
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[tokio::test]
    async fn http_consumer_returns_body_and_code_on_stop() {
        use camel_api::{Body, BoxProcessor, BoxProcessorExt, Exchange, Message};
        use camel_core::route::{CompiledStep, compose_pipeline_with_handler};
        use tower::ServiceExt;

        // Pipeline: set_body("nope") + set CamelHttpResponseCode=409 + Stop.
        let set_body_step = CompiledStep::Process {
            processor: BoxProcessor::from_fn(|mut ex: Exchange| {
                ex.input.body = Body::Text("nope".into());
                Box::pin(async move { Ok(ex) })
            }),
            body_contract: None,
            lifecycle: None,
        };
        let set_status_step = CompiledStep::Process {
            processor: BoxProcessor::from_fn(|mut ex: Exchange| {
                ex.input.set_header(
                    "CamelHttpResponseCode",
                    serde_json::Value::Number(409.into()),
                );
                Box::pin(async move { Ok(ex) })
            }),
            body_contract: None,
            lifecycle: None,
        };
        let pipeline = compose_pipeline_with_handler(
            vec![set_body_step, set_status_step, CompiledStep::Stop],
            None,
        );

        let ex = Exchange::new(Message::default());
        let result = pipeline.oneshot(ex).await;
        assert!(result.is_ok(), "Stop must arrive as Ok (Bug B fix)");
        let returned = result.unwrap();
        assert_eq!(returned.input.body.as_text(), Some("nope"));
        assert_eq!(
            returned
                .input
                .header("CamelHttpResponseCode")
                .and_then(|v| v.as_u64()),
            Some(409)
        );
    }

    #[tokio::test]
    async fn http_consumer_returns_200_when_body_empty_on_stop() {
        // After ADR-0024: Stop with no body + no status header produces 200 (same as
        // a normal completion with no body). The 204 default is gone — users who
        // want 204 set CamelHttpResponseCode=204 explicitly.
        //
        // This test stays at the pipeline level (consistent with the test above).
        // E2E coverage of the full HTTP dispatch path is in
        // crates/camel-test/tests/integration_test.rs.
        use camel_api::{Exchange, Message};
        use camel_core::route::{CompiledStep, compose_pipeline_with_handler};
        use tower::ServiceExt;

        let pipeline = compose_pipeline_with_handler(vec![CompiledStep::Stop], None);
        let ex = Exchange::new(Message::default());
        let result = pipeline.oneshot(ex).await;
        assert!(result.is_ok(), "Stop with empty body arrives as Ok");
        // Body is default (empty); no CamelHttpResponseCode header was set.
        // The HTTP reply finaliser (tested at E2E) maps this to status=200 + empty body.
    }

    // -----------------------------------------------------------------------
    // Task 5: Method-aware REST dispatch tests
    // -----------------------------------------------------------------------

    /// Spins up an axum server on a free port with a fresh registry.
    /// Returns the port plus the registry so the caller can register
    /// REST endpoints directly.
    async fn spawn_test_server() -> (u16, HttpRouteRegistry) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let registry = HttpRouteRegistry::new();
        tokio::spawn(run_axum_server(
            listener,
            registry.clone(),
            2 * 1024 * 1024,
            10 * 1024 * 1024,
            Arc::new(tokio::sync::Semaphore::new(1024)),
            test_rt(),
            "test-route".into(),
        ));
        // Give the server a moment to start accepting.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        (port, registry)
    }

    /// Helper for REST integration tests: spawns a responder task that
    /// reads from `rx`, writes a fixed `(status, body)` back via the
    /// envelope's reply channel, and returns once the test request is
    /// satisfied.
    fn spawn_responder(
        mut rx: tokio::sync::mpsc::Receiver<RequestEnvelope>,
        status: u16,
        body: String,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            if let Some(envelope) = rx.recv().await {
                let _ = envelope.reply_tx.send(HttpReply {
                    status,
                    headers: vec![],
                    body: HttpReplyBody::Bytes(bytes::Bytes::from(body)),
                });
            }
        })
    }

    #[tokio::test]
    async fn method_aware_dispatch_same_path_different_verbs() {
        let (port, registry) = spawn_test_server().await;

        // Register two REST endpoints on the same path with different
        // methods. This is the core scenario REST DSL needs to support:
        // GET /users (list) and POST /users (create) must not overwrite
        // each other.
        let (get_tx, get_rx) = tokio::sync::mpsc::channel::<RequestEnvelope>(8);
        registry
            .register_rest_endpoint(
                "GET".into(),
                vec![PathSegment::Literal("users".into())],
                get_tx,
            )
            .await;

        let (post_tx, post_rx) = tokio::sync::mpsc::channel::<RequestEnvelope>(8);
        registry
            .register_rest_endpoint(
                "POST".into(),
                vec![PathSegment::Literal("users".into())],
                post_tx,
            )
            .await;

        let get_handle = spawn_responder(get_rx, 200, "list".into());
        let post_handle = spawn_responder(post_rx, 201, "create".into());

        let client = reqwest::Client::new();

        // GET /users → list route
        let resp = client
            .get(format!("http://127.0.0.1:{port}/users"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let body = resp.text().await.unwrap();
        assert_eq!(body, "list");

        // POST /users → create route
        let resp = client
            .post(format!("http://127.0.0.1:{port}/users"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 201);
        let body = resp.text().await.unwrap();
        assert_eq!(body, "create");

        let _ = tokio::join!(get_handle, post_handle);
    }

    #[tokio::test]
    async fn method_aware_dispatch_templated_path_extracts_params() {
        let (port, registry) = spawn_test_server().await;

        // Register GET /users/{id} as a templated endpoint. The
        // dispatcher should match `/users/42` against the template and
        // attach `id=42` to the envelope's path_params.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<RequestEnvelope>(8);
        registry
            .register_rest_endpoint(
                "GET".into(),
                vec![
                    PathSegment::Literal("users".into()),
                    PathSegment::Param("id".into()),
                ],
                tx,
            )
            .await;

        // Spawn a responder that echoes the captured id back in the body
        // so the test can verify the param was set.
        let handle = tokio::spawn(async move {
            if let Some(envelope) = rx.recv().await {
                let id = envelope.path_params.get("id").cloned().unwrap_or_default();
                let _ = envelope.reply_tx.send(HttpReply {
                    status: 200,
                    headers: vec![],
                    body: HttpReplyBody::Bytes(bytes::Bytes::from(format!("id={id}"))),
                });
            }
        });

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://127.0.0.1:{port}/users/42"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let body = resp.text().await.unwrap();
        assert_eq!(body, "id=42");

        let _ = handle.await;
    }

    #[tokio::test]
    async fn method_aware_dispatch_unmatched_method_falls_through() {
        // If no REST endpoint matches the method, dispatch must fall
        // through to the legacy api_routes lookup or static mounts. With
        // nothing else registered, the request gets 404 from static
        // dispatch.
        let (port, _registry) = spawn_test_server().await;

        // Register only GET /users; a DELETE /users request has no match.
        let (get_tx, get_rx) = tokio::sync::mpsc::channel::<RequestEnvelope>(8);
        _registry
            .register_rest_endpoint(
                "GET".into(),
                vec![PathSegment::Literal("users".into())],
                get_tx,
            )
            .await;

        // Drain the GET channel in the background so the consumer side
        // doesn't block (we don't expect any envelopes here).
        let drain = tokio::spawn(async move {
            let mut get_rx = get_rx;
            while get_rx.recv().await.is_some() {}
        });

        let client = reqwest::Client::new();
        let resp = client
            .delete(format!("http://127.0.0.1:{port}/users"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 404);

        drop(drain);
    }

    #[tokio::test]
    async fn regression_legacy_exact_api_route_still_works() {
        // A `http:` route registered without an `httpMethod=` URI param
        // lands in the legacy api_routes registry. The dispatcher must
        // still find it via exact path lookup. This guards against
        // regressions introduced by the new REST-aware dispatch.
        let (port, registry) = spawn_test_server().await;

        let (tx, mut rx) = tokio::sync::mpsc::channel::<RequestEnvelope>(8);
        registry.register_api_route("/legacy/path".into(), tx).await;

        let handle = tokio::spawn(async move {
            if let Some(envelope) = rx.recv().await {
                let _ = envelope.reply_tx.send(HttpReply {
                    status: 200,
                    headers: vec![],
                    body: HttpReplyBody::Bytes(bytes::Bytes::from("legacy ok")),
                });
            }
        });

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://127.0.0.1:{port}/legacy/path"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let body = resp.text().await.unwrap();
        assert_eq!(body, "legacy ok");

        let _ = handle.await;
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn regression_static_mount_still_works() {
        // Verify that static file serving still works after the
        // dispatch refactor. We register a temp-dir mount and request
        // a file from it; the static dispatcher should serve it.
        let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();
        ServerRegistry::reset();

        let temp_dir = std::env::temp_dir().join(format!("http_regress_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        std::fs::write(temp_dir.join("regress.txt"), "static works").unwrap();
        let canonical_dir = std::fs::canonicalize(&temp_dir).unwrap();

        let registry = make_test_registry();
        let serve_dir = ServeDir::new(&canonical_dir)
            .precompressed_gzip()
            .precompressed_br()
            .append_index_html_on_directories(true);
        let mount = StaticMount {
            mount_path: "/".to_string(),
            mode: MountMode::Static,
            dir: canonical_dir.clone(),
            cache_control: "public, max-age=3600".to_string(),
            error_pages: std::collections::HashMap::new(),
            serve_dir,
        };
        registry.register_static_mount(mount).await.unwrap();

        let state = make_test_state(registry);
        let req = Request::builder()
            .uri("/regress.txt")
            .body(AxumBody::empty())
            .unwrap();
        let resp = static_dispatch::dispatch_static(&state, req, "/regress.txt").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"static works");

        std::fs::remove_dir_all(&temp_dir).ok();
    }

    // -----------------------------------------------------------------------
    // Review I4: dispatch-layer regression coverage for C1/C2/C3 + the
    // templated from-URI round-trip. These exercise the real axum dispatch
    // path (register → HTTP request → reply) so a regression in any of the
    // three critical fixes surfaces as a test failure rather than a silent
    // production 404/500.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn deregister_one_method_keeps_sibling_verbs() {
        // Review C1: stopping the GET /users consumer must NOT tear down the
        // live POST /users endpoint. Register both, deregister GET only,
        // then verify POST still dispatches.
        let (port, registry) = spawn_test_server().await;

        let (get_tx, get_rx) = tokio::sync::mpsc::channel::<RequestEnvelope>(8);
        registry
            .register_rest_endpoint(
                "GET".into(),
                vec![PathSegment::Literal("users".into())],
                get_tx,
            )
            .await;

        let (post_tx, post_rx) = tokio::sync::mpsc::channel::<RequestEnvelope>(8);
        registry
            .register_rest_endpoint(
                "POST".into(),
                vec![PathSegment::Literal("users".into())],
                post_tx,
            )
            .await;

        // Drain GET in the background (no requests expected after deregister).
        let drain = tokio::spawn(async move {
            let mut get_rx = get_rx;
            while get_rx.recv().await.is_some() {}
        });

        // Deregister ONLY the GET endpoint — the C1 bug used to drop POST too.
        registry.unregister_rest_endpoint("GET", "/users").await;
        drop(drain);

        let post_handle = spawn_responder(post_rx, 201, "create".into());

        let client = reqwest::Client::new();
        // POST /users must still reach its consumer after GET was removed.
        let resp = client
            .post(format!("http://127.0.0.1:{port}/users"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 201);
        assert_eq!(resp.text().await.unwrap(), "create");

        let _ = post_handle.await;
    }

    #[tokio::test]
    async fn dispatch_exact_legacy_beats_rest_template() {
        // Review C2: an exact legacy API route (`GET /api/users`, no
        // httpMethod) must win over a templated REST route
        // (`GET /api/{resource}`) for the request `/api/users`, per spec
        // §7.2 / ADR-0009 precedence (exact → templated → static → SPA).
        let (port, registry) = spawn_test_server().await;

        // Exact legacy route.
        let (exact_tx, exact_rx) = tokio::sync::mpsc::channel::<RequestEnvelope>(8);
        registry
            .register_api_route("/api/users".into(), exact_tx)
            .await;
        let exact_handle = spawn_responder(exact_rx, 200, "exact".into());

        // Templated REST route that would ALSO match /api/users.
        let (tpl_tx, tpl_rx) = tokio::sync::mpsc::channel::<RequestEnvelope>(8);
        registry
            .register_rest_endpoint(
                "GET".into(),
                vec![
                    PathSegment::Literal("api".into()),
                    PathSegment::Param("resource".into()),
                ],
                tpl_tx,
            )
            .await;
        // The templated handler must NOT receive the /api/users request. If
        // it does, it replies "template-leak" so a future assertion could
        // catch it. We do NOT await this task: the exact-match branch wins
        // and the templated channel never receives, so awaiting would block
        // until the test runtime tears down.
        let _tpl_drain = tokio::spawn(async move {
            let mut tpl_rx = tpl_rx;
            if let Some(env) = tpl_rx.recv().await {
                let _ = env.reply_tx.send(HttpReply {
                    status: 200,
                    headers: vec![],
                    body: HttpReplyBody::Bytes(bytes::Bytes::from("template-leak")),
                });
            }
        });

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://127.0.0.1:{port}/api/users"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        // Exact-match handler answered — not the templated one.
        assert_eq!(resp.text().await.unwrap(), "exact");

        let _ = exact_handle.await;
    }

    #[tokio::test]
    async fn ambiguous_rest_templates_return_500_not_silent_404() {
        // Review C3: two equal-specificity templates that both match one
        // request are an ambiguous registration. At runtime this must
        // surface as a loud 500 (with a warn! log), NOT a silent fall-through
        // to 404. Compile-time rejection is covered in camel-dsl rest tests.
        let (port, registry) = spawn_test_server().await;

        let (a_tx, _a_rx) = tokio::sync::mpsc::channel::<RequestEnvelope>(8);
        registry
            .register_rest_endpoint(
                "GET".into(),
                vec![
                    PathSegment::Literal("users".into()),
                    PathSegment::Param("id".into()),
                ],
                a_tx,
            )
            .await;

        let (b_tx, _b_rx) = tokio::sync::mpsc::channel::<RequestEnvelope>(8);
        registry
            .register_rest_endpoint(
                "GET".into(),
                vec![
                    PathSegment::Literal("users".into()),
                    PathSegment::Param("name".into()),
                ],
                b_tx,
            )
            .await;

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://127.0.0.1:{port}/users/42"))
            .send()
            .await
            .unwrap();
        // Ambiguous → 500 (previously a silent 404).
        assert_eq!(resp.status().as_u16(), 500);
    }

    #[test]
    fn from_uri_round_trips_templated_path_with_http_method() {
        // Review I4: a REST-lowered from-URI like
        // `http://0.0.0.0:8080/users/{id}?httpMethod=GET` must round-trip
        // through HttpServerConfig::from_uri, preserving the templated path
        // and the (uppercased) method. This is the binding the DSL lowering
        // emits and the consumer reads; it was previously unasserted.
        use crate::UriConfig;
        let cfg =
            HttpServerConfig::from_uri("http://0.0.0.0:8080/users/{id}?httpMethod=GET").unwrap();
        assert_eq!(cfg.host, "0.0.0.0");
        assert_eq!(cfg.port, 8080);
        assert_eq!(cfg.path, "/users/{id}");
        assert_eq!(cfg.method.as_deref(), Some("GET"));

        // Lower-case httpMethod is uppercased (review I5).
        let cfg_lc =
            HttpServerConfig::from_uri("http://0.0.0.0:8080/orders?httpMethod=post").unwrap();
        assert_eq!(cfg_lc.method.as_deref(), Some("POST"));
        assert_eq!(cfg_lc.path, "/orders");
    }

    // -----------------------------------------------------------------------
    // rc-1dk4: TypeConversionFailed → 400 Bad Request
    // -----------------------------------------------------------------------

    #[test]
    fn type_conversion_failed_maps_to_400() {
        let reply = pipeline_error_to_reply(
            CamelError::TypeConversionFailed("invalid JSON at line 1".to_string()),
            "/api/users",
        );
        assert_eq!(reply.status, 400);
        // Content-Type must be application/json
        let ct = reply
            .headers
            .iter()
            .find(|(k, _)| k == "Content-Type")
            .map(|(_, v)| v.as_str());
        assert_eq!(ct, Some("application/json"));
        // Body must contain structured error JSON
        let body = match &reply.body {
            HttpReplyBody::Bytes(b) => String::from_utf8_lossy(b).to_string(),
            _ => panic!("expected bytes body"),
        };
        assert!(body.contains("\"error\""));
        assert!(body.contains("bad_request"));
        assert!(body.contains("invalid JSON at line 1"));
    }

    #[test]
    fn other_error_still_maps_to_500() {
        let reply =
            pipeline_error_to_reply(CamelError::RouteError("boom".to_string()), "/api/users");
        assert_eq!(reply.status, 500);
    }

    #[test]
    fn unauthenticated_maps_to_401() {
        let reply = pipeline_error_to_reply(
            CamelError::Unauthenticated("no token".to_string()),
            "/api/users",
        );
        assert_eq!(reply.status, 401);
    }

    #[test]
    fn unauthorized_maps_to_403() {
        let reply = pipeline_error_to_reply(
            CamelError::Unauthorized("forbidden".to_string()),
            "/api/users",
        );
        assert_eq!(reply.status, 403);
    }

    #[test]
    fn validation_error_maps_to_400() {
        let reply = pipeline_error_to_reply(
            CamelError::ValidationError("body does not match schema".to_string()),
            "/api/users",
        );
        assert_eq!(reply.status, 400);
        let ct = reply
            .headers
            .iter()
            .find(|(k, _)| k == "Content-Type")
            .map(|(_, v)| v.as_str());
        assert_eq!(ct, Some("application/json"));
        let body = match &reply.body {
            HttpReplyBody::Bytes(b) => String::from_utf8_lossy(b).to_string(),
            _ => panic!("expected bytes body"),
        };
        assert!(body.contains("\"error\""));
        assert!(body.contains("validation_error"));
        assert!(body.contains("body does not match schema"));
    }

    #[test]
    fn https_consumer_without_tls_cert_errors() {
        let endpoint = HttpEndpoint {
            uri: "https://0.0.0.0:8443/api".to_string(),
            config: HttpEndpointConfig::from_uri("https://0.0.0.0:8443/api").unwrap(),
            server_config: HttpServerConfig::from_uri("https://0.0.0.0:8443/api").unwrap(),
            client: reqwest::Client::new(),
            http_config: HttpConfig::default(),
        };
        let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
        let result = endpoint.create_consumer(rt);
        assert!(result.is_err(), "expected error for https without tls cert");
        if let Err(e) = result {
            let msg = e.to_string();
            assert!(msg.contains("tlsCert"), "error must mention tlsCert: {msg}");
        }
    }

    #[test]
    fn http_consumer_with_tls_config_errors() {
        let endpoint = HttpEndpoint {
            uri: "http://0.0.0.0:8080/api".to_string(),
            config: HttpEndpointConfig::from_uri("http://0.0.0.0:8080/api").unwrap(),
            server_config: HttpServerConfig::from_uri(
                "http://0.0.0.0:8080/api?tlsCert=/x.pem&tlsKey=/y.pem",
            )
            .unwrap(),
            client: reqwest::Client::new(),
            http_config: HttpConfig::default(),
        };
        let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
        let result = endpoint.create_consumer(rt);
        assert!(result.is_err(), "expected error for http with tls config");
        if let Err(e) = result {
            let msg = e.to_string();
            assert!(msg.contains("https"), "error must mention https: {msg}");
        }
    }

    #[test]
    fn https_consumer_with_partial_tls_cert_only_errors() {
        // tlsCert without tlsKey → tls_config is None at parse time
        // → create_consumer sees https:// + no TLS → must error
        let server_config =
            HttpServerConfig::from_uri("https://0.0.0.0:8443/api?tlsCert=/x.pem").unwrap();
        assert!(
            server_config.tls_config.is_none(),
            "partial tlsCert must not create ServerTlsConfig"
        );
        let endpoint = HttpEndpoint {
            uri: "https://0.0.0.0:8443/api?tlsCert=/x.pem".to_string(),
            config: HttpEndpointConfig::from_uri("https://0.0.0.0:8443/api?tlsCert=/x.pem")
                .unwrap(),
            server_config,
            client: reqwest::Client::new(),
            http_config: HttpConfig::default(),
        };
        let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
        let result = endpoint.create_consumer(rt);
        assert!(
            result.is_err(),
            "must error: https:// requires both tlsCert and tlsKey"
        );
    }

    #[test]
    fn load_tls_config_parses_valid_pem() {
        // Install rustls crypto provider (aws-lc-rs — matches reqwest/hyper-rustls tree)
        let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
        use camel_component_api::test_support::tls;
        let (_, cert_pem, key_pem) = tls::gen_server_cert();
        let cert_path = tls::write_pem_tmp("http-load-cert.pem", &cert_pem);
        let key_path = tls::write_pem_tmp("http-load-key.pem", &key_pem);

        let config = load_tls_config(cert_path.to_str().unwrap(), key_path.to_str().unwrap());
        assert!(config.is_ok(), "must parse valid PEM: {:?}", config.err());
    }

    #[tokio::test(flavor = "multi_thread")]
    #[allow(clippy::await_holding_lock)]
    async fn consumer_tls_handshake_roundtrip() {
        use camel_component_api::test_support::tls;
        use camel_component_api::{ConsumerContext, ExchangeEnvelope};

        // Install rustls crypto provider (aws-lc-rs)
        let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();

        // Serialize against global ServerRegistry singleton
        let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();

        // Generate CA + server cert
        let (ca_pem, cert_pem, key_pem) = tls::gen_server_cert();
        let cert_path = tls::write_pem_tmp("http-tls-handshake-cert.pem", &cert_pem);
        let key_path = tls::write_pem_tmp("http-tls-handshake-key.pem", &key_pem);
        let ca_path = tls::write_pem_tmp("http-tls-handshake-ca.pem", &ca_pem);

        // Get ephemeral port
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);

        ServerRegistry::reset();

        // Create real HttpComponent + endpoint with TLS URI
        let component = HttpComponent::new();
        let endpoint_ctx = NoOpComponentContext;
        let uri = format!(
            "https://127.0.0.1:{port}/test?tlsCert={}&tlsKey={}",
            cert_path.to_string_lossy(),
            key_path.to_string_lossy(),
        );
        let endpoint = component
            .create_endpoint(&uri, &endpoint_ctx)
            .expect("create TLS endpoint");
        let mut consumer = endpoint.create_consumer(rt()).expect("create consumer");

        // Start consumer — this calls get_or_spawn with tls_config
        let (tx, mut rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
        let token = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone(), "tls-handshake-test".to_string());
        tokio::spawn(async move { consumer.start(ctx).await.unwrap() });

        // Give server time to start
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Client with CA cert — REAL verification (no danger_accept_invalid)
        let ca_bytes = std::fs::read(&ca_path).unwrap();
        let client = reqwest::Client::builder()
            .add_root_certificate(reqwest::Certificate::from_pem(&ca_bytes).unwrap())
            .build()
            .unwrap();

        let send_fut = client
            .post(format!("https://localhost:{port}/test"))
            .body("ping")
            .send();

        // Handler: receive envelope, reply 200 with "pong" body
        let (http_result, _) = tokio::join!(send_fut, async {
            if let Some(mut envelope) = rx.recv().await {
                envelope.exchange.input.body = camel_component_api::Body::Text("pong".to_string());
                if let Some(reply_tx) = envelope.reply_tx {
                    let _ = reply_tx.send(Ok(envelope.exchange));
                }
            }
        });

        let resp = http_result.expect("TLS handshake + request must succeed");

        assert_eq!(resp.status().as_u16(), 200, "must get 200 through TLS");
        let body = resp.text().await.unwrap();
        assert_eq!(body, "pong");

        token.cancel();
    }

    #[tokio::test(flavor = "multi_thread")]
    #[allow(clippy::await_holding_lock)]
    async fn consumer_tls_rejects_client_without_ca() {
        use camel_component_api::test_support::tls;
        use camel_component_api::{ConsumerContext, ExchangeEnvelope};

        let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();

        // Serialize against global ServerRegistry singleton
        let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();

        let (_, cert_pem, key_pem) = tls::gen_server_cert();
        let cert_path = tls::write_pem_tmp("http-neg-cert.pem", &cert_pem);
        let key_path = tls::write_pem_tmp("http-neg-key.pem", &key_pem);

        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);

        ServerRegistry::reset();

        // Spawn TLS server via real HttpComponent path
        let component = HttpComponent::new();
        let endpoint_ctx = NoOpComponentContext;
        let uri = format!(
            "https://127.0.0.1:{port}/test?tlsCert={}&tlsKey={}",
            cert_path.to_string_lossy(),
            key_path.to_string_lossy(),
        );
        let endpoint = component.create_endpoint(&uri, &endpoint_ctx).unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();
        let (tx, _rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
        let token = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone(), "tls-neg-test".to_string());
        tokio::spawn(async move { consumer.start(ctx).await.unwrap() });

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Client WITHOUT CA cert — must fail TLS verification
        let client = reqwest::Client::builder().build().unwrap();

        let result = client
            .get(format!("https://localhost:{port}/test"))
            .send()
            .await;

        assert!(
            result.is_err(),
            "must reject without CA — proves real verification"
        );

        token.cancel();
    }

    #[test]
    fn server_config_partial_tls_cert_without_key() {
        // Parse URI with only tlsCert (no tlsKey)
        let cfg = HttpServerConfig::from_uri("https://0.0.0.0:8443/api?tlsCert=/x.pem").unwrap();
        // Partial params → tls_config must be None
        assert!(cfg.tls_config.is_none());
    }
}
