//! RabbitMQ consumer: explicit readiness, delivery mapping, and a bounded
//! stop path.
//!
//! Readiness and the minimal (payload-only) body mapping are task 2.1; the
//! ack-after-route / failure-disposition policy is task 2.2; header/property
//! mapping is task 2.4; prefetch/concurrency is task 2.3. The delivery engine
//! is generic over its stream so tests drive it with a fake `InboundDelivery`
//! source.
//!
//! # At-least-once
//!
//! A delivery is acked only after the route completes; a route failure nacks
//! (requeue per the endpoint's `requeueOnFailure` option, default reject to
//! the DLX). On stop, a delivery blocked on the route is dropped *without* a
//! disposition, so the broker redelivers it — the component is at-least-once
//! and consumers tolerate duplicates.
//!
//! # Reconnect safety
//!
//! Each engine captures the connection generation of the channel it consumes
//! on. A disposition is applied only while the manager's current generation
//! still matches; a delivery whose connection has been replaced (broker
//! restart, connection death) is dropped without an ack/nack, so the broker
//! redelivers it after reconnect (at-least-once).
//!
//! When a delivery stream ends the engine inspects the status of the origin
//! connection its channel was opened on. A *channel-local* termination on a
//! still-healthy connection (`basic.cancel` after a queue deletion, or a soft
//! channel close) re-opens a subscription on the SAME manager
//! connection/generation — the manager is never demoted, so sibling engines on
//! that connection keep their generation and their acks stay valid. Only an
//! actually dead origin is reported to the manager, which fences on the
//! generation (a manager that already reconnected is not torn down again) and
//! lets the retry policy own reconnection. A re-open failure on a healthy
//! connection (e.g. a 404 queue not found) is retried under a bounded
//! cancel-aware backoff and never invalidates the shared connection. The retry
//! policy, not lapin auto-recovery, owns reconnection.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use camel_api::{Body, Message};
use camel_component_api::{
    CamelError, Consumer, ConsumerContext, ConsumerStartupMode, Exchange, RuntimeObservability,
};
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use lapin::options::{BasicAckOptions, BasicConsumeOptions, BasicNackOptions, BasicQosOptions};
use lapin::types::{FieldTable, ShortString};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::RabbitEndpointConfig;
use crate::connection::RabbitConnectionManager;
use crate::error::RabbitError;
use crate::topology::topology_check;

mod reply;
use reply::{maybe_reply, send_reply_on_managed_channel};

/// How long `start()` waits for the broker connection under the retry policy.
///
/// Unlike the producer's bounded 2 s publish wait
/// ([`crate::connection::PUBLISH_DISCONNECTED_BOUND`]), a consumer route MUST
/// stay not-ready while the initial connect retries: `start()` blocks here and
/// only calls `mark_ready()` once the connection, channel, and broker-side
/// `basic.consume` have all succeeded. The wait is delegated to the connection
/// manager's spawned reconnect worker, so a runtime shutdown token cancels it.
/// The cap sits below camel-core's 90 s `CONSUMER_STARTUP_BUDGET`, so a broker
/// that never appears surfaces this precise component error first.
pub(crate) const CONSUMER_START_BOUND: Duration = Duration::from_secs(60);

/// Stop join budget, matching the MQTT/Kafka consumers: in-flight route
/// futures get this long to drain before the engine task is aborted.
pub(crate) const CONSUMER_STOP_BOUND: Duration = Duration::from_secs(10);

/// Per-iteration bound for the engine's cancel-aware wait for a reconnected
/// broker.
///
/// The reconnect policy owns the retry cadence; this only caps how long one
/// `connection_within` wait blocks before the engine re-checks cancellation
/// and re-arms a settled manager. It is short enough that `stop()` joins
/// promptly (its own aggregate bound is `CONSUMER_STOP_BOUND`).
const RECONNECT_WAIT_BOUND: Duration = Duration::from_secs(2);

/// Backoff after a reconnect-wait iteration that settled without a live
/// connection.
///
/// With a disabled or exhausted retry policy `connection_within` can return
/// almost immediately; this keeps the engine from spinning on a broker that is
/// not coming back, while the policy stays the only reconnect driver.
const RECONNECT_RETRY_BACKOFF: Duration = Duration::from_millis(100);

