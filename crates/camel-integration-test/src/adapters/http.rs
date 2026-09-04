//! The HTTP partner adapter (ADR-0069 §5, §8; feature `http`).
//!
//! [`HttpPartner`] is the harness-owned far side of an HTTP wire: it
//! binds a loopback listener (`127.0.0.1:0` only — no free-port
//! probing, ADR-0069 §8) that records every request that reaches
//! the wire, and it drives the client role that performs real HTTP
//! requests to a configured address.
//!
//! Wire roles:
//!
//! - Outbound (the system under test sends): the partner's listener
//!   records method, path, headers, and exact body bytes, serves the
//!   first scripted response whose method and path match, and queues
//!   the arrival per request path; `receive` dequeues it as an
//!   [`IncomingMessage`] carrying the request line (`method`, `path`)
//!   with `status: None` (requests carry no status). The recording and
//!   the queue are the normative proof of what crossed the wire
//!   (ADR-0069 §5).
//! - Inbound (the scenario drives): `send` performs a real HTTP
//!   request to the target endpoint; `receive` awaits the response
//!   bounded by the action deadline and returns its status, headers,
//!   and body.
//!
//! `receive` resolves the role by dispatch state: a response parked by
//! this partner's own `send` (client role) wins; otherwise the call
//! awaits the next listener arrival (server role) for the endpoint's
//! request path, bounded by the deadline. The v1 bound, reconsidered
//! for the outbound queue: server-role arrivals queue per path (depth
//! [`ARRIVAL_LANE_CAPACITY`]); the client role stays one response in
//! flight per endpoint URI (v1 inbound scenarios drive one exchange at
//! a time — a second `send` to the same endpoint replaces the parked
//! response).
//!
//! The listener uses the same hyper 1 stack that sits under the
//! workspace's reqwest users, driven directly so one dependency set
//! serves both roles.

use std::collections::BTreeMap;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use bytes::Bytes;
use camel_api::Value;
use futures::future::BoxFuture;
use http::HeaderName;
use http::HeaderValue;
use http::Method;
use http::Request;
use http::Response;
use http::StatusCode;
use http::Uri;
use http_body_util::BodyExt;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::client::conn::http1;
use hyper::server::conn::http1::Builder as ServerBuilder;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::watch;

use crate::adapters::IncomingMessage;
use crate::adapters::OutgoingMessage;
use crate::adapters::PartnerAdapter;
use crate::adapters::ReceiveError;
use crate::adapters::ReceiveTimeout;
use crate::adapters::TransportError;
use crate::adapters::lock_through;
use crate::document::EndpointRef;

/// The status served when no scripted response matches a request:
/// a scripting gap is a partner-side defect, never a verdict.
const UNMATCHED_STATUS: u16 = 500;

/// Per-path arrival queue depth. Requests that reach the listener
/// while the scenario has not received yet queue per request path;
/// beyond this depth an arrival is dropped from the queue (it stays on
/// the recorder). The v1 bound: a scenario addresses one request per
/// `receive` action, so 64 queued arrivals on one path is a scripting
/// defect, not a workload.
const ARRIVAL_LANE_CAPACITY: usize = 64;

/// One scripted partner response, served to the first wire request
/// that matches.
///
/// A `None` matcher field matches any request. When no scripted
/// response matches, the listener serves status 500 with an empty
/// body and still records the request — unless the partner started
/// with a permissive default ([`HttpPartner::start_permissive`]),
/// which answers every unmatched request non-consumingly.
#[derive(Debug, Clone, Default)]
pub struct ScriptedResponse {
    /// Match this request method, case-insensitive; `None` matches
    /// any method.
    pub method: Option<String>,
    /// Match this request path (and query, when present) exactly;
    /// `None` matches any path.
    pub path: Option<String>,
    /// Response status to serve. `Default` is 200.
    pub status: u16,
    /// Response headers to serve.
    pub headers: BTreeMap<String, String>,
    /// Response body bytes to serve.
    pub body: Vec<u8>,
}

