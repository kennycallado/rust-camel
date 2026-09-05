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
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::time::Duration;

use camel_api::Body;
use camel_api::CamelError;
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
}

/// Nothing reached the partner endpoint before the deadline
/// (`receive-timeout`, ADR-0069 §7).
///
/// Verdict class: the scenario ran and the system under test failed
/// it. A struct, not an enum: the taxonomy has one receive failure.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("nothing reached {endpoint} within {deadline:?} (waited {elapsed:?})")]
pub struct ReceiveTimeout {
    /// The endpoint URI that delivered nothing.
    pub endpoint: String,
    /// The deadline that elapsed.
    pub deadline: Duration,
    /// How long the receive actually waited before giving up.
    pub elapsed: Duration,
}

/// Why a `receive` call did not deliver a message (ADR-0069 §7).
///
/// The variants carry the failure class: [`ReceiveError::Timeout`] is
/// verdict class (the system under test delivered nothing in time);
/// [`ReceiveError::Transport`] is apparatus class (the receive failed
/// at the transport before the scenario got a meaningful answer).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ReceiveError {
    /// Nothing reached the partner endpoint before the deadline
    /// (`receive-timeout`, verdict class).
    #[error("{0}")]
    Timeout(ReceiveTimeout),
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
    /// the lane key. Adapters without a client role keep the default
    /// (a transport failure naming the gap); the http partner is one
    /// such adapter — the router's own client lane performs every
    /// http client-role send.
    fn send<'a>(
        &'a self,
        lane_key: &'a str,
        target_uri: &'a str,
        msg: OutgoingMessage,
    ) -> BoxFuture<'a, Result<(), TransportError>> {
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
}

/// Dispatches adapter calls by declared endpoint key to the
/// endpoint-keyed adapter map it wraps, and owns the shared http
/// client lane (feature `http`).
///
/// Sends split into two keys — the declared endpoint string and the
/// interpolated wire address — and http-scheme sends route through
/// the router's own [`ClientLane`](http::ClientLane) (feature
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
}

impl PartnerRouter {
    /// Builds a router over the given endpoint-keyed adapters.
    pub fn new(adapters: BTreeMap<String, Box<dyn PartnerAdapter>>) -> Self {
        Self {
            adapters,
            #[cfg(feature = "http")]
            client_lane: Arc::new(http::ClientLane::new()),
        }
    }

    /// The adapter registered under `key`, if any.
    pub fn adapter(&self, key: &str) -> Option<&dyn PartnerAdapter> {
        self.adapters.get(key).map(|boxed| boxed.as_ref())
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
    /// the http cases).
    pub async fn send(
        &self,
        declared: &str,
        interpolated: &str,
        msg: OutgoingMessage,
    ) -> Result<(), TransportError> {
        #[cfg(feature = "http")]
        if interpolated.starts_with("http://") {
            return self.send_http(declared, interpolated, msg).await;
        }
        match self.adapters.get(declared) {
            Some(adapter) => adapter.send(declared, interpolated, msg).await,
            None => Err(TransportError::Unbound {
                endpoint: declared.to_string(),
            }),
        }
    }

    /// The http-scheme send dispatch (feature `http`): every case goes
    /// through the router's own client lane.
    #[cfg(feature = "http")]
    async fn send_http(
        &self,
        declared: &str,
        interpolated: &str,
        msg: OutgoingMessage,
    ) -> Result<(), TransportError> {
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
                    .await;
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
                .await;
        }
        // (c) Neither: a plain-string reference dials its literal URI
        // with no partner involved.
        Arc::clone(&self.client_lane)
            .launch(declared, interpolated, msg)
            .await
    }

    /// Receives under the two-key contract, client-role-first: derive
    /// the lane key ([`Self::lane_key_for`], falling back to the
    /// declared string for plain strings), return a roundtrip parked
    /// by the router's own client lane when one exists, and otherwise
    /// delegate the server-role receive to the adapter registered
    /// under that key. [`TransportError::Unbound`] only when neither a
    /// parked roundtrip nor a registered adapter exists.
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
        if let Some(parked) = self.client_lane.take(&lane_key) {
            return self
                .client_lane
                .await_parked(interpolated, deadline, parked)
                .await;
        }
        match self.adapters.get(lane_key.as_str()) {
            Some(adapter) => adapter.receive(&lane_key, interpolated, deadline).await,
            None => Err(ReceiveError::Transport(TransportError::Unbound {
                endpoint: declared.to_string(),
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
    ) -> BoxFuture<'a, Result<(), TransportError>> {
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
            Ok(())
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
    ) -> BoxFuture<'a, Result<(), TransportError>> {
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
                    // reply carries no scenario meaning in v1.
                    Ok(_reply) => return Ok(()),
                    Err(e) => {
                        // The direct producer types the startup race as
                        // EndpointCreationFailed at both error sites:
                        // poll_ready ("direct endpoint '{}' not registered",
                        // camel-direct/src/lib.rs) and call ("no consumer
                        // registered for direct:{name}"). No string matching.
                        let is_startup_race = matches!(e, CamelError::EndpointCreationFailed(_));
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
