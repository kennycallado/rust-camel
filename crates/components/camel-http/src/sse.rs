//! Server-Sent Events frame parsing for the SSE consumer.
//!
//! The parser is transport-agnostic: callers feed raw body bytes and receive
//! dispatched [`SseFrame`]s. Dispatch follows the SSE grammar — an event
//! dispatches only on the blank-line delimiter, so a stream that ends
//! mid-block emits nothing for that block.
//!
//! Data values accumulate as raw bytes and are decoded strictly at dispatch:
//! invalid UTF-8 is a loud, non-retryable task error, never a
//! U+FFFD-silenced payload.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::time::Duration;

use camel_api::Body;
use camel_api::Exchange;
use camel_api::Message;
use camel_api::component_metadata::ComponentMetadata;
use camel_component_api::{
    BoxProcessor, CamelError, Component, ComponentContext, ConcurrencyModel, Consumer,
    ConsumerContext, ConsumerStartupMode, Endpoint, NetworkRetryPolicy, ProducerContext,
    RuntimeObservability, UriConfig,
};
use futures::StreamExt;
use http::header::{HeaderName, HeaderValue};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::HttpEndpointConfig;
use crate::build_client;
use crate::client_cache::{PINNED_CLIENT_MAX_ENTRIES, PINNED_CLIENT_TTL, PinnedClientCache};
use crate::config::HttpConfig;
use crate::ssrf;

/// Upper bound for an event id that may be stored for `Last-Event-ID`
/// resend. Longer ids still reach the exchange but are flagged.
const MAX_RESEND_ID_BYTES: usize = 1024;

/// Memory bound for a single non-`data` field line (`event:`, `id:`,
/// `retry:`). Generous versus [`MAX_RESEND_ID_BYTES`]; it only exists so a
/// newline-less flood cannot buffer without bound.
const MAX_FIELD_LINE_BYTES: usize = 65536;

/// Error text for an event whose `data` bytes exceed `maxEventBytes`.
const OVERSIZE_EVENT_MESSAGE: &str = "sse event exceeds maxEventBytes bound";
/// Error text for a non-`data` field line past the field-line memory bound.
const OVERSIZE_FIELD_LINE_MESSAGE: &str = "sse field line exceeds maxEventBytes bound";
/// Error text for an event whose `data` bytes are not valid UTF-8.
const INVALID_UTF8_MESSAGE: &str = "sse event contains invalid UTF-8 data";

/// One dispatched SSE event, already split into its fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseEventFrame {
    pub event: Option<String>,
    pub id: Option<String>,
    pub retry_ms: Option<u64>,
    pub data: String,
    /// `true` when `id` exceeds [`MAX_RESEND_ID_BYTES`]: keep it on the
    /// exchange, never persist it for a resend.
    pub id_over_cap: bool,
}

/// A frame produced by [`SseFrameParser::push`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SseFrame {
    Event(SseEventFrame),
    Comment,
}

/// Incremental SSE parser: tolerates arbitrary chunk boundaries.
#[derive(Debug)]
pub(crate) struct SseFrameParser {
    max_event_bytes: usize,
    /// Bytes of the current, not-yet-terminated line.
    line_buf: Vec<u8>,
    /// Accumulated `data:` bytes for the current block, `\n`-joined.
    data: Vec<u8>,
    event: Option<String>,
    id: Option<String>,
    id_over_cap: bool,
    retry_ms: Option<u64>,
    /// A `\r` terminated the previous line; a following `\n` belongs to it.
    cr_pending: bool,
    /// The leading UTF-8 BOM is stripped from the very first chunk only.
    first_chunk: bool,
}

impl SseFrameParser {
    pub(crate) fn new(max_event_bytes: usize) -> Self {
        Self {
            max_event_bytes,
            line_buf: Vec::new(),
            data: Vec::new(),
            event: None,
            id: None,
            id_over_cap: false,
            retry_ms: None,
            cr_pending: false,
            first_chunk: true,
        }
    }