impl ScriptedResponse {
    /// Whether this response matches the recorded wire request.
    fn matches(&self, request: &HttpWireRequest) -> bool {
        let method_ok = self
            .method
            .as_deref()
            .is_none_or(|m| m.eq_ignore_ascii_case(&request.method));
        let path_ok = self.path.as_deref().is_none_or(|p| p == request.path);
        method_ok && path_ok
    }
}

/// One request that reached the partner listener, recorded as it
/// crossed the wire (ADR-0069 §5).
#[derive(Debug, Clone, PartialEq)]
pub struct HttpWireRequest {
    /// Request method, uppercased (`POST`, `GET`, ...).
    pub method: String,
    /// Request path with query, as received (`/orders`, `/q?a=1`).
    pub path: String,
    /// Request headers. Names are lowercase (hyper normalization);
    /// repeated names are joined with `, `.
    pub headers: BTreeMap<String, String>,
    /// Request body, the exact bytes that reached the wire.
    pub body: Vec<u8>,
}

/// A handle onto an [`HttpPartner`]'s recorded wire requests, valid
/// after the partner itself moved into a
/// [`PartnerRouter`](crate::adapters::PartnerRouter).
#[derive(Clone)]
pub struct HttpRecorder {
    requests: Arc<Mutex<Vec<HttpWireRequest>>>,
}

impl HttpRecorder {
    /// A snapshot of every request that reached the partner listener,
    /// in arrival order.
    pub fn recorded_requests(&self) -> Vec<HttpWireRequest> {
        lock_through(&self.requests).clone()
    }
}

/// One queued-arrival lane: the sender lives in the lane map for the
/// partner's lifetime; the receiver is taken (under the async mutex,
/// one `receive` at a time) by the server-role receive path.
struct ArrivalLane {
    /// Enqueue side, fed by `serve`.
    tx: mpsc::Sender<IncomingMessage>,
    /// Dequeue side; the async mutex serializes concurrent receives on
    /// the same path.
    rx: AsyncMutex<mpsc::Receiver<IncomingMessage>>,
}

/// Listener-side state, shared with the connection tasks.
struct ServerState {
    /// Scripted responses, consumed in order by matcher hit.
    scripted: Mutex<Vec<ScriptedResponse>>,
    /// Non-consuming fallback status for requests no scripted
    /// response matches; `None` keeps the unmatched-500 marker.
    fallback_status: Option<u16>,
    /// Recorded wire requests, shared with `HttpRecorder` handles.
    requests: Arc<Mutex<Vec<HttpWireRequest>>>,
    /// Outbound arrival queue, keyed by request path — the part of the
    /// endpoint URI a listener can discriminate (one partner owns one
    /// authority, so path identifies the endpoint).
    arrivals: Mutex<BTreeMap<String, Arc<ArrivalLane>>>,
}

/// Shared partner state: the bound listener's bookkeeping and the
/// client role's in-flight requests.
struct HttpInner {
    /// The address the listener bound.
    bound: SocketAddr,
    /// Listener-side scripting and recording.
    server: Arc<ServerState>,
    /// One parked response receiver per endpoint URI, filled by
    /// `send` and consumed by `receive`.
    in_flight: Mutex<BTreeMap<String, oneshot::Receiver<Result<IncomingMessage, TransportError>>>>,
    /// Signals the accept loop to stop when the partner drops.
    shutdown: Mutex<Option<watch::Sender<bool>>>,
}

/// The harness-owned far side of an HTTP wire (ADR-0069 §5).
///
/// The constructor binds `127.0.0.1:0`; [`HttpPartner::bound_addr`]
/// reports the bound address for endpoint URIs and bind variables.
/// Dropping the partner stops its accept loop; recorded requests
/// stay readable through the [`HttpRecorder`] handle.
pub struct HttpPartner {
    inner: Arc<HttpInner>,
}

impl HttpPartner {
    /// Starts a partner that serves the given scripted responses on
    /// a loopback listener bound to `127.0.0.1:0`.
    pub async fn start(scripted: Vec<ScriptedResponse>) -> io::Result<Self> {
        Self::start_with(scripted, None).await
    }

