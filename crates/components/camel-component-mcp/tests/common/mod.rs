//! Shared helpers for camel-component-mcp integration tests.
//!
//! This module is compiled once per integration-test binary (`client_producer_test`,
//! `server_consumer_test`); each binary uses only a subset of these helpers, so
//! dead_code warnings are inherent per binary and silenced at the module level.
#![allow(dead_code)]
//!
//! Provides:
//! - [`RecordedRequest`] + an axum middleware layer that records every POST
//!   (method, headers, JSON body) — handlers never see HTTP headers, so the
//!   recording must happen at the HTTP layer, not in a `ServerHandler`.
//! - [`warn_capture`] — returns a handle over the process-wide recording
//!   tracing subscriber (installed once via `set_global_default`); assert on
//!   captured `(level, message)` pairs filtered by message substring.
//! - [`spawn_mock`] — an in-process rmcp Streamable-HTTP server on
//!   `127.0.0.1:0` with parameterizable `supported_protocol_versions()` and
//!   canned `tools/call` / `resources/read` answers.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use axum::body::Body;
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, DiscoverResult, ProtocolVersion,
    ReadResourceRequestParams, ReadResourceResponse, ServerCapabilities, ServerInfo,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use tracing_subscriber::layer::SubscriberExt;

/// Cap on a recorded request body (JSON-RPC messages are small).
const MAX_RECORDED_BODY_BYTES: usize = 1 << 20;

/// Canned `tools/call` text: `<tool>:ok`.
pub const CANNED_TOOL_SUFFIX: &str = "ok";
/// Canned `resources/read` text.
pub const CANNED_RESOURCE_TEXT: &str = "mock-resource-contents";

/// One HTTP request as seen by the wire (headers included).
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    /// HTTP method (`POST`).
    pub method: String,
    /// Header names lowercased → values.
    pub headers: HashMap<String, String>,
    /// Parsed JSON-RPC body (`Null` when not valid JSON).
    pub body: serde_json::Value,
}

/// Sink of recorded requests shared between the middleware and the test.
type RequestSink = Arc<Mutex<Vec<RecordedRequest>>>;

/// A running mock remote MCP server.
pub struct MockRemote {
    /// Base URL of the mock's Streamable-HTTP endpoint.
    pub url: String,
    requests: RequestSink,
}

impl MockRemote {
    /// All requests recorded so far.
    pub fn recorded(&self) -> Vec<RecordedRequest> {
        self.requests.lock().expect("request sink poisoned").clone()
    }
}

/// Behavior knobs of the mock remote.
#[derive(Debug, Clone)]
pub struct MockOptions {
    /// Versions the mock advertises in `server/discover`.
    pub supported_versions: Vec<ProtocolVersion>,
    /// Answer `server/discover` with JSON-RPC `METHOD_NOT_FOUND` (legacy remote
    /// without the discover method).
    pub discover_not_found: bool,
    /// Answer `tools/call` with a tool-level error result (`isError: true`)
    /// instead of the canned success.
    pub tool_error: bool,
    /// Loopback bind address of the mock listener. Tests pick a distinct
    /// `127.0.0.x` per test file to keep parallel binaries from colliding.
    pub bind: String,
}

impl MockOptions {
    /// A remote that advertises exactly `supported_versions` and answers
    /// `server/discover` normally.
    pub fn advertises(supported_versions: Vec<ProtocolVersion>) -> Self {
        Self {
            supported_versions,
            discover_not_found: false,
            tool_error: false,
            bind: "127.0.0.1".to_owned(),
        }
    }

    /// A discover-capable-shape remote that rejects `server/discover`.
    pub fn no_discover() -> Self {
        Self {
            supported_versions: vec![ProtocolVersion::V_2026_07_28],
            discover_not_found: true,
            tool_error: false,
            bind: "127.0.0.1".to_owned(),
        }
    }
}

/// The mock's `ServerHandler`: canned answers, parameterizable versions.
#[derive(Clone)]
struct MockHandler {
    options: Arc<MockOptions>,
}

