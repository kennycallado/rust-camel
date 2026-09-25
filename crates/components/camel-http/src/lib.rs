pub mod bundle;
pub(crate) mod client_cache;
pub mod config;
mod header_policy;
pub mod health;
pub mod registry;
pub(crate) mod rest_match;
pub(crate) mod ssrf;
pub mod static_config;
pub mod static_dispatch;
pub mod static_endpoint;
pub(crate) mod tls;
#[cfg(test)]
mod tls_harness;
pub(crate) mod tls_reload;
use crate::config::parse_ok_status_code_range;
pub use bundle::HttpBundle;
pub use bundle::HttpStaticBundle;
pub(crate) use client_cache::{
    HttpComponentKind, PINNED_CLIENT_MAX_ENTRIES, PINNED_CLIENT_TTL, PinnedClientCache,
};
pub use config::HttpConfig;
pub use health::HttpHealthCheck;
pub use registry::HttpRouteRegistry;
pub use static_config::HttpStaticConfig;
pub use static_endpoint::{HttpStaticComponent, HttpStaticConsumer, HttpStaticEndpoint};
#[cfg(test)]
pub(crate) use tls::{
    FORCE_FALLBACK_REBUILD_FAIL, FORCE_WEBPKI_FALLBACK, build_client_call_count,
    build_client_fallback_count,
};
#[cfg(test)]
// Doc-link shim: tls_harness.rs links `crate::FallbackTrigger::Forced` and
// `crate::webpki_fallback_client` from doc comments only — no code use, so
// the re-export must be allowed unused to keep those links resolving.
#[allow(unused_imports)]
pub(crate) use tls::{FallbackTrigger, webpki_fallback_client};
pub(crate) use tls::{build_client, client_or_emergency, strict_tls_error};

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
use camel_api::component_metadata::ComponentMetadata;
use camel_auth::bearer_token_layer::BearerTokenLayer;
use camel_auth::oauth2::TokenProvider;
use camel_component_api::tls_source::ServerTlsSource;
use camel_component_api::{Body, BoxProcessor, CamelError, Exchange, StreamBody, StreamMetadata};
use camel_component_api::{Component, Consumer, Endpoint, ProducerContext, RuntimeObservability};
use camel_component_api::{UriComponents, UriConfig, parse_uri, raw_query_pairs};
use futures::StreamExt;
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
#[derive(Clone)]
pub struct HttpEndpointConfig {
    pub base_url: String,
    pub http_method: Option<String>,
    pub throw_exception_on_failure: bool,
    pub ok_status_code_range: (u16, u16),
    pub response_timeout: Option<Duration>,
    /// Programmatic query parameters, serialized in declaration order with
    /// minimal RFC-3986 encoding (`%20`, never `+`). Never populated from
    /// the endpoint URI — set by callers via config construction.
    pub query_params: Vec<(String, String)>,
    /// Authored query bytes from the endpoint URI, verbatim (no decode, no
    /// re-encode, no `RAW(...)` unwrapping). `Some("")` preserves a bare
    /// `?` marker. Sole carrier of URI-authored pairs; consumed option
    /// keys are filtered out at serialization time.
    pub raw_query: Option<String>,
    pub allow_internal: bool,
    /// Public-cleartext transport consent (ADR-0081): when `false` (the
    /// default), cleartext `http://` requests to PUBLIC targets are
    /// rejected. Independent of `allow_internal` — internal-network
    /// consent and cleartext-transport consent are separate decisions, and
    /// there is deliberately no global cleartext lever.
    pub allow_cleartext: bool,
    pub blocked_hosts: Vec<String>,
    pub max_body_size: usize,
    pub read_timeout_ms: u64,
    pub max_response_bytes: usize,
    pub auth: HttpAuth,
    pub token_provider: Option<Arc<dyn TokenProvider>>,
    pub user_agent: Option<String>,
    pub bridge_endpoint: bool,
    pub connection_close: bool,
    pub skip_request_headers: Vec<String>,
    pub skip_response_headers: Vec<String>,
    pub follow_redirects: bool,
    pub max_redirects: usize,
    /// CamelHttpUri host fence (`allowedUriHosts`): `None` when the option
    /// is absent (override behavior unchanged); `Some` arms the fail-closed
    /// fence. Parsed entries only — never re-serialized into the outbound
    /// query.
    pub allowed_uri_hosts: Option<Vec<AllowedUriHost>>,
}

/// ADR-0051 redact-by-construction, ADR-0076 strictest-wins: query bytes
/// (authored `raw_query` and programmatic `query_params`) may carry
/// credentials. The display-surface Debug renders the raw view
/// blanket-masked (mirroring `redact_url_for_diagnostics`) and programmatic
/// values masked, mirroring `UriComponents`' sensitive-value masking.
/// `base_url` routes through the canonical
/// [`camel_api::redact::redact_url`] (string surgery, no `url::Url`
/// roundtrip, so authored bytes are never WHATWG-normalized): userinfo is
/// masked in every authority window, query and fragment bytes are dropped
/// behind their sentinels, and the result is capped at 256 bytes (rc-yvjp3
/// converged the former byte-preserving local variant). Wire fidelity is
/// unaffected.
impl std::fmt::Debug for HttpEndpointConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpEndpointConfig")
            .field("base_url", &camel_api::redact::redact_url(&self.base_url))
            .field("http_method", &self.http_method)
            .field(
                "throw_exception_on_failure",
                &self.throw_exception_on_failure,
            )
            .field("ok_status_code_range", &self.ok_status_code_range)
            .field("response_timeout", &self.response_timeout)
            .field(
                "query_params",
                &self
                    .query_params
                    .iter()
                    .map(|(key, _)| (key, "***"))
                    .collect::<Vec<_>>(),
            )
            .field("raw_query", &self.raw_query.as_ref().map(|_| "?[redacted]"))
            .field("allow_internal", &self.allow_internal)
            .field("allow_cleartext", &self.allow_cleartext)
            .field("blocked_hosts", &self.blocked_hosts)
            .field("max_body_size", &self.max_body_size)
            .field("read_timeout_ms", &self.read_timeout_ms)
            .field("max_response_bytes", &self.max_response_bytes)
            .field("auth", &self.auth)
            .field("token_provider", &self.token_provider)
            .field("user_agent", &self.user_agent)
            .field("bridge_endpoint", &self.bridge_endpoint)
            .field("connection_close", &self.connection_close)
            .field("skip_request_headers", &self.skip_request_headers)
            .field("skip_response_headers", &self.skip_response_headers)
            .field("follow_redirects", &self.follow_redirects)
            .field("max_redirects", &self.max_redirects)
            .field("allowed_uri_hosts", &self.allowed_uri_hosts)
            .finish()
    }
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

/// Whether `key` names a camel-http endpoint option consumed at parse time.
///
/// Single metadata-driven owner of OUTBOUND option filtering (ADR-0041):
/// derived from the `#[uri_param]` metadata behind
/// [`HttpEndpointConfig::uri_options`], so the raw query filter consumes
/// exactly the keys the component documents — no duplicated handwritten
/// key lists. `from_components`'s manual typed parsing stays direct and
/// unchanged; this predicate never re-wires it.
fn is_consumed_option(key: &str) -> bool {
    HttpEndpointConfig::uri_options()
        .iter()
        .any(|option| option.name == key || option.aliases.iter().any(|alias| alias == key))
}

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

        // Public-cleartext transport consent (ADR-0081) — per-endpoint
        // only, deliberately never inherited from the global HttpConfig.
        let allow_cleartext = match parts.params.get("allowCleartext") {
            Some(v) => parse_bool_param_http(v).map_err(|e| {
                CamelError::InvalidUri(format!("invalid value for allowCleartext: {e}"))
            })?,
            None => false, // Default: reject cleartext http:// to public targets
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

        if parts.params.contains_key("cookieHandling") {
            return Err(CamelError::InvalidUri(
                "cookieHandling is not supported".into(),
            ));
        }

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

        // CamelHttpUri host fence: parsed eagerly so a malformed or empty
        // allowlist fails endpoint creation (fail-closed), not resolution.
        let allowed_uri_hosts = match parts.params.get("allowedUriHosts") {
            Some(v) => Some(parse_allowed_uri_hosts(v)?),
            None => None,
        };

        // Authored pairs ride raw_query verbatim (the sole carrier);
        // query_params is programmatic-only — never auto-populated from
        // URI leftovers. Consumed option keys are filtered at
        // serialization time by `is_consumed_option`.
        let raw_query = parts.raw_query.clone();

        Ok(Self {
            base_url,
            http_method,
            throw_exception_on_failure,
            ok_status_code_range,
            response_timeout,
            query_params: Vec::new(),
            raw_query,
            allow_internal,
            allow_cleartext,
            blocked_hosts,
            max_body_size,
            read_timeout_ms,
            max_response_bytes,
            auth,
            token_provider: None,
            user_agent,
            bridge_endpoint,
            connection_close,
            skip_request_headers,
            skip_response_headers,
            follow_redirects,
            max_redirects,
            allowed_uri_hosts,
        })
    }
}

/// Private container for macro-derived `uri_options()` and `metadata()`.
///
/// Mirrors the URI query parameters parsed by `HttpEndpointConfig::from_components`.
/// `HttpEndpointConfig` holds typed fields (tuples, `Duration`, `HttpAuth`,
/// `Arc<dyn TokenProvider>`) that the derive cannot represent, so metadata
/// derivation targets this inner type whose fields are all URI-param-compatible.
#[derive(Debug, Clone, UriConfig)]
#[allow(dead_code)]
#[uri_scheme = "http"]
#[uri_config(
    skip_impl,
    metadata(
        scheme = "http",
        description = "HTTP client and server component",
        producer,
        consumer,
        streaming
    ),
    crate = "camel_component_api"
)]
struct HttpEndpointUriConfig {
    #[allow(dead_code)]
    _base_url: String,