    /// Starts a partner with a non-consuming permissive default: any
    /// request no scripted response matches is answered with `status`
    /// (empty body) for the partner's whole lifetime. Scripted
    /// entries still consume in order and win over the default; with
    /// none scripted, every request gets the permissive status. The
    /// CLI full-boot path uses this so a document whose route hits
    /// the same harness endpoint more than once never meets the
    /// unmatched-500 scripting gap its author never scripted.
    pub async fn start_permissive(status: u16) -> io::Result<Self> {
        Self::start_with(Vec::new(), Some(status)).await
    }

    /// The shared constructor behind [`start`](Self::start) and
    /// [`start_permissive`](Self::start_permissive).
    async fn start_with(
        scripted: Vec<ScriptedResponse>,
        fallback_status: Option<u16>,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let bound = listener.local_addr()?;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let server = Arc::new(ServerState {
            scripted: Mutex::new(scripted),
            fallback_status,
            requests: Arc::clone(&requests),
            arrivals: Mutex::new(BTreeMap::new()),
        });
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let accept_state = Arc::clone(&server);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _peer)) = accepted else {
                            continue;
                        };
                        let server = Arc::clone(&accept_state);
                        tokio::spawn(async move {
                            let service = service_fn(move |request| {
                                let server = Arc::clone(&server);
                                async move { Ok::<_, std::convert::Infallible>(serve(server, request).await) }
                            });
                            // A peer that misbehaves mid-connection
                            // only ends that connection; the partner
                            // keeps listening.
                            let _ = ServerBuilder::new()
                                .serve_connection(TokioIo::new(stream), service)
                                .await;
                        });
                    }
                }
            }
        });
        Ok(Self {
            inner: Arc::new(HttpInner {
                bound,
                server,
                in_flight: Mutex::new(BTreeMap::new()),
                shutdown: Mutex::new(Some(shutdown_tx)),
            }),
        })
    }

    /// The address the listener bound; endpoint URIs address it as
    /// `http://{bound_addr}/path`.
    pub fn bound_addr(&self) -> SocketAddr {
        self.inner.bound
    }

    /// A handle onto the wire requests this partner recorded.
    pub fn recorder(&self) -> HttpRecorder {
        HttpRecorder {
            requests: Arc::clone(&self.inner.server.requests),
        }
    }

    /// Validates the endpoint URI for the client role and launches
    /// the HTTP roundtrip; the response is parked for `receive`.
    fn launch_request(&self, endpoint: &str, msg: OutgoingMessage) -> Result<(), TransportError> {
        let target = ParsedTarget::parse(endpoint)?;
        let (tx, rx) = oneshot::channel();
        lock_through(&self.inner.in_flight).insert(endpoint.to_string(), rx);
        tokio::spawn(async move {
            let result = perform_request(&target, msg).await;
            // The receiver drops when the scenario never receives;
            // that is normal, not an error to report.
            let _ = tx.send(result);
        });
        Ok(())
    }

    /// Awaits the response parked for this endpoint (client role),
    /// bounded by the deadline.
    async fn await_response(
        &self,
        endpoint: &str,
        deadline: Duration,
        rx: oneshot::Receiver<Result<IncomingMessage, TransportError>>,
    ) -> Result<IncomingMessage, ReceiveError> {
        let started = tokio::time::Instant::now();
        match tokio::time::timeout(deadline, rx).await {
            Err(_) => Err(ReceiveError::Timeout(ReceiveTimeout {
                endpoint: endpoint.to_string(),
                deadline,
                elapsed: started.elapsed(),
            })),
            Ok(Ok(result)) => result.map_err(ReceiveError::Transport),
            Ok(Err(_cancelled)) => Err(ReceiveError::Transport(TransportError::Other {
                message: "http request task ended without delivering a response".to_string(),
            })),
        }
    }

    /// Awaits the next listener arrival queued for the endpoint's
    /// request path (server role), bounded by the deadline.
    async fn await_arrival(
        &self,
        endpoint: &str,
        deadline: Duration,
    ) -> Result<IncomingMessage, ReceiveError> {
        // Same origin-form shape the listener keys lanes by: path and
        // query, `/` when the URI carries none.
        let path = ParsedTarget::parse(endpoint)
            .map(|target| target.target)
            .unwrap_or_else(|_| "/".to_string());
        let lane = lane_for(&self.inner.server.arrivals, &path);
        let mut rx = lane.rx.lock().await;
        let started = tokio::time::Instant::now();
        match tokio::time::timeout(deadline, rx.recv()).await {
            Ok(Some(message)) => Ok(message),
            // The lane's sender never drops (it lives in the lane map),
            // so a closed queue is unreachable; map it to a timeout so
            // the call still never hangs.
            Ok(None) | Err(_) => Err(ReceiveError::Timeout(ReceiveTimeout {
                endpoint: endpoint.to_string(),
                deadline,
                elapsed: started.elapsed(),
            })),
        }
    }
}

