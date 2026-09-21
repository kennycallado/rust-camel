//! Partner adapters and the endpoint-keyed router (ADR-0069 §5, §7).
//!
//! A [`PartnerAdapter`] is the harness-owned far side of the wire: the
//! listener or client that observes what the system under test puts on
//! the wire. The runner talks to adapters through this trait only, so
//! the scenario vocabulary stays transport-agnostic.
//!
//! Every adapter operation is bounded: `receive` carries the action's
//! deadline, and the runner bounds `send` with a fixed timeout. No
//! adapter call hangs (ADR-0069 §7).
//!
//! [`FakeAdapter`] is the in-memory test double: it records sent
//! messages, plays a scripted incoming queue, and can fail sends and
//! receives on demand.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::RwLock;
use std::time::Duration;

use camel_api::Body;
use camel_api::Exchange;
use camel_api::Message;
use camel_api::Value;
use camel_component_api::NoOpComponentContext;
use camel_core::CamelContext;
use futures::future::BoxFuture;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::mpsc;
use tower::ServiceExt;

/// The HTTP partner adapter (feature `http`): a loopback listener
/// plus client that play both wire roles against the system under
/// test.
#[cfg(feature = "http")]
pub mod http;

/// A message the scenario sends to a partner endpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct OutgoingMessage {
    /// Message body; `Null` when the action declares none.
    pub body: Value,
    /// Message headers; empty when the action declares none.
    pub headers: BTreeMap<String, Value>,
    /// The resolved HTTP method for client-role sends (explicit from
    /// the action's `method`, or the inferred `GET`/`POST`). Validated
    /// as an HTTP token at parse time.
    pub method: String,
}

/// A message the harness received from a partner endpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct IncomingMessage {
    /// Received body.
    pub body: Value,
    /// Received headers.
    pub headers: BTreeMap<String, Value>,
    /// Transport status code when the partner protocol carries one
    /// (HTTP response status); `None` for transports without a status
    /// concept.
    pub status: Option<u16>,
    /// Request method when the partner protocol carries a request line
    /// (HTTP server role: the method of the request that reached the
    /// partner listener); `None` otherwise.
    pub method: Option<String>,
    /// Request path (with query, when present) when the partner
    /// protocol carries a request line; `None` otherwise.
    pub path: Option<String>,
    /// Monotonic wire-arrival instant — when the transport finished
    /// receiving this message — NOT when a receive action consumed it
    /// (ADR-0069 §5: the wire is the proof).
    pub arrival: std::time::Instant,
}

/// A send or receive failed at the transport layer, before any
/// assertion ran (`action-transport-failure`, ADR-0069 §7).
///
/// Apparatus class: the scenario never got a meaningful answer.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum TransportError {
    /// No adapter is registered for the endpoint URI.
    #[error("no partner adapter bound for endpoint {endpoint}")]
    Unbound {
        /// The endpoint URI the scenario referenced.
        endpoint: String,
    },
    /// The transport reported a failure.
    #[error("{message}")]
    Other {
        /// Transport-reported failure detail.
        message: String,
    },
    /// The runner's bounded send deadline elapsed.
    #[error("send did not complete within {after:?}")]
    Deadline {
        /// The bounded send deadline the call exceeded.
        after: Duration,
    },
    /// The client lane's per-key FIFO is full: the launch refused to
    /// book another in-flight response instead of silently
    /// overwriting a parked roundtrip (apparatus class, ADR-0069 §7).
    #[error(
        "client lane FIFO overflow for {lane_key}: \
         {bound} in-flight responses already parked"
    )]
    LaneFifoOverflow {
        /// The lane key whose FIFO refused the launch.
        lane_key: String,
        /// The FIFO bound the send exceeded.
        bound: usize,
    },
}

/// Nothing reached the partner endpoint before the deadline
/// (`receive-timeout`, ADR-0069 §7).
///
/// Verdict class: the scenario ran and the system under test failed
/// it. A struct, not an enum: the taxonomy has one receive failure.
#[derive(Debug, Clone, PartialEq)]
pub struct ReceiveTimeout {
    /// The endpoint URI that delivered nothing.
    pub endpoint: String,
    /// The deadline that elapsed.
    pub deadline: Duration,
    /// How long the receive actually waited before giving up.
    pub elapsed: Duration,
    /// The wire `path_and_query` strings that arrived in the matching
    /// lane group, ALREADY REDACTED for diagnostics (ADR-0051
    /// positive secret rule) by the construction site; empty when no
    /// lane recorded an arrival. Appended to [`Display`](fmt::Display)
    /// so byte divergence is diagnosed without external packet
    /// capture.
    pub lanes_recorded: Vec<String>,
}