    /// Feeds raw body bytes and returns every frame dispatched by them.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<SseFrame>, CamelError> {
        let mut frames = Vec::new();
        let mut bytes = chunk;
        if self.first_chunk {
            self.first_chunk = false;
            if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
                bytes = &bytes[3..];
            }
        }
        for &byte in bytes {
            if self.cr_pending {
                self.cr_pending = false;
                if byte == b'\n' {
                    continue;
                }
            }
            match byte {
                b'\r' => {
                    self.take_line(&mut frames)?;
                    self.cr_pending = true;
                }
                b'\n' => self.take_line(&mut frames)?,
                _ => {
                    self.line_buf.push(byte);
                    self.check_line_bound()?;
                }
            }
        }
        Ok(frames)
    }

    /// EOF: discards the unterminated block and the partial line. Events
    /// dispatch only on the blank-line delimiter, so nothing is emitted.
    pub(crate) fn finish(&mut self) -> Result<Vec<SseFrame>, CamelError> {
        self.line_buf.clear();
        self.data.clear();
        self.event = None;
        self.id = None;
        self.id_over_cap = false;
        self.retry_ms = None;
        self.cr_pending = false;
        Ok(Vec::new())
    }

    /// Bounds memory while a line is still unterminated. A `data:` line can
    /// only belong to an over-cap event once its value bytes plus the
    /// accumulated block payload pass `max_event_bytes`; any other line is
    /// held to [`MAX_FIELD_LINE_BYTES`].
    fn check_line_bound(&self) -> Result<(), CamelError> {
        match self.pending_data_value_len() {
            Some(value_len) => {
                let separator = if self.data.is_empty() { 0 } else { 1 };
                if self.data.len() + separator + value_len > self.max_event_bytes {
                    return Err(CamelError::Io(OVERSIZE_EVENT_MESSAGE.to_string()));
                }
            }
            None if self.line_buf.len() > MAX_FIELD_LINE_BYTES => {
                return Err(CamelError::Io(OVERSIZE_FIELD_LINE_MESSAGE.to_string()));
            }
            None => {}
        }
        Ok(())
    }

    /// Bytes already accumulated for the `data` value of the line in
    /// `line_buf`, or `None` when the line is not a `data:` field line.
    fn pending_data_value_len(&self) -> Option<usize> {
        let rest = self.line_buf.strip_prefix(b"data:")?;
        let value = if rest.first() == Some(&b' ') {
            &rest[1..]
        } else {
            rest
        };
        Some(value.len())
    }

    fn take_line(&mut self, frames: &mut Vec<SseFrame>) -> Result<(), CamelError> {
        let line = std::mem::take(&mut self.line_buf);
        if line.is_empty() {
            return self.dispatch(frames);
        }
        if line[0] == b':' {
            frames.push(SseFrame::Comment);
            return Ok(());
        }
        let (field, value) = match line.iter().position(|&byte| byte == b':') {
            Some(colon) => {
                let rest = &line[colon + 1..];
                let value = if rest.first() == Some(&b' ') {
                    &rest[1..]
                } else {
                    rest
                };
                (&line[..colon], value)
            }
            None => (line.as_slice(), &b""[..]),
        };
        let Ok(field) = std::str::from_utf8(field) else {
            return Ok(());
        };
        match field {
            "event" => {
                self.event = if value.is_empty() {
                    None
                } else {
                    Some(String::from_utf8_lossy(value).into_owned())
                };
            }
            "id" => {
                self.id_over_cap = value.len() > MAX_RESEND_ID_BYTES;
                self.id = if value.is_empty() {
                    None
                } else {
                    Some(String::from_utf8_lossy(value).into_owned())
                };
            }
            "retry" => {
                self.retry_ms = std::str::from_utf8(value)
                    .ok()
                    .and_then(|text| text.parse::<u64>().ok());
            }
            "data" => self.push_data(value)?,
            _ => {}
        }
        Ok(())
    }

    fn push_data(&mut self, value: &[u8]) -> Result<(), CamelError> {
        let separator = if self.data.is_empty() { 0 } else { 1 };
        if self.data.len() + separator + value.len() > self.max_event_bytes {
            return Err(CamelError::Io(OVERSIZE_EVENT_MESSAGE.to_string()));
        }
        if separator == 1 {
            self.data.push(b'\n');
        }
        self.data.extend_from_slice(value);
        Ok(())
    }

    fn dispatch(&mut self, frames: &mut Vec<SseFrame>) -> Result<(), CamelError> {
        let data = std::mem::take(&mut self.data);
        let event = self.event.take();
        let id = self.id.take();
        let retry_ms = self.retry_ms.take();
        let id_over_cap = std::mem::take(&mut self.id_over_cap);
        if data.is_empty() {
            return Ok(());
        }
        let data = String::from_utf8(data)
            .map_err(|_| CamelError::Io(INVALID_UTF8_MESSAGE.to_string()))?;
        frames.push(SseFrame::Event(SseEventFrame {
            event,
            id,
            retry_ms,
            data,
            id_over_cap,
        }));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// SseUriConfig
// ---------------------------------------------------------------------------

/// Lower bound for the retry delays (`retryInitialMs` / `retryMaxMs`).
const RETRY_DELAY_FLOOR_MS: u64 = 1_000;
/// Upper bound for the retry delays (`retryInitialMs` / `retryMaxMs`).
const RETRY_DELAY_CEILING_MS: u64 = 60_000;

/// Resolved configuration for one `sse:` endpoint.
///
/// URI options are macro-derived (the [`UriConfig`] derive) and parsed by the
/// `http`-crate query parser, which percent-decodes values. The pinned bounds
/// are applied at parse time, i.e. at endpoint creation.
#[derive(Clone, UriConfig)]
#[uri_scheme = "sse"]
#[uri_config(
    metadata(scheme = "sse", description = "Server-Sent Events consumer", consumer),
    crate = "camel_component_api"
)]
pub(crate) struct SseUriConfig {
    /// Authority and path exactly as parsed from the URI (`//host:port/path`).
    /// Excluded from `uri_options()` — the derive treats the first field
    /// without `#[uri_param]` as the path.
    pub(crate) path: String,