impl Drop for HttpPartner {
    fn drop(&mut self) {
        if let Some(shutdown) = lock_through(&self.inner.shutdown).take() {
            // A closed receiver only means the accept loop already
            // stopped; nothing to report.
            let _ = shutdown.send(true);
        }
    }
}

impl PartnerAdapter for HttpPartner {
    fn send<'a>(
        &'a self,
        target: &'a EndpointRef,
        msg: OutgoingMessage,
    ) -> BoxFuture<'a, Result<(), TransportError>> {
        Box::pin(async move { self.launch_request(target.endpoint.as_str(), msg) })
    }

    fn receive<'a>(
        &'a self,
        source: &'a EndpointRef,
        deadline: Duration,
    ) -> BoxFuture<'a, Result<IncomingMessage, ReceiveError>> {
        Box::pin(async move {
            // Client role first: a response parked by this partner's
            // own `send` wins. Otherwise the server role: await the
            // next listener arrival queued for the request path.
            let parked = lock_through(&self.inner.in_flight).remove(source.endpoint.as_str());
            match parked {
                Some(rx) => self.await_response(&source.endpoint, deadline, rx).await,
                None => self.await_arrival(&source.endpoint, deadline).await,
            }
        })
    }
}

/// The client role's parsed endpoint: a plain `http` authority and
/// request target.
struct ParsedTarget {
    /// Host from the endpoint URI.
    host: String,
    /// Port from the endpoint URI; 80 when absent.
    port: u16,
    /// Origin-form request target (path and query); `/` when the
    /// URI carries none.
    target: String,
}

impl ParsedTarget {
    /// Parses an endpoint URI for the client role. Only scheme
    /// `http` is supported: the partner speaks plain loopback.
    fn parse(endpoint: &str) -> Result<Self, TransportError> {
        let invalid = |detail: String| TransportError::Other {
            message: format!("endpoint {endpoint}: {detail}"),
        };
        let uri = Uri::try_from(endpoint).map_err(|e| invalid(format!("invalid uri: {e}")))?;
        match uri.scheme_str() {
            Some("http") => {}
            other => {
                return Err(invalid(format!(
                    "unsupported scheme {} (the http partner speaks plain http)",
                    other.unwrap_or("<none>")
                )));
            }
        }
        let host = uri
            .host()
            .ok_or_else(|| invalid("no host".to_string()))?
            .to_string();
        let port = uri.port_u16().unwrap_or(80);
        let target = uri
            .path_and_query()
            .map(|pq| pq.as_str().to_string())
            .filter(|pq| !pq.is_empty())
            .unwrap_or_else(|| "/".to_string());
        Ok(Self { host, port, target })
    }
}