impl fmt::Display for ReceiveTimeout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "nothing reached {} within {:?} (waited {:?})",
            self.endpoint, self.deadline, self.elapsed
        )?;
        f.write_str(&lanes_suffix(&self.lanes_recorded))
    }
}

impl std::error::Error for ReceiveTimeout {}

/// The lane-evidence suffix shared by receive-timeout diagnostics:
/// `; no arrival matched; lanes recorded: [/a, /b]`, or empty when
/// the construction site saw no lanes.
pub(crate) fn lanes_suffix(lanes_recorded: &[String]) -> String {
    if lanes_recorded.is_empty() {
        String::new()
    } else {
        format!(
            "; no arrival matched; lanes recorded: [{}]",
            lanes_recorded.join(", ")
        )
    }
}

/// The partner's arrival lane dropped arrivals while the scenario was
/// not receiving (`arrival-lane-overflow`, ADR-0069 §7).
///
/// Apparatus class: the dropped arrivals never competed for a receive
/// — the harness lost them before the system under test could fail the
/// scenario on substance.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrivalLaneOverflow {
    /// The endpoint URI whose lane dropped arrivals.
    pub endpoint: String,
    /// How many arrivals the lane dropped while full.
    pub dropped: usize,
}

impl fmt::Display for ArrivalLaneOverflow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // No drain-window claim: the dropped counter is cumulative
        // across the lane's lifetime, so "while no receive drained
        // the lane" would mislead once an intervening receive ran.
        write!(
            f,
            "arrival lane overflow: {} dropped {} arrivals",
            self.endpoint, self.dropped
        )
    }
}

impl std::error::Error for ArrivalLaneOverflow {}

/// Why a `receive` call did not deliver a message (ADR-0069 §7).
///
/// The variants carry the failure class: [`ReceiveError::Timeout`] is
/// verdict class (the system under test delivered nothing in time);
/// [`ReceiveError::Transport`] and [`ReceiveError::Overflow`] are
/// apparatus class (the receive failed at the transport, or the lane
/// dropped arrivals, before the scenario got a meaningful answer).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ReceiveError {
    /// Nothing reached the partner endpoint before the deadline
    /// (`receive-timeout`, verdict class).
    #[error("{0}")]
    Timeout(ReceiveTimeout),
    /// The partner's arrival lane dropped arrivals while the scenario
    /// was not receiving (`arrival-lane-overflow`, apparatus class).
    #[error("{0}")]
    Overflow(ArrivalLaneOverflow),
    /// The receive failed at the transport layer
    /// (`action-transport-failure`, apparatus class).
    #[error("{0}")]
    Transport(TransportError),
}