/// One inbound delivery decoupled from lapin, so the consume engine is
/// unit-testable over a fake stream.
///
/// The header/property mapping is task 2.4's [`crate::headers::inbound`];
/// `acker` carries the task 2.2 disposition.
pub(crate) struct InboundDelivery {
    pub payload: Vec<u8>,
    pub props: lapin::BasicProperties,
    pub headers: FieldTable,
    pub redelivered: bool,
    pub acker: Box<dyn DeliveryAcker>,
}

/// A boxed delivery stream owned by one engine connection.
///
/// The real lapin consumer is not `Unpin`; boxing keeps the reconnect loop's
/// per-connection stream uniform while [`run_loop`] stays generic over any
/// stream for unit tests.
pub(crate) type DeliveryStream =
    std::pin::Pin<Box<dyn futures::Stream<Item = InboundDelivery> + Send>>;

/// Ack/nack capability carried by each [`InboundDelivery`].
///
/// The production implementation wraps `lapin::Acker`; unit-test fakes
/// implement the same trait. Task 2.2's [`apply_disposition`] is the first
/// policy caller.
#[async_trait]
pub(crate) trait DeliveryAcker: Send + Sync {
    async fn ack(&self) -> Result<(), CamelError>;
    async fn nack(&self, requeue: bool) -> Result<(), CamelError>;
}

#[async_trait]
impl DeliveryAcker for lapin::Acker {
    async fn ack(&self) -> Result<(), CamelError> {
        self.ack(BasicAckOptions::default())
            .await
            .map(|_| ())
            .map_err(|error| {
                RabbitError::PublishFailed(format!("rabbitmq delivery ack failed: {error}")).into()
            })
    }

    async fn nack(&self, requeue: bool) -> Result<(), CamelError> {
        self.nack(BasicNackOptions {
            requeue,
            ..BasicNackOptions::default()
        })
        .await
        .map(|_| ())
        .map_err(|error| {
            RabbitError::PublishFailed(format!("rabbitmq delivery nack failed: {error}")).into()
        })
    }
}

/// Ack/nack decision for one delivery, derived from the route outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Disposition {
    /// Route completed: positively acknowledge the delivery.
    Ack,
    /// Route failed: negatively acknowledge; `requeue` decides redelivery.
    Nack { requeue: bool },
}

/// Outcome of one consume, mapped onto the component-operations metric.
///
/// A `Success`/`Failure` is only ever claimed once the broker disposition for
/// that outcome was actually written (see [`DispositionApplied`]); a stale tag
/// or a failed ack/nack claims neither.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConsumeOutcome {
    /// The route completed and the delivery was acked.
    Success,
    /// The route failed and the delivery was nacked.
    Failure,
}

/// Emit one `camel_component_operations_total{component="rabbitmq",
/// operation="consume"}` observation through `rt` (the producer 1.4 emission
/// shape, shared through [`RuntimeObservability`]).
pub(crate) fn record_consume(rt: &dyn RuntimeObservability, outcome: ConsumeOutcome) {
    rt.component_metrics()
        .observe("rabbitmq", "consume", outcome == ConsumeOutcome::Failure);
}

/// Why one connection's delivery loop ended.
///
/// The reconnect wrapper ([`run_engine`]) uses this to decide whether to
/// release the channel, terminate, or take a fresh channel and re-consume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EngineExit {
    /// Stop cancelled the engine. The in-flight delivery is abandoned with no
    /// disposition; the channel close is owned by `stop()`.
    Cancelled,
    /// The delivery stream ended (channel/broker death). Reconnect.
    StreamEnded,
    /// Route transport/lifecycle loss (`ChannelClosed`): abandon the delivery
    /// with no disposition, close this engine's own channel, terminate.
    RouteTransportLoss,
}

/// Map a route outcome to a disposition (the kafka/mqtt contract).
///
/// `Ok` → [`Disposition::Ack`]; `Err` → [`Disposition::Nack`] whose `requeue`
/// comes from the endpoint's `requeueOnFailure` option (default `false`, i.e.
/// reject to a dead-letter exchange).
///
/// Generic over the success payload so the engine can classify the actual
/// `Result<Exchange, CamelError>` route outcome (task 4.3 preserves the
/// exchange for reply publishing) without cloning the error or collapsing it to
/// `()`.
pub(crate) fn disposition<T>(
    route_result: &Result<T, CamelError>,
    requeue_on_failure: bool,
) -> Disposition {
    match route_result {
        Ok(_) => Disposition::Ack,
        Err(_) => Disposition::Nack {
            requeue: requeue_on_failure,
        },
    }
}

