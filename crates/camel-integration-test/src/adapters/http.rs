//! The HTTP partner adapter (ADR-0069 §5, §8; feature `http`).
//!
//! HttpPartner is the harness-owned SERVER side of an HTTP wire:
//! it binds a loopback listener (`127.0.0.1:0` only — no free-port
//! probing, ADR-0069 §8) that records every request that reaches the
//! wire, serves the first matching scripted response (each entry
//! `times` requests, after its `delay`, or applying its `fault`
//! instead), and queues the arrival per request path for the
//! server-role `receive`.
//!
//! The CLIENT role lives one level up: [`PartnerRouter`]'s
//! ClientLane performs every http client-role send (the dial and
//! the parked roundtrip), keyed by lane key, so a send addressed to a
//! declared endpoint and a receive addressed through a dynamic
//! reference find the same lane. `HttpPartner` reports its bound
//! address through [`PartnerAdapter::bound_authority`] so the router
//! can resolve declared `:0` endpoints and interpolated authorities
//! to real wire targets.
//!
//! Wire roles:
//!
//! - Outbound (the system under test sends): the partner's listener
//!   records method, path, headers, and exact body bytes, serves the
//!   first matching scripted response (each entry `times` requests,
//!   after its `delay`, or applying its fault instead), and queues
//!   the arrival per request path; `receive` dequeues it as an
//!   [`IncomingMessage`] carrying the request line (`method`, `path`)
//!   with `status: None` (requests carry no status). The recording and
//!   the queue are the normative proof of what crossed the wire
//!   (ADR-0069 §5).
//! - Inbound (the scenario drives): the router's ClientLane
//!   `launch` performs a real HTTP request to the target URI; the
//!   router's `receive` returns the parked response bounded by the
//!   action deadline — its status, headers, and body.
//!
//! Server-role arrivals queue per path (depth
//! ARRIVAL_LANE_CAPACITY); the client lane parks same-key
//! responses in a bounded FIFO (depth LANE_FIFO_CAPACITY):
//! receives consume the oldest parked response first, in wire
//! arrival order, and a launch beyond the bound fails apparatus-class
//! ([`TransportError::LaneFifoOverflow`]) — never a silent overwrite.
//!
//! The listener uses the same hyper 1 stack that sits under the
//! workspace's reqwest users, driven directly so one dependency set
//! serves both roles.
//!
//! [`PartnerRouter`]: crate::adapters::PartnerRouter
//! [`PartnerAdapter::bound_authority`]: crate::adapters::PartnerAdapter::bound_authority

use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::io;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
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

use crate::adapters::ArrivalLaneOverflow;
use crate::adapters::IncomingMessage;
use crate::adapters::OutgoingMessage;
use crate::adapters::PartnerAdapter;
use crate::adapters::ReceiveError;
use crate::adapters::ReceiveTimeout;
use crate::adapters::TransportError;
use crate::adapters::lock_through;
use crate::adapters::redact_wire_path;
use crate::document::PartnerFault;

/// The status served when no scripted response matches a request:
/// a scripting gap is a partner-side defect, never a verdict.
const UNMATCHED_STATUS: u16 = 500;

/// Per-path arrival queue depth. Requests that reach the listener
/// while the scenario has not received yet queue per request path;
/// beyond this depth an arrival is dropped from the queue (it stays on
/// the recorder). The v1 bound: a scenario addresses one request per
/// `receive` action, so 64 queued arrivals on one path is a scripting
/// defect, not a workload.
pub(crate) const ARRIVAL_LANE_CAPACITY: usize = 64;

/// Per-lane-key client FIFO depth. Same-key sends park in arrival
/// order; beyond this depth a launch fails apparatus-class
/// ([`TransportError::LaneFifoOverflow`]) instead of silently
/// overwriting a parked roundtrip. The v1 bound: a scenario driving
/// more than 64 unanswered same-key exchanges at once is a scripting
/// defect, not a workload.
const LANE_FIFO_CAPACITY: usize = 64;

/// One scripted partner response, served to the matching wire
/// requests: the first matching entry with remaining `times` serves
/// (after its `delay`, if any) or applies its `fault` instead.
///
/// A `None` matcher field matches any request. When no scripted
/// response matches, the listener serves status 500 with an empty
/// body and still records the request — unless the partner started
/// with a permissive default ([`HttpPartner::start_permissive`]),
/// which answers every unmatched request non-consumingly.
#[derive(Debug, Clone)]
pub struct ScriptedResponse {
    /// Match this request method, case-insensitive; `None` matches
    /// any method.
    pub method: Option<String>,
    /// Match this request path (and query, when present) exactly;
    /// `None` matches any path.
    pub path: Option<String>,
    /// How many matching requests this entry serves before it is
    /// exhausted (removed from the script). `Default` is 1: serve
    /// once.
    pub times: u32,
    /// How long the partner waits before serving the response or
    /// applying the fault; `None` acts immediately.
    pub delay: Option<Duration>,
    /// The fault applied instead of serving the response; `None`
    /// serves `status`, `headers`, and `body`.
    pub fault: Option<PartnerFault>,
    /// Response status to serve. `Default` is 200.
    pub status: u16,
    /// Response headers to serve.
    pub headers: BTreeMap<String, String>,
    /// Response body bytes to serve.
    pub body: Vec<u8>,
}