    #[uri_param(
        name = "httpMethod",
        desc = "HTTP method. Defaults to CamelHttpMethod header or POST/GET"
    )]
    http_method: Option<String>,

    #[uri_param(
        name = "throwExceptionOnFailure",
        default = "true",
        desc = "Throw on non-2xx status"
    )]
    throw_exception_on_failure: bool,

    #[uri_param(
        name = "okStatusCodeRange",
        default = "200-299",
        desc = "Success status code range"
    )]
    ok_status_code_range: String,

    #[uri_param(name = "responseTimeout", desc = "Response timeout in milliseconds")]
    response_timeout: Option<u64>,

    #[uri_param(
        name = "connectTimeout",
        desc = "Connection timeout in milliseconds (consumed option; effective timeout comes from the global http config)"
    )]
    connect_timeout: Option<u64>,

    #[uri_param(
        name = "allowInternal",
        default = "false",
        desc = "Allow private/internal network destinations (SSRF)"
    )]
    allow_internal: bool,

    #[uri_param(
        name = "allowCleartext",
        default = "false",
        desc = "Allow cleartext http:// to public targets (transport consent; ADR-0081)"
    )]
    allow_cleartext: bool,

    #[uri_param(name = "blockedHosts", desc = "Comma-separated blocked host list")]
    blocked_hosts: Option<String>,

    #[uri_param(
        name = "maxBodySize",
        default = "10485760",
        desc = "Max request/response body bytes"
    )]
    max_body_size: u64,

    #[uri_param(name = "readTimeout", desc = "Socket read timeout in milliseconds")]
    read_timeout: Option<u64>,

    #[uri_param(name = "maxResponseBytes", desc = "Max response body bytes")]
    max_response_bytes: Option<u64>,

    #[uri_param(
        name = "authMethod",
        kind = "enum:None,Basic,Bearer",
        desc = "Authentication method"
    )]
    auth_method: Option<String>,

    #[uri_param(name = "authUsername", secret, desc = "Basic auth username")]
    auth_username: Option<String>,

    #[uri_param(name = "authPassword", secret, desc = "Basic auth password")]
    auth_password: Option<String>,

    #[uri_param(name = "authBearerToken", secret, desc = "Bearer auth token")]
    auth_bearer_token: Option<String>,

    #[uri_param(name = "userAgent", desc = "User-Agent header")]
    user_agent: Option<String>,

    #[uri_param(
        name = "bridgeEndpoint",
        default = "false",
        desc = "Bridge endpoint mode"
    )]
    bridge_endpoint: bool,

    #[uri_param(
        name = "connectionClose",
        default = "false",
        desc = "Send Connection: close"
    )]
    connection_close: bool,

    #[uri_param(
        name = "skipRequestHeaders",
        desc = "Comma-separated request headers to skip"
    )]
    skip_request_headers: Option<String>,

    #[uri_param(
        name = "skipResponseHeaders",
        desc = "Comma-separated response headers to skip"
    )]
    skip_response_headers: Option<String>,

    #[uri_param(
        name = "followRedirects",
        default = "false",
        desc = "Follow HTTP redirects"
    )]
    follow_redirects: bool,

    #[uri_param(name = "maxRedirects", default = "10", desc = "Max redirect hops")]
    max_redirects: u64,

    #[uri_param(
        name = "allowedUriHosts",
        desc = "Comma-separated allowlist of CamelHttpUri override hosts (host or host:port)"
    )]
    allowed_uri_hosts: Option<String>,
}

impl HttpEndpointConfig {
    /// Component metadata for the http/https scheme, derived from the
    /// `#[uri_param]` fields on `HttpEndpointUriConfig`.
    pub fn metadata() -> ComponentMetadata {
        HttpEndpointUriConfig::metadata()
    }

    /// URI option definitions, derived from `#[uri_param]` fields.
    pub fn uri_options() -> Vec<camel_api::component_metadata::UriOption> {
        HttpEndpointUriConfig::uri_options()
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
    /// bd rc-ns3yc: values above `tokio::sync::Semaphore::MAX_PERMITS` are
    /// rejected at parse, `create_consumer`, start, and `spawn_entry` (the
    /// inflight semaphore panics above that bound); `0` remains a
    /// representable reject-everything value.
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
        // bd rc-ns3yc: reject values above the semaphore primitive's bound
        // at parse time — `Semaphore::new` panics above MAX_PERMITS
        // (mirror of grpc consumer_concurrency_limit, commit 101327e5).
        let max_inflight_requests = max_inflight_requests_limit(max_inflight_requests)?;

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
    /// Actual local address of the served listening socket (differs from the
    /// configured `host:port` when spawning from a staged/pre-bound listener).
    bound_addr: std::net::SocketAddr,
    max_request_body: usize,
    max_response_body: usize,
    max_inflight_requests: usize,
    is_tls: bool,
    tls_cert_path: Option<String>,
    tls_key_path: Option<String>,
    /// JoinHandle for the monitor_axum_task wrapper. `is_finished()` is the
    /// dead-server eviction signal in `get_or_spawn`.
    monitor_task: tokio::task::JoinHandle<()>,
    /// Abort handle for the Axum server task itself. The JoinHandle is
    /// consumed by `monitor_axum_task`; this survives on the handle so
    /// crashed-server tests (and future ops tooling) can deterministically
    /// kill the shared transport to exercise the death path.
    /// Test-only today — no production reader yet (rc-szmob).
    #[allow(dead_code)]
    server_abort: tokio::task::AbortHandle,
    // Retained so the reload handler (Task 7) can call reload_from_config()
    // to hot-swap certs without restarting the server.
    tls_config: Option<axum_server::tls_rustls::RustlsConfig>,
    tls_source: Option<ServerTlsSource>,
}

/// Internal registry state: live server entries plus pre-bound listeners
/// staged for consumption by the next spawn on the same key.
#[derive(Default)]
struct RegistryState {
    entries: HashMap<ServerKey, Arc<OnceCell<ServerHandle>>>,
    staged: HashMap<ServerKey, tokio::net::TcpListener>,
}

/// Process-global registry mapping (host, port) → running Axum server handle.
pub struct ServerRegistry {
    inner: Mutex<RegistryState>,
}

impl ServerRegistry {
    /// Returns the global singleton.
    pub fn global() -> &'static Self {
        static INSTANCE: OnceLock<ServerRegistry> = OnceLock::new();
        INSTANCE.get_or_init(|| ServerRegistry {
            inner: Mutex::new(RegistryState::default()),
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
        self.get_or_spawn_internal(
            host,
            port,
            max_request_body,
            max_response_body,
            max_inflight_requests,
            runtime,
            route_id,
            tls_config,
            None,
        )
        .await
    }

    /// Like [`ServerRegistry::get_or_spawn`], but serves `listener` instead
    /// of binding `host:port`. The registry key is derived from the listener's
    /// actual local address, so callers must query that port afterwards. If an
    /// entry for the key already holds a live server, the same compatibility
    /// checks as `get_or_spawn` apply and the entry is reused; the passed
    /// listener is simply dropped.
    #[allow(clippy::too_many_arguments)]
    pub async fn get_or_spawn_with_listener(
        &'static self,
        listener: tokio::net::TcpListener,
        max_request_body: usize,
        max_response_body: usize,
        max_inflight_requests: usize,
        runtime: Arc<dyn RuntimeObservability>,
        route_id: String,
        tls_config: Option<crate::config::ServerTlsConfig>,
    ) -> Result<HttpRouteRegistry, CamelError> {
        let addr = listener
            .local_addr()
            .map_err(|e| CamelError::EndpointCreationFailed(format!("listener local_addr: {e}")))?;
        self.get_or_spawn_internal(
            &addr.ip().to_string(),
            addr.port(),
            max_request_body,
            max_response_body,
            max_inflight_requests,
            runtime,
            route_id,
            tls_config,
            Some(listener),
        )
        .await
    }

    /// Stage a pre-bound listener so the next `get_or_spawn` for its
    /// `(ip, port)` key serves this socket instead of binding a new one.
    ///
    /// The staged listener is consumed by exactly one spawn: the exact-key
    /// `get_or_spawn` takes it under the registry lock, eliminating the bind
    /// window between a port probe and server startup (itest-bound-ports).
    pub async fn stage_listener(
        &'static self,
        listener: tokio::net::TcpListener,
    ) -> Result<(), CamelError> {
        let addr = listener
            .local_addr()
            .map_err(|e| CamelError::EndpointCreationFailed(format!("listener local_addr: {e}")))?;
        let host = addr.ip().to_string();
        use std::collections::hash_map::Entry;
        let mut guard = self.inner.lock().map_err(|_| {
            CamelError::EndpointCreationFailed("ServerRegistry lock poisoned".into())
        })?;
        match guard.staged.entry((host.clone(), addr.port())) {
            Entry::Occupied(_) => Err(CamelError::EndpointCreationFailed(format!(
                "listener already staged for {host}:{}",
                addr.port()
            ))),
            Entry::Vacant(slot) => {
                slot.insert(listener);
                Ok(())
            }
        }
    }

    /// Returns the bound address of the live server entry for `(host, port)`,
    /// if one is initialized.
    pub fn bound_addr(&'static self, host: &str, port: u16) -> Option<std::net::SocketAddr> {
        let guard = self.inner.lock().ok()?;
        guard
            .entries
            .get(&(host.to_string(), port))
            .and_then(|cell| cell.get())
            .map(|handle| handle.bound_addr)
    }

    #[allow(clippy::too_many_arguments)]
    async fn get_or_spawn_internal(
        &'static self,
        host: &str,
        port: u16,
        max_request_body: usize,
        max_response_body: usize,
        max_inflight_requests: usize,
        runtime: Arc<dyn RuntimeObservability>,
        route_id: String,
        tls_config: Option<crate::config::ServerTlsConfig>,
        provided: Option<tokio::net::TcpListener>,
    ) -> Result<HttpRouteRegistry, CamelError> {
        let host_owned = host.to_string();
        let key = (host.to_string(), port);

        let cell = {
            let mut guard = self.inner.lock().map_err(|_| {
                CamelError::EndpointCreationFailed("ServerRegistry lock poisoned".into())
            })?;
            // Evict dead server so a fresh one can spawn (matches gRPC D-L2 pattern).
            // The monitor task awaits the server task, so monitor_task.is_finished()
            // is a reliable proxy for the server being gone (either crashed or aborted).
            if let Some(existing) = guard.entries.get(&key)
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
                guard.entries.remove(&key);
            }
            guard
                .entries
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
                let key = (host_owned.clone(), port);
                async move {
                    // Resolve the listener source inside the init body so
                    // exactly one caller — the init winner — consumes a
                    // staged listener. Resolving it before the cell init let
                    // a racing caller strand the staged socket in the
                    // loser's hands: the winner then bound the same port and
                    // failed with EADDRINUSE. The sync registry lock here is
                    // never held across an await. Occupied cells never run
                    // this body, so they never touch the staged map.
                    let source = match provided {
                        Some(listener) => ListenerSource::Staged(listener),
                        None => {
                            let mut guard = self.inner.lock().map_err(|_| {
                                CamelError::EndpointCreationFailed(
                                    "ServerRegistry lock poisoned".into(),
                                )
                            })?;
                            match guard.staged.remove(&key) {
                                Some(listener) => ListenerSource::Staged(listener),
                                // Conflict check before any entry is
                                // initialized so the error leaves the staged
                                // slot untouched.
                                None => {
                                    if let Some((staged_host, _)) = guard
                                        .staged
                                        .keys()
                                        .find(|(_, staged_port)| *staged_port == port)
                                    {
                                        let staged_host = staged_host.clone();
                                        return Err(CamelError::EndpointCreationFailed(
                                            format!(
                                                "staged listener conflict on port {port}: staged under host {staged_host}, requested {host_owned}"
                                            ),
                                        ));
                                    }
                                    ListenerSource::Bind
                                }
                            }
                        }
                    };
                    spawn_entry(
                        key,
                        source,
                        max_request_body,
                        max_response_body,
                        max_inflight_requests,
                        rt,
                        rid,
                        tls_config,
                    )
                    .await
                    .and_then(|handle| {
                        // spawn_entry returns a freshly created Arc (refcount
                        // 1), so unwrapping it back into the owned handle for
                        // the cell always succeeds here.
                        Arc::try_unwrap(handle).map_err(|_| {
                            CamelError::EndpointCreationFailed(
                                "spawned server handle has dangling clones".into(),
                            )
                        })
                    })
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
            host = camel_api::redact::redact_host(host),
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
        guard.entries.clear();
        guard.staged.clear();
    }
}

/// Where a spawned server's listening socket comes from: a fresh bind on
/// `key`, or a listener pre-bound (staged or passed) by the caller.
enum ListenerSource {
    Bind,
    Staged(tokio::net::TcpListener),
}

/// Create the server handle for a vacant registry entry: serve `key` via a
/// freshly bound or caller-provided listener. This is the OnceCell init body
/// of `get_or_spawn`, extracted so the legacy and staged entry points share
/// one spawn path.
#[allow(clippy::too_many_arguments)]
async fn spawn_entry(
    key: ServerKey,
    source: ListenerSource,
    max_request_body: usize,
    max_response_body: usize,
    max_inflight_requests: usize,
    runtime: Arc<dyn RuntimeObservability>,
    route_id: String,
    tls_config: Option<crate::config::ServerTlsConfig>,
) -> Result<Arc<ServerHandle>, CamelError> {
    // bd rc-ns3yc: defense-in-depth — validate before any listener side
    // effect (fresh bind or staged-listener consumption), before the
    // registry/CancellationToken work, and immediately guarding the
    // `tokio::sync::Semaphore::new` construction below (panics above
    // MAX_PERMITS).
    max_inflight_requests_limit(max_inflight_requests)?;
    let rt = Arc::clone(&runtime);
    let rid = route_id.clone();
    let (host_owned, port) = key;
    let listener = match source {
        ListenerSource::Bind => {
            let addr = format!("{host_owned}:{port}");
            tokio::net::TcpListener::bind(&addr).await.map_err(|e| {
                CamelError::EndpointCreationFailed(format!("Failed to bind {addr}: {e}"))
            })?
        }
        ListenerSource::Staged(listener) => listener,
    };
    let bound_addr = listener
        .local_addr()
        .map_err(|e| CamelError::EndpointCreationFailed(format!("listener local_addr: {e}")))?;
    let server_exited = tokio_util::sync::CancellationToken::new();
    let registry = HttpRouteRegistry::new_with_server_exited(server_exited.clone());
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
        let rustls_cfg =
            axum_server::tls_rustls::RustlsConfig::from_config(std::sync::Arc::new(rustls_config));
        tls_rustls_cfg = Some(rustls_cfg.clone());
        tls_source = Some(source);
        // Convert tokio listener to std for axum-server
        let std_listener = listener.into_std().map_err(|e| {
            CamelError::EndpointCreationFailed(format!("TLS listener conversion: {e}"))
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
    let server_abort = server_task.abort_handle();
    let monitor_task = tokio::spawn(monitor_axum_task(
        server_task,
        addr_for_monitor,
        Arc::clone(&rt),
        rid,
        server_exited,
    ));
    let handle = ServerHandle {
        registry,
        bound_addr,
        max_request_body,
        max_response_body,
        max_inflight_requests,
        is_tls: tls_config.is_some(),
        tls_cert_path: tls_config.as_ref().map(|t| t.cert_path.clone()),
        tls_key_path: tls_config.as_ref().map(|t| t.key_path.clone()),
        monitor_task,
        server_abort,
        tls_config: tls_rustls_cfg,
        tls_source,
    };
    // Register reload handler (exactly-once: inside OnceCell init closure).
    // Note: HTTP servers are process-lifetime (no release/eviction path),
    // so handlers are never unregistered. If eviction is added later,
    // add TlsReloadRegistry::global().unregister() there.
    if let (Some(tls_cfg), Some(source)) = (handle.tls_config.as_ref(), handle.tls_source.as_ref())
    {
        let handler = Arc::new(crate::tls_reload::HttpReloadHandler::new(
            tls_cfg.clone(),
            source.clone(),
            host_owned.clone(),
            port,
        ));
        camel_component_api::tls_source::TlsReloadRegistry::global().register(handler);
    }
    Ok(Arc::new(handle))
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

/// Hard wall-clock limit for one inbound request on the consumer side
/// (audit 2026-08-31, F2-1). A slow-drip client otherwise holds an
/// `inflight` semaphore permit (and its connection) indefinitely, starving
/// the consumer into 503s. 30s matches the documented component default
/// timeouts. Applies to the whole dispatch; streaming bodies are additionally
/// protected by the byte cap in `dispatch_handler`.
const CONSUMER_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

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
    let app = Router::new()
        .fallback(dispatch_handler)
        .with_state(state)
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            CONSUMER_REQUEST_TIMEOUT,
        ));

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
    let app = Router::new()
        .fallback(dispatch_handler)
        .with_state(state)
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            CONSUMER_REQUEST_TIMEOUT,
        ));