/// Whether [`apply_disposition`] actually wrote a disposition to the broker.
///
/// Distinguishes a real ack/nack from a dropped stale tag, so the caller can
/// count only real outcomes (no ghost success/failure for a redelivered tag).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DispositionApplied {
    /// The ack/nack was written to the broker.
    Applied,
    /// The delivery tag predates a reconnect: no disposition was sent and the
    /// broker redelivers the message (at-least-once).
    Stale,
}

/// Apply `decision` to `acker`, unless the delivery tag is stale.
///
/// `generation_matches == false` means the delivery predates a reconnect: the
/// broker redelivers it, so no ack/nack is sent, no error is returned, and
/// [`DispositionApplied::Stale`] tells the caller no outcome may be counted.
pub(crate) async fn apply_disposition(
    decision: Disposition,
    generation_matches: bool,
    acker: &dyn DeliveryAcker,
) -> Result<DispositionApplied, CamelError> {
    if !generation_matches {
        return Ok(DispositionApplied::Stale);
    }
    match decision {
        Disposition::Ack => acker.ack().await.map(|()| DispositionApplied::Applied),
        Disposition::Nack { requeue } => acker
            .nack(requeue)
            .await
            .map(|()| DispositionApplied::Applied),
    }
}

/// Convert one lapin delivery into the engine's decoupled form.
fn to_inbound(delivery: lapin::message::Delivery) -> InboundDelivery {
    let props = delivery.properties;
    let headers = props.headers().clone().unwrap_or_default();
    InboundDelivery {
        payload: delivery.data,
        props,
        headers,
        redelivered: delivery.redelivered,
        acker: Box::new(delivery.acker),
    }
}

/// Build the routed [`Exchange`] from one delivery: payload bytes plus the
/// full basic-property/free-form header mapping (task 2.4).
fn build_exchange(delivery: InboundDelivery) -> (Exchange, Box<dyn DeliveryAcker>) {
    let InboundDelivery {
        payload,
        props,
        headers,
        redelivered,
        acker,
    } = delivery;
    let body = if payload.is_empty() {
        Body::Empty
    } else {
        Body::Bytes(bytes::Bytes::from(payload))
    };
    let mut message = Message::new(body);
    message.headers = crate::headers::inbound(&headers, &props, redelivered);
    (Exchange::new(message), acker)
}