impl Default for ScriptedResponse {
    /// Manual because the derived `Default` would zero `status` and
    /// `times`: the well-formed default is serve `status: 200` once,
    /// matching any request, with no delay and no fault.
    fn default() -> Self {
        Self {
            method: None,
            path: None,
            times: 1,
            delay: None,
            fault: None,
            status: 200,
            headers: BTreeMap::new(),
            body: Vec::new(),
        }
    }
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

/// A handle onto an HttpPartner's recorded wire requests, valid
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
    /// How many arrivals this lane dropped because it was full while
    /// no receive drained it. Overflow evidence for the
    /// apparatus-class receive error (rc-7mli).
    dropped: AtomicUsize,
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

/// Shared partner state: the bound listener's bookkeeping. The client
/// role (in-flight roundtrips, dialing) lives in the router's
/// ClientLane, not here.
struct HttpInner {
    /// The address the listener bound.
    bound: SocketAddr,
    /// Listener-side scripting and recording.
    server: Arc<ServerState>,
    /// Signals the accept loop to stop when the partner drops.
    shutdown: Mutex<Option<watch::Sender<bool>>>,
    /// The secret-marked query-key set for diagnostic redaction
    /// (ADR-0051): set through the router's fan-out after the
    /// partner moved into its box, read when a receive-timeout
    /// renders lane evidence.
    secret_query_keys: RwLock<Vec<String>>,
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
                                async move { serve(server, request).await }
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
                shutdown: Mutex::new(Some(shutdown_tx)),
                secret_query_keys: RwLock::new(Vec::new()),
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