/// Performs one real HTTP/1.1 roundtrip and maps the response into
/// an [`IncomingMessage`].
async fn perform_request(
    target: &ParsedTarget,
    msg: OutgoingMessage,
) -> Result<IncomingMessage, TransportError> {
    let transport = |detail: String| TransportError::Other { message: detail };
    let stream = TcpStream::connect((target.host.as_str(), target.port))
        .await
        .map_err(|e| {
            transport(format!(
                "connect to {}:{} failed: {e}",
                target.host, target.port
            ))
        })?;
    let (mut sender, connection) = http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|e| transport(format!("http handshake failed: {e}")))?;
    tokio::spawn(async move {
        // Connection errors after the response are keep-alive
        // teardown noise; the exchange already completed.
        let _ = connection.await;
    });
    sender
        .ready()
        .await
        .map_err(|e| transport(format!("http connection not ready: {e}")))?;

    let body = value_to_wire(&msg.body);
    let method = if msg.body.is_null() {
        Method::GET
    } else {
        Method::POST
    };
    let mut builder = Request::builder()
        .method(method)
        .uri(target.target.clone())
        .header("host", format!("{}:{}", target.host, target.port))
        .header("connection", "close");
    for (name, value) in &msg.headers {
        // Hyper writes the header name as declared and lowercases it
        // on the wire; the value is the exact scenario string.
        builder = builder.header(name.as_str(), value_to_header(value));
    }
    let request = builder
        .body(Full::new(Bytes::from(body)))
        .map_err(|e| transport(format!("http request build failed: {e}")))?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|e| transport(format!("http request failed: {e}")))?;

    let (parts, response_body) = response.into_parts();
    let bytes = response_body
        .collect()
        .await
        .map_err(|e| transport(format!("http response body failed: {e}")))?
        .to_bytes();
    let content_type = parts
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_ascii_lowercase());
    Ok(IncomingMessage {
        status: Some(parts.status.as_u16()),
        headers: wire_headers_to_value(&parts.headers),
        body: wire_body_to_value(content_type.as_deref(), &bytes),
        // The client role receives a response: no request line.
        method: None,
        path: None,
    })
}

/// Returns the arrival lane for `path`, creating an empty lane on
/// first use. Locking is bounded to map access; the lane's channel is
/// lock-free beyond it.
fn lane_for(arrivals: &Mutex<BTreeMap<String, Arc<ArrivalLane>>>, path: &str) -> Arc<ArrivalLane> {
    let mut lanes = lock_through(arrivals);
    lanes
        .entry(path.to_string())
        .or_insert_with(|| {
            let (tx, rx) = mpsc::channel(ARRIVAL_LANE_CAPACITY);
            Arc::new(ArrivalLane {
                tx,
                rx: AsyncMutex::new(rx),
            })
        })
        .clone()
}

/// Maps a wire request into an [`IncomingMessage`] and queues it on the
/// request path's lane. Requests carry no status (the scripted response
/// status is harness-known), so `status` is `None`. Beyond the lane
/// capacity the arrival is dropped from the queue — it stays on the
/// recorder — and the response is still served.
fn enqueue_arrival(
    arrivals: &Mutex<BTreeMap<String, Arc<ArrivalLane>>>,
    wire: &HttpWireRequest,
    request_headers: &http::HeaderMap,
    bytes: &[u8],
) {
    let content_type = request_headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_ascii_lowercase());
    let arrival = IncomingMessage {
        body: wire_body_to_value(content_type.as_deref(), bytes),
        headers: wire_headers_to_value(request_headers),
        status: None,
        method: Some(wire.method.clone()),
        path: Some(wire.path.clone()),
    };
    let lane = lane_for(arrivals, &wire.path);
    // try_send, not send: a full lane means the scenario is not
    // receiving; backpressure would stall the connection's response
    // and, through it, the system under test's producer.
    if lane.tx.try_send(arrival).is_err() {
        // log-policy: harness-defect — a full lane is a scripting
        // defect, not a workload; the arrival stays on the recorder.
        tracing::warn!(
            path = %wire.path,
            capacity = ARRIVAL_LANE_CAPACITY,
            "arrival lane full; arrival recorded but not queued for receive"
        );
    }
}