    #[uri_param(
        name = "maxEventBytes",
        default = "1048576",
        desc = "Upper bound in bytes for one event's accumulated data"
    )]
    pub(crate) max_event_bytes: usize,

    #[uri_param(
        name = "idleTimeoutMs",
        default = "90000",
        desc = "Idle timeout between body reads in milliseconds"
    )]
    pub(crate) idle_timeout_ms: u64,

    #[uri_param(
        name = "connectTimeoutMs",
        default = "10000",
        desc = "Connect timeout in milliseconds"
    )]
    pub(crate) connect_timeout_ms: u64,

    #[uri_param(
        name = "retryInitialMs",
        default = "3000",
        desc = "Initial reconnect delay in milliseconds (clamped to [1000, 60000])"
    )]
    pub(crate) retry_initial_ms: u64,

    #[uri_param(
        name = "retryMaxMs",
        default = "60000",
        desc = "Maximum reconnect delay in milliseconds (clamped to [1000, 60000])"
    )]
    pub(crate) retry_max_ms: u64,

    #[uri_param(
        name = "allowInternal",
        default = "false",
        desc = "Allow private/internal network destinations (SSRF)"
    )]
    pub(crate) allow_internal: bool,

    #[uri_param(
        name = "allowCleartext",
        default = "false",
        desc = "Allow cleartext http:// to public targets (transport consent; ADR-0081)"
    )]
    pub(crate) allow_cleartext: bool,

    #[uri_param(
        name = "authHeaders",
        secret,
        desc = "Outbound headers as Name1=Value1,Name2=Value2"
    )]
    pub(crate) auth_headers: Option<String>,
}

impl std::fmt::Debug for SseUriConfig {
    /// ADR-0051 redact-by-construction: `auth_headers` carries credential
    /// material, so the display surface masks it wholesale.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SseUriConfig")
            .field("path", &self.path)
            .field("max_event_bytes", &self.max_event_bytes)
            .field("idle_timeout_ms", &self.idle_timeout_ms)
            .field("connect_timeout_ms", &self.connect_timeout_ms)
            .field("retry_initial_ms", &self.retry_initial_ms)
            .field("retry_max_ms", &self.retry_max_ms)
            .field("allow_internal", &self.allow_internal)
            .field("allow_cleartext", &self.allow_cleartext)
            .field("auth_headers", &self.auth_headers.as_ref().map(|_| "***"))
            .finish()
    }
}

impl SseUriConfig {
    /// Pinned bounds, applied by the derive-generated `from_components`
    /// (`parse_uri_components(parts)?.validate()` — the `UriConfig::validate`
    /// hook, camel-endpoint/src/config.rs:19-22), i.e. at endpoint creation:
    /// retry delays clamp into [`RETRY_DELAY_FLOOR_MS`,
    /// `RETRY_DELAY_CEILING_MS`] with `retryMaxMs >= retryInitialMs`, and
    /// `maxEventBytes` must be positive.
    ///
    /// Inherent so the derive-generated `from_components` resolves it ahead of
    /// the trait's pass-through default.
    fn validate(mut self) -> Result<Self, CamelError> {
        if self.max_event_bytes == 0 {
            return Err(CamelError::Config(
                "sse: maxEventBytes must be greater than 0".to_string(),
            ));
        }
        self.retry_initial_ms = self
            .retry_initial_ms
            .clamp(RETRY_DELAY_FLOOR_MS, RETRY_DELAY_CEILING_MS);
        self.retry_max_ms = self
            .retry_max_ms
            .clamp(RETRY_DELAY_FLOOR_MS, RETRY_DELAY_CEILING_MS);
        if self.retry_max_ms < self.retry_initial_ms {
            self.retry_max_ms = self.retry_initial_ms;
        }
        Ok(self)
    }
}

/// Attempt budget for the resubscription loop when the endpoint stays down.
/// The zero value would mean unlimited in [`NetworkRetryPolicy`], but the SSE
/// consumer is expected to surface exhaustion to route supervision (ADR-0007)
/// rather than retry forever; ten attempts is the policy's own default.
const SSE_MAX_RETRY_ATTEMPTS: u32 = 10;

/// Reconnect policy derived from the clamped endpoint config: exponential
/// backoff from `retryInitialMs` up to `retryMaxMs`, no jitter (one consumer
/// per endpoint, and a deterministic schedule keeps the resubscription
/// contract testable without clock injection).
fn retry_policy_for(cfg: &SseUriConfig) -> NetworkRetryPolicy {
    NetworkRetryPolicy {
        enabled: true,
        max_attempts: SSE_MAX_RETRY_ATTEMPTS,
        initial_delay: Duration::from_millis(cfg.retry_initial_ms),
        max_delay: Duration::from_millis(cfg.retry_max_ms),
        jitter_factor: 0.0,
        ..NetworkRetryPolicy::default()
    }
}

/// Pure reconnect-delay schedule: a server `retry:` override wins over the
/// policy backoff, and either value is clamped into
/// [`RETRY_DELAY_FLOOR_MS`], [`RETRY_DELAY_CEILING_MS`]. `attempt` is the
/// zero-based retry index (0 schedules the initial `retryInitialMs` delay).
pub(crate) fn next_delay(
    attempt: u32,
    retry_override_ms: Option<u64>,
    cfg: &SseUriConfig,
) -> Duration {
    let delay_ms = match retry_override_ms {
        Some(ms) => ms,
        None => retry_policy_for(cfg).delay_for(attempt).as_millis() as u64,
    };
    Duration::from_millis(delay_ms.clamp(RETRY_DELAY_FLOOR_MS, RETRY_DELAY_CEILING_MS))
}

// ---------------------------------------------------------------------------
// SseComponent
// ---------------------------------------------------------------------------