/// The per-connection consume engine: dispatch each delivery into the route,
/// then apply the ack/nack disposition after the route completes.
///
/// Generic over the delivery stream so tests drive it with a fake source. The
/// `cancel` token is the consumer-local child of the runtime token; it breaks
/// both the delivery wait and an in-flight route wait, so `stop()` can join
/// promptly even when the route is blocked. A delivery abandoned by stop is
/// dropped *without* a disposition, so the broker redelivers it.
///
/// `requeue_on_failure` is the endpoint's `requeueOnFailure` option.
///
/// `generation` is the connection generation of the channel this stream was
/// registered on. A disposition is applied only while `manager`'s current
/// generation still matches: after a reconnect the pre-reconnect tag is stale
/// and dropped, so the broker redelivers the message (at-least-once).
///
/// The return value tells [`run_engine`] why the loop ended. Channel cleanup is
/// owned by the caller, never here.
///
/// `rt` receives one `operation="consume"` component observation per delivery
/// whose disposition was actually applied (route outcome, post-ack/nack). A
/// stale tag, a `ChannelClosed` transport loss, and a failed ack/nack emit no
/// outcome — they are not business route results. A failed reply publish for an
/// otherwise successful route also does not change the route outcome (G-10):
/// the delivery is disposed normally, and the infra failure records the
/// ADR-0012 b-prime signal `b-prime:rabbitmq:reply-publish` instead.
pub(crate) async fn run_loop<S>(
    stream: S,
    ctx: ConsumerContext,
    requeue_on_failure: bool,
    generation: u64,
    manager: Arc<RabbitConnectionManager>,
    rt: Arc<dyn RuntimeObservability>,
    cancel: CancellationToken,
) -> EngineExit
where
    S: futures::Stream<Item = InboundDelivery>,
{
    // The delivery stream is pinned locally so the engine accepts any stream
    // (the real lapin consumer is not `Unpin`) without imposing a bound on
    // callers.
    let mut stream = std::pin::pin!(stream);
    loop {
        let delivery = tokio::select! {
            biased;
            _ = cancel.cancelled() => return EngineExit::Cancelled,
            next = stream.next() => match next {
                Some(delivery) => delivery,
                None => return EngineExit::StreamEnded,
            },
        };

        let (exchange, acker) = build_exchange(delivery);
        // Snapshot the request's own replyTo/correlationId BEFORE the route
        // runs: a route must never be able to redirect a reply (or its
        // correlation id) to another request's direct reply-to channel.
        let original_headers = exchange.input.headers.clone();
        // The route future is polled BEFORE the cancel token: a result that
        // completed just before stop must still be acknowledged (drain
        // in-flight route futures). A route still blocked on the pipeline
        // falls through to the cancel arm and is abandoned, so the broker
        // redelivers it (at-least-once).
        let route_result = tokio::select! {
            biased;
            result = ctx.send_and_wait(exchange) => result,
            _ = cancel.cancelled() => return EngineExit::Cancelled,
        };

        // Route transport/lifecycle loss: the route receiver or the reply
        // oneshot is gone (`ChannelClosed`). This is NOT a business route
        // failure — acking would misreport success and nacking (requeue=false)
        // would drop the message. Abandon it with no disposition and leave the
        // loop; the caller closes this consumer's channel so the broker
        // requeues the still-unacked delivery (at-least-once).
        if matches!(&route_result, Err(CamelError::ChannelClosed)) {
            return EngineExit::RouteTransportLoss;
        }

        // Task 4.3: for a successful route whose ORIGINAL request carried both
        // a replyTo and a correlationId, publish the OUT body/headers to the
        // replyTo BEFORE the ack. The reply is sent at most once per delivery;
        // a business route failure (or missing headers) sends none and falls
        // through to the normal disposition below.
        if let Some(intent) = maybe_reply(&route_result, &original_headers) {
            // The send is bounded by the reply module AND cancel-aware: a stop
            // during the publish abandons it (the channel closes on drop) so the
            // engine still joins promptly.
            let reply = send_reply_on_managed_channel(&intent, &manager);
            let reply_result = tokio::select! {
                biased;
                _ = cancel.cancelled() => return EngineExit::Cancelled,
                result = reply => result,
            };
            if let Err(error) = reply_result {
                // G-10: a reply publish failure is an infrastructure side
                // effect, NOT a business route failure. Emit the ADR-0012
                // b-prime signal and warn, then fall through: the route's own
                // disposition is UNCHANGED (a successful route is still acked
                // below). A failed reply never resets the healthy shared
                // connection; if the connection is actually dead the ack below
                // fails on its own and the existing stale-generation drop lets
                // the broker redeliver.
                rt.metrics()
                    .increment_errors(ctx.route_id(), "b-prime:rabbitmq:reply-publish");
                tracing::warn!(error = %error, "rabbitmq consumer reply publish failed");
            }
        }

        // kafka/mqtt contract: ack only after the route completes.
        //
        // Stale-tag guard: a delivery whose connection has been replaced must
        // not be acked on the new channel. Dropping it lets the broker
        // redeliver (at-least-once).
        let current_generation = manager.current_generation();
        let generation_matches = current_generation == generation;
        if !generation_matches {
            tracing::debug!(
                delivery_generation = generation,
                current_generation,
                "rabbitmq dropping stale delivery tag after reconnect"
            );
        }
        let decision = disposition(&route_result, requeue_on_failure);
        match apply_disposition(decision, generation_matches, acker.as_ref()).await {
            Ok(DispositionApplied::Applied) => {
                // Count the outcome only after the disposition was actually
                // written: a route that *would* have succeeded but whose ack
                // failed must not be reported as a success.
                let outcome = if route_result.is_ok() {
                    ConsumeOutcome::Success
                } else {
                    ConsumeOutcome::Failure
                };
                record_consume(&*rt, outcome);
            }
            // Stale tag: no disposition was sent, so claim no outcome here.
            // The broker redelivers the delivery (at-least-once).
            Ok(DispositionApplied::Stale) => {}
            // A failed disposition leaves the delivery unacked (the broker
            // redelivers it) and is an infrastructure event, not a business
            // route failure: log it and claim neither outcome. Never abort the
            // engine on one ack/nack failure.
            Err(error) => {
                tracing::warn!(error = %error, "rabbitmq delivery disposition failed");
            }
        }
    }
}

/// Build a delivery stream from a broker-side consumer.
///
/// A stream error means the connection/channel is gone: log it and end the
/// stream so [`run_engine`] reconnects.
fn consume_stream(consumer: lapin::Consumer) -> DeliveryStream {
    Box::pin(consumer.filter_map(|result| async move {
        match result {
            Ok(delivery) => Some(to_inbound(delivery)),
            Err(error) => {
                tracing::warn!(error = %error, "rabbitmq delivery stream error");
                None
            }
        }
    }))
}