/// The harness-owned far side of the wire for one endpoint family.
///
/// Implementations must be `Send + Sync`; calls return boxed futures
/// so the trait stays object-safe behind `Box<dyn PartnerAdapter>`.
///
/// The two-key contract splits the lane from the wire: `lane_key` is
/// the registered router key whose lane (queue, parked roundtrip) the
/// call belongs to, `target_uri`/`source_uri` is the resolved address
/// the scenario referenced. Adapters that own a listener (the http
/// partner) read their arrival lane by the registered key's request
/// path; adapters that dial treat the target URI as the wire address.
pub trait PartnerAdapter: Send + Sync {
    /// Send a message to the target URI, parking any roundtrip under
    /// the lane key, and return the synchronous reply when the
    /// adapter produces one: the context-stimulus `direct:` send
    /// returns the routed exchange (`Ok(Some(..))`) so an
    /// `expectReply` assertion can read it (rc-qvz6); partner and
    /// fake adapters answer `Ok(None)`. Adapters without a client
    /// role keep the default (a transport failure naming the gap);
    /// the http partner is one such adapter — the router's own client
    /// lane performs every http client-role send.
    fn send<'a>(
        &'a self,
        lane_key: &'a str,
        target_uri: &'a str,
        msg: OutgoingMessage,
    ) -> BoxFuture<'a, Result<Option<Exchange>, TransportError>> {
        let _ = (lane_key, target_uri, msg);
        Box::pin(async {
            Err(TransportError::Other {
                message: "adapter does not implement client-role sends".to_string(),
            })
        })
    }

    /// Receive a message from the source URI before the deadline
    /// passes. Implementations must respect the deadline; they never
    /// hang past it.
    fn receive<'a>(
        &'a self,
        lane_key: &'a str,
        source_uri: &'a str,
        deadline: Duration,
    ) -> BoxFuture<'a, Result<IncomingMessage, ReceiveError>>;

    /// The host:port authority this adapter's listener bound, when it
    /// owns one (the http partner); `None` otherwise. The router uses
    /// it to resolve declared and dynamic endpoint references to real
    /// wire addresses.
    fn bound_authority(&self) -> Option<String> {
        None
    }

    /// The wire requests this adapter recorded, in arrival order —
    /// the recorded-traffic snapshot a `partner` validate asserts
    /// against (feature `http`). The default is empty: adapters
    /// without a listener record nothing.
    #[cfg(feature = "http")]
    fn recorded_requests(&self) -> Vec<http::HttpWireRequest> {
        Vec::new()
    }

    /// Stores the query keys classified secret (ADR-0051 positive
    /// secret rule) for the adapter's diagnostic redaction; the
    /// router fans the set out from the booted context's component
    /// metadata. Default noop: adapters without wire-path
    /// diagnostics render nothing to redact.
    fn set_secret_query_keys(&self, keys: &[String]) {
        let _ = keys;
    }
}

/// Dispatches adapter calls by declared endpoint key to the
/// endpoint-keyed adapter map it wraps, and owns the shared http
/// client lane (feature `http`).
///
/// Sends split into two keys — the declared endpoint string and the
/// interpolated wire address — and http-scheme sends route through
/// the router's own `ClientLane` (camel-component-http, feature
/// `http`): a declared `:0` harness key dials the partner's bound
/// address, a dynamic reference resolves by interpolated authority,
/// and a plain string dials its literal URI — no `Unbound` failure
/// for http schemes. Non-http schemes dispatch to the registered
/// adapter as before: an endpoint URI with no registered adapter
/// fails the send at the transport ([`TransportError::Unbound`]) and
/// the receive at the transport too
/// ([`ReceiveError::Transport`]) — no partner exists that could ever
/// deliver, the failure is apparatus class, and the call never hangs.
///
/// Receives are client-role-first: a roundtrip parked by the router's
/// own client lane wins over the partner adapter's server-role
/// arrivals.
pub struct PartnerRouter {
    /// Declared endpoint key to adapter.
    adapters: BTreeMap<String, Box<dyn PartnerAdapter>>,
    /// The shared http client lane (feature `http`): one parked
    /// roundtrip per lane key, filled by every http-scheme send.
    /// `Arc`-shared because a launch's spawned exchange keeps the
    /// handle alive to park its own failure.
    #[cfg(feature = "http")]
    client_lane: Arc<http::ClientLane>,
    /// The secret-marked query-key set (ADR-0051 positive secret
    /// rule), fanned out from the booted context's component
    /// metadata by [`set_secret_query_keys`](Self::set_secret_query_keys):
    /// the redaction source of every wire-path diagnostic.
    secret_query_keys: Arc<RwLock<Vec<String>>>,
}