    /// The stored secret-marked query-key set (ADR-0051), set through
    /// the router's fan-out.
    fn stored_secret_query_keys(&self) -> Vec<String> {
        self.inner
            .secret_query_keys
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// The wire paths that arrived on this partner, unique, in
    /// arrival order, each redacted through the stored secret-key set
    /// (ADR-0051) — the lane evidence a receive-timeout reports.
    fn recorded_lane_paths_redacted(&self) -> Vec<String> {
        let secret_keys = self.stored_secret_query_keys();
        let mut unique: Vec<String> = Vec::new();
        for request in lock_through(&self.inner.server.requests).iter() {
            if !unique.contains(&request.path) {
                unique.push(request.path.clone());
            }
        }
        unique
            .iter()
            .map(|path| redact_wire_path(path, &secret_keys))
            .collect()
    }

    /// Awaits the next listener arrival queued for the endpoint's
    /// request path (server role), bounded by the deadline. The path
    /// comes from the interpolated reference — the receive's own
    /// path-and-query — and `source_uri` names the failure.
    async fn await_arrival(
        &self,
        source_uri: &str,
        deadline: Duration,
    ) -> Result<IncomingMessage, ReceiveError> {
        // Same origin-form shape the listener keys lanes by: path and
        // query. An empty or absent path is an apparatus error — the
        // declaration names no lane. The parse renders its declaration
        // echo through the stored secret-key set (ADR-0051).
        let secret_keys = self.stored_secret_query_keys();
        let path = ParsedTarget::parse(source_uri, &secret_keys)
            .map_err(ReceiveError::Transport)?
            .target;
        let lane = lane_for(&self.inner.server.arrivals, &path);
        let mut rx = lane.rx.lock().await;
        let started = tokio::time::Instant::now();
        match tokio::time::timeout(deadline, rx.recv()).await {
            Ok(Some(message)) => Ok(message),
            // The lane's sender never drops (it lives in the lane map),
            // so a closed queue is unreachable; map it to a timeout so
            // the call still never hangs.
            // The endpoint name itself may carry query bytes (a
            // query-bearing declaration): render it and the lane
            // evidence both redacted (ADR-0051).
            Ok(None) | Err(_) => {
                let dropped = lane.dropped.load(Ordering::Relaxed);
                if dropped > 0 {
                    // A lane that dropped arrivals while the scenario
                    // was not receiving is a harness defect, not a
                    // system-under-test verdict: the timed-out wait
                    // reports the overflow evidence instead of a
                    // receive-timeout (rc-7mli).
                    return Err(ReceiveError::Overflow(ArrivalLaneOverflow {
                        endpoint: redact_wire_path(source_uri, &secret_keys),
                        dropped,
                    }));
                }
                Err(ReceiveError::Timeout(ReceiveTimeout {
                    endpoint: redact_wire_path(source_uri, &secret_keys),
                    deadline,
                    elapsed: started.elapsed(),
                    lanes_recorded: self.recorded_lane_paths_redacted(),
                }))
            }
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
    // No `send` override: the http client role belongs to the
    // router's ClientLane; the trait default declines client-role
    // sends. This partner keeps listener, scripting, recording, and
    // server-role duties only.

    fn receive<'a>(
        &'a self,
        _lane_key: &'a str,
        source_uri: &'a str,
        deadline: Duration,
    ) -> BoxFuture<'a, Result<IncomingMessage, ReceiveError>> {
        // Server role only: await the next listener arrival queued for
        // the interpolated reference's request path — `_lane_key`
        // stays the router's adapter-lookup key and names no lane. The
        // client-role-first dispatch lives in the router, over the
        // shared ClientLane.
        Box::pin(async move { self.await_arrival(source_uri, deadline).await })
    }

    fn bound_authority(&self) -> Option<String> {
        Some(self.bound_addr().to_string())
    }

    fn recorded_requests(&self) -> Vec<HttpWireRequest> {
        self.recorder().recorded_requests()
    }

    fn set_secret_query_keys(&self, keys: &[String]) {
        *self
            .inner
            .secret_query_keys
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = keys.to_vec();
    }
}

/// One parked response in a lane key's FIFO: the generation it was
/// booked under and the parked roundtrip receiver.
struct LaneEntry {
    /// The launch-unique generation; [`fail_lane_entry`] refuses to
    /// touch an entry carrying any other value.
    generation: u64,
    /// The parked roundtrip the client-role receive consumes.
    rx: oneshot::Receiver<Result<IncomingMessage, TransportError>>,
}

/// The client-lane map key: the registered partner key joined with
/// the wire request path-and-query through `\x1f` (the US separator,
/// a byte no valid URI authority or origin-form path carries, so the
/// join is collision-free). Client-role parking is path-aware (bd
/// rc-cr5yf): two paths of one partner park on distinct keys, and
/// the bounded-FIFO wire-arrival canon holds per key. Both the
/// launch and the take side compose through this one function, so
/// the two keys agree by construction.
fn lane_map_key(lane_key: &str, target_path: &str) -> String {
    format!("{lane_key}\x1f{target_path}")
}

/// The router-owned http client role: every http-scheme `send` the
/// router dispatches dials through this lane, and every http
/// `receive` checks it first for the parked roundtrip
/// (client-role-first).
///
/// Parking is path-aware (bd rc-cr5yf): the map key is the
/// registered partner key joined with the wire path-and-query
/// (lane_map_key), so roundtrips on different paths of one
/// partner never share a FIFO. Same-key sends park their responses
/// in a bounded FIFO (depth LANE_FIFO_CAPACITY) in wire arrival
/// order; a launch beyond the bound fails apparatus-class
/// ([`TransportError::LaneFifoOverflow`]). The map lock is a
/// `std::sync::Mutex`, held only for map access and never across an
/// await.
pub struct ClientLane {
    /// Per composite lane key (lane_map_key), the bounded FIFO of
    /// parked responses, filled by [`launch`](Self::launch) and
    /// drained oldest-first by [`take`](Self::take). `Arc`-shared
    /// with the spawned exchanges so a post-connect failure can park
    /// its error under its own generation
    /// ([`fail_lane_entry`](Self::fail_lane_entry)).
    in_flight: Arc<Mutex<BTreeMap<String, VecDeque<LaneEntry>>>>,
    /// Monotonic source of entry generations: every launch stamps its
    /// entry with a fresh value, so the spawned exchange's failure
    /// transition can tell its own entry from a later send's.
    next_generation: AtomicU64,
    /// The wire request targets launched through this lane, unique,
    /// in launch order — the lane evidence an
    /// [`await_parked`](Self::await_parked) receive-timeout reports
    /// (redacted at render time, ADR-0051).
    launched_wire_paths: Mutex<Vec<String>>,
    /// The secret-marked query-key set for diagnostic redaction
    /// (ADR-0051): set through the router's fan-out, read when a
    /// timeout renders the launched wire paths.
    secret_query_keys: RwLock<Vec<String>>,
}

impl ClientLane {
    /// An empty lane.
    pub(crate) fn new() -> Self {
        Self {
            in_flight: Arc::new(Mutex::new(BTreeMap::new())),
            next_generation: AtomicU64::new(0),
            launched_wire_paths: Mutex::new(Vec::new()),
            secret_query_keys: RwLock::new(Vec::new()),
        }
    }

    /// Stores the secret-marked query-key set (ADR-0051): the router
    /// fans it out; the [`await_parked`](Self::await_parked) timeout
    /// renders lane evidence through it.
    pub(crate) fn set_secret_query_keys(&self, keys: &[String]) {
        *self
            .secret_query_keys
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = keys.to_vec();
    }

