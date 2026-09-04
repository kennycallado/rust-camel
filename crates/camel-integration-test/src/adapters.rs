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

use crate::document::EndpointRef;

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
pub trait PartnerAdapter: Send + Sync {
    /// Send a message to the target endpoint.
    fn send<'a>(
        &'a self,
        target: &'a EndpointRef,
        msg: OutgoingMessage,
    ) -> BoxFuture<'a, Result<(), TransportError>>;

    /// Receive a message from the source endpoint before the deadline
    /// passes. Implementations must respect the deadline; they never
    /// hang past it.
    fn receive<'a>(
        &'a self,
        source: &'a EndpointRef,
        deadline: Duration,
    ) -> BoxFuture<'a, Result<IncomingMessage, ReceiveError>>;
}

/// Dispatches adapter calls by [`EndpointRef`] endpoint equality to
/// the endpoint-keyed adapter map it wraps.
///
/// An endpoint URI with no registered adapter fails the send at the
/// transport ([`TransportError::Unbound`]) and fails the receive at
/// the transport too ([`ReceiveError::Transport`]): no partner
/// exists that could ever deliver, the failure is apparatus class,
/// and the call never hangs.
pub struct PartnerRouter {
    /// Endpoint URI to adapter.
    adapters: BTreeMap<String, Box<dyn PartnerAdapter>>,
}

impl PartnerRouter {
    /// Builds a router over the given endpoint-keyed adapters.
    pub fn new(adapters: BTreeMap<String, Box<dyn PartnerAdapter>>) -> Self {
        Self { adapters }
    }
}

impl PartnerAdapter for PartnerRouter {
    fn send<'a>(
        &'a self,
        target: &'a EndpointRef,
        msg: OutgoingMessage,
    ) -> BoxFuture<'a, Result<(), TransportError>> {
        Box::pin(async move {
            match self.adapters.get(&target.endpoint) {
                Some(adapter) => adapter.send(target, msg).await,
                None => Err(TransportError::Unbound {
                    endpoint: target.endpoint.clone(),
                }),
            }
        })
    }

    fn receive<'a>(
        &'a self,
        source: &'a EndpointRef,
        deadline: Duration,
    ) -> BoxFuture<'a, Result<IncomingMessage, ReceiveError>> {
        Box::pin(async move {
            match self.adapters.get(&source.endpoint) {
                Some(adapter) => adapter.receive(source, deadline).await,
                None => Err(ReceiveError::Transport(TransportError::Unbound {
                    endpoint: source.endpoint.clone(),
                })),
            }
        })
    }
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
        target: &'a EndpointRef,
        msg: OutgoingMessage,
    ) -> BoxFuture<'a, Result<(), TransportError>> {
        Box::pin(async move {
            if let Some(reason) = &self.inner.fail_send {
                return Err(TransportError::Other {
                    message: reason.clone(),
                });
            }
            lock_through(&self.inner.sent).push(RecordedSend {
                endpoint: target.endpoint.clone(),
                message: msg,
            });
            Ok(())
        })
    }

    fn receive<'a>(
        &'a self,
        source: &'a EndpointRef,
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
                    endpoint: source.endpoint.clone(),
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
        target: &'a EndpointRef,
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
                    let endpoint = component
                        .create_endpoint(target.endpoint.as_str(), &*ctx)
                        .map_err(|e| {
                            transport(format!(
                                "failed to create endpoint {}: {e}",
                                target.endpoint
                            ))
                        })?;
                    endpoint
                        .create_producer(Arc::new(NoOpComponentContext), &producer_ctx)
                        .map_err(|e| {
                            transport(format!(
                                "failed to create producer for {}: {e}",
                                target.endpoint
                            ))
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
                        return Err(transport(format!(
                            "send to {} failed: {e}",
                            target.endpoint
                        )));
                    }
                }
            }
        })
    }

    fn receive<'a>(
        &'a self,
        source: &'a EndpointRef,
        _deadline: Duration,
    ) -> BoxFuture<'a, Result<IncomingMessage, ReceiveError>> {
        Box::pin(async move {
            Err(ReceiveError::Transport(TransportError::Other {
                message: format!(
                    "{} is a context stimulus endpoint; receive is a partner role",
                    source.endpoint
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