/// Best-effort close of one engine-owned channel under the stop bound.
async fn close_engine_channel(channel: &lapin::Channel) {
    let _ = tokio::time::timeout(
        CONSUMER_STOP_BOUND,
        channel.close(200, ShortString::from("rabbitmq consumer engine exit")),
    )
    .await;
}

/// Wait, cancel-aware, until the manager has a live connection again.
///
/// The retry policy owns the reconnect cadence; this loops
/// [`RabbitConnectionManager::connection_within`] (which re-arms a settled
/// manager) so an outage longer than one bound still recovers. Returns `false`
/// when the engine is cancelled first.
async fn wait_connected(
    manager: &Arc<RabbitConnectionManager>,
    cancel: &CancellationToken,
) -> bool {
    loop {
        let wait = manager.connection_within(RECONNECT_WAIT_BOUND);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return false,
            result = wait => {
                if result.is_ok() {
                    return true;
                }
            }
        }
        // `connection_within` already re-armed via `ensure_connecting`; the
        // backoff only matters when the policy settles almost immediately
        // (disabled/exhausted), keeping the engine from spinning.
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return false,
            _ = tokio::time::sleep(RECONNECT_RETRY_BACKOFF) => {}
        }
    }
}

/// Cancel-aware backoff after a channel re-open failure.
///
/// A soft re-open failure (for example a 404 queue not found) on a still-healthy
/// connection must not spin the engine: this bounds the retry cadence while the
/// engine waits for the topology to reappear. Returns `false` when the engine is
/// cancelled first.
async fn reopen_backoff(cancel: &CancellationToken) -> bool {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(RECONNECT_RETRY_BACKOFF) => true,
    }
}

/// Set QoS and register the consumer on `channel`, returning the delivery stream.
///
/// The channel's origin connection is owned by the caller, which inspects its
/// status if registration fails: a channel-local failure on a still-connected
/// origin (e.g. a 404 queue not found) must not demote the healthy shared
/// connection.
async fn register_consumer(
    channel: &lapin::Channel,
    queue: &str,
    prefetch: u16,
) -> Result<DeliveryStream, CamelError> {
    channel
        .basic_qos(prefetch, BasicQosOptions::default())
        .await
        .map_err(|error| {
            RabbitError::PublishFailed(format!("rabbitmq consumer basic_qos failed: {error}"))
        })?;
    let consumer = channel
        .basic_consume(
            ShortString::from(queue),
            ShortString::default(),
            BasicConsumeOptions::default(),
            FieldTable::default(),
        )
        .await
        .map_err(|error| {
            RabbitError::PublishFailed(format!(
                "rabbitmq consumer basic_consume on queue '{queue}' failed: {error}"
            ))
        })?;
    Ok(consume_stream(consumer))
}