/// Component factory for the `sse:` scheme (consumer-only).
///
/// Holds the HTTP client configuration and its own
/// [`PinnedClientCache`], mirroring the per-component cache instances
/// [`HttpComponent`](crate::HttpComponent) and
/// [`HttpsComponent`](crate::HttpsComponent) build.
pub(crate) struct SseComponent {
    config: Arc<HttpConfig>,
    pinned_cache: Arc<PinnedClientCache>,
    /// Unpinned shared client for IP-literal targets (mirrors the
    /// `HttpProducer` `shared_client` role). Hostname targets use the
    /// DNS-pinned client from [`PinnedClientCache`] instead.
    client: reqwest::Client,
    /// Set at construction when `tls.strict` is on and the configured
    /// material fails to load; surfaced as an endpoint-creation failure
    /// (rc-ayrwk), mirroring `HttpComponent`/`HttpsComponent`.
    strict_tls_error: Option<CamelError>,
}

impl SseComponent {
    pub(crate) fn new(config: HttpConfig) -> Self {
        let strict_err = crate::strict_tls_error(&config);
        let (client, build_err) = crate::client_or_emergency(&config);
        Self {
            config: Arc::new(config),
            pinned_cache: Arc::new(PinnedClientCache::new(
                PINNED_CLIENT_TTL,
                PINNED_CLIENT_MAX_ENTRIES,
            )),
            client,
            strict_tls_error: strict_err.or(build_err),
        }
    }

    /// Builds the typed endpoint. Shared by [`Component::create_endpoint`] and
    /// the unit tests, which inspect the resolved config directly.
    fn build_endpoint(&self, uri: &str) -> Result<SseEndpoint, CamelError> {
        // Fail closed on unusable strict TLS material: the emergency client
        // built above can never complete a verified handshake, so serving it
        // would silently downgrade `tls.strict`.
        if let Some(err) = &self.strict_tls_error {
            return Err(err.clone());
        }
        reject_userinfo(uri)?;
        let config = SseUriConfig::from_uri(uri)?;
        let auth_headers = parse_auth_headers(config.auth_headers.as_deref())?;
        Ok(SseEndpoint {
            uri: uri.to_string(),
            config,
            auth_headers,
            http_config: Arc::clone(&self.config),
            pinned_cache: Arc::clone(&self.pinned_cache),
            client: self.client.clone(),
        })
    }
}

impl Component for SseComponent {
    fn scheme(&self) -> &str {
        "sse"
    }

    fn metadata(&self) -> ComponentMetadata {
        SseUriConfig::metadata()
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(self.build_endpoint(uri)?))
    }
}

/// `sse:` defines no username/password URI params — credentials come only from
/// `authHeaders`, so userinfo in the URL is rejected at endpoint creation.
fn reject_userinfo(uri: &str) -> Result<(), CamelError> {
    let parsed = url::Url::parse(uri)
        .map_err(|e| CamelError::EndpointCreationFailed(format!("sse: invalid URL: {e}")))?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(CamelError::EndpointCreationFailed(
            "sse: URL must not carry userinfo".to_string(),
        ));
    }
    Ok(())
}

/// Parses `authHeaders=Name1=Value1,Name2=Value2` into wire-ready header pairs.
/// Error messages name the header (never the value) so credentials stay out of
/// diagnostics.
fn parse_auth_headers(raw: Option<&str>) -> Result<Vec<(HeaderName, HeaderValue)>, CamelError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let mut headers = Vec::new();
    for entry in raw.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let Some((name, value)) = entry.split_once('=') else {
            return Err(CamelError::EndpointCreationFailed(
                "sse: invalid authHeaders entry, expected Name=Value".to_string(),
            ));
        };
        let name = name.trim();
        let value = value.trim();
        let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
            CamelError::EndpointCreationFailed(format!(
                "sse: invalid authHeaders header name '{name}'"
            ))
        })?;
        let header_value = HeaderValue::from_str(value).map_err(|_| {
            CamelError::EndpointCreationFailed(format!(
                "sse: invalid authHeaders value for header '{name}'"
            ))
        })?;
        headers.push((header_name, header_value));
    }
    Ok(headers)
}

// ---------------------------------------------------------------------------
// SseEndpoint
// ---------------------------------------------------------------------------

/// Endpoint for an `sse:` consumer route.
pub(crate) struct SseEndpoint {
    uri: String,
    config: SseUriConfig,
    auth_headers: Vec<(HeaderName, HeaderValue)>,
    http_config: Arc<HttpConfig>,
    pinned_cache: Arc<PinnedClientCache>,
    client: reqwest::Client,
}

impl std::fmt::Debug for SseEndpoint {
    /// Header values are credential material (ADR-0051): only the names are
    /// rendered on the display surface.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let header_names: Vec<&HeaderName> = self
            .auth_headers
            .iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>();
        f.debug_struct("SseEndpoint")
            .field("uri", &crate::redact_url_for_diagnostics(&self.uri))
            .field("config", &self.config)
            .field("auth_headers", &header_names)
            .finish_non_exhaustive()
    }
}

impl Endpoint for SseEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(SseConsumer::new(
            self.uri.clone(),
            self.config.clone(),
            self.auth_headers.clone(),
            Arc::clone(&self.http_config),
            Arc::clone(&self.pinned_cache),
            self.client.clone(),
            rt,
        )))
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Err(CamelError::Config("sse: is consumer-only".to_string()))
    }
}