impl PartnerRouter {
    /// Builds a router over the given endpoint-keyed adapters.
    pub fn new(adapters: BTreeMap<String, Box<dyn PartnerAdapter>>) -> Self {
        Self {
            adapters,
            #[cfg(feature = "http")]
            client_lane: Arc::new(http::ClientLane::new()),
            secret_query_keys: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Stores the secret-marked query-key set (ADR-0051) and fans it
    /// out to every redaction site: kept here for partner-validate
    /// mismatch rendering, set on the router-owned http client lane
    /// and on every registered adapter for their receive-timeout
    /// diagnostics. The set crosses the CLI→harness boundary as plain
    /// strings, derived from the booted context's component metadata.
    pub fn set_secret_query_keys(&self, keys: Vec<String>) {
        *self
            .secret_query_keys
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = keys.clone();
        #[cfg(feature = "http")]
        self.client_lane.set_secret_query_keys(&keys);
        for adapter in self.adapters.values() {
            adapter.set_secret_query_keys(&keys);
        }
    }

    /// The stored secret-marked query-key set: the redaction source
    /// of every diagnostic that quotes a declared endpoint or wire
    /// path (partner-validate mismatch rendering, `Unbound`
    /// backstops, lastReceived validation subjects).
    pub(crate) fn secret_query_keys(&self) -> Vec<String> {
        self.secret_query_keys
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// The adapter registered under `key`, if any.
    pub fn adapter(&self, key: &str) -> Option<&dyn PartnerAdapter> {
        self.adapters.get(key).map(|boxed| boxed.as_ref())
    }

    /// The wire requests the partner registered under `key` recorded,
    /// in arrival order (feature `http`); empty when no adapter is
    /// registered under `key` or the registered adapter records
    /// nothing — an unregistered key reads as an empty snapshot, so
    /// a partner validate on it fails the count, never the run.
    #[cfg(feature = "http")]
    pub fn recorded_requests(&self, key: &str) -> Vec<http::HttpWireRequest> {
        self.adapters
            .get(key)
            .map(|adapter| adapter.recorded_requests())
            .unwrap_or_default()
    }

    /// Every registered partner that owns a bound authority, as
    /// `(declared key, bound authority)` pairs.
    pub fn authorities(&self) -> Vec<(String, String)> {
        self.adapters
            .iter()
            .filter_map(|(key, adapter)| Some((key.clone(), adapter.bound_authority()?)))
            .collect()
    }

    /// The lane key a receive under `(declared, interpolated)` reads:
    /// the declared string itself when it names a registered partner
    /// key (lane reads by declared key, today's behavior); otherwise
    /// the registered key of the partner whose bound authority equals
    /// the interpolated URI's authority. `None` when neither resolves
    /// (plain strings): the caller falls back to the declared string.
    pub fn lane_key_for(&self, declared: &str, interpolated: &str) -> Option<String> {
        if self.adapters.contains_key(declared) {
            return Some(declared.to_string());
        }
        let authority = uri_authority(interpolated)?;
        self.authorities()
            .into_iter()
            .find(|(_, bound)| bound == authority)
            .map(|(key, _)| key)
    }

    /// The wire target a send under `(declared_key, interpolated_uri)`
    /// dials, when it differs from plain literal dialing.
    ///
    /// - `declared_key` names a registered partner AND carries the
    ///   unroutable port-0 authority (the harness-declared form,
    ///   ADR-0069 §8): rewrite only the authority of
    ///   `interpolated_uri` to that partner's bound authority,
    ///   preserving the interpolated path and query.
    /// - `declared_key` names no partner but the interpolated URI's
    ///   authority equals a bound partner's authority: return that
    ///   partner's authority rewrite (path preserved) — the resolved
    ///   URI for a dynamic reference.
    /// - Anything else — a routable declared key (a partner registered
    ///   under its own bound address, or under a foreign endpoint as
    ///   a client-role vehicle) or an address no partner owns — is
    ///   `None`: the caller dials the interpolated URI literally.
    pub fn wire_target(&self, declared_key: &str, interpolated_uri: &str) -> Option<String> {
        if let Some(adapter) = self.adapters.get(declared_key) {
            let bound = adapter.bound_authority()?;
            if !authority_is_port_zero(declared_key) {
                return None;
            }
            return rewrite_authority(interpolated_uri, &bound);
        }
        self.partner_by_authority(interpolated_uri)
            .and_then(|(_, bound)| rewrite_authority(interpolated_uri, &bound))
    }

    /// Sends `msg` under the two-key contract: dispatch by declared
    /// endpoint key, dial by resolved address (see the type docs for
    /// the http cases). The synchronous reply of a context-stimulus
    /// `direct:` send travels back in the `Ok` slot; every partner
    /// path answers `None`.
    pub async fn send(
        &self,
        declared: &str,
        interpolated: &str,
        msg: OutgoingMessage,
    ) -> Result<Option<Exchange>, TransportError> {
        #[cfg(feature = "http")]
        if interpolated.starts_with("http://") {
            return self.send_http(declared, interpolated, msg).await;
        }
        match self.adapters.get(declared) {
            Some(adapter) => adapter.send(declared, interpolated, msg).await,
            // Backstop (the CLI pre-validates wiring), still redacted:
            // the router holds the secret set.
            None => Err(TransportError::Unbound {
                endpoint: redact_wire_path(declared, &self.secret_query_keys()),
            }),
        }
    }

    /// The http-scheme send dispatch (feature `http`): every case goes
    /// through the router's own client lane. The lane parks the
    /// roundtrip for a later `receive`, so the send itself answers
    /// `Ok(None)` — no synchronous reply exists to hand back.
    #[cfg(feature = "http")]
    async fn send_http(
        &self,
        declared: &str,
        interpolated: &str,
        msg: OutgoingMessage,
    ) -> Result<Option<Exchange>, TransportError> {
        // (a) The declared key registers an http partner: the
        // harness-declared endpoint — dial its bound address when the
        // `:0` form resolves one, the literal URI otherwise. A
        // non-http adapter registered under an http-scheme key keeps
        // today's equality dispatch.
        if let Some(adapter) = self.adapters.get(declared) {
            if adapter.bound_authority().is_some() {
                let target = self
                    .wire_target(declared, interpolated)
                    .unwrap_or_else(|| interpolated.to_string());
                return Arc::clone(&self.client_lane)
                    .launch(declared, &target, msg)
                    .await
                    .map(|()| None);
            }
            return adapter.send(declared, interpolated, msg).await;
        }
        // (b) The declared key is not registered, but the interpolated
        // authority resolves to a partner: dial the resolved URI under
        // that partner's REGISTERED key, so `lane_key_for` finds the
        // roundtrip on receive.
        if let Some((lane_key, target)) = self
            .partner_by_authority(interpolated)
            .and_then(|(key, bound)| Some((key, rewrite_authority(interpolated, &bound)?)))
        {
            return Arc::clone(&self.client_lane)
                .launch(&lane_key, &target, msg)
                .await
                .map(|()| None);
        }
        // (c) Neither: a plain-string reference dials its literal URI
        // with no partner involved.
        Arc::clone(&self.client_lane)
            .launch(declared, interpolated, msg)
            .await
            .map(|()| None)
    }

    /// Receives under the two-key contract, client-role-first: derive
    /// the lane key ([`Self::lane_key_for`], falling back to the
    /// declared string for plain strings), return a roundtrip parked
    /// by the router's own client lane when one exists — probed under
    /// the composite of the lane key and the interpolated reference's
    /// own path (path-aware parking, bd rc-cr5yf), so a receive
    /// drains its own path's roundtrip and never another path's —
    /// and otherwise delegate the server-role receive to the adapter
    /// registered under that key. [`TransportError::Unbound`] only
    /// when neither a parked roundtrip nor a registered adapter
    /// exists.
    pub async fn receive(
        &self,
        declared: &str,
        interpolated: &str,
        deadline: Duration,
    ) -> Result<IncomingMessage, ReceiveError> {
        let lane_key = self
            .lane_key_for(declared, interpolated)
            .unwrap_or_else(|| declared.to_string());
        #[cfg(feature = "http")]
        if let Some(parked) = self.client_lane.take(&lane_key, interpolated) {
            return self
                .client_lane
                .await_parked(interpolated, deadline, parked)
                .await;
        }
        match self.adapters.get(lane_key.as_str()) {
            Some(adapter) => adapter.receive(&lane_key, interpolated, deadline).await,
            // Backstop (the CLI pre-validates wiring), still redacted:
            // the router holds the secret set.
            None => Err(ReceiveError::Transport(TransportError::Unbound {
                endpoint: redact_wire_path(declared, &self.secret_query_keys()),
            })),
        }
    }

    /// The registered partner whose bound authority equals the URI's
    /// authority, as `(registered key, bound authority)`; the
    /// post-interpolation resolution of a dynamic reference.
    fn partner_by_authority(&self, uri: &str) -> Option<(String, String)> {
        let authority = uri_authority(uri)?;
        self.authorities()
            .into_iter()
            .find(|(_, bound)| bound == authority)
    }
}

/// The authority span of an absolute URI (`scheme://authority/rest`);
/// `None` when the string carries no `://` separator. Userinfo is not
/// part of this grammar.
fn uri_authority(uri: &str) -> Option<&str> {
    let start = uri.find("://")? + 3;
    let rest = &uri[start..];
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    Some(&rest[..end])
}

/// Whether the URI's authority is the unroutable port-0 placeholder —
/// the harness-declared endpoint form (`http://127.0.0.1:0/...`,
/// ADR-0069 §8). A declared key with any other authority addresses a
/// routable endpoint and dials literally.
fn authority_is_port_zero(uri: &str) -> bool {
    let Some(authority) = uri_authority(uri) else {
        return false;
    };
    match authority.rsplit_once(':') {
        Some((_, port)) => port == "0",
        None => false,
    }
}

/// Rewrites the URI's authority, preserving scheme, path, and query.
fn rewrite_authority(uri: &str, authority: &str) -> Option<String> {
    let rest_start = uri.find("://")? + 3;
    let rest = &uri[rest_start..];
    let path_start = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let mut rewritten = String::with_capacity(uri.len());
    rewritten.push_str(&uri[..rest_start]);
    rewritten.push_str(authority);
    rewritten.push_str(&rest[path_start..]);
    Some(rewritten)
}

/// One recorded send: the endpoint the scenario addressed and the
/// message it put on the wire.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedSend {
    /// Endpoint URI the message was sent to.
    pub endpoint: String,
    /// The message as sent.
    pub message: OutgoingMessage,
}

/// A handle onto a [`FakeAdapter`]'s recorded sends, valid after the
/// adapter itself moved into the router.
#[derive(Clone)]
pub struct FakeRecorder {
    sent: Arc<Mutex<Vec<RecordedSend>>>,
}

impl FakeRecorder {
    /// A snapshot of everything the scenario sent through the fake.
    pub fn sent_messages(&self) -> Vec<RecordedSend> {
        lock_through(&self.sent).clone()
    }
}

/// Inner state shared between a `FakeAdapter` and its clones.
struct FakeInner {
    /// When set, every send fails with this reason.
    fail_send: Option<String>,
    /// When set, every receive fails at the transport with this
    /// reason.
    fail_receive: Option<String>,
    /// Recorded sends, in order, shared with `FakeRecorder` handles.
    sent: Arc<Mutex<Vec<RecordedSend>>>,
    /// The receiving half of the scripted queue; the sender half is
    /// dropped after seeding, so a drained queue reports closed and
    /// receive maps that to a timeout.
    queue_rx: AsyncMutex<mpsc::Receiver<IncomingMessage>>,
}

/// In-memory [`PartnerAdapter`] for tests: records sent messages,
/// plays a scripted incoming queue, and can fail sends on demand.
///
/// `Clone` shares state; keep a clone (or a [`FakeRecorder`]) to
/// inspect sends after moving the adapter into a [`PartnerRouter`].
#[derive(Clone)]
pub struct FakeAdapter {
    inner: Arc<FakeInner>,
}

impl FakeAdapter {
    /// A fake that plays the given messages, in order, on receive;
    /// once the queue drains, further receives time out.
    pub fn scripted(queue: Vec<IncomingMessage>) -> Self {
        let capacity = queue.len().max(1);
        let (queue_tx, queue_rx) = mpsc::channel(capacity);
        for message in queue {
            // Capacity equals the queue length, so the try-send cannot
            // hit a full buffer; a closed receiver is impossible here.
            if queue_tx.try_send(message).is_err() {
                break;
            }
        }
        drop(queue_tx);
        Self {
            inner: Arc::new(FakeInner {
                fail_send: None,
                fail_receive: None,
                sent: Arc::new(Mutex::new(Vec::new())),
                queue_rx: AsyncMutex::new(queue_rx),
            }),
        }
    }

    /// A fake whose every send fails at the transport.
    pub fn failing_send(reason: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(FakeInner {
                fail_send: Some(reason.into()),
                fail_receive: None,
                sent: Arc::new(Mutex::new(Vec::new())),
                queue_rx: AsyncMutex::new(mpsc::channel(1).1),
            }),
        }
    }

    /// A fake whose every receive fails at the transport, so a
    /// mid-scenario receive transport failure is expressible.
    pub fn failing_receive(reason: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(FakeInner {
                fail_send: None,
                fail_receive: Some(reason.into()),
                sent: Arc::new(Mutex::new(Vec::new())),
                queue_rx: AsyncMutex::new(mpsc::channel(1).1),
            }),
        }
    }