/// The reconnect wrapper around [`run_loop`].
///
/// Owns the engine's current channel. When a delivery stream ends the engine
/// inspects the status of the ORIGIN connection its channel was opened on (never
/// a later connection the manager may have published in the meantime):
///
/// - A still-healthy origin means a channel-local termination (`basic.cancel`
///   from a deleted queue, or a soft channel close): the engine re-opens a
///   subscription on the SAME manager connection/generation without demoting it,
///   so sibling consumers on that connection keep their generation and their
///   acks stay valid. A re-open failure on a healthy connection (for example a
///   404 queue that has not been recreated yet) is retried with a bounded
///   cancel-aware backoff and never invalidates the shared connection.
/// - A dead origin is reported with the engine's captured generation; the
///   manager fences on it (a manager that already reconnected is never torn
///   down) and [`NetworkRetryPolicy`] owns the reconnect — lapin auto-recovery
///   stays off.
///
/// A route transport loss closes only this engine's channel and terminates; a
/// stop returns without touching the channel that `stop()` owns.
pub(crate) async fn run_engine(
    mut stream: DeliveryStream,
    first: (lapin::Channel, u64, Arc<lapin::Connection>),
    manager: Arc<RabbitConnectionManager>,
    runtime: Arc<dyn RuntimeObservability>,
    config: RabbitEndpointConfig,
    ctx: ConsumerContext,
    cancel: CancellationToken,
) {
    let queue = config.queue.clone().unwrap_or_default();
    let prefetch = config.prefetch;
    let requeue_on_failure = config.requeue_on_failure;
    let (mut channel, mut generation, mut origin) = first;
    // The first channel is also owned by `RabbitConsumer` (stop closes it); a
    // re-opened channel is engine-owned and released here on exit.
    let mut channel_owned_by_stop = true;

    loop {
        let exit = run_loop(
            stream,
            ctx.clone(),
            requeue_on_failure,
            generation,
            Arc::clone(&manager),
            Arc::clone(&runtime),
            cancel.clone(),
        )
        .await;

        match exit {
            EngineExit::Cancelled => {
                if !channel_owned_by_stop {
                    close_engine_channel(&channel).await;
                }
                return;
            }
            EngineExit::RouteTransportLoss => {
                // The shared connection stays healthy: release only this
                // engine's channel and terminate without a disposition.
                close_engine_channel(&channel).await;
                return;
            }
            EngineExit::StreamEnded => {
                // Technical marker (no queue name): the delivery stream ended.
                // Tests gate on this to synchronize a deliberately triggered
                // `basic.cancel` before probing sibling consumption.
                tracing::debug!("RabbitMQ consumer stream ended");

                // Release the superseded channel before re-opening so a local
                // cancellation does not leak its registration. Best-effort: on
                // connection death the channel is already gone. Sibling
                // channels and the shared connection are never touched.
                close_engine_channel(&channel).await;

                // Classify against the ORIGIN connection this engine was
                // consuming on. A healthy origin is a channel-local
                // termination; only a dead origin is reported to the manager,
                // which fences on the generation. No caller-side
                // check-then-act: the fence lives inside the manager.
                if !origin.status().connected() {
                    manager.note_failure_for_generation(generation);
                }

                // Wait for a live connection, then open a fresh channel. Retry
                // until a channel is up or the engine is cancelled. A re-open
                // failure is only propagated to the manager when the origin it
                // rode is actually dead; a soft failure on a healthy shared
                // connection (404 queue not found) is retried under the bounded
                // backoff and never invalidates that connection.
                let (new_channel, new_generation, new_stream, new_origin) = loop {
                    if !wait_connected(&manager, &cancel).await {
                        return;
                    }
                    let (channel, generation, conn) = match manager.consumer_channel().await {
                        Ok(triple) => triple,
                        Err(error) => {
                            // `consumer_channel` already reports the failure
                            // against the CAPTURED origin connection (dead-only)
                            // inside the manager; the engine must not re-label
                            // it with the current generation after the await.
                            // Back off and re-wait for a live connection.
                            tracing::warn!(
                                error = %error,
                                "rabbitmq consumer channel re-open failed"
                            );
                            if !reopen_backoff(&cancel).await {
                                return;
                            }
                            continue;
                        }
                    };
                    match register_consumer(&channel, &queue, prefetch).await {
                        Ok(new_stream) => break (channel, generation, new_stream, conn),
                        Err(error) => {
                            tracing::warn!(
                                error = %error,
                                "rabbitmq consumer channel re-open failed"
                            );
                            close_engine_channel(&channel).await;
                            // A soft channel-local failure — e.g. 404 queue not
                            // found — must not invalidate a healthy shared
                            // connection; only a dead origin is reported.
                            if !conn.status().connected() {
                                manager.note_failure_for_generation(generation);
                            }
                            if !reopen_backoff(&cancel).await {
                                return;
                            }
                        }
                    }
                };
                channel = new_channel;
                generation = new_generation;
                origin = new_origin;
                stream = new_stream;
                channel_owned_by_stop = false;
            }
        }
    }
}

/// RabbitMQ consumer implementing the component-api [`Consumer`] contract.
///
/// `startup_mode()` is [`ConsumerStartupMode::Explicit`]: the route stays
/// not-ready until `start()` has connected, created every engine's channel,
/// and completed each broker-side `basic.consume`. A pre-consume failure calls
/// `ctx.mark_failed` so the runtime surfaces a precise startup error instead of
/// hanging (task 3.3/3.4 reuse this for topology failures).
pub struct RabbitConsumer {
    config: RabbitEndpointConfig,
    manager: Arc<RabbitConnectionManager>,
    runtime: Arc<dyn RuntimeObservability>,
    cancel_token: Option<CancellationToken>,
    /// One cancel child per engine, cancelled on stop.
    engine_cancels: Vec<CancellationToken>,
    /// Consumer-owned channel per engine. Closed on stop so only this
    /// consumer's subscriptions go away — the shared connection keeps serving
    /// producers. Each engine holds a second clone so an unexpected exit can
    /// release its own channel.
    channels: Vec<lapin::Channel>,
    engine_handles: Vec<JoinHandle<()>>,
}