    /// Validates the target URI and launches the HTTP roundtrip. The
    /// dial happens inline: a parse or connect failure returns
    /// [`TransportError`] from this call with NO lane entry inserted,
    /// so the caller observes the failure on the send itself (the
    /// whole send stays under the runner's send deadline). The lane
    /// books under the composite of the lane key and the parsed
    /// target path (lane_map_key): parking is path-aware, so two
    /// paths of one partner never share a FIFO. Only a live
    /// connection books the generation-stamped entry whose response
    /// the router's `receive` consumes; when the composite key's FIFO
    /// already holds LANE_FIFO_CAPACITY parked responses, the
    /// launch fails apparatus-class
    /// ([`TransportError::LaneFifoOverflow`]) instead of overwriting
    /// a parked roundtrip — refused BEFORE the dial, so an overflow
    /// leaves no wire effect: no connection, no launched-path
    /// evidence, no stamped generation. The lane handle stays alive
    /// in the spawned exchange so a post-connect failure parks
    /// through [`fail_lane_entry`](Self::fail_lane_entry).
    pub(crate) async fn launch(
        self: Arc<Self>,
        lane_key: &str,
        target_uri: &str,
        msg: OutgoingMessage,
    ) -> Result<(), TransportError> {
        // (a) Validate the target URI first — the composite map key
        // needs the parsed path. The parse renders its declaration
        // echo through the lane's stored secret-key set (ADR-0051).
        let secret_keys = self
            .secret_query_keys
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let target = ParsedTarget::parse(target_uri, &secret_keys)?;
        let map_key = lane_map_key(lane_key, &target.target);
        // The overflow diagnostic names the lane key and its path
        // half, each redacted (ADR-0051): the raw composite — and any
        // secret-marked query value it embeds — never leaves the
        // module, and the runner's render-site redaction stays a
        // second, idempotent defense.
        let rendered_key = format!(
            "{} {}",
            redact_wire_path(lane_key, &secret_keys),
            redact_wire_path(&target.target, &secret_keys)
        );
        // (b) Refuse a full FIFO before any wire effect: the
        // overflow fails before the dial, before the wire-path
        // evidence records, and before a generation is stamped. The
        // booking site re-checks under the insert lock, so the bound
        // holds under concurrent launches too.
        if lock_through(&self.in_flight)
            .get(map_key.as_str())
            .is_some_and(|fifo| fifo.len() >= LANE_FIFO_CAPACITY)
        {
            return Err(TransportError::LaneFifoOverflow {
                lane_key: rendered_key,
                bound: LANE_FIFO_CAPACITY,
            });
        }
        // (c) Dial inline: connection refused fails the send here.
        let stream = TcpStream::connect((target.host.as_str(), target.port))
            .await
            .map_err(|e| TransportError::Other {
                message: format!("connect to {}:{} failed: {e}", target.host, target.port),
            })?;
        // (d) A live connection: book the entry, stamped with a fresh
        // generation, and record the wire target this launch puts on
        // the wire (the lane evidence a later timeout reports). The
        // booking re-enforces the FIFO bound in the same critical
        // section as the insert: a FIFO that filled between the
        // pre-check and here fails the launch before any exchange
        // runs.
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut launched = lock_through(&self.launched_wire_paths);
            if !launched.contains(&target.target) {
                launched.push(target.target.clone());
            }
        }
        {
            let mut lanes = lock_through(&self.in_flight);
            let fifo = lanes.entry(map_key.clone()).or_default();
            if fifo.len() >= LANE_FIFO_CAPACITY {
                return Err(TransportError::LaneFifoOverflow {
                    lane_key: rendered_key,
                    bound: LANE_FIFO_CAPACITY,
                });
            }
            fifo.push_back(LaneEntry { generation, rx });
        }
        // (e) The exchange runs on the connected stream. A
        // post-connect failure parks the error under its own
        // generation only: any other entry on the key's FIFO (an
        // earlier or later send's) stays intact.
        let lane = Arc::clone(&self);
        let key = map_key;
        tokio::spawn(async move {
            let result = perform_exchange(stream, &target, msg).await;
            match result {
                Ok(response) => {
                    // The receiver drops when the scenario never
                    // receives; that is normal, not an error to
                    // report.
                    let _ = tx.send(Ok(response));
                }
                Err(error) => {
                    if !lane.fail_lane_entry(&key, generation, error.clone()) {
                        // The entry was already taken (a receive is
                        // consuming the old channel): normal.
                        let _ = tx.send(Err(error));
                    }
                }
            }
        });
        Ok(())
    }

    /// Takes the response parked under the composite of `lane_key`
    /// and `uri`'s parsed path-and-query (lane_map_key), if any;
    /// the router's client-role-first receive calls this before any
    /// server-role delegation. A reference that fails to parse (a
    /// bare authority carries no path) misses every path-aware key:
    /// the probe answers `None` and the server-role receive raises
    /// its own apparatus error (ps97b canon). The composite key's
    /// FIFO drains oldest-first (wire arrival order); the empty FIFO
    /// leaves the map.
    pub(crate) fn take(
        &self,
        lane_key: &str,
        uri: &str,
    ) -> Option<oneshot::Receiver<Result<IncomingMessage, TransportError>>> {
        // The secret-key set is empty here: the parse failure is
        // discarded, so its declaration echo never renders.
        let target = ParsedTarget::parse(uri, &[]).ok()?;
        let map_key = lane_map_key(lane_key, &target.target);
        let mut lanes = lock_through(&self.in_flight);
        let fifo = lanes.get_mut(map_key.as_str())?;
        let entry = fifo.pop_front()?;
        if fifo.is_empty() {
            lanes.remove(map_key.as_str());
        }
        Some(entry.rx)
    }

    /// The atomic failure transition: when the key's FIFO still
    /// carries an entry with `generation`, that entry's receiver is
    /// replaced in place with one already resolved to `error` and the
    /// call returns true; any other state returns false and touches
    /// nothing. Only the exchange's own entry is ever touched — no
    /// other generation on the FIFO can be removed or overwritten by
    /// an older exchange's failure.
    pub(crate) fn fail_lane_entry(
        &self,
        key: &str,
        generation: u64,
        error: TransportError,
    ) -> bool {
        fail_lane_map_entry(&self.in_flight, key, generation, error)
    }