// ---------------------------------------------------------------------------
// SseConsumer
// ---------------------------------------------------------------------------

/// Terminal error text for a response that is not an event stream.
const UNEXPECTED_CONTENT_TYPE_MESSAGE: &str =
    "sse: unexpected Content-Type, expected text/event-stream";
/// Grace period `stop()` grants the background task before aborting it.
const STOP_BUDGET: Duration = Duration::from_secs(10);

/// How one connect + stream attempt ended.
///
/// The reconnect loop treats `Retriable` and `Completed` as resubscribe
/// triggers (gated by the retry budget), while `Terminal` and the `Err` arm
/// of [`SseCore::run_stream_once`] end the task.
enum StreamOutcome {
    /// The cancel token fired; stop without reading or dispatching further.
    Cancelled,
    /// Transport-class failure: idle timeout, reset, mid-stream error, 5xx.
    Retriable(String),
    /// Non-retriable response gate (non-5xx status, content type).
    Terminal(CamelError),
    /// Clean EOF after a frame boundary: the server closed the stream, so a
    /// fresh subscribe is due.
    Completed,
}

/// A connect failure split by whether reconnecting can help.
enum ConnectFailure {
    /// Policy/config rejection or resolver failure (SSRF validation, DNS
    /// resolution, client build): a retry is not the recovery path, route
    /// supervision (ADR-0007) is.
    Terminal(CamelError),
    /// Transport-class failure (send error, connect timeout): retry.
    Retriable(String),
}

/// Prefix the shared redirect seam gives its request-send failures
/// (`ssrf::send_with_ssrf_safe_redirects`). Every other seam error is a
/// policy/config rejection on a redirect hop (missing `Location`, fenced or
/// blocked target), which a retry cannot fix.
const SEND_TRANSPORT_ERROR_PREFIX: &str = "HTTP request failed: ";

/// Classifies a connect-phase send error: transport failures retry, redirect
/// policy rejections do not.
fn classify_send_error(err: CamelError) -> ConnectFailure {
    match &err {
        CamelError::ProcessorError(message) if message.starts_with(SEND_TRANSPORT_ERROR_PREFIX) => {
            ConnectFailure::Retriable(err.to_string())
        }
        _ => ConnectFailure::Terminal(err),
    }
}

/// Mutable resubscription bookkeeping owned by the loop task (one consumer
/// per endpoint): the zero-based retry attempt and the last valid server
/// `retry:` override observed on the wire.
#[derive(Default)]
struct ReconnectState {
    attempt: u32,
    retry_override_ms: Option<u64>,
}

/// Immutable, shareable consumer state. The spawned task owns an `Arc` of
/// it, so the loop can outlive `start()` without cloning request material.
struct SseCore {
    uri: String,
    config: SseUriConfig,
    auth_headers: Vec<(HeaderName, HeaderValue)>,
    http_config: Arc<HttpConfig>,
    pinned_cache: Arc<PinnedClientCache>,
    client: reqwest::Client,
    runtime: Arc<dyn RuntimeObservability>,
    /// Reconnect policy derived from the URI config. Tests override the
    /// attempt budget here (the URI carries no attempt knob).
    retry_policy: NetworkRetryPolicy,
    /// Last event id stored for `Last-Event-ID` resend. Survives reconnects
    /// (Task 1.4 reuses the same core); ids over the resend cap never land
    /// here.
    last_event_id: Mutex<Option<String>>,
}

impl SseCore {
    /// Guarded access to the stored resend id. A poisoned lock (another
    /// thread panicked while holding it) is recovered: the payload is a plain
    /// `Option<String>`, so the value stays usable.
    fn locked_event_id(&self) -> MutexGuard<'_, Option<String>> {
        self.last_event_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The URL used on the wire. SSE rides HTTP: `https://` is the default,
    /// `http://` only when the SSRF policy grants cleartext consent for the
    /// target (loopback/internal under `allowInternal`, anything under
    /// `allowCleartext` — ADR-0081). A component configured with TLS material
    /// always speaks `https://`.
    fn wire_url(&self) -> String {
        format!("{}://{}", self.wire_scheme(), self.wire_authority())
    }

    /// Authority and path of the configured `sse:` URI (`host:port/path`).
    fn wire_authority(&self) -> &str {
        self.config
            .path
            .strip_prefix("//")
            .unwrap_or(self.config.path.as_str())
    }