impl RabbitConsumer {
    /// Build a consumer over a resolved endpoint config and its broker manager.
    pub fn new(
        config: RabbitEndpointConfig,
        manager: Arc<RabbitConnectionManager>,
        runtime: Arc<dyn RuntimeObservability>,
    ) -> Self {
        Self {
            config,
            manager,
            runtime,
            cancel_token: None,
            engine_cancels: Vec::new(),
            channels: Vec::new(),
            engine_handles: Vec::new(),
        }
    }
}

/// Close every channel created during a failed startup under ONE aggregate
/// bound — never a per-channel timeout multiplied across engines. Best-effort:
/// the broker also reaps the subscriptions when the channels drop, but closing
/// explicitly cancels each partial `basic.consume` immediately.
///
/// The shared connection is never touched.
async fn close_owned_channels(channels: Vec<lapin::Channel>) {
    let close_all = async {
        for channel in &channels {
            let _ = channel
                .close(200, ShortString::from("rabbitmq consumer channel cleanup"))
                .await;
        }
    };
    let _ = tokio::time::timeout(CONSUMER_STOP_BOUND, close_all).await;
}

/// Join every engine task under ONE aggregate stop bound.
///
/// The whole set of engines shares `CONSUMER_STOP_BOUND` (not once per engine),
/// so `concurrentConsumers` cannot multiply the stop budget. A completed engine
/// is removed from the outstanding set as soon as it finishes, so a later
/// timeout only aborts and drains the engines still running — a `JoinHandle`
/// that already resolved is never polled again (that panics). On timeout every
/// still-outstanding handle is aborted and drained so no engine task outlives
/// stop. A completed-just-before-stop route still drains its result: the
/// engine's own biased select polls the route future before the cancel token.
async fn join_engines(handles: Vec<JoinHandle<()>>) -> Result<(), CamelError> {
    let mut pending: FuturesUnordered<JoinHandle<()>> = handles.into_iter().collect();
    let mut first_error: Option<CamelError> = None;

    let timed_out = tokio::time::timeout(CONSUMER_STOP_BOUND, async {
        while let Some(result) = pending.next().await {
            match result {
                Ok(()) => {}
                Err(error) if error.is_cancelled() => {}
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(
                            RabbitError::PublishFailed(format!(
                                "rabbitmq consumer engine panicked: {error}"
                            ))
                            .into(),
                        );
                    }
                }
            }
        }
    })
    .await
    .is_err();

    if timed_out {
        // Abort and drain ONLY the outstanding engines. Completed engines were
        // already removed by the loop above, so no handle is re-polled.
        for handle in pending.iter() {
            handle.abort();
        }
        while pending.next().await.is_some() {}
        return Err(RabbitError::PublishFailed(format!(
            "rabbitmq consumer engines did not stop within {CONSUMER_STOP_BOUND:?}; aborted"
        ))
        .into());
    }

    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[async_trait]
impl Consumer for RabbitConsumer {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        if self.cancel_token.is_some() {
            return Err(CamelError::EndpointCreationFailed(
                "rabbitmq consumer already started".to_string(),
            ));
        }

        let queue = self.config.queue.clone().ok_or_else(|| {
            CamelError::Config("rabbitmq consumer requires the 'queue' parameter".to_string())
        })?;

        let cancel = context.cancel_token();
        // Consumer-local child tokens. Cancelling them stops this consumer's
        // engines without touching sibling producers on the shared connection.
        // Start state is NOT stored until every engine is up: a failed start
        // leaves no active state, so a restart is not falsely rejected by the
        // already-started guard.

        // Bounded connect first: fail fast with a precise error before any
        // engine channel is created.
        if let Err(error) = self.manager.connection_within(CONSUMER_START_BOUND).await {
            context.mark_failed(error.to_string());
            return Err(error);
        }

        // Topology check (tasks 3.3/3.4): ONE probe on a dedicated short-lived
        // channel, BEFORE any consume channel is created, so a missing queue or
        // a conflicting active declare fails fast and never registers a
        // subscription. `autoDeclare=false` (default) passively checks the
        // queue; `autoDeclare=true` actively declares exchange/queue/binding.
        // The start error names the queue (or the failing declare target); the
        // probe soft error is channel-local, so the shared connection stays
        // healthy for sibling consumers/producers.
        if let Err(error) = topology_check(&self.manager, &self.config).await {
            context.mark_failed(error.to_string());
            return Err(error);
        }