    /// A handle onto this fake's recorded sends.
    pub fn recorder(&self) -> FakeRecorder {
        FakeRecorder {
            sent: Arc::clone(&self.inner.sent),
        }
    }
}

impl PartnerAdapter for FakeAdapter {
    fn send<'a>(
        &'a self,
        lane_key: &'a str,
        _target_uri: &'a str,
        msg: OutgoingMessage,
    ) -> BoxFuture<'a, Result<Option<Exchange>, TransportError>> {
        Box::pin(async move {
            if let Some(reason) = &self.inner.fail_send {
                return Err(TransportError::Other {
                    message: reason.clone(),
                });
            }
            lock_through(&self.inner.sent).push(RecordedSend {
                endpoint: lane_key.to_string(),
                message: msg,
            });
            // The fake records sends; it produces no synchronous
            // reply for an `expectReply` assertion to read.
            Ok(None)
        })
    }

    fn receive<'a>(
        &'a self,
        _lane_key: &'a str,
        source_uri: &'a str,
        deadline: Duration,
    ) -> BoxFuture<'a, Result<IncomingMessage, ReceiveError>> {
        Box::pin(async move {
            if let Some(reason) = &self.inner.fail_receive {
                return Err(ReceiveError::Transport(TransportError::Other {
                    message: reason.clone(),
                }));
            }
            let mut queue_rx = self.inner.queue_rx.lock().await;
            let started = tokio::time::Instant::now();
            let outcome = tokio::time::timeout(deadline, queue_rx.recv()).await;
            match outcome {
                Ok(Some(message)) => Ok(message),
                Ok(None) | Err(_) => Err(ReceiveError::Timeout(ReceiveTimeout {
                    endpoint: source_uri.to_string(),
                    deadline,
                    elapsed: started.elapsed(),
                    lanes_recorded: Vec::new(),
                })),
            }
        })
    }
}