    /// Awaits the response parked under the lane key, bounded by the
    /// deadline; `endpoint` names the failure. A timeout reports the
    /// wire paths this lane launched, every secret-marked query value
    /// masked (ADR-0051).
    pub(crate) async fn await_parked(
        &self,
        endpoint: &str,
        deadline: Duration,
        rx: oneshot::Receiver<Result<IncomingMessage, TransportError>>,
    ) -> Result<IncomingMessage, ReceiveError> {
        let started = tokio::time::Instant::now();
        match tokio::time::timeout(deadline, rx).await {
            Err(_) => {
                let secret_keys = self
                    .secret_query_keys
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                let lanes_recorded = lock_through(&self.launched_wire_paths)
                    .iter()
                    .map(|path| redact_wire_path(path, &secret_keys))
                    .collect();
                // The endpoint name may carry query bytes (a
                // query-bearing target): render it redacted too.
                Err(ReceiveError::Timeout(ReceiveTimeout {
                    endpoint: redact_wire_path(endpoint, &secret_keys),
                    deadline,
                    elapsed: started.elapsed(),
                    lanes_recorded,
                }))
            }
            Ok(Ok(result)) => result.map_err(ReceiveError::Transport),
            Ok(Err(_cancelled)) => Err(ReceiveError::Transport(TransportError::Other {
                message: "http request task ended without delivering a response".to_string(),
            })),
        }
    }
}

/// The client role's parsed endpoint: a plain `http` authority and
/// request target.
#[derive(Debug)]
struct ParsedTarget {
    /// Host from the endpoint URI.
    host: String,
    /// Port from the endpoint URI; 80 when absent.
    port: u16,
    /// Origin-form request target (path and query). The declaration
    /// must carry a non-empty one: an empty or absent path is an
    /// apparatus-class parse error, never a silent `/`.
    target: String,
}

impl ParsedTarget {
    /// Parses an endpoint URI for the client role. Only scheme
    /// `http` is supported: the partner speaks plain loopback. The
    /// declaration echo inside apparatus errors renders through
    /// [`redact_wire_path`] with `secret_keys`, so a secret-marked
    /// query value never prints (ADR-0051).
    fn parse(endpoint: &str, secret_keys: &[String]) -> Result<Self, TransportError> {
        let invalid = |detail: String| TransportError::Other {
            message: format!(
                "endpoint {}: {detail}",
                redact_wire_path(endpoint, secret_keys)
            ),
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
        // The `http` crate normalizes an absent path to `/`; the
        // harness reads the AUTHORED bytes, so a declaration like
        // `http://host` (no path after the authority) fails as an
        // apparatus error naming the declaration — never a silent
        // `/` lane. Once the authored tail starts with `/`,
        // `path_and_query()` is that non-empty tail.
        let authored_target = endpoint
            .split_once("://")
            .map(|(_, rest)| {
                let start = rest.find(['/', '?', '#']).unwrap_or(rest.len());
                &rest[start..]
            })
            .unwrap_or("");
        if !authored_target.starts_with('/') {
            // An empty or absent path names no lane: an apparatus
            // error, never a silent `/` lane. The declaration echo
            // renders through `invalid`, whose redaction masks any
            // secret-marked query value (ADR-0051).
            return Err(invalid(
                "empty or absent path: a harness target must declare a request path, never a silent `/` lane"
                    .to_string(),
            ));
        }
        let target = uri
            .path_and_query()
            .map(|pq| pq.as_str().to_string())
            .unwrap_or_default();
        Ok(Self { host, port, target })
    }
}

/// Runs the HTTP/1.1 exchange on the already-connected `stream` and
/// maps the response into an [`IncomingMessage`]. Only post-connect
/// failures surface here — the dial itself happened inline in
/// [`ClientLane::launch`].
async fn perform_exchange(
    stream: TcpStream,
    target: &ParsedTarget,
    msg: OutgoingMessage,
) -> Result<IncomingMessage, TransportError> {
    let transport = |detail: String| TransportError::Other { message: detail };
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
    let method = Method::from_str(&msg.method)
        .map_err(|e| transport(format!("invalid http method `{}`: {e}", msg.method)))?;
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
        // Stamped at response receipt: when the transport finished
        // receiving, not when a receive action consumes it.
        arrival: std::time::Instant::now(),
    })
}