        let engine_count = self.config.concurrent_consumers as usize;
        let mut channels: Vec<lapin::Channel> = Vec::with_capacity(engine_count);
        let mut generations: Vec<u64> = Vec::with_capacity(engine_count);
        let mut origins: Vec<Arc<lapin::Connection>> = Vec::with_capacity(engine_count);
        let mut consumers: Vec<lapin::Consumer> = Vec::with_capacity(engine_count);

        // Set up every engine before spawning any. A failure at engine N closes
        // the N-1 channels already opened (aggregate bound) and leaves the
        // consumer restartable.
        for _ in 0..engine_count {
            let (channel, generation, origin) = match self.manager.consumer_channel().await {
                // The generation is the stale-tag seam: each engine compares
                // it against `manager.current_generation()` at disposition
                // time and drops a pre-reconnect tag. The origin connection is
                // captured under the same read lock so the engine can later
                // classify a stream end against the connection it actually
                // consumed on.
                Ok(triple) => triple,
                Err(error) => {
                    close_owned_channels(std::mem::take(&mut channels)).await;
                    context.mark_failed(error.to_string());
                    return Err(error);
                }
            };

            // QoS must be set BEFORE the consumer registers: it bounds the
            // in-flight (unacked) deliveries the broker pushes to this channel.
            if let Err(error) = channel
                .basic_qos(self.config.prefetch, BasicQosOptions::default())
                .await
            {
                channels.push(channel);
                let error: CamelError = RabbitError::PublishFailed(format!(
                    "rabbitmq consumer basic_qos failed: {error}"
                ))
                .into();
                close_owned_channels(std::mem::take(&mut channels)).await;
                context.mark_failed(error.to_string());
                return Err(error);
            }

            match channel
                .basic_consume(
                    ShortString::from(queue.as_str()),
                    ShortString::default(),
                    BasicConsumeOptions::default(),
                    FieldTable::default(),
                )
                .await
            {
                Ok(consumer) => {
                    channels.push(channel);
                    generations.push(generation);
                    origins.push(origin);
                    consumers.push(consumer);
                }
                Err(error) => {
                    channels.push(channel);
                    let error: CamelError = RabbitError::PublishFailed(format!(
                        "rabbitmq consumer basic_consume on queue '{queue}' failed: {error}"
                    ))
                    .into();
                    close_owned_channels(std::mem::take(&mut channels)).await;
                    context.mark_failed(error.to_string());
                    return Err(error);
                }
            }
        }

        // Every engine is registered: only now is the consumer (and thus the
        // route) ready. No separate readiness state exists — the Explicit
        // startup protocol IS the gate, count-gated across all engines.
        context.mark_ready();
        self.cancel_token = Some(cancel.clone());

        for (((channel, generation), origin), consumer) in channels
            .into_iter()
            .zip(generations)
            .zip(origins)
            .zip(consumers)
        {
            let engine_cancel = cancel.child_token();
            self.engine_cancels.push(engine_cancel.clone());

            // The engine owns a second handle to its channel so an unexpected
            // engine exit can release it (requeueing unacked deliveries);
            // stop() keeps its own handle for the normal close. A reconnected
            // channel is engine-owned and released by `run_engine`.
            let engine_channel = channel.clone();
            self.channels.push(channel);
            self.engine_handles.push(tokio::spawn(run_engine(
                consume_stream(consumer),
                (engine_channel, generation, origin),
                Arc::clone(&self.manager),
                Arc::clone(&self.runtime),
                self.config.clone(),
                context.clone(),
                engine_cancel,
            )));
        }
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        for engine_cancel in self.engine_cancels.drain(..) {
            engine_cancel.cancel();
        }

        // Join every engine under a single aggregate bound BEFORE closing the
        // channels. A route that completed just before stop still has its
        // result; each engine drains it (its ack/nack is written) and only then
        // observes the cancel. Closing first would race that disposition and
        // silently requeue an acked message. A route still blocked on the
        // pipeline is abandoned by the cancel, so the close below requeues it
        // (at-least-once).
        let result = join_engines(std::mem::take(&mut self.engine_handles)).await;

        // Close only this consumer's own channels; never the shared connection,
        // so producers on the same broker are unaffected.
        close_owned_channels(std::mem::take(&mut self.channels)).await;

        self.cancel_token = None;
        result
    }

    fn startup_mode(&self) -> ConsumerStartupMode {
        ConsumerStartupMode::Explicit
    }
}

#[cfg(test)]
mod tests;