    fn wire_scheme(&self) -> &'static str {
        // TLS material configured for this component means the endpoint asks
        // for TLS regardless of the internal-cleartext allowance below — the
        // allowance only ever covers plaintext endpoints.
        if self.http_config.tls.as_ref().is_some_and(|tls| tls.enabled) {
            return "https";
        }
        if self.config.allow_cleartext {
            return "http";
        }
        if self.config.allow_internal && self.target_is_internal() {
            return "http";
        }
        "https"
    }

    /// Whether the configured target is an internal destination: a
    /// blocklisted IP literal or a local domain. Only consulted under
    /// `allowInternal`.
    fn target_is_internal(&self) -> bool {
        let Ok(parsed) = url::Url::parse(&format!("https://{}", self.wire_authority())) else {
            return false;
        };
        match parsed.host() {
            Some(url::Host::Ipv4(ip)) => camel_api::is_ssrf_blocked_ip(&std::net::IpAddr::V4(ip)),
            Some(url::Host::Ipv6(ip)) => camel_api::is_ssrf_blocked_ip(&std::net::IpAddr::V6(ip)),
            Some(url::Host::Domain(domain)) => {
                let domain = domain.to_ascii_lowercase();
                domain == "localhost" || domain.ends_with(".local")
            }
            None => false,
        }
    }

    /// The [`HttpEndpointConfig`] the shared SSRF seams consume. Constructed
    /// from the translated wire URL with the endpoint's SSRF policy carried
    /// over; redirects are followed hop-by-hop through
    /// [`ssrf::send_with_ssrf_safe_redirects`] (per-hop validation + auth
    /// stripping).
    fn endpoint_config(&self, wire_url: &str) -> Result<HttpEndpointConfig, CamelError> {
        let mut config = HttpEndpointConfig::from_uri(wire_url)?;
        config.allow_internal = self.config.allow_internal;
        config.allow_cleartext = self.config.allow_cleartext;
        config.follow_redirects = true;
        Ok(config)
    }

    /// Outbound headers: configured `authHeaders`, `Accept: text/event-stream`
    /// and `Last-Event-ID` when an id is stored.
    fn request_headers(&self) -> Vec<(reqwest::header::HeaderName, reqwest::header::HeaderValue)> {
        let mut headers: Vec<(reqwest::header::HeaderName, reqwest::header::HeaderValue)> = self
            .auth_headers
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        headers.push((
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("text/event-stream"),
        ));
        if let Some(id) = self.locked_event_id().clone()
            && let Ok(value) = reqwest::header::HeaderValue::from_str(&id)
        {
            headers.push((
                reqwest::header::HeaderName::from_static("last-event-id"),
                value,
            ));
        }
        headers
    }

    /// One connect attempt. Runs identically on every (re)connect: SSRF
    /// validation on the wire URL, DNS pinning, then the request through the
    /// shared redirect seam bounded by `connectTimeoutMs`.
    ///
    /// Failures are classified here so the reconnect loop never retries a
    /// policy rejection and never gives up on a transport hiccup.
    async fn connect_once(&self) -> Result<reqwest::Response, ConnectFailure> {
        let wire_url = self.wire_url();
        let endpoint_config = self
            .endpoint_config(&wire_url)
            .map_err(ConnectFailure::Terminal)?;
        let headers = self.request_headers();
        let connect_timeout = Duration::from_millis(self.config.connect_timeout_ms.max(1));

        tracing::debug!(
            uri = %camel_api::redact::redact_url(&self.uri),
            url = %camel_api::redact::redact_url(&wire_url),
            "sse connect"
        );

        let connect = async {
            ssrf::validate_url_for_ssrf(&wire_url, &endpoint_config)
                .map_err(ConnectFailure::Terminal)?;
            let resolved = ssrf::resolve_initial_url_for_ssrf(
                &wire_url,
                endpoint_config.allow_internal,
                endpoint_config.allow_cleartext,
            )
            .await
            .map_err(ConnectFailure::Terminal)?;
            let client = match &resolved {
                Some((host, addrs)) => self
                    .pinned_cache
                    .get_or_build(host.as_str(), addrs, || {
                        build_client(&self.http_config, Some((host.as_str(), addrs)))
                    })
                    .await
                    .map_err(ConnectFailure::Terminal)?,
                None => self.client.clone(),
            };
            // The seam returns after the response head: the body stream is
            // never buffered, so `response_timeout` stays `None` — a total
            // request timeout would abort the stream mid-flight.
            ssrf::send_with_ssrf_safe_redirects(
                &client,
                &self.client,
                &self.pinned_cache,
                &self.http_config,
                &endpoint_config,
                reqwest::Method::GET,
                &wire_url,
                headers,
                None,
                endpoint_config.max_redirects,
                None,
            )
            .await
            .map_err(classify_send_error)
        };

        tokio::time::timeout(connect_timeout, connect)
            .await
            .map_err(|_| {
                ConnectFailure::Retriable(format!(
                    "connect timed out after {} ms",
                    self.config.connect_timeout_ms
                ))
            })?
    }

    /// Terminal HTTP-status error with a redacted URL (ADR-0051).
    fn status_failure(&self, status: reqwest::StatusCode) -> CamelError {
        CamelError::HttpOperationFailed {
            method: "GET".to_string(),
            url: camel_api::redact::redact_url(&self.wire_url()),
            status_code: status.as_u16(),
            status_text: status.canonical_reason().unwrap_or("Unknown").to_string(),
            response_body: None,
        }
    }

    /// Attributes a parser error to its metric class by EXACT message
    /// equality (never `contains`: the two classes must stay distinguishable).
    fn record_parser_error(&self, route_id: &str, err: &CamelError) {
        let CamelError::Io(message) = err else {
            return;
        };
        if message == OVERSIZE_EVENT_MESSAGE || message == OVERSIZE_FIELD_LINE_MESSAGE {
            self.runtime
                .metrics()
                .increment_errors(route_id, "b-prime:sse:oversize");
        } else if message == INVALID_UTF8_MESSAGE {
            self.runtime
                .metrics()
                .increment_errors(route_id, "b-prime:sse:utf8");
        }
    }

    /// Stores the event id for `Last-Event-ID` resend. Over-cap ids and the
    /// empty id (treated as absent) leave the stored value untouched.
    fn remember_event_id(&self, event: &SseEventFrame) {
        if event.id_over_cap {
            return;
        }
        let Some(id) = event.id.as_ref().filter(|id| !id.is_empty()) else {
            return;
        };
        *self.locked_event_id() = Some(id.clone());
    }

    /// Gates the response, then reads the body chunk-wise, dispatching one
    /// exchange per event. Every read and every send races the cancel token
    /// (`biased`, so cancellation wins).
    ///
    /// The first complete frame (event or comment) parsed on this stream
    /// resets the retry budget; HTTP 200 alone and partial bytes do not.
    async fn run_stream_once(
        &self,
        response: reqwest::Response,
        ctx: &ConsumerContext,
        cancel: &CancellationToken,
        state: &mut ReconnectState,
    ) -> Result<StreamOutcome, CamelError> {
        let status = response.status();
        // 204 and every non-5xx, non-2xx status are terminal: no stream can
        // follow. A 5xx means the endpoint is unavailable, so the response is
        // dropped unread (never consuming the body) and the subscribe retried.
        if status == reqwest::StatusCode::NO_CONTENT {
            return Ok(StreamOutcome::Terminal(self.status_failure(status)));
        }
        if status.is_server_error() {
            return Ok(StreamOutcome::Retriable(format!(
                "http status {}",
                status.as_u16()
            )));
        }
        if !status.is_success() {
            return Ok(StreamOutcome::Terminal(self.status_failure(status)));
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        if !content_type.starts_with("text/event-stream") {
            return Ok(StreamOutcome::Terminal(CamelError::Io(
                UNEXPECTED_CONTENT_TYPE_MESSAGE.to_string(),
            )));
        }

        let idle = Duration::from_millis(self.config.idle_timeout_ms.max(1));
        let route_id = ctx.route_id().to_string();
        let mut parser = SseFrameParser::new(self.config.max_event_bytes);
        let mut body = response.bytes_stream();
        let mut budget_reset = false;

        loop {
            let read = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Ok(StreamOutcome::Cancelled),
                read = tokio::time::timeout(idle, body.next()) => read,
            };
            let chunk = match read {
                // Any silence past the idle budget — after any bytes, comments
                // included — closes this connection as a retriable failure.
                Err(_elapsed) => {
                    return Ok(StreamOutcome::Retriable(format!(
                        "idle timeout after {} ms",
                        self.config.idle_timeout_ms
                    )));
                }
                Ok(None) => {
                    // EOF discards an unterminated block; nothing dispatches.
                    parser.finish()?;
                    return Ok(StreamOutcome::Completed);
                }
                Ok(Some(Err(err))) => {
                    return Ok(StreamOutcome::Retriable(format!(
                        "body stream error: {err}"
                    )));
                }
                Ok(Some(Ok(chunk))) => chunk,
            };

            let frames = match parser.push(&chunk) {
                Ok(frames) => frames,
                Err(err) => {
                    self.record_parser_error(&route_id, &err);
                    return Err(err);
                }
            };
            for frame in frames {
                // The first complete frame proves this stream is alive: reset
                // the attempt budget (never on HTTP 200, never on partial
                // bytes). Comments count, per the resubscription contract.
                if !budget_reset {
                    state.attempt = 0;
                    budget_reset = true;
                }
                let SseFrame::Event(event) = frame else {
                    // Comment: the read itself reset the idle timer.
                    continue;
                };
                if let Some(retry_ms) = event.retry_ms {
                    // Overrides the policy delay for subsequent reconnects
                    // until a later valid `retry:` field changes it.
                    state.retry_override_ms = Some(retry_ms);
                }
                let exchange = build_exchange(&event);
                // Dispatch BEFORE reading further bytes: no internal channel
                // (ADR-0044), backpressure is the pipeline's.
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Ok(StreamOutcome::Cancelled),
                    sent = ctx.send(exchange) => sent?,
                }
                self.remember_event_id(&event);
            }
        }
    }

    /// One full attempt: connect (cancellable), gate, stream.
    ///
    /// Connect failures are classified: policy rejections end the task,
    /// transport failures become a retriable outcome for the reconnect loop.
    async fn run_attempt(
        &self,
        ctx: &ConsumerContext,
        cancel: &CancellationToken,
        state: &mut ReconnectState,
    ) -> Result<StreamOutcome, CamelError> {
        let connected = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(StreamOutcome::Cancelled),
            connected = self.connect_once() => connected,
        };
        let response = match connected {
            Ok(response) => response,
            Err(ConnectFailure::Terminal(err)) => return Ok(StreamOutcome::Terminal(err)),
            Err(ConnectFailure::Retriable(reason)) => {
                return Ok(StreamOutcome::Retriable(reason));
            }
        };
        self.run_stream_once(response, ctx, cancel, state).await
    }

    /// Resubscription loop: one full connect + stream attempt per iteration,
    /// gated and spaced by the retry policy. Budget exhaustion is a task
    /// error; route supervision owns recovery (ADR-0007).
    async fn run(
        &self,
        ctx: &ConsumerContext,
        cancel: &CancellationToken,
    ) -> Result<(), CamelError> {
        let mut state = ReconnectState::default();
        loop {
            let outcome = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Ok(()),
                outcome = self.run_attempt(ctx, cancel, &mut state) => outcome?,
            };
            match outcome {
                StreamOutcome::Cancelled => return Ok(()),
                StreamOutcome::Terminal(err) => return Err(err),
                StreamOutcome::Retriable(reason) => {
                    tracing::debug!(
                        route_id = %ctx.route_id(),
                        reason = %reason,
                        "sse stream ended, resubscribing"
                    );
                }
                StreamOutcome::Completed => {
                    tracing::debug!(
                        route_id = %ctx.route_id(),
                        "sse stream completed, resubscribing"
                    );
                }
            }

            if !self.retry_policy.should_retry(state.attempt) {
                return Err(CamelError::Io(format!(
                    "sse: retry budget exhausted after {} attempts",
                    state.attempt
                )));
            }
            let delay = next_delay(state.attempt, state.retry_override_ms, &self.config);
            // log-policy: outside-contract
            tracing::warn!(
                route_id = %ctx.route_id(),
                attempt = state.attempt,
                delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                "sse reconnecting"
            );
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Ok(()),
                _ = tokio::time::sleep(delay) => {}
            }
            state.attempt = state.attempt.saturating_add(1);
        }
    }
}