/// The lane-map mutation behind [`ClientLane::fail_lane_entry`]:
/// one sync lock guard, one in-place map mutation, no await point —
/// so the generation check and the error-parking write are one
/// uninterrupted critical section. The key's FIFO is scanned for the
/// one entry whose generation matches; other entries stay untouched.
fn fail_lane_map_entry(
    in_flight: &Mutex<BTreeMap<String, VecDeque<LaneEntry>>>,
    key: &str,
    generation: u64,
    error: TransportError,
) -> bool {
    let mut lanes = lock_through(in_flight);
    let Some(entry) = lanes
        .get_mut(key)
        .and_then(|fifo| fifo.iter_mut().find(|entry| entry.generation == generation))
    else {
        return false;
    };
    let (tx, rx) = oneshot::channel();
    // The replaced receiver is still held by the entry, so the
    // resolution cannot fail.
    let _ = tx.send(Err(error));
    entry.rx = rx;
    true
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
                dropped: AtomicUsize::new(0),
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
        // Stamped here at the enqueue point: the transport finished
        // receiving the request, before any receive action can
        // consume it from the lane.
        arrival: std::time::Instant::now(),
    };
    let lane = lane_for(arrivals, &wire.path);
    // try_send, not send: a full lane means the scenario is not
    // receiving; backpressure would stall the connection's response
    // and, through it, the system under test's producer.
    if lane.tx.try_send(arrival).is_err() {
        // Overflow evidence for the receive path: a timed-out receive
        // on a lane that dropped arrivals reports the overflow, not a
        // plain timeout (rc-7mli).
        lane.dropped.fetch_add(1, Ordering::Relaxed);
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
/// answer with the first matching scripted response — consuming one
/// of its `times`, honoring its `delay`, or applying its fault. A
/// service error drops the connection without a response byte; the
/// close fault relies on exactly that hyper behavior.
async fn serve(
    state: Arc<ServerState>,
    request: Request<Incoming>,
) -> io::Result<Response<Full<Bytes>>> {
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
        // Consume under the lock: the first matching entry with
        // remaining times serves this request, and the entry leaves
        // the script at zero. The lock never spans an await — the
        // guard drops with the block, before the delay sleep.
        let mut queue = lock_through(&state.scripted);
        let idx = queue.iter().position(|s| s.matches(&wire) && s.times > 0);
        idx.map(|idx| {
            queue[idx].times -= 1;
            let entry = queue[idx].clone();
            if queue[idx].times == 0 {
                queue.remove(idx);
            }
            entry
        })
    };
    let Some(scripted) = scripted else {
        // A permissive default (when the partner started with one) is
        // non-consuming: it holds for every unmatched request.
        return Ok(empty_response(
            state.fallback_status.unwrap_or(UNMATCHED_STATUS),
        ));
    };
    if let Some(delay) = scripted.delay {
        tokio::time::sleep(delay).await;
    }
    if scripted.fault == Some(PartnerFault::Close) {
        // The fault replaces the response: the service error makes
        // hyper drop the connection with no HTTP response bytes, so
        // the client sees a transport-level failure, never a status.
        return Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "partner fault: close",
        ));
    }
    Ok(build_response(
        scripted.status,
        scripted.headers,
        scripted.body,
    ))
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
/// as compact JSON. The client send path and the partner script
/// mapping share this encoding so both wire roles serve the same
/// bytes for the same value.
pub(crate) fn value_to_wire(body: &Value) -> Vec<u8> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::PartnerRouter;
    use crate::test_util::router_for;

    /// The default scripted response is well-formed serve-once OK:
    /// the derived `Default` would zero both `status` and `times`,
    /// so the manual impl must be pinned by assertion.
    #[test]
    fn scripted_response_default_is_ok_once() {
        let scripted = ScriptedResponse::default();
        assert_eq!(scripted.status, 200);
        assert_eq!(scripted.times, 1);
        assert_eq!(scripted.method, None);
        assert_eq!(scripted.path, None);
        assert_eq!(scripted.delay, None);
        assert_eq!(scripted.fault, None);
        assert!(scripted.headers.is_empty());
        assert!(scripted.body.is_empty());
    }

    /// A harness target declaration whose path is empty or absent is
    /// an apparatus-class parse error naming the declaration — never
    /// a silent `/` lane (the recorded-authority-form default at
    /// `serve` is a wire-recording concern, not a declaration parse).
    #[test]
    fn parsed_target_empty_path_is_apparatus_error() {
        let error = ParsedTarget::parse("http://host", &[]).expect_err("an empty path must fail");
        match error {
            TransportError::Other { message } => {
                assert!(
                    message.contains("http://host"),
                    "must name the declaration: {message}"
                );
                assert!(
                    message.contains("path"),
                    "must name the missing path: {message}"
                );
            }
            other => panic!("expected an apparatus-class transport error, got {other:?}"),
        }
    }

    /// The empty-path apparatus error redacts a secret-marked query
    /// value (ADR-0051): the declaration echo keeps the raw
    /// `authPassword=` key span but masks the value — the secret
    /// never prints.
    #[test]
    fn empty_path_error_redacts_secret_query_value() {
        let error =
            ParsedTarget::parse("http://host?authPassword=x", &["authPassword".to_string()])
                .expect_err("an empty path must fail");
        match error {
            TransportError::Other { message } => {
                assert!(
                    message.contains("authPassword=***"),
                    "the secret value must be masked: {message}"
                );
                assert!(
                    !message.contains("authPassword=x"),
                    "the secret must never print raw: {message}"
                );
            }
            other => panic!("expected an apparatus-class transport error, got {other:?}"),
        }
    }

    /// With no secret keys configured the empty-path error names the
    /// declaration's query unchanged — the diagnostic stays maximally
    /// informative for non-secret pairs.
    #[test]
    fn empty_path_error_keeps_plain_query_diagnostic() {
        let error =
            ParsedTarget::parse("http://host?flag=a", &[]).expect_err("an empty path must fail");
        match error {
            TransportError::Other { message } => {
                assert!(
                    message.contains("http://host?flag=a"),
                    "must name the declaration unchanged: {message}"
                );
            }
            other => panic!("expected an apparatus-class transport error, got {other:?}"),
        }
    }

    /// The replace-then-fail race contract, deterministic: the
    /// failure transition lands only on the entry still carrying its
    /// own generation; a stale generation touches nothing, and the
    /// parked error surfaces on the lane key's receive.
    #[test]
    fn fail_lane_entry_is_conditional() {
        let lane = ClientLane::new();
        let map_key = lane_map_key("K", "/x");
        let (_, rx) = oneshot::channel();
        lock_through(&lane.in_flight).insert(
            map_key.clone(),
            VecDeque::from([LaneEntry { generation: 2, rx }]),
        );
        let error = || TransportError::Other {
            message: "boom".to_string(),
        };

        // A stale generation is rejected; the FIFO's only entry stays
        // untouched.
        assert!(!lane.fail_lane_entry(&map_key, 1, error()));
        assert_eq!(
            lock_through(&lane.in_flight)
                .get(&map_key)
                .and_then(|fifo| fifo.front())
                .map(|entry| entry.generation),
            Some(2)
        );

        // The entry's own generation parks the error in place.
        assert!(lane.fail_lane_entry(&map_key, 2, error()));
        let mut rx = lane
            .take("K", "http://h/x")
            .expect("the entry stays present");
        match rx.try_recv() {
            Ok(Err(TransportError::Other { message })) => assert_eq!(message, "boom"),
            other => panic!("the parked error must surface on receive, got {other:?}"),
        }
    }

    /// Path-aware parking (bd rc-cr5yf): entries booked under two
    /// paths of one lane key drain by their own path — a crossed
    /// probe order cannot cross-match, and a probe naming a third
    /// path misses both.
    #[test]
    fn take_drains_only_the_probed_path() {
        let lane = ClientLane::new();
        let book = |path: &str, generation| {
            let (_, rx) = oneshot::channel();
            lock_through(&lane.in_flight).insert(
                lane_map_key("K", path),
                VecDeque::from([LaneEntry { generation, rx }]),
            );
        };
        book("/a", 1);
        book("/b", 2);

        // The crossed probes each drain their own path's entry.
        assert!(lane.take("K", "http://h/b").is_some());
        assert!(lane.take("K", "http://h/a").is_some());
        // A third path of the same key parks nothing: the probe
        // misses and the receive falls through to the server role.
        assert!(lane.take("K", "http://h/c").is_none());
        // A bare authority composes no path-aware key: the probe
        // misses (the server-role receive owns that error).
        assert!(lane.take("K", "http://h").is_none());
        // The query bytes join the composite key: an exact-query
        // probe drains, a divergent query misses.
        book("/q?v=1", 3);
        assert!(lane.take("K", "http://h/q?v=1").is_some());
        assert!(lane.take("K", "http://h/q?v=2").is_none());
    }

    // -----------------------------------------------------------------
    // Wire-path diagnostics and redaction (spec: integration-tier)
    // -----------------------------------------------------------------

    /// One raw HTTP/1.1 exchange straight to the partner's bound
    /// address: the exact authored target bytes reach the wire with
    /// no client-side normalization, and the partner records before
    /// it answers, so a returned call means a recorded arrival.
    async fn raw_request(authority: &str, method: &str, target: &str) {
        use tokio::io::AsyncReadExt;
        use tokio::io::AsyncWriteExt;
        let mut stream = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::net::TcpStream::connect(authority),
        )
        .await
        .expect("connect to partner timed out after 5s")
        .expect("the partner's bound address must accept");
        let request = format!(
            "{method} {target} HTTP/1.1\r\nhost: {authority}\r\nconnection: close\r\ncontent-length: 0\r\n\r\n"
        );
        stream
            .write_all(request.as_bytes())
            .await
            .expect("the raw request must leave");
        let mut sink = Vec::new();
        stream
            .read_to_end(&mut sink)
            .await
            .expect("the partner must close after its response");
    }

    /// A router over one started permissive partner registered under
    /// `declared`, plus the partner's bound authority.
    async fn partner_router(declared: &str) -> (PartnerRouter, String) {
        let partner = HttpPartner::start_permissive(200)
            .await
            .expect("partner must bind 127.0.0.1:0");
        let authority = partner.bound_addr().to_string();
        (router_for(declared, partner), authority)
    }

    /// The rendered timeout message of a receive on `declared` under
    /// an already-expired budget.
    async fn expired_receive_message(router: &PartnerRouter, declared: &str) -> String {
        match router.receive(declared, declared, Duration::ZERO).await {
            Err(ReceiveError::Timeout(timeout)) => timeout.to_string(),
            other => panic!("expected a receive timeout, got {other:?}"),
        }
    }

    /// A receive-timeout lists the wire paths that arrived in the
    /// partner's lanes, so byte divergence is diagnosed from the
    /// failure text alone. `x` is not secret-marked: the path stays
    /// in clear.
    #[tokio::test]
    async fn receive_timeout_message_lists_arrived_wire_paths() {
        let (router, authority) = partner_router("http://127.0.0.1:0/other").await;
        raw_request(&authority, "GET", "/api?x=1").await;
        let message = expired_receive_message(&router, "http://127.0.0.1:0/other").await;
        assert!(
            message.contains("lanes recorded: [/api?x=1]"),
            "must list the arrived wire path: {message}"
        );
    }

    /// The arrivals-lane receive-timeout redacts secret-marked query
    /// values (ADR-0051): the key set is derived from a directly
    /// constructed `ComponentMetadata` the way the CLI derives it
    /// from the booted context — no context boot needed. The secret
    /// value is masked, non-secret pairs stay visible, the secret
    /// value never prints.
    #[tokio::test]
    async fn arrivals_lane_timeout_redacts_secrets() {
        use camel_api::component_metadata::{ComponentMetadata, OptionKind, UriOption};
        let (router, authority) = partner_router("http://127.0.0.1:0/elsewhere").await;
        let mut metadata = ComponentMetadata::minimal("http");
        metadata.uri_options.push(
            UriOption::new(
                "authPassword",
                "partner authentication password",
                OptionKind::String,
            )
            .secret(),
        );
        let secret_keys: Vec<String> = metadata
            .uri_options
            .iter()
            .filter(|option| option.secret)
            .map(|option| option.name.clone())
            .collect();
        router.set_secret_query_keys(secret_keys);
        raw_request(&authority, "GET", "/login?authPassword=hunter2&x=1").await;
        let message = expired_receive_message(&router, "http://127.0.0.1:0/elsewhere").await;
        assert!(
            message.contains("authPassword=***"),
            "the secret value must be masked: {message}"
        );
        assert!(
            !message.contains("hunter2"),
            "the secret must never print: {message}"
        );
        assert!(
            message.contains("x=1"),
            "non-secret pairs must stay visible: {message}"
        );
    }

    /// A percent-encoded secret key matches its DECODED form: the raw
    /// key span stays as authored, only the value is masked.
    #[tokio::test]
    async fn encoded_secret_query_key_redacts() {
        let (router, authority) = partner_router("http://127.0.0.1:0/elsewhere").await;
        router.set_secret_query_keys(vec!["authPassword".to_string()]);
        raw_request(&authority, "GET", "/login?%61uthPassword=hunter2&x=1").await;
        let message = expired_receive_message(&router, "http://127.0.0.1:0/elsewhere").await;
        assert!(
            message.contains("%61uthPassword=***&x=1"),
            "the raw key span stays, the value masks: {message}"
        );
        assert!(
            !message.contains("hunter2"),
            "the secret must never print: {message}"
        );
        assert!(
            message.contains("x=1"),
            "non-secret pairs stay visible: {message}"
        );
    }

    /// The client-lane (`await_parked`) receive-timeout redacts the
    /// same way: a send parks a roundtrip the partner holds (a
    /// scripted delay longer than the test), the receive under an
    /// expired budget times out on the parked response, and the
    /// launched wire path renders masked.
    #[tokio::test]
    async fn await_parked_timeout_redacts_secrets() {
        let scripted = ScriptedResponse {
            path: Some("/login?authPassword=hunter2&x=1".to_string()),
            delay: Some(Duration::from_secs(30)),
            ..ScriptedResponse::default()
        };
        let partner = HttpPartner::start(vec![scripted])
            .await
            .expect("partner must bind 127.0.0.1:0");
        let authority = partner.bound_addr().to_string();
        let declared = format!("http://{authority}/login?authPassword=hunter2&x=1");
        let router = router_for(&declared, partner);
        router.set_secret_query_keys(vec!["authPassword".to_string()]);
        router
            .send(
                &declared,
                &declared,
                OutgoingMessage {
                    body: Value::Null,
                    headers: BTreeMap::new(),
                    method: "GET".to_string(),
                },
            )
            .await
            .expect("the send must dial the partner");
        // The partner records before it holds the response: wait for
        // the recording so the timeout renders settled evidence.
        for _ in 0..2000 {
            if !router.recorded_requests(&declared).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let message = expired_receive_message(&router, &declared).await;
        assert!(
            message.contains("authPassword=***"),
            "the secret value must be masked: {message}"
        );
        assert!(
            !message.contains("hunter2"),
            "the secret must never print: {message}"
        );
        assert!(
            message.contains("x=1"),
            "non-secret pairs must stay visible: {message}"
        );
    }

    /// End to end: a send targeting a declared endpoint's authored
    /// query bytes lands in the matching lane — the arrival key
    /// equals the declared endpoint's wire `path_and_query` — and the
    /// client-role receive returns the parked roundtrip.
    #[tokio::test]
    async fn query_bearing_receive_matches_end_to_end() {
        let partner = HttpPartner::start_permissive(200)
            .await
            .expect("partner must bind 127.0.0.1:0");
        let authority = partner.bound_addr().to_string();
        let declared = format!("http://{authority}/api?flag=a&x=1");
        let router = router_for(&declared, partner);
        router
            .send(
                &declared,
                &declared,
                OutgoingMessage {
                    body: Value::Null,
                    headers: BTreeMap::new(),
                    method: "GET".to_string(),
                },
            )
            .await
            .expect("the send must dial the declared endpoint");
        let message = router
            .receive(&declared, &declared, Duration::from_secs(5))
            .await
            .expect("the roundtrip must complete");
        assert_eq!(
            message.status,
            Some(200),
            "the permissive partner serves 200"
        );
        let recorded = router.recorded_requests(&declared);
        assert_eq!(recorded.len(), 1, "exactly one request crossed the wire");
        assert_eq!(
            recorded[0].path, "/api?flag=a&x=1",
            "the arrival key equals the declared wire path_and_query"
        );
        assert_eq!(recorded[0].method, "GET");
    }
}