    // RustlsConfig is now constructed once in get_or_spawn and retained on
    // ServerHandle so the reload handler can call reload_from_config() on it.

    // axum-server 0.8: from_tcp_rustls is fallible (TLS acceptor setup).
    let server = match axum_server::from_tcp_rustls(listener, tls_cfg) {
        Ok(server) => server,
        Err(e) => {
            runtime
                .metrics()
                .increment_errors(&route_id, "e:http:accept-tls");
            // log-policy: outside-contract
            tracing::error!(error = %e, "Axum TLS server setup error");
            return;
        }
    };

    server
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

/// Monitors the shared Axum server task of one (host, port).
///
/// On unexpected exit (panic or abort) it records the structured error
/// event and cancels the server's `server_exited` token. Every
/// `HttpConsumer` hosted on that server observes the cancellation in its
/// `start()` loop and returns `Err`, which camel-core's consumer watcher
/// turns into a per-route `CrashNotification` → `FailRoute` → supervision
/// backoff restart (ADR-0007). A clean exit (`Ok(())` — process shutdown)
/// cancels nothing: route stops own their termination.
async fn monitor_axum_task(
    handle: tokio::task::JoinHandle<()>,
    addr: String,
    runtime: Arc<dyn RuntimeObservability>,
    route_id: String,
    server_exited: tokio_util::sync::CancellationToken,
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
            // Fail every hosted route's consumer: each `start()` returns Err
            // and camel-core emits one CrashNotification per route (ADR-0007
            // parity with per-route transport death).
            server_exited.cancel();
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

        // Build StreamBody from Axum body WITHOUT materializing.
        // SECURITY (audit 2026-08-31, F2-1): the Content-Length pre-check above
        // cannot see chunked/no-length requests. Wrap the stream with a hard
        // byte cap so ANY downstream consumption fails closed once
        // max_request_body is exceeded — the cap travels with the body.
        let content_type = headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let data_stream: BodyDataStream = req.into_body().into_data_stream();
        let max_body = state.max_request_body;
        let mut seen: u64 = 0;
        let capped_stream =
            data_stream
                .map_err(|e| CamelError::Io(e.to_string()))
                .map(move |chunk| match chunk {
                    Ok(bytes) => {
                        seen = seen.saturating_add(bytes.len() as u64);
                        if seen > max_body as u64 {
                            Err(CamelError::ProcessorError(format!(
                                "Request body exceeds configured limit of {max_body} bytes"
                            )))
                        } else {
                            Ok(bytes)
                        }
                    }
                    Err(e) => Err(e),
                });
        let boxed: BoxStream<'static, Result<bytes::Bytes, CamelError>> = Box::pin(capped_stream);

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

/// Kernel authentication state captured from a route's [`SecurityContext`]
/// (`unify-transport-auth`, Task 2.9).
///
/// Same construction-order lifecycle as gRPC's `GrpcKernelAuth` (Task 2.1):
/// the compiled plan and the provider registry arrive via
/// `Consumer::set_security_context` before `start()` accepts requests. A
/// context lacking either piece keeps `kernel = None` — a plan without
/// providers can never mint a principal (fail-closed, never a silently
/// unauthenticated route: the controller's strict-mode dispatch check then
/// denies carrier-less Exchanges on non-Public plans).
pub(crate) struct HttpKernelAuth {
    pub(crate) plan: camel_api::security_policy::RouteSecurityPlan,
    pub(crate) providers: Arc<camel_auth::ProviderRegistry>,
}

impl HttpKernelAuth {
    /// Capture the kernel state from a route's security context.
    ///
    /// `None` unless both the compiled plan and the provider registry are
    /// present.
    pub(crate) fn from_security_context(
        ctx: &camel_component_api::SecurityContext,
    ) -> Option<Self> {
        Some(Self {
            plan: ctx.plan.clone()?,
            providers: ctx.providers.clone()?,
        })
    }
}

/// Capacity for the per-route RequestEnvelope channel.
///
/// Each in-flight request holds exactly one `maxInflightRequests` semaphore
/// permit from before `send()` until its reply, so at most N envelopes can be
/// outstanding at any time. A buffer of N therefore can never fill before the
/// semaphore exhausts: dispatcher `send()` calls never park on a full buffer
/// and the semaphore stays the single, URI-configurable backpressure point.
/// A hardcoded smaller buffer would act as a second, hidden inflight cap
/// (rc-3y6j: 64 vs default 1024 permits).
///
/// `max(1)`: `maxInflightRequests=0` is a representable "reject everything"
/// configuration, but `tokio::sync::mpsc::channel(0)` panics — keep consumer
/// start panic-free (the empty semaphore still 503s every request).
fn envelope_channel_capacity(max_inflight_requests: usize) -> usize {
    max_inflight_requests.max(1)
}

/// Upper bound check for `maxInflightRequests`.
///
/// bd rc-ns3yc: `tokio::sync::Semaphore::new` panics for permit counts above
/// [`tokio::sync::Semaphore::MAX_PERMITS`], so any representable `usize`
/// above that bound must be rejected fail-closed with a typed configuration
/// error naming the parameter, the configured value, and the limit — before
/// the semaphore primitive is ever constructed. Enforced at URI parse,
/// `create_consumer`, consumer start, and `spawn_entry` (defense-in-depth);
/// mirror of the grpc `consumer_concurrency_limit` fix (commit 101327e5).
///
/// `0` is deliberately NOT normalized here (rc-3y6j): it remains a
/// representable reject-everything value (`envelope_channel_capacity`
/// separately guards the one primitive — the envelope channel — that
/// cannot take 0).
pub(crate) fn max_inflight_requests_limit(configured: usize) -> Result<usize, CamelError> {
    if configured > tokio::sync::Semaphore::MAX_PERMITS {
        return Err(CamelError::Config(format!(
            "maxInflightRequests {configured} exceeds the supported upper bound {} (tokio::sync::Semaphore::MAX_PERMITS)",
            tokio::sync::Semaphore::MAX_PERMITS
        )));
    }
    Ok(configured)
}

pub struct HttpConsumer {
    config: HttpServerConfig,
    /// Runtime observability handle for ADR-0012 metrics and health calls.
    runtime: Arc<dyn RuntimeObservability>,
    /// Kernel authentication state (plan + providers), set via
    /// `set_security_context` before `start()` (Task 2.9). `None` for routes
    /// without route-level security (Public under the per-bind gate).
    kernel: Option<Arc<HttpKernelAuth>>,
}

impl HttpConsumer {
    pub fn new(config: HttpServerConfig, runtime: Arc<dyn RuntimeObservability>) -> Self {
        Self {
            config,
            runtime,
            kernel: None,
        }
    }
}

#[async_trait::async_trait]
impl Consumer for HttpConsumer {
    async fn start(&mut self, ctx: camel_component_api::ConsumerContext) -> Result<(), CamelError> {
        use camel_component_api::{Body, Exchange, Message};

        // bd rc-ns3yc: fail-closed BEFORE shared-server registry
        // interaction, listener binding, envelope-channel construction, or
        // inflight-semaphore construction — an oversized value must never
        // reach Semaphore::new (panic).
        max_inflight_requests_limit(self.config.max_inflight_requests)?;

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

        // Create channel for this path and register it. Capacity matches the
        // dispatcher's inflight semaphore (see envelope_channel_capacity) so
        // the channel can never become a second backpressure point.
        let (env_tx, mut env_rx) = tokio::sync::mpsc::channel::<RequestEnvelope>(
            envelope_channel_capacity(self.config.max_inflight_requests),
        );
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

        // rc-w1u9: Signal readiness AFTER (1) TcpListener::bind succeeded
        // (inside get_or_spawn above), (2) the axum server task was spawned,
        // and (3) this route's path/REST endpoint was registered. At this
        // point the listener is genuinely accepting connections and any
        // request to this route will be dispatched (not 404'd). The runtime
        // uses this signal to publish RouteStarted and to release
        // ctx.start() so external benchmarks can emit a reliable
        // listener-bound marker.
        ctx.mark_ready();

        // rc-nftni (drainclaim): capture the context-global counter once;
        // every envelope this raw-sender consumer constructs carries a
        // claim minted at the acceptance dequeue below.
        let in_flight = ctx.in_flight_counter();

        let path = self.config.path.clone();
        let registry_for_cleanup = registry.clone();
        let server_exited = registry.server_exited.clone();
        let cancel_token = ctx.cancel_token();
        let kernel = self.kernel.clone();
        // Set when the loop exits because the shared server died. The
        // post-loop cleanup still runs, then `start()` returns Err so
        // camel-core's consumer watcher emits a CrashNotification for THIS
        // route and supervision backoff engages (ADR-0007).
        let mut server_died = false;
        loop {
            tokio::select! {
                _ = ctx.cancelled() => {
                    break;
                }
                _ = server_exited.cancelled() => {
                    // Shared transport death: this route's consumer cannot
                    // continue. Fail (do NOT hang in Running) — parity with
                    // per-route transport death, which also surfaces as a
                    // consumer-task error.
                    server_died = true;
                    break;
                }
                 envelope = env_rx.recv() => {
                    let Some(envelope) = envelope else { break; };

                    // rc-nftni: mint at acceptance — the dequeue of the
                    // dispatcher's RequestEnvelope is where this consumer
                    // takes ownership of the wire request. The claim is held
                    // across the authn await, the route channel, and the
                    // pipeline; every early exit in the per-request task
                    // (cancel-503, auth denial) drops it, and a failed push
                    // rolls it back with the dropped envelope (RAII).
                    let claim =
                        in_flight.as_ref().map(camel_component_api::InFlightClaim::attach);

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
                    // Task 2.9 boundary-auth inputs: the raw header map and
                    // the request URI (path + query) feed kernel credential
                    // extraction inside the per-request task.
                    let auth_headers = envelope.headers.clone();
                    let auth_uri: http::Uri = {
                        let full = if envelope.query.is_empty() {
                            envelope.path.clone()
                        } else {
                            format!("{}?{}", envelope.path, envelope.query)
                        };
                        // A malformed path cannot become a valid `Uri`; the
                        // empty default then carries no credentials, so
                        // extraction finds nothing and authn fails closed.
                        full.parse().unwrap_or_default()
                    };
                    let kernel = kernel.clone();

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

                        // ADR-0061 Task 2.9: kernel authentication at the
                        // request boundary. A `Public` plan passes through
                        // with no extraction; any other mode extracts per
                        // the plan's sources, authenticates through the
                        // kernel, and installs the typed carrier BEFORE the
                        // pipeline runs. A denial renders in the HTTP idiom
                        // (401 via `pipeline_error_to_reply`) and the route
                        // body never sees the request.
                        if let Some(kernel) = kernel.as_ref()
                            && !matches!(
                                kernel.plan.access_mode,
                                camel_api::security_policy::AccessMode::Public
                            )
                        {
                            let principal = match camel_auth::extract_token_multi(
                                &auth_headers,
                                &auth_uri,
                                &kernel.plan.credential_sources,
                            ) {
                                Some(extracted) => {
                                    match camel_auth::kernel_authenticate(
                                        &kernel.plan,
                                        &kernel.providers,
                                        &extracted,
                                    )
                                    .await
                                    {
                                        Ok(principal) => principal,
                                        Err(e) => {
                                            // log-policy: handler-owned
                                            tracing::warn!(
                                                path = %path_clone,
                                                error = %e,
                                                "HTTP request authentication failed"
                                            );
                                            let _ = reply_tx.send(pipeline_error_to_reply(
                                                e,
                                                &path_clone,
                                            ));
                                            return;
                                        }
                                    }
                                }
                                None => {
                                    // log-policy: handler-owned
                                    tracing::warn!(
                                        path = %path_clone,
                                        "HTTP request rejected: no credential found in any source"
                                    );
                                    let _ = reply_tx.send(pipeline_error_to_reply(
                                        CamelError::Unauthenticated(
                                            "no credential found in any source".to_string(),
                                        ),
                                        &path_clone,
                                    ));
                                    return;
                                }
                            };
                            camel_auth::install_carrier(&mut exchange, &principal);
                        }

                        // Send through pipeline and await result
                        let (tx, rx) = tokio::sync::oneshot::channel();
                        let envelope = camel_component_api::consumer::ExchangeEnvelope {
                            exchange,
                            reply_tx: Some(tx),
                            // rc-nftni: the acceptance-minted claim rides the
                            // envelope; the pipeline drain sites take it and
                            // hold it across the pipeline (release at
                            // completion; rejection paths above already
                            // dropped it).
                            in_flight_claim: claim,
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
                                    // Empty and future variants produce an empty reply body.
                                    _ => (HttpReplyBody::Bytes(bytes::Bytes::new()), None),
                                };

                                let resp_headers = select_response_headers(
                                    &out.input.headers,
                                    user_content_type,
                                    inferred_content_type,
                                );

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

        // Leave the shared-server entry: `unregister` is a no-op today (no
        // refcount exists — stale D-L10 wording removed, rc-szmob review).
        // Dead servers are evicted lazily by `get_or_spawn_internal`, which
        // checks `monitor_task.is_finished()` and rebinds on the next spawn
        // (e.g. a supervision restart after this consumer's Err).
        ServerRegistry::global()
            .unregister(&self.config.host, self.config.port)
            .await;

        if server_died {
            // log-policy: system-broken
            tracing::error!(
                host = %camel_api::redact::redact_host(&self.config.host),
                port = self.config.port,
                path = %path,
                "Shared HTTP server exited — failing consumer to engage route supervision (ADR-0007)"
            );
            // The error value is logged upstream by supervision (ADR-0076):
            // the host must ride the canonical masker, message structure
            // unchanged (bd rc-8bxeo item 3).
            return Err(CamelError::RouteError(format!(
                "shared HTTP server for {}:{} exited unexpectedly; route transport is dead",
                camel_api::redact::redact_host(&self.config.host),
                self.config.port
            )));
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }

    fn concurrency_model(&self) -> camel_component_api::ConcurrencyModel {
        camel_component_api::ConcurrencyModel::Concurrent { max: None }
    }

    // rc-w1u9: HTTP consumer binds a TcpListener inside start() (via
    // ServerRegistry::get_or_spawn) and only THEN can it accept connections.
    // Opting into Explicit startup makes ctx.start() await the bind+register
    // completion so listeners fail fast on bind errors (previously a silent
    // background log) and external markers can reliably detect listener-bound
    // state.
    fn startup_mode(&self) -> camel_component_api::ConsumerStartupMode {
        camel_component_api::ConsumerStartupMode::Explicit
    }

    // Task 2.9: capture the kernel state (compiled plan + provider registry)
    // wired by the route controller before start(). See `HttpKernelAuth`.
    fn set_security_context(&mut self, ctx: camel_component_api::SecurityContext) {
        self.kernel = HttpKernelAuth::from_security_context(&ctx).map(Arc::new);
    }
}

// ---------------------------------------------------------------------------
// HttpComponent / HttpsComponent
// ---------------------------------------------------------------------------

pub struct HttpComponent {
    config: HttpConfig,
    pinned_cache: std::sync::Arc<PinnedClientCache>,
    client: reqwest::Client,
    /// Set at construction when `tls.strict` is on and the configured
    /// material fails to load; surfaced as an endpoint-creation failure
    /// (rc-ayrwk).
    strict_tls_error: Option<CamelError>,
}

impl HttpComponent {
    pub fn new() -> Self {
        let config = HttpConfig::default();
        let strict_err = strict_tls_error(&config);
        let (client, build_err) = client_or_emergency(&config);
        Self {
            client,
            config,
            pinned_cache: std::sync::Arc::new(PinnedClientCache::new(
                PINNED_CLIENT_TTL,
                PINNED_CLIENT_MAX_ENTRIES,
            )),
            strict_tls_error: strict_err.or(build_err),
        }
    }

    pub fn with_config(config: HttpConfig) -> Self {
        let strict_err = strict_tls_error(&config);
        let (client, build_err) = client_or_emergency(&config);
        Self {
            client,
            config,
            pinned_cache: std::sync::Arc::new(PinnedClientCache::new(
                PINNED_CLIENT_TTL,
                PINNED_CLIENT_MAX_ENTRIES,
            )),
            strict_tls_error: strict_err.or(build_err),
        }
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
        HttpEndpointConfig::metadata()
    }

    fn create_endpoint(
        &self,
        uri: &str,
        ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        if let Some(err) = &self.strict_tls_error {
            return Err(err.clone());
        }
        self.config.validate()?;
        let config = HttpEndpointConfig::from_uri_with_defaults(uri, &self.config)?;
        let server_config = HttpServerConfig::from_uri_with_defaults(uri, &self.config)?;
        ctx.register_current_route_health_check(Arc::new(HttpHealthCheck::new(
            server_config.host.clone(),
            server_config.port,
        )));
        self.pinned_cache
            .wire(HttpComponentKind::Http, ctx.metrics());
        Ok(Box::new(HttpEndpoint {
            uri: uri.to_string(),
            config,
            server_config,
            client: self.client.clone(),
            pinned_cache: std::sync::Arc::clone(&self.pinned_cache),
            http_config: self.config.clone(),
        }))
    }
}

pub struct HttpsComponent {
    config: HttpConfig,
    pinned_cache: std::sync::Arc<PinnedClientCache>,
    client: reqwest::Client,
    /// Set at construction when `tls.strict` is on and the configured
    /// material fails to load; surfaced as an endpoint-creation failure
    /// (rc-ayrwk).
    strict_tls_error: Option<CamelError>,
}

impl HttpsComponent {
    pub fn new() -> Self {
        let config = HttpConfig::default();
        let strict_err = strict_tls_error(&config);
        let (client, build_err) = client_or_emergency(&config);
        Self {
            client,
            config,
            pinned_cache: std::sync::Arc::new(PinnedClientCache::new(
                PINNED_CLIENT_TTL,
                PINNED_CLIENT_MAX_ENTRIES,
            )),
            strict_tls_error: strict_err.or(build_err),
        }
    }

    pub fn with_config(config: HttpConfig) -> Self {
        let strict_err = strict_tls_error(&config);
        let (client, build_err) = client_or_emergency(&config);
        Self {
            client,
            config,
            pinned_cache: std::sync::Arc::new(PinnedClientCache::new(
                PINNED_CLIENT_TTL,
                PINNED_CLIENT_MAX_ENTRIES,
            )),
            strict_tls_error: strict_err.or(build_err),
        }
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
        // HTTPS shares the same URI option surface and capabilities as HTTP.
        // Only the scheme and description differ.
        let mut meta = HttpEndpointConfig::metadata();
        meta.scheme = "https".to_string();
        meta.description = "HTTPS client and server component (TLS over HTTP)".to_string();
        meta
    }

    fn create_endpoint(
        &self,
        uri: &str,
        ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        if let Some(err) = &self.strict_tls_error {
            return Err(err.clone());
        }
        self.config.validate()?;
        let config = HttpEndpointConfig::from_uri_with_defaults(uri, &self.config)?;
        let server_config = HttpServerConfig::from_uri_with_defaults(uri, &self.config)?;
        ctx.register_current_route_health_check(Arc::new(HttpHealthCheck::new(
            server_config.host.clone(),
            server_config.port,
        )));
        self.pinned_cache
            .wire(HttpComponentKind::Https, ctx.metrics());
        Ok(Box::new(HttpEndpoint {
            uri: uri.to_string(),
            config,
            server_config,
            client: self.client.clone(),
            pinned_cache: std::sync::Arc::clone(&self.pinned_cache),
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
    pinned_cache: std::sync::Arc<PinnedClientCache>,
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
        // bd rc-ns3yc: fail-closed BEFORE any HttpConsumer is constructed —
        // a directly-built HttpServerConfig never passed the parse seam, and
        // the inflight semaphore panics above the primitive bound
        // (mirror of grpc consumer_concurrency_limit, commit 101327e5).
        max_inflight_requests_limit(self.server_config.max_inflight_requests)?;
        Ok(Box::new(HttpConsumer::new(self.server_config.clone(), rt)))
    }

    fn create_producer(
        &self,
        rt: Arc<dyn camel_component_api::RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        let producer = HttpProducer {
            config: Arc::new(self.config.clone()),
            client: self.client.clone(),
            pinned_cache: std::sync::Arc::clone(&self.pinned_cache),
            http_config: Arc::new(self.http_config.clone()),
            runtime: rt,
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
    pinned_cache: std::sync::Arc<PinnedClientCache>,
    http_config: Arc<HttpConfig>,
    /// Runtime observability handle powering the component-ops facade at
    /// the request boundary (`("http","request")`, dashboard-observability
    /// Task 4.3). The retained `e:http:accept*` labels are consumer-side
    /// (server accept loop) — different boundary, no collision with
    /// `e:http:request`.
    runtime: Arc<dyn RuntimeObservability>,
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

    fn resolve_url(exchange: &Exchange, config: &HttpEndpointConfig) -> Result<String, CamelError> {
        // bridgeEndpoint=true: exchange URL headers (CamelHttpUri,
        // CamelHttpPath, CamelHttpQuery) are ignored per Apache Camel
        // bridging semantics. The endpoint's own query still rides: the
        // same raw-preserving, consumed-option-filtered query as the
        // non-bridge path (bridgeEndpoint itself is a consumed option),
        // with programmatic query_params appending absent keys after the
        // raw base. This check MUST come before the CamelHttpUri override
        // so bridging wins over that header.
        if config.bridge_endpoint {
            let Some(query) = resolve_endpoint_query(config)? else {
                return Ok(config.base_url.clone());
            };
            // Validation only (rc-ph7z2): a malformed base still errors
            // through the redacted-diagnostic path below. The parsed value
            // is NEVER re-emitted — assembly is verbatim string
            // composition, authored bytes end-to-end: no WHATWG
            // normalization (dot-segment collapse, default-port strip,
            // scheme/host lowercasing), matching every other arm (Papal
            // Direction A).
            let _: url::Url = url::Url::parse(&config.base_url).map_err(|e| {
                CamelError::ProcessorError(format!(
                    "invalid base URL '{}': {e}",
                    redact_url_for_diagnostics(&config.base_url)
                ))
            })?;
            let mut url = config.base_url.clone();
            url.push('?');
            url.push_str(&query);
            return Ok(url);
        }

        if let Some(uri) = exchange
            .input
            .header("CamelHttpUri")
            .and_then(|v| v.as_str())
        {
            // Host fence (allowedUriHosts): opt-in, fail-closed. Evaluated
            // on the raw override before any path/query assembly; a
            // rejection renders the URL only through the diagnostics
            // redaction path (ADR-0051).
            if let Some(fence) = &config.allowed_uri_hosts
                && !uri_host_allowed(uri, fence)?
            {
                return Err(CamelError::ProcessorError(format!(
                    "CamelHttpUri host not allowed by allowedUriHosts fence: {}",
                    redact_url_for_diagnostics(uri)
                )));
            }
            // The override replaces the base URL; its own query is the
            // higher-precedence source for composition (ADR-0071) — the
            // endpoint base query does not ride an override. Split at the
            // first `?` so CamelHttpPath applies to the path component
            // and the queries merge at pair level, never a second `?`
            // marker.
            let (base, override_query) = match uri.split_once('?') {
                Some((base, query)) => (base, Some(query)),
                None => (uri, None),
            };
            // Resolve-time span validation for the override URI's own query
            // (rc-m4xk1): a forbidden byte is a resolve error naming the
            // byte, never a verbatim ride that later surfaces as a reqwest
            // send error. Covers both downstream arms — the verbatim push
            // and merge_header_query, which validates only the header side.
            if let Some(query) = override_query {
                for (_key, span) in raw_query_pairs(query)? {
                    validate_raw_query_span(span)?;
                }
            }
            let mut url = base.to_string();
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
                if let Some(merged) = merge_header_query(override_query, query)? {
                    url.push('?');
                    url.push_str(&merged);
                }
                return Ok(url);
            }
            if let Some(query) = override_query {
                url.push('?');
                url.push_str(query);
            }
            return Ok(url);
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
            // Compose: the endpoint query (raw-preserving,
            // consumed-option-filtered) comes first and wins collisions;
            // header pairs append verbatim for absent keys (ADR-0071).
            // An empty header leaves the endpoint query unchanged.
            if let Some(merged) =
                merge_header_query(resolve_endpoint_query(config)?.as_deref(), query)?
            {
                url.push('?');
                url.push_str(&merged);
            }
            return Ok(url);
        }

        if let Some(query) = resolve_endpoint_query(config)? {
            url.push('?');
            url.push_str(&query);
        }

        Ok(url)
    }

    fn is_ok_status(status: u16, range: (u16, u16)) -> bool {
        status >= range.0 && status <= range.1
    }
}

/// One allowlist entry of the `CamelHttpUri` host fence (`allowedUriHosts`
/// endpoint option). DNS hosts are stored ASCII-lowercased; IPv6 literals
/// in bracketed canonical form (the `url` crate's host serialization). A
/// `port` of `None` is a host-only entry and permits any port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllowedUriHost {
    /// Canonical host: lowercased DNS name or bracketed IPv6 literal.
    pub host: String,
    /// `Some` pins the entry to one effective port; `None` permits any.
    pub port: Option<u16>,
}

/// Parse the `allowedUriHosts` option value: comma-separated `host` or
/// `host:port` entries. Bracketed IPv6 is supported (`[::1]:8443`, bare
/// `[::1]` host-only). Empty segments are dropped. Segments are parsed
/// through the `url` crate (with an `http://` scheme injected) so DNS
/// names are lowercased and ports range-checked; anything it rejects is a
/// malformed entry. A value yielding zero valid entries is also an error.
/// Both failure modes fail endpoint creation (fail-closed).
fn parse_allowed_uri_hosts(raw: &str) -> Result<Vec<AllowedUriHost>, CamelError> {
    let mut entries = Vec::new();
    for segment in raw.split(',') {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        let parsed = url::Url::parse(&format!("http://{segment}"))
            .map_err(|_| invalid_allowed_uri_host_entry(segment))?;
        // A segment carrying a path or userinfo is a typo'd entry — the
        // spec's "any other malformed entry" clause. Silently narrowing it
        // to its hostname would widen or skew the fence.
        if parsed.path() != "/" || !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(invalid_allowed_uri_host_entry(segment));
        }
        let Some(host) = parsed.host_str() else {
            return Err(invalid_allowed_uri_host_entry(segment));
        };
        entries.push(AllowedUriHost {
            host: host.to_string(),
            port: parsed.port(),
        });
    }
    if entries.is_empty() {
        return Err(CamelError::InvalidUri(
            "allowedUriHosts declares no valid host entries".to_string(),
        ));
    }
    Ok(entries)
}

fn invalid_allowed_uri_host_entry(segment: &str) -> CamelError {
    CamelError::InvalidUri(format!("invalid allowedUriHosts entry '{segment}'"))
}

/// Whether `url_str` matches the fence. Parse failure or a host-less URL
/// is fail-closed (`Ok(false)`). DNS hosts compare case-insensitively
/// (both sides are lowercased by the `url` crate); IPv6 compares in
/// bracketed canonical form. A host-only entry permits any port; a
/// `host:port` entry matches only the effective port — the explicit port
/// or the scheme default (443 for https, 80 for http).
pub(crate) fn uri_host_allowed(
    url_str: &str,
    fence: &[AllowedUriHost],
) -> Result<bool, CamelError> {
    let Ok(parsed) = url::Url::parse(url_str) else {
        return Ok(false);
    };
    let Some(host) = parsed.host_str() else {
        return Ok(false);
    };
    let effective_port = parsed.port().or(match parsed.scheme() {
        "https" => Some(443_u16),
        "http" => Some(80),
        _ => None,
    });
    Ok(fence.iter().any(|entry| {
        entry.host == host
            && match entry.port {
                None => true,
                Some(port) => effective_port == Some(port),
            }
    }))
}

/// Serialize the outbound query for the endpoint base.
///
/// Authored raw pairs come first, byte-for-byte minus consumed option keys
/// (order, separators and authored escapes — including `RAW(...)` text —
/// preserved); then programmatic `query_params` entries whose key is absent
/// from the authored pairs, in declaration order with minimal RFC-3986
/// encoding (`%20`, never `+`). Authored keys always win — no duplication,
/// no override.
///
/// Returns `Ok(None)` when no query component is emitted: no pairs at all,
/// or a non-empty raw query whose every pair was consumed. A bare `?`
/// marker (`raw_query == Some("")`) always emits the query component.
fn resolve_endpoint_query(config: &HttpEndpointConfig) -> Result<Option<String>, CamelError> {
    let mut parts: Vec<String> = Vec::new();
    let mut authored_keys = std::collections::HashSet::new();

    if let Some(raw) = config.raw_query.as_deref() {
        for (key, span) in raw_query_pairs(raw)? {
            authored_keys.insert(key.clone());
            if is_consumed_option(&key) {
                continue;
            }
            validate_raw_query_span(span)?;
            parts.push(span.to_string());
        }
    }

    for (key, value) in &config.query_params {
        if !authored_keys.contains(key.as_str()) {
            parts.push(format!(
                "{}={}",
                encode_query_component(key),
                encode_query_component(value)
            ));
        }
    }

    if parts.is_empty() && config.raw_query.as_deref() != Some("") {
        return Ok(None);
    }
    Ok(Some(parts.join("&")))
}

/// Compose the outbound query when a `CamelHttpQuery` exchange header is
/// present (ADR-0071). `higher_precedence` — the endpoint query in the
/// base arm, the override URI's own query in the override arm — comes
/// first and wins any key collision; header pairs append verbatim for
/// absent keys only. An empty header leaves the higher-precedence query
/// unchanged (no additional `?` marker). Header spans are validated, not
/// re-encoded: a byte forbidden in a query component is a resolve error
/// naming the byte (Wave-A law).
fn merge_header_query(
    higher_precedence: Option<&str>,
    header_query: &str,
) -> Result<Option<String>, CamelError> {
    if header_query.is_empty() {
        return Ok(higher_precedence.map(str::to_string));
    }
    let mut parts: Vec<String> = Vec::new();
    let mut higher_keys = std::collections::HashSet::new();
    for (key, span) in raw_query_pairs(higher_precedence.unwrap_or(""))? {
        higher_keys.insert(key);
        parts.push(span.to_string());
    }
    for (key, span) in raw_query_pairs(header_query)? {
        validate_raw_query_span(span)?;
        if !higher_keys.contains(key.as_str()) {
            parts.push(span.to_string());
        }
    }
    if parts.is_empty() {
        return Ok(None);
    }
    Ok(Some(parts.join("&")))
}

/// Bytes that may appear unescaped in a URI query component. RFC 3986
/// (`query = *( pchar / "/" / "?" )`) admits unreserved, sub-delims, `:`,
/// `@`, `/`, `?`, and `%` — with ONE deliberate exclusion from the RFC set:
/// the apostrophe (`'`, 0x27). reqwest's WHATWG URL parser re-encodes 0x27
/// to `%27` in the special-query percent-encode set (http/https), so an
/// authored apostrophe can never ride the wire verbatim; admitting it would
/// silently normalize authored bytes (rc-nmupb). Authors write `%27`
/// explicitly when they mean the byte on the wire. The WHATWG set's other
/// extras (`"`, `` ` ``, `<`, `>`) are already rejected here — they are not
/// RFC 3986 query-legal bytes, so no special exclusion is needed for them.
fn is_legal_query_byte(byte: u8) -> bool {
    matches!(byte,
        b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z'
        | b'-' | b'.' | b'_' | b'~'
        | b'!' | b'$' | b'&' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'='
        | b':' | b'@' | b'/' | b'?'
        | b'%')
}

/// Reject an authored raw pair carrying a byte that is not legal in a query
/// component (e.g. literal space, `#`, non-ASCII). The serializer never
/// silently re-encodes operator-authored bytes: "byte-for-byte" is bounded
/// to wire-legal bytes, and the check fires before the resolved string
/// reaches any consumer (SSRF pre-check, diagnostics redaction).
fn validate_raw_query_span(span: &str) -> Result<(), CamelError> {
    for &byte in span.as_bytes() {
        if !is_legal_query_byte(byte) {
            return Err(CamelError::ProcessorError(format!(
                "raw query pair '{span}' contains byte 0x{byte:02X}, which is not legal in a URL query component"
            )));
        }
    }
    Ok(())
}

/// Minimal RFC-3986 percent-encoding for one programmatic query component:
/// unreserved bytes pass through, every other byte encodes as uppercase
/// hex. A space encodes as `%20`, never `+`.
fn encode_query_component(component: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(component.len());
    for &byte in component.as_bytes() {
        match byte {
            b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push('%');
                out.push(HEX[(byte >> 4) as usize] as char);
                out.push(HEX[(byte & 0x0f) as usize] as char);
            }
        }
    }
    out
}

/// Redact credentials from a URL before it reaches logs or error values
/// (ADR-0051 redact-by-construction). Masks userinfo (`user:pass@`) and
/// the query string (which commonly carries API keys/tokens). Host and
/// path stay visible for diagnosability. Fragments are never echoed: a
/// fragment (OAuth2 callback tokens such as `#access_token=...`) is
/// dropped and replaced with the `#[redacted]` sentinel in both the
/// parsed arm and the unparseable arm. Fail-closed: when the parse fails
/// and any authority window contains `@`, only the `[redacted]`
/// sentinel is returned. Every authority window is scanned: windows are
/// enumerated over maximal runs of `/` and `\` — pure-slash runs of two
/// or more characters, backslash-bearing runs only behind an RFC 3986
/// scheme prefix (see [`camel_api::redact`] for the canonical window
/// rule) — each window starts immediately after the run (so evaders like
/// `scheme:////user:pass@evil/` cannot hide a `@` behind a slash run)
/// and ends at the next `/`, `?`, or `#`; scanning all windows keeps
/// later `//user:pass@` substrings from hiding behind a benign first
/// window.
///
/// The parsed arm keeps `url::Url::parse` (the authority can only be
/// judged by the parser) and masks the real authority accessors, then
/// delegates wholesale to the canonical string surgery in
/// [`camel_api::redact::redact_url`]: rust-url can park later-window
/// userinfo bytes in the path (`https://h//user:pass@evil/`), and the
/// canonical helper owns window masking, `?`/`#` sentinel composition
/// (one per distinct introducer, first-occurrence order), and the
/// 256-byte UTF-8 cap. The unparseable arm delegates to
/// [`camel_api::redact::redact_url_fail_closed`].
pub(crate) fn redact_url_for_diagnostics(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(mut u) => {
            // Fail closed when an authority marker was accepted but no
            // host was stored: userinfo-shaped bytes can hide in the path
            // behind the marker, and empty-host schemes (`file:///us@r/x`,
            // `unix:///@socket`) can put a `@` in that window too. Such
            // inputs are sentineled wholesale — deliberate fail-closed
            // over-redaction per ADR-0051.
            if !u.cannot_be_a_base()
                && u.host_str().is_none()
                && camel_api::redact::window_has_at_sign(raw)
            {
                return "[redacted]".to_string();
            }
            if !u.username().is_empty() || u.password().is_some() {
                let _ = u.set_username("***");
                let _ = u.set_password(None);
            }
            // Query and fragment stay on the rendered URL; the canonical
            // redactor drops them and composes the sentinels.
            let s = u.to_string();
            camel_api::redact::redact_url(&s)
        }
        Err(_) => camel_api::redact::redact_url_fail_closed(raw),
    }
}

/// Maximum bytes of an upstream error response body embedded into
/// `CamelError::HttpOperationFailed`. The body is attacker-controllable (a
/// malicious or compromised upstream), so it is truncated and lossy-decoded to
/// bound log injection / DLQ payload size.
const MAX_ERROR_RESPONSE_BODY_BYTES: usize = 4096;

fn truncate_error_body(body: &[u8]) -> String {
    if body.len() <= MAX_ERROR_RESPONSE_BODY_BYTES {
        String::from_utf8_lossy(body).into_owned()
    } else {
        let mut s = String::from_utf8_lossy(&body[..MAX_ERROR_RESPONSE_BODY_BYTES]).into_owned();
        s.push_str("...[truncated]");
        s
    }
}

impl HttpProducer {
    /// Whether the HTTP method is entity-enclosing (may carry a request
    /// body). Follows Apache Camel's `HttpMethods` set: POST, PUT, PATCH are
    /// entity-enclosing; GET, HEAD, DELETE, OPTIONS, TRACE are not (RFC 9110
    /// §9.3.1/§9.3.2).
    fn is_entity_enclosing(method: &str) -> bool {
        matches!(method, "POST" | "PUT" | "PATCH")
    }
}

impl Service<Exchange> for HttpProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let config = self.config.clone();
        let shared_client = self.client.clone();
        let pinned_cache = std::sync::Arc::clone(&self.pinned_cache);
        let http_config = self.http_config.clone();
        let component_metrics = self.runtime.component_metrics();

        Box::pin(async move {
            let mut exchange = exchange;
            let outcome = async {
                let method_str = HttpProducer::resolve_method(&exchange, &config);
                // Entity-enclosing gate (RFC 9110 §9.3.1/§9.3.2): only POST, PUT
                // and PATCH may carry a request body. Any other resolved method
                // drops the exchange body before the request is built (Apache
                // Camel `HttpMethods` parity).
                let suppress_body = !HttpProducer::is_entity_enclosing(&method_str);
                let url = HttpProducer::resolve_url(&exchange, &config)?;

                // SECURITY: Validate URL for SSRF
                ssrf::validate_url_for_ssrf(&url, &config)?;

                // Resolve hostname and pin validated IPs to prevent DNS-rebinding TOCTOU
                // (L-H2). When the URL uses a domain name and SSRF protection is active,
                // reuse the endpoint's cached DNS-pinned client for that validated
                // (host, addrs) pair — built once with resolve_to_addrs, then shared so
                // repeated requests keep one connection pool without re-resolving DNS.
                // Per-request SSRF validation and DNS pinning are unchanged. IP-literal
                // URLs use the endpoint's unpinned shared client.
                let resolved = ssrf::resolve_initial_url_for_ssrf(
                    &url,
                    config.allow_internal,
                    config.allow_cleartext,
                )
                .await?;
                let client: reqwest::Client = if let Some((ref host, ref addrs)) = resolved {
                    // A failed pinned build (strict fail-closed material
                    // conflict) propagates into the request error path —
                    // no degraded client is built or served.
                    pinned_cache
                        .get_or_build(host.as_str(), addrs, || {
                            build_client(&http_config, Some((host.as_str(), addrs)))
                        })
                        .await?
                } else {
                    shared_client.clone()
                };

                debug!(
                    correlation_id = %exchange.correlation_id(),
                    method = %method_str,
                    url = %redact_url_for_diagnostics(&url),
                    "HTTP request"
                );

                let method = method_str.parse::<reqwest::Method>().map_err(|e| {
                    CamelError::ProcessorError(format!(
                        "Invalid HTTP method '{}': {}",
                        method_str, e
                    ))
                })?;

                // Collect headers for potential redirect replay
                let mut collected_headers: Vec<(
                    reqwest::header::HeaderName,
                    reqwest::header::HeaderValue,
                )> = Vec::new();

                if let Some(user_agent) = &config.user_agent
                    && !config.bridge_endpoint
                {
                    match constructed_header("user-agent", user_agent) {
                        Ok((_, val)) => {
                            collected_headers.push((reqwest::header::USER_AGENT, val));
                        }
                        Err(drop) => debug!(
                            correlation_id = %exchange.correlation_id(),
                            header = %drop.name,
                            "outbound header dropped: {}",
                            drop.reason
                        ),
                    }
                }

                // Inject W3C TraceContext headers for distributed tracing (opt-in via "otel" feature)
                #[cfg(feature = "otel")]
                let should_inject_otel = !config.bridge_endpoint;
                #[cfg(feature = "otel")]
                if should_inject_otel {
                    let mut otel_headers = HashMap::new();
                    camel_otel::inject_from_exchange(&exchange, &mut otel_headers);
                    for (k, v) in otel_headers {
                        match constructed_header(&k, &v) {
                            Ok((name, val)) => collected_headers.push((name, val)),
                            Err(drop) => debug!(
                                correlation_id = %exchange.correlation_id(),
                                header = %drop.name,
                                "outbound header dropped: {}",
                                drop.reason
                            ),
                        }
                    }
                }

                let conn_tokens = header_policy::connection_tokens(
                    exchange
                        .input
                        .headers
                        .iter()
                        .filter(|(k, _)| k.eq_ignore_ascii_case("connection"))
                        .filter_map(|(_, v)| v.as_str()),
                );

                let outbound = select_outbound_headers(
                    &exchange.input.headers,
                    &config.skip_request_headers,
                    &conn_tokens,
                );
                for drop in &outbound.drops {
                    if let Some(value_kind) = drop.value_kind {
                        debug!(
                            correlation_id = %exchange.correlation_id(),
                            header = %drop.name,
                            value_kind = value_kind,
                            "outbound header dropped: {}",
                            drop.reason
                        );
                    } else {
                        debug!(
                            correlation_id = %exchange.correlation_id(),
                            header = %drop.name,
                            "outbound header dropped: {}",
                            drop.reason
                        );
                    }
                }
                collected_headers.extend(outbound.accepted);

                // Auth headers
                if !config.bridge_endpoint {
                    match &config.auth {
                        HttpAuth::None => {}
                        HttpAuth::Basic { username, password } => {
                            use base64::Engine;
                            // allow-secret: credentials combined for base64 Basic auth header
                            let credentials = format!("{username}:{password}");
                            let encoded =
                                base64::engine::general_purpose::STANDARD.encode(credentials);
                            // Base64 output is always header-safe; the guard is kept
                            // for uniformity with Bearer.
                            match constructed_header("authorization", &format!("Basic {encoded}")) {
                                Ok((_, val)) => {
                                    collected_headers.push((reqwest::header::AUTHORIZATION, val));
                                }
                                Err(drop) => debug!(
                                    correlation_id = %exchange.correlation_id(),
                                    header = %drop.name,
                                    "outbound header dropped: {}",
                                    drop.reason
                                ),
                            }
                        }
                        HttpAuth::Bearer { token } => {
                            // allow-secret: Bearer token in Authorization header
                            let bearer = format!("Bearer {token}");
                            match constructed_header("authorization", &bearer) {
                                Ok((_, val)) => {
                                    collected_headers.push((reqwest::header::AUTHORIZATION, val));
                                }
                                Err(drop) => debug!(
                                    correlation_id = %exchange.correlation_id(),
                                    header = %drop.name,
                                    "outbound header dropped: {}",
                                    drop.reason
                                ),
                            }
                        }
                    }

                    if config.connection_close {
                        collected_headers.push((
                            reqwest::header::CONNECTION,
                            reqwest::header::HeaderValue::from_static("close"),
                        ));
                    }
                }

                // Materialize body
                let is_stream_body = matches!(exchange.input.body, Body::Stream(_));
                let materialized_body: Option<Vec<u8>> = if is_stream_body {
                    if suppress_body {
                        // A stream body dropped under a non-entity-enclosing
                        // method always warns (its emptiness is unknowable) and
                        // stays consumed (mem::take). The stream attach arm below
                        // still runs its outer flag check, but the inner `if let
                        // Body::Stream` re-match fails on the now-Empty body, so
                        // no stream is attached and no AlreadyConsumed error can
                        // fire.
                        std::mem::take(&mut exchange.input.body);
                        // log-policy: handler-owned
                        tracing::warn!(
                            correlation_id = %exchange.correlation_id(),
                            method = %method_str,
                            "dropping request body for non-entity-enclosing HTTP method"
                        );
                    }
                    None // Streams can't be replayed on redirect
                } else {
                    let body = std::mem::take(&mut exchange.input.body);
                    let bytes = body.into_bytes(config.max_body_size).await?;
                    if bytes.is_empty() {
                        // Empty body: nothing to send and nothing to warn about.
                        None
                    } else if suppress_body {
                        // log-policy: handler-owned
                        tracing::warn!(
                            correlation_id = %exchange.correlation_id(),
                            method = %method_str,
                            "dropping request body for non-entity-enclosing HTTP method"
                        );
                        None
                    } else {
                        Some(bytes.to_vec())
                    }
                };

                let response = if config.follow_redirects && !is_stream_body {
                    // Use manual redirect loop with per-hop SSRF validation.
                    // `client` is the pinned-or-shared binding for the initial
                    // request (a hostname initial request keeps its DNS-pinned
                    // client); `shared_client` is the unpinned endpoint client
                    // reused by IP-literal redirect hops.
                    ssrf::send_with_ssrf_safe_redirects(
                        &client,
                        &shared_client,
                        &pinned_cache,
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

                    request.send().await.map_err(|e| {
                        CamelError::ProcessorError(format!("HTTP request failed: {e}"))
                    })?
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
                        // ADR-0051 redact-by-construction: never embed
                        // userinfo/query credentials in the error value.
                        url: redact_url_for_diagnostics(&url),
                        status_code,
                        status_text,
                        response_body: Some(truncate_error_body(&response_body)),
                    });
                }

                if !response_body.is_empty() {
                    exchange.input.body = Body::Bytes(bytes::Bytes::from(response_body.to_vec()));
                }

                debug!(
                    correlation_id = %exchange.correlation_id(),
                    status = status_code,
                    url = %redact_url_for_diagnostics(&url),
                    "HTTP response"
                );
                Ok(exchange)
            }
            .await;
            // ("http","request") facade (dashboard-observability 4.3): the
            // request boundary is the full client round-trip — SSRF checks,
            // send, response read, and (with throwExceptionOnFailure) the
            // status gate. http runs no retry_async and the producer
            // previously emitted nothing, so no label collides with
            // e:http:request.
            component_metrics.observe("http", "request", outcome.is_err());
            outcome
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
/// in use". This mutex does not give blanket protection by itself. It
/// helps only where every participant follows the mutex law: the
/// consumer-test readiness helper holds it from `stage_listener` until
/// readiness-complete (http-test-harness spec, requirement
/// "Registry-mutation serialization during setup"), and each `reset()`
/// caller takes it before the reset.
#[cfg(test)]
pub(crate) static REGISTRY_TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Poison-recovering acquire of REGISTRY_TEST_MUTEX (httpflake).
///
/// The mutex guards test SERIALIZATION only - the registry own data is
/// protected by its inner lock - so a sibling test that panics while
/// holding the guard must not poison the mutex and cascade failures
/// into every other holder. Recovery via into_inner is therefore safe
/// and keeps one failing test failing as ONE test.
#[cfg(test)]
pub(crate) fn lock_registry_test_mutex() -> std::sync::MutexGuard<'static, ()> {
    REGISTRY_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Serializes tests that mutate (or assert on) the process-global
/// SSL_CERT_FILE/SSL_CERT_DIR CA-probe env vars (rc-3j4mq). Poison-
/// recovering for the same reason as REGISTRY_TEST_MUTEX.
#[cfg(test)]
static CA_STORE_TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn lock_ca_store_test_mutex() -> std::sync::MutexGuard<'static, ()> {
    CA_STORE_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Map a pipeline error to an HTTP reply.
///
/// Extracted from the inline `match` in `dispatch_handler` for unit
/// testability (rc-1dk4). Client-fault errors map to their 4xx codes
/// with a structured JSON error body: `TypeConversionFailed`/
/// `ValidationError` → 400, `UnsupportedMediaType` → 415 (media
/// negotiation gate, REST lowering), `NotAcceptable` → 406 (same
/// gate); `Unauthenticated`/`Unauthorized` keep their `401`/`403`
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
            json_error_reply(400, "bad_request", msg)
        }
        CamelError::ValidationError(msg) => {
            tracing::warn!(error = %msg, path = %path, "Schema validation failed (bad request)");
            json_error_reply(400, "validation_error", msg)
        }
        CamelError::ConsumerStopping => {
            tracing::debug!(path = %path, "Pipeline aborted during route shutdown");
            HttpReply {
                status: 503,
                headers: vec![],
                body: HttpReplyBody::Bytes(bytes::Bytes::from("Service Unavailable")),
            }
        }
        CamelError::UnsupportedMediaType { consumed, declared } => {
            tracing::warn!(error = %consumed, declared = %declared, path = %path, "Unsupported media type (bad request)");
            json_error_reply(
                415,
                "unsupported_media_type",
                format!("consumed {consumed}, declared {declared}"),
            )
        }
        CamelError::NotAcceptable { accept, produced } => {
            tracing::warn!(error = %accept, produced = %produced, path = %path, "Not acceptable (bad request)");
            json_error_reply(
                406,
                "not_acceptable",
                format!("accept {accept}, produced {produced}"),
            )
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

/// Build a JSON error reply with the given status, error code, and message.
///
/// Shared by the `TypeConversionFailed`/`ValidationError` (400),
/// `UnsupportedMediaType` (415), and `NotAcceptable` (406) arms of
/// `pipeline_error_to_reply` so the four replies cannot drift apart. The
/// `unwrap_or_else(|_| "{}".to_string())` fallback keeps the reply valid
/// JSON even if serialization fails.
fn json_error_reply(status: u16, code: &str, message: String) -> HttpReply {
    let body = serde_json::to_string(&serde_json::json!({
        "error": code,
        "message": message,
    }))
    .unwrap_or_else(|_| "{}".to_string()); // allow-unwrap
    HttpReply {
        status,
        headers: vec![("Content-Type".to_string(), "application/json".to_string())],
        body: HttpReplyBody::Bytes(bytes::Bytes::from(body)),
    }
}

/// Lowercase kind name for a JSON value, used in drop diagnostics so log
/// readers see *why* a header had no scalar string form without the value
/// itself ever entering diagnostics.
const fn json_value_kind(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Scalar string form of a JSON value: strings pass through, `Number` and
/// `Bool` are stringified, everything else has no single-value form.
/// Shared by the consumer reply finaliser and the producer outbound filter
/// so the two directions cannot drift apart (rc-lidtk / rc-8l23a).
fn scalar_string_form(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Select the HTTP response headers emitted by the consumer reply finaliser
/// (ADR-0057 / rc-2jj2). Extracted from the inline filter in
/// `dispatch_handler` for unit testability.
///
/// Drops Camel-namespace headers, hop-by-hop/framing, request-only, and
/// server-owned headers, plus `content-length`/`content-type` (re-derived),
/// and any header named by a `Connection` token. Scalar non-string values
/// (`Number`/`Bool`) are stringified so `set_header("X-Retries", 3)` reaches
/// the wire instead of being silently discarded (rc-lidtk); `null`, objects,
/// and arrays have no single-value form and are dropped. Every drop is
/// logged at DEBUG with the header name and reason — names only, never
/// values, so credentials cannot leak into diagnostics (ADR-0051).
/// Appends a single `Content-Type` from `user_content_type` falling back to
/// `inferred_content_type` when either is present.
fn select_response_headers(
    headers: &HashMap<String, serde_json::Value>,
    user_content_type: Option<String>,
    inferred_content_type: Option<String>,
) -> Vec<(String, String)> {
    let conn_tokens = header_policy::connection_tokens(
        headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("connection"))
            .filter_map(|(_, v)| v.as_str()),
    );
    let mut selected: Vec<(String, String)> = Vec::new();
    for (k, v) in headers {
        if k.starts_with("Camel") {
            debug!(header = %k, "reply header dropped: Camel namespace");
            continue;
        }
        if header_policy::excluded_response(k, &conn_tokens) {
            debug!(header = %k, "reply header dropped: emission policy");
            continue;
        }
        match scalar_string_form(v) {
            Some(s) => selected.push((k.clone(), s)),
            None => debug!(
                header = %k,
                value_kind = json_value_kind(v),
                "reply header dropped: no scalar string form"
            ),
        }
    }
    if let Some(ct) = user_content_type.or(inferred_content_type) {
        selected.push(("Content-Type".to_string(), ct));
    }
    selected
}

/// One outbound header drop: the exchange header name, a stable reason
/// string, and — when the drop was caused by the value having no scalar
/// string form — the JSON value kind. Names and kinds only, never values
/// (ADR-0051).
#[derive(Debug)]
struct OutboundHeaderDrop<'a> {
    name: &'a str,
    reason: &'static str,
    value_kind: Option<&'static str>,
}

/// Outbound exchange-header selection result: headers accepted for the
/// wire plus drop records for call-site DEBUG logging.
struct OutboundHeaderSelection<'a> {
    accepted: Vec<(reqwest::header::HeaderName, reqwest::header::HeaderValue)>,
    drops: Vec<OutboundHeaderDrop<'a>>,
}

/// Select the exchange headers the HTTP producer forwards on the outbound
/// request (ADR-0057 / rc-8l23a). Extracted from the inline filter in
/// `HttpProducer::call` for unit testability.
///
/// Drops `Camel`-namespace headers, names listed in `skip_request_headers`,
/// hop-by-hop/framing and connection-token-named headers excluded by the
/// outbound emission policy, and headers whose name or stringified value
/// fails `HeaderName`/`HeaderValue` construction. Scalar non-string values
/// (`Number`/`Bool`) are stringified so `set_header("X-Retries", 3)` reaches
/// the wire instead of being silently discarded (rc-8l23a); `null`, objects,
/// and arrays have no single-value form and are dropped. Drops are returned
/// rather than logged so the call site can attach the correlation id; log
/// consumers see names and kinds only, never values (ADR-0051).
fn select_outbound_headers<'a>(
    headers: &'a HashMap<String, serde_json::Value>,
    skip_request_headers: &[String],
    conn_tokens: &[String],
) -> OutboundHeaderSelection<'a> {
    let mut accepted = Vec::new();
    let mut drops = Vec::new();
    for (key, value) in headers {
        if key.starts_with("Camel") {
            drops.push(OutboundHeaderDrop {
                name: key,
                reason: "Camel namespace",
                value_kind: None,
            });
            continue;
        }
        if skip_request_headers
            .iter()
            .any(|h| h.eq_ignore_ascii_case(key))
        {
            drops.push(OutboundHeaderDrop {
                name: key,
                reason: "skip_request_headers",
                value_kind: None,
            });
            continue;
        }
        if header_policy::excluded_outbound(key, conn_tokens) {
            drops.push(OutboundHeaderDrop {
                name: key,
                reason: "outbound emission policy",
                value_kind: None,
            });
            continue;
        }
        let Some(val_str) = scalar_string_form(value) else {
            drops.push(OutboundHeaderDrop {
                name: key,
                reason: "no scalar string form",
                value_kind: Some(json_value_kind(value)),
            });
            continue;
        };
        match constructed_header(key, &val_str) {
            Ok((name, val)) => accepted.push((name, val)),
            Err(drop) => drops.push(drop),
        }
    }
    OutboundHeaderSelection { accepted, drops }
}

/// Construct a wire-ready `(HeaderName, HeaderValue)` pair for one outbound
/// header, or a drop record when the name or value fails construction
/// (rc-jbs1v). Drop records carry name and reason only, never values
/// (ADR-0051).
fn constructed_header<'a>(
    name: &'a str,
    value: &str,
) -> Result<(reqwest::header::HeaderName, reqwest::header::HeaderValue), OutboundHeaderDrop<'a>> {
    let header_name = match reqwest::header::HeaderName::from_bytes(name.as_bytes()) {
        Ok(header_name) => header_name,
        Err(_) => {
            return Err(OutboundHeaderDrop {
                name,
                reason: "invalid header name",
                value_kind: None,
            });
        }
    };
    let header_value = match reqwest::header::HeaderValue::from_str(value) {
        Ok(header_value) => header_value,
        Err(_) => {
            return Err(OutboundHeaderDrop {
                name,
                reason: "invalid header value",
                value_kind: None,
            });
        }
    };
    Ok((header_name, header_value))
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