/// Builds the InOnly exchange for one SSE event.
fn build_exchange(event: &SseEventFrame) -> Exchange {
    let mut message = Message::new(Body::Text(event.data.clone()));
    message.set_header(
        "CamelSseEvent",
        event.event.clone().unwrap_or_else(|| "message".to_string()),
    );
    if let Some(id) = &event.id {
        message.set_header("CamelSseId", id.clone());
    }
    if let Some(retry_ms) = event.retry_ms {
        message.set_header("CamelSseRetry", retry_ms.to_string());
    }
    Exchange::new(message)
}

/// SSE consumer.
pub(crate) struct SseConsumer {
    core: Arc<SseCore>,
    task_handle: Option<JoinHandle<Result<(), CamelError>>>,
    cancel_token: Option<CancellationToken>,
    route_id: Option<String>,
}

impl SseConsumer {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        uri: String,
        config: SseUriConfig,
        auth_headers: Vec<(HeaderName, HeaderValue)>,
        http_config: Arc<HttpConfig>,
        pinned_cache: Arc<PinnedClientCache>,
        client: reqwest::Client,
        runtime: Arc<dyn RuntimeObservability>,
    ) -> Self {
        let retry_policy = retry_policy_for(&config);
        Self {
            core: Arc::new(SseCore {
                uri,
                config,
                auth_headers,
                http_config,
                pinned_cache,
                client,
                runtime,
                retry_policy,
                last_event_id: Mutex::new(None),
            }),
            task_handle: None,
            cancel_token: None,
            route_id: None,
        }
    }

    /// Test-only: shrink the retry budget so exhaustion is observable in a
    /// bounded test. The URI config carries no attempt knob by design.
    #[cfg(test)]
    pub(crate) fn set_retry_max_attempts(&mut self, max_attempts: u32) {
        let core = Arc::get_mut(&mut self.core).expect("consumer core is not shared before start");
        core.retry_policy.max_attempts = max_attempts;
    }
}