/// Locks, recovering the guard through poisoning: fake state is plain
/// data, a poisoned lock carries no invariant to protect.
fn lock_through<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Masks secret-marked query values in a recorded wire path for
/// diagnostics (ADR-0051 positive secret rule): a pair whose DECODED
/// key matches `secret_keys` case-insensitively (rc-dhkeo — an authored
/// `AuthPassword` masks against a declared `authpassword`; URI-key
/// casing carries no meaning here, unlike option matching in
/// `is_consumed_option`, which stays exact per Camel convention) keeps
/// its raw key span but has its value masked as `***`; every other pair
/// keeps its authored bytes, unknown keys included — apparatus-internal
/// diagnostics stay maximally informative. A percent-encoded secret key
/// (`%61uthPassword`) matches its decoded form. When
/// [`raw_query_pairs`](camel_component_api::raw_query_pairs) rejects
/// the query (a malformed key escape), the ENTIRE query portion is
/// masked fail-safe: the redactor never panics and never prints an
/// undecodable secret.
pub(crate) fn redact_wire_path(path_and_query: &str, secret_keys: &[String]) -> String {
    let Some(question_mark) = path_and_query.find('?') else {
        return path_and_query.to_string();
    };
    let (path, query) = path_and_query.split_at(question_mark + 1);
    if query.is_empty() {
        return path_and_query.to_string();
    }
    let rendered = match camel_component_api::raw_query_pairs(query) {
        Ok(pairs) => pairs
            .into_iter()
            .map(|(decoded_key, raw_pair)| {
                if secret_keys
                    .iter()
                    .any(|secret| secret.eq_ignore_ascii_case(&decoded_key))
                {
                    match raw_pair.split_once('=') {
                        // Mask the value; the raw key span stays as
                        // authored (an encoded secret key stays
                        // visibly encoded).
                        Some((raw_key, _)) => format!("{raw_key}=***"),
                        // A bare key carries no value to mask.
                        None => raw_pair.to_string(),
                    }
                } else {
                    raw_pair.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("&"),
        Err(_) => "***".to_string(),
    };
    format!("{path}{rendered}")
}

// ---------------------------------------------------------------------------
// Route stimulus through the booted context
// ---------------------------------------------------------------------------

/// Startup-race retry sleep for `direct:` producer delivery (the
/// camel-test / camel-run stimulus mechanism).
const STIMULUS_RETRY_SLEEP: Duration = Duration::from_millis(20);
/// Startup-race retry deadline for `direct:` producer delivery.
const STIMULUS_RETRY_DEADLINE: Duration = Duration::from_secs(1);

/// The route stimulus for a booted scenario (ADR-0069 section 5): a
/// scenario `send` addressed to a CONTEXT component endpoint
/// (`direct:`) must reach the booted system under test, not a
/// partner. This adapter delivers the message through the context's
/// own producer path — a fresh `direct:` endpoint and producer per
/// send, one `oneshot` per exchange, retrying the consumer-startup
/// race — the same mechanism `camel-test` and `camel run` use to
/// stimulate routes.
///
/// Key the router map by the exact endpoint URI the scenario's `send`
/// addresses (`direct:start`). `receive` is not a context role: it
/// fails at the transport, apparatus class.
pub struct DirectStimulus {
    /// The booted context, shared with the boot-owning caller (the
    /// caller wraps `ScenarioRun::ctx` after
    /// [`boot_scenario`](crate::boot_scenario) returns).
    ctx: Arc<AsyncMutex<CamelContext>>,
}

impl DirectStimulus {
    /// Wraps the booted context the scenario sends into.
    pub fn new(ctx: Arc<AsyncMutex<CamelContext>>) -> Self {
        Self { ctx }
    }
}

impl PartnerAdapter for DirectStimulus {
    fn send<'a>(
        &'a self,
        lane_key: &'a str,
        _target_uri: &'a str,
        msg: OutgoingMessage,
    ) -> BoxFuture<'a, Result<Option<Exchange>, TransportError>> {
        Box::pin(async move {
            let exchange = stimulus_exchange(msg);
            let transport = |detail: String| TransportError::Other { message: detail };
            let deadline = tokio::time::Instant::now() + STIMULUS_RETRY_DEADLINE;
            loop {
                let producer = {
                    let ctx = self.ctx.lock().await;
                    let producer_ctx = ctx.producer_context();
                    let component = ctx
                        .registry()
                        .get("direct")
                        .ok_or_else(|| transport("direct component not registered".to_string()))?;
                    let endpoint = component.create_endpoint(lane_key, &*ctx).map_err(|e| {
                        transport(format!("failed to create endpoint {lane_key}: {e}"))
                    })?;
                    endpoint
                        .create_producer(Arc::new(NoOpComponentContext), &producer_ctx)
                        .map_err(|e| {
                            transport(format!("failed to create producer for {lane_key}: {e}"))
                        })?
                };
                match producer.oneshot(exchange.clone()).await {
                    // The stimulus exchange completed the route; the
                    // routed exchange is the synchronous reply an
                    // `expectReply` assertion reads (rc-qvz6).
                    Ok(reply) => return Ok(Some(reply)),
                    Err(e) => {
                        // Structural startup-race classification via the
                        // shared seda predicate (rc-utx98):
                        // EndpointCreationFailed minus the no-active-consumers
                        // gate, which fails fast (rc-tgaxf). The full
                        // rationale lives on the predicate's rustdoc.
                        let is_startup_race = camel_component_seda::is_direct_startup_race(&e);
                        if is_startup_race && tokio::time::Instant::now() < deadline {
                            tokio::time::sleep(STIMULUS_RETRY_SLEEP).await;
                            continue;
                        }
                        return Err(transport(format!("send to {lane_key} failed: {e}")));
                    }
                }
            }
        })
    }

    fn receive<'a>(
        &'a self,
        _lane_key: &'a str,
        source_uri: &'a str,
        _deadline: Duration,
    ) -> BoxFuture<'a, Result<IncomingMessage, ReceiveError>> {
        Box::pin(async move {
            Err(ReceiveError::Transport(TransportError::Other {
                message: format!(
                    "{source_uri} is a context stimulus endpoint; receive is a partner role"
                ),
            }))
        })
    }
}

/// Builds the stimulus exchange: strings pass through as text bodies,
/// `Null` is empty, structured values travel as JSON; headers carry
/// over verbatim.
fn stimulus_exchange(msg: OutgoingMessage) -> Exchange {
    let body = match &msg.body {
        Value::Null => Body::Empty,
        Value::String(text) => Body::Text(text.clone()),
        other => Body::Json(other.clone()),
    };
    let mut message = Message::new(body);
    for (name, value) in &msg.headers {
        message.set_header(name.clone(), value.clone());
    }
    Exchange::new(message)
}