/// Serves one connection request: record the wire request, then
/// answer with the first matching scripted response.
async fn serve(state: Arc<ServerState>, request: Request<Incoming>) -> Response<Full<Bytes>> {
    let (parts, body) = request.into_parts();
    // An unreadable body (peer disconnected mid-send) records as
    // empty: the partial request still crossed the wire.
    let bytes = body
        .collect()
        .await
        .map(|collected| collected.to_bytes())
        .unwrap_or_default();
    let wire = HttpWireRequest {
        method: parts.method.as_str().to_ascii_uppercase(),
        path: parts
            .uri
            .path_and_query()
            .map(|pq| pq.as_str().to_string())
            .unwrap_or_else(|| "/".to_string()),
        headers: wire_headers_to_string(&parts.headers),
        body: bytes.to_vec(),
    };
    lock_through(&state.requests).push(wire.clone());
    enqueue_arrival(&state.arrivals, &wire, &parts.headers, &bytes);

    let scripted = {
        let mut queue = lock_through(&state.scripted);
        queue
            .iter()
            .position(|s| s.matches(&wire))
            .map(|idx| queue.remove(idx))
    };
    let Some(scripted) = scripted else {
        // A permissive default (when the partner started with one) is
        // non-consuming: it holds for every unmatched request.
        return empty_response(state.fallback_status.unwrap_or(UNMATCHED_STATUS));
    };
    build_response(scripted.status, scripted.headers, scripted.body)
}

/// Builds a response with exact status, headers, and body bytes.
/// Header names or values that hyper rejects are skipped: the
/// scripted pair is test input, not a runtime failure.
fn build_response(
    status: u16,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
) -> Response<Full<Bytes>> {
    let mut builder = Response::builder()
        .status(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR));
    for (name, value) in &headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::try_from(name.as_str()),
            HeaderValue::from_str(value),
        ) {
            builder = builder.header(name, value);
        }
    }
    builder
        .body(Full::new(Bytes::from(body)))
        .unwrap_or_else(|_| empty_response(UNMATCHED_STATUS))
}

/// An empty-body response with the given status.
fn empty_response(status: u16) -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
        .body(Full::new(Bytes::new()))
        .unwrap_or_else(|_| {
            // The empty 500 response is always constructible; this
            // branch exists only for the type checker.
            Response::new(Full::new(Bytes::new()))
        })
}

/// Encodes a scenario body value onto the wire: strings pass through
/// as exact bytes, `Null` is empty, and structured values serialize
/// as compact JSON.
fn value_to_wire(body: &Value) -> Vec<u8> {
    match body {
        Value::Null => Vec::new(),
        Value::String(text) => text.clone().into_bytes(),
        other => other.to_string().into_bytes(),
    }
}

/// Encodes a scenario header value: strings pass through exactly,
/// structured values serialize as compact JSON.
fn value_to_header(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Folds wire headers into a map: names lowercase, repeated names
/// joined with `, `, non-UTF-8 values taken lossily.
fn fold_wire_headers<V>(
    headers: &http::HeaderMap,
    mut render: impl FnMut(String) -> V,
) -> BTreeMap<String, V> {
    let mut folded: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, value) in headers.iter() {
        folded
            .entry(name.as_str().to_string())
            .or_default()
            .push(String::from_utf8_lossy(value.as_bytes()).into_owned());
    }
    folded
        .into_iter()
        .map(|(name, values)| (name, render(values.join(", "))))
        .collect()
}

/// Wire headers as string values (recording side).
fn wire_headers_to_string(headers: &http::HeaderMap) -> BTreeMap<String, String> {
    fold_wire_headers(headers, |joined| joined)
}

/// Wire headers as scenario values (client receive side).
fn wire_headers_to_value(headers: &http::HeaderMap) -> BTreeMap<String, Value> {
    fold_wire_headers(headers, Value::String)
}

/// Decodes a response body into a scenario value: JSON when the
/// content type says so (falling back to text on parse failure),
/// otherwise the UTF-8 text, taken lossily for non-UTF-8 bytes.
fn wire_body_to_value(content_type: Option<&str>, bytes: &[u8]) -> Value {
    if content_type.is_some_and(|ct| ct.contains("application/json")) && !bytes.is_empty() {
        return serde_json::from_slice(bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(bytes).into_owned()));
    }
    Value::String(String::from_utf8_lossy(bytes).into_owned())
}