impl rmcp::ServerHandler for MockHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::default()
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Owned(self.options.supported_versions.clone())
    }

    async fn discover(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> Result<DiscoverResult, rmcp::ErrorData> {
        if self.options.discover_not_found {
            Err(rmcp::ErrorData::method_not_found::<
                rmcp::model::DiscoverRequestMethod,
            >())
        } else {
            Ok(DiscoverResult::new(
                self.options.supported_versions.clone(),
                ServerCapabilities::default(),
            ))
        }
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        let content = vec![rmcp::model::ContentBlock::text(format!(
            "{}:{CANNED_TOOL_SUFFIX}",
            request.name
        ))];
        let result = if self.options.tool_error {
            rmcp::model::CallToolResult::error(content)
        } else {
            rmcp::model::CallToolResult::success(content)
        };
        Ok(CallToolResponse::Complete(result))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, rmcp::ErrorData> {
        Ok(ReadResourceResponse::Complete(
            rmcp::model::ReadResourceResult::new(vec![rmcp::model::ResourceContents::text(
                CANNED_RESOURCE_TEXT,
                request.uri,
            )]),
        ))
    }
}

/// Spawn the mock remote on `127.0.0.1:0` (ephemeral port).
///
/// The rmcp `StreamableHttpService` runs stateless (no session store — the
/// `NeverSessionManager` never assigns `Mcp-Session-Id`) and replies with plain
/// JSON (`json_response`), behind an axum layer that records every POST.
pub async fn spawn_mock(options: MockOptions) -> MockRemote {
    let requests: RequestSink = Arc::new(Mutex::new(Vec::new()));
    let bind_host = options.bind.clone();
    let bind = format!("{bind_host}:0");
    let handler = MockHandler {
        options: Arc::new(options),
    };
    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        Arc::new(NeverSessionManager::default()),
        // Allow the configured bind host (`127.0.0.x` per-file convention);
        // JSON replies keep the client off the SSE path.
        StreamableHttpServerConfig::default()
            .with_json_response(true)
            .with_allowed_hosts(vec![bind_host]),
    );
    let sink = requests.clone();
    let app = axum::Router::new()
        .route("/mcp", axum::routing::any_service(service))
        .layer(axum::middleware::from_fn(
            move |request: Request, next: Next| {
                let sink = sink.clone();
                async move { record_request(request, next, sink).await }
            },
        ));
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .expect("bind ephemeral mock port");
    let address = listener.local_addr().expect("read mock local address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    MockRemote {
        url: format!("http://{address}/mcp"),
        requests,
    }
}

/// Axum middleware: snapshot every POST, then pass it on untouched.
async fn record_request(request: Request, next: Next, sink: RequestSink) -> Response {
    if request.method() == axum::http::Method::POST {
        let (parts, body) = request.into_parts();
        let bytes = axum::body::to_bytes(body, MAX_RECORDED_BODY_BYTES)
            .await
            .unwrap_or_default();
        let headers = parts
            .headers
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    value.to_str().unwrap_or_default().to_owned(),
                )
            })
            .collect();
        let body_json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        sink.lock()
            .expect("request sink poisoned")
            .push(RecordedRequest {
                method: "POST".to_owned(),
                headers,
                body: body_json,
            });
        return next
            .run(Request::from_parts(parts, Body::from(bytes)))
            .await;
    }
    next.run(request).await
}

/// Sink of captured `(level, message)` pairs, shared process-wide.
type WarnSink = Arc<Mutex<Vec<(tracing::Level, String)>>>;

/// Handle over the process-wide recording sink.
pub struct WarnCapture {
    records: WarnSink,
}

/// Return a handle over the process-wide recording subscriber.
///
/// The subscriber is installed once per process (`set_global_default`, guarded
/// by a `OnceLock`). A thread-local `set_default` misses warns emitted on tasks
/// polled off the capturing thread (the rmcp transport worker runs on its own
/// task), which flakes exactly-once warn assertions under parallel test runs;
/// a global subscriber captures from every thread. Because every test in a
/// binary shares one sink, each test must assert on its own unique message
/// marker (server name, peer address, or version).
pub fn warn_capture() -> WarnCapture {
    WarnCapture {
        records: global_warn_sink().clone(),
    }
}

/// The process-wide recording sink, installed as the global subscriber on
/// first use.
fn global_warn_sink() -> &'static WarnSink {
    static SINK: OnceLock<WarnSink> = OnceLock::new();
    SINK.get_or_init(|| {
        let records: WarnSink = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(RecordingLayer {
            records: records.clone(),
        });
        // The OnceLock guards the single call; no other global default exists
        // in these test binaries, so an install failure is a hard test-infra
        // error rather than a silently unrecorded warn.
        tracing::subscriber::set_global_default(subscriber)
            .expect("global tracing subscriber must install exactly once");
        records
    })
}

impl WarnCapture {
    /// True when any captured `WARN` message contains `needle`.
    pub fn has_warn_containing(&self, needle: &str) -> bool {
        self.records
            .lock()
            .expect("warn records poisoned")
            .iter()
            .any(|(level, message)| *level == tracing::Level::WARN && message.contains(needle))
    }

    /// Number of captured `WARN` messages containing `needle` — for
    /// exactly-once warn assertions.
    pub fn warn_count_containing(&self, needle: &str) -> usize {
        self.records
            .lock()
            .expect("warn records poisoned")
            .iter()
            .filter(|(level, message)| *level == tracing::Level::WARN && message.contains(needle))
            .count()
    }

    /// The captured `WARN` messages containing `needle`, for scoping
    /// multi-marker assertions to one unique per-test marker (e.g. a peer
    /// address) so sibling tests sharing the sink cannot over-count.
    pub fn warn_messages_containing(&self, needle: &str) -> Vec<String> {
        self.records
            .lock()
            .expect("warn records poisoned")
            .iter()
            .filter(|(level, message)| *level == tracing::Level::WARN && message.contains(needle))
            .map(|(_, message)| message.clone())
            .collect()
    }
}

/// `tracing_subscriber` layer recording `(level, message)` pairs.
struct RecordingLayer {
    records: Arc<Mutex<Vec<(tracing::Level, String)>>>,
}

impl<S> tracing_subscriber::Layer<S> for RecordingLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        self.records
            .lock()
            .expect("warn records poisoned")
            .push((*event.metadata().level(), visitor.0));
    }
}

/// Field visitor extracting the `message` field.
struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.0 = value.to_owned();
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}