#[async_trait::async_trait]
impl Consumer for SseConsumer {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        if self.task_handle.is_some() {
            return Err(CamelError::Config(
                "sse consumer already started".to_string(),
            ));
        }
        let cancel_token = context.cancel_token();
        self.cancel_token = Some(cancel_token.clone());
        self.route_id = Some(context.route_id().to_string());

        // Readiness means "the connect/stream task is spawned and will
        // (re)connect asynchronously" — the first connect is bounded in the
        // task by `connectTimeoutMs`, so no handshake is awaited here.
        context.mark_ready();

        let core = Arc::clone(&self.core);
        let handle = tokio::spawn(async move { core.run(&context, &cancel_token).await });
        self.task_handle = Some(handle);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        if let Some(token) = &self.cancel_token {
            token.cancel();
        }
        let route_id = self.route_id.as_deref().unwrap_or("?");
        let mut result = Ok(());
        if let Some(mut handle) = self.task_handle.take() {
            match tokio::time::timeout(STOP_BUDGET, &mut handle).await {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(err))) => result = Err(err),
                Ok(Err(join_err)) => {
                    // log-policy: system-broken
                    tracing::error!(
                        route_id = %route_id,
                        "sse consumer task panicked: {join_err}"
                    );
                    result = Err(CamelError::ProcessorError(format!(
                        "task panicked: {join_err}"
                    )));
                }
                Err(_) => {
                    handle.abort();
                    let _ = handle.await;
                    // log-policy: handler-owned
                    tracing::warn!(route_id = %route_id, "sse consumer did not stop in 10s; aborted");
                }
            }
        }
        self.cancel_token = None;
        result
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Sequential
    }

    fn startup_mode(&self) -> ConsumerStartupMode {
        // Networked consumer: an unreachable endpoint must not leave the
        // route "started" while the spawned task retries forever. Explicit
        // lets the runtime await `mark_ready()`.
        ConsumerStartupMode::Explicit
    }

    fn background_task_handle(&mut self) -> Option<JoinHandle<Result<(), CamelError>>> {
        self.task_handle.take()
    }
}

#[cfg(test)]
#[path = "sse_tests.rs"]
mod sse_tests;
