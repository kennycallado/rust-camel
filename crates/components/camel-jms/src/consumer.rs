use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use camel_component_api::{
    Body, CamelError, ConcurrencyModel, Consumer, ConsumerContext, ConsumerStartupMode, Exchange,
    Message, NetworkRetryPolicy, RuntimeObservability,
};
use futures::stream::{FuturesUnordered, StreamExt as _};
use tokio::task::{AbortHandle, JoinHandle};
use tokio_util::sync::CancellationToken;
use tonic::transport::Channel;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::component::{
    BRIDGE_TRANSPORT_ERROR_PREFIX, BridgeState, JmsBridgePool, OPERATOR_REPAIR_HINT,
    bridge_service_client, is_bridge_transport_error, parked_operator_error,
};
use crate::config::{DestinationType, ExchangePattern, JmsEndpointConfig, JmsTransactionMode};
use crate::diagnostics::render_failure_message;
use crate::headers::apply_jms_headers;
use crate::proto::{CompletionOutcome, CompletionRequest, JmsMessage, SubscribeRequest};

pub struct JmsConsumer {
    pool: Arc<JmsBridgePool>,
    broker_name: String,
    endpoint_config: JmsEndpointConfig,
    reconnect: NetworkRetryPolicy,
    /// Requested-shutdown token, cancelled by `stop()`. It is a child of the
    /// route cancel token, so a context cancellation also marks a requested
    /// shutdown, while a failure-driven sibling cancel never does.
    cancel_token: Option<CancellationToken>,
    /// Shared sibling token: every concurrent consumer task observes it, and a
    /// task failure cancels it so its siblings stop cooperatively (task 4.5).
    /// It is a child of the requested-shutdown token.
    sibling_token: Option<CancellationToken>,
    /// The requested-shutdown token the supervisor classifies exits against. A
    /// requested shutdown (context cancel or explicit `stop()`) is normal; an
    /// abort or error outside a requested shutdown is a failure (task 4.5).
    requested_token: Option<CancellationToken>,
    /// Component-owned abort authority over the worker tasks. Retained after
    /// the supervised handle is handed to the runtime so `stop()` can always
    /// clean up, even if the runtime aborts the supervisor (task 4.5).
    worker_aborts: Vec<AbortHandle>,
    task_handles: Vec<JoinHandle<Result<(), CamelError>>>,
    /// Used for `metrics().increment_errors(…)` at consumer-send failure
    /// sites (`b-prime:jms:consumer-send`).
    runtime: Arc<dyn RuntimeObservability>,
}

impl JmsConsumer {
    pub fn new(
        pool: Arc<JmsBridgePool>,
        broker_name: String,
        endpoint_config: JmsEndpointConfig,
        reconnect: NetworkRetryPolicy,
        runtime: Arc<dyn RuntimeObservability>,
    ) -> Self {
        Self {
            pool,
            broker_name,
            endpoint_config,
            reconnect,
            cancel_token: None,
            sibling_token: None,
            requested_token: None,
            worker_aborts: Vec::new(),
            task_handles: Vec::new(),
            runtime,
        }
    }
}

fn build_exchange(msg: &JmsMessage, map_jms_headers: bool) -> Exchange {
    let body_bytes = msg.body.clone();
    let body = if msg.content_type.starts_with("text/") {
        match String::from_utf8(body_bytes.clone()) {
            Ok(s) => Body::Text(s),
            Err(_) => Body::Bytes(bytes::Bytes::from(body_bytes)),
        }
    } else if msg.content_type.contains("json") {
        match serde_json::from_slice::<serde_json::Value>(&body_bytes) {
            Ok(v) => Body::Json(v),
            Err(_) => Body::Bytes(bytes::Bytes::from(body_bytes)),
        }
    } else if body_bytes.is_empty() {
        Body::Empty
    } else {
        Body::Bytes(bytes::Bytes::from(body_bytes))
    };

    let mut exchange = Exchange::new(Message::new(body));
    // Task 3.1: the bridge reservation count travels as an exchange property,
    // never as a header, so the outbound header path stays unchanged.
    exchange.set_property("CamelJmsReservationCount", msg.reservation_count);
    // JMS-018: gate header mapping behind config flag
    if map_jms_headers {
        apply_jms_headers(&mut exchange, msg);
    }
    exchange
}

fn destination(endpoint_config: &JmsEndpointConfig) -> String {
    format!(
        "{}:{}",
        match endpoint_config.destination_type {
            DestinationType::Queue => "queue",
            DestinationType::Topic => "topic",
        },
        endpoint_config.destination_name
    )
}

/// Outcome of the shared consumer refresh action (task 2.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryOutcome {
    /// The refresh was attempted; callers keep their existing retry flow.
    Continue,
    /// Cancellation won the refresh select; callers return `Ok(())`.
    Cancelled,
}

/// Shared consumer refresh/cancel/wake action for both retry sites.
///
/// Cancellation is raced against the real `refresh_slot_channel` call. A
/// failed refresh (including the fixed two-second expiration) logs the
/// credential-safe warning and requests one coalesced liveness probe by
/// waking the monitor; it never restarts or degrades the slot. The caller
/// keeps every counter increment, threshold check, and reset at its original
/// site.
async fn refresh_channel_or_request_probe(
    pool: &JmsBridgePool,
    broker_name: &str,
    destination: &str,
    idx: usize,
    cancel: &CancellationToken,
    ctx: &ConsumerContext,
) -> RecoveryOutcome {
    let refresh = tokio::select! {
        _ = cancel.cancelled() => return RecoveryOutcome::Cancelled,
        _ = ctx.cancelled() => return RecoveryOutcome::Cancelled,
        result = pool.refresh_slot_channel(broker_name) => result,
    };
    if let Err(refresh_err) = refresh {
        warn!(
            broker = %broker_name,
            destination = %destination,
            consumer_idx = idx,
            error = %refresh_err,
            "JMS channel refresh failed; requesting liveness probe"
        );
        pool.restart_slot(broker_name);
    }
    RecoveryOutcome::Continue
}

/// Classifies a gRPC status as a permanent, route-only failure.
///
/// The gRPC code is lost once a `tonic::Status` is converted into a
/// `CamelError`, so this classifier must run on the raw status BEFORE
/// conversion. Only `FailedPrecondition` is permanent: it is the bridge's
/// signal for a corrupt journal, a busy lock, a replay or capacity failure, or
/// an unkeyable `JMSMessageID`. Every transport code (for example
/// `Unavailable`) stays transient and keeps the bounded reconnect behavior.
pub(crate) fn is_permanent_status(status: &tonic::Status) -> bool {
    status.code() == tonic::Code::FailedPrecondition
}

/// Builds the route-only permanent error for a gRPC `FAILED_PRECONDITION`
/// returned by the bridge (a lock, replay, or capacity failure, or an
/// unkeyable `JMSMessageID`) (tasks 4.4/4.7).
///
/// The permanent/transient decision is made on the raw [`tonic::Status`] code
/// BEFORE this conversion (see [`is_permanent_status`]), so the classification
/// survives. Only the stable gRPC code, the broker name, and the destination
/// label are carried — all safe, local context. The remote status message is
/// deliberately dropped: it is remote-controlled and may embed arbitrary
/// payload or header bytes, which must never appear in an operator-facing
/// error. The shared operator action is appended so every permanent error
/// names the repair.
fn permanent_destination_error(
    broker_name: &str,
    operation: &str,
    destination: &str,
    status: &tonic::Status,
) -> CamelError {
    CamelError::ProcessorError(format!(
        "JMS broker '{broker_name}' {operation} permanent failure for destination \
         '{destination}' (gRPC {:?}): {OPERATOR_REPAIR_HINT}",
        status.code()
    ))
}

/// Builds the broker-connection failure detail for the consumer error path
/// (task 8.1).
///
/// Reads the last classified broker failure from the broker handle — the same
/// [`crate::component::BridgeSlot`] the consumer already reaches for broker
/// identity. When the bridge reported a class other than UNSPECIFIED, the
/// detail is the LOCAL rendering of that class ([`render_failure_message`])
/// instead of any remote message, preserving the never-echo-remote-text policy
/// above. When no class is on record, the caller's `fallback` is returned
/// unchanged: retry classification keys on the caller's error prefix and is
/// unaffected by which detail the message carries.
pub(crate) fn connection_failure_detail(
    pool: &JmsBridgePool,
    broker_name: &str,
    fallback: &str,
) -> String {
    let classified = pool.slots.get(broker_name).and_then(|slot| {
        let last = slot
            .last_broker_failure
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        last.clone()
    });
    match classified {
        Some(failure) if failure.failure_class != 0 => render_failure_message(
            failure.failure_class,
            broker_name,
            &failure.expected_identity,
            &failure.connected_host,
        ),
        _ => fallback.to_string(),
    }
}

/// Typed outcome of a failed [`await_ready_channel`], preserving the
/// permanent/transient distinction BEFORE any `CamelError` erasure.
///
/// A `CamelError` cannot carry this split (its message is opaque), so the
/// decision is structural rather than a classifier: terminal slot states build
/// `Terminal`, every recoverable waiter error builds `Transient`.
/// `consumer_loop` returns a `Terminal` error immediately instead of spending
/// the (possibly unlimited) reconnect budget on a state no retry can recover;
/// a `Transient` error keeps the bounded retry and exhaustion semantics.
enum WaitReadyError {
    /// A `Parked` or `Stopped` slot can never become `Ready`: fail the route
    /// now. The payload is the operator-facing error.
    Terminal(CamelError),
    /// A recoverable waiter error: retry under the reconnect policy.
    Transient(CamelError),
}

impl std::fmt::Display for WaitReadyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Terminal(e) | Self::Transient(e) => write!(f, "{e}"),
        }
    }
}

/// True when the endpoint configures a non-blank message selector (task 1.2).
///
/// A blank/whitespace-only value is rejected at URI parsing, so this is a
/// defensive second look at the stored decoded selector: it arms the shared
/// capability requirement and the consumer-attach gate.
fn selector_required(endpoint_config: &JmsEndpointConfig) -> bool {
    endpoint_config
        .message_selector
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty())
}

async fn await_ready_channel(
    pool: &JmsBridgePool,
    broker_name: &str,
    selector_required: bool,
) -> Result<Channel, WaitReadyError> {
    let slot = pool
        .get_or_create_slot(broker_name, selector_required)
        .await
        .map_err(WaitReadyError::Transient)?;
    let mut rx = slot.state_rx.clone();

    loop {
        // Clone the state out of the watch borrow so no `watch::Ref` guard
        // spans the attach-gate await below (the future must stay `Send`).
        let state = rx.borrow().clone();
        match state {
            BridgeState::Ready { channel } => {
                // Task 1.2: consumer-attach selector gate. The shared
                // requirement is already latched (get_or_create_slot above),
                // and the gate issues ONE fresh Health RPC before the
                // consumer's first Subscribe — at slot start or late. A
                // failure parks the slot permanently and fails this consumer
                // terminally (no silent start).
                if selector_required {
                    pool.selector_attach_gate(&slot, &channel)
                        .await
                        .map_err(WaitReadyError::Terminal)?;
                }
                return Ok(channel);
            }
            BridgeState::Parked(reason) => {
                // Fail closed: a parked slot is terminal; waiting can never
                // succeed until an operator repairs it and restarts the
                // context (tasks 4.3/4.7). Mark it terminal so the caller does
                // not spend an unlimited reconnect budget on it (Finding 1).
                return Err(WaitReadyError::Terminal(parked_operator_error(
                    broker_name,
                    &reason,
                )));
            }
            BridgeState::Stopped => {
                // A stopped slot is terminal too: `publish` never overwrites it
                // (see `BridgeSlot`), so no retry can recover.
                return Err(WaitReadyError::Terminal(CamelError::ProcessorError(
                    format!("JMS broker '{}' is stopped", broker_name),
                )));
            }
            _ => {}
        }

        if rx.changed().await.is_err() {
            // The slot was dropped or replaced; `get_or_create_slot` can hand
            // back a fresh slot on the next attempt, so stay retryable.
            return Err(WaitReadyError::Transient(CamelError::ProcessorError(
                format!("JMS broker '{}' state channel closed", broker_name),
            )));
        }
    }
}

/// Grace period for siblings to stop cooperatively after one task fails,
/// before the stragglers are aborted so the group never waits forever. A
/// cooperative worker observes the shared token and exits immediately; this
/// bound only matters for a worker stuck in an uninterruptible wait.
const SIBLING_DRAIN_TIMEOUT: Duration = Duration::from_secs(3);

/// Aborts the supervised workers if the supervisor itself is dropped. When the
/// runtime aborts the background-task handle, the supervisor future is dropped
/// and would otherwise detach its workers by dropping their `JoinHandle`s;
/// this guard keeps them component-owned.
struct AbortWorkersOnDrop(Vec<AbortHandle>);

impl Drop for AbortWorkersOnDrop {
    fn drop(&mut self) {
        for abort in &self.0 {
            abort.abort();
        }
    }
}

/// Classifies one supervised task exit against the requested-shutdown token.
///
/// Returns the failure to preserve, or `None` for a normal exit. Classification
/// happens BEFORE the shared sibling token is cancelled, so a requested
/// shutdown is never mistaken for a failure and an unexpected abort is never
/// mistaken for a requested shutdown.
fn classify_supervised_exit(
    joined: Result<Result<(), CamelError>, tokio::task::JoinError>,
    requested: &CancellationToken,
) -> Option<CamelError> {
    match joined {
        Ok(Ok(())) => None,
        // A requested cancellation is normal; an error outside a requested
        // shutdown is a route failure.
        Ok(Err(e)) => (!requested.is_cancelled()).then_some(e),
        Err(join_err) => {
            if requested.is_cancelled() {
                // The runtime aborted the task as part of a requested
                // shutdown: normal.
                None
            } else if join_err.is_panic() {
                Some(CamelError::ProcessorError(format!(
                    "JMS consumer task panicked: {join_err}"
                )))
            } else {
                // Aborted without a requested cancellation: a failure, never a
                // silent normal exit.
                Some(CamelError::ProcessorError(
                    "JMS consumer task aborted unexpectedly".to_string(),
                ))
            }
        }
    }
}

/// Supervises every concurrent consumer task as one route-level unit.
///
/// The group is raced through `FuturesUnordered` so a failure is observed as
/// soon as any task exits. On the first failure the shared sibling token is
/// cancelled (cooperative shutdown), the failure is preserved, and the
/// remaining siblings get a bounded drain before being aborted. A requested
/// cancellation is not a failure: the token is cancelled from outside, every
/// task exits `Ok`, and the group resolves to `Ok(())`. Panics and unexpected
/// aborts surface as `Err`, never swallowed.
fn spawn_supervisor(
    handles: Vec<JoinHandle<Result<(), CamelError>>>,
    shared: CancellationToken,
    requested: CancellationToken,
) -> JoinHandle<Result<(), CamelError>> {
    let aborts: Vec<AbortHandle> = handles.iter().map(JoinHandle::abort_handle).collect();
    // The guard is built BEFORE the spawn so it is captured by the task future.
    // If the runtime aborts the supervisor before its first poll, the captured
    // guard is still dropped, aborting the workers instead of detaching them.
    let workers_guard = AbortWorkersOnDrop(aborts);
    tokio::spawn(async move {
        let mut inflight: FuturesUnordered<JoinHandle<Result<(), CamelError>>> =
            handles.into_iter().collect();
        let mut first_err: Option<CamelError> = None;

        // Phase 1: observe exits. Long-running consumers must not be killed
        // for being idle, so no timeout applies until a failure is seen.
        while let Some(joined) = inflight.next().await {
            if let Some(e) = classify_supervised_exit(joined, &requested) {
                // Classify first, then cancel siblings.
                shared.cancel();
                first_err = Some(e);
                break;
            }
        }

        // Phase 2: after a failure, drain siblings within a bound; abort any
        // straggler so a stuck sibling cannot suppress the failure forever.
        if first_err.is_some() {
            let drained = tokio::time::timeout(SIBLING_DRAIN_TIMEOUT, async {
                while inflight.next().await.is_some() {}
            })
            .await
            .is_ok();
            if !drained {
                for abort in &workers_guard.0 {
                    abort.abort();
                }
            }
        }

        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    })
}

/// Background consumer loop, extracted so it can be spawned multiple times
/// for concurrent consumers (JMS-011).
///
/// Returns `Ok(())` only on a requested cancellation. A permanent destination
/// failure or reconnect exhaustion returns `Err` so the supervised handle
/// fails the route (ADR-0007, task 4.5).
#[allow(clippy::too_many_arguments)]
async fn consumer_loop(
    pool: &JmsBridgePool,
    broker_name: &str,
    endpoint_config: &JmsEndpointConfig,
    reconnect: &NetworkRetryPolicy,
    cancel: CancellationToken,
    ctx: &ConsumerContext,
    idx: u32,
    runtime: Arc<dyn RuntimeObservability>,
) -> Result<(), CamelError> {
    let destination = destination(endpoint_config);
    let map_headers = endpoint_config.map_jms_headers;
    let mut consecutive_transport_failures: u32 = 0;
    let mut attempt: u32 = 0;

    // Manual retry loops (not retry_async / retry_async_cancelable) — three
    // retry sites nested inside this outer loop:
    // - Retries are embedded in `tokio::select!` with cancellation tokens
    //   (cancel.cancelled(), ctx.cancelled()); retry_async_cancelable
    //   honours a single CancellationToken but does not interleave
    //   cancellation checks between every operation step.
    // - Inter-attempt side effects involve async lifecycle orchestration:
    //   transport failure counting, pool.restart_slot() for channel
    //   refresh, attempt counter shared across channel-await, subscribe,
    //   and stream-error retry sites. These are async operations, not
    //   just mutable borrows. The HRTB variant (bd rc-cvq) would only
    //   solve &mut state, not async side-effect orchestration.
    loop {
        let channel = tokio::select! {
            _ = cancel.cancelled() => {
                info!(
                    broker = %broker_name,
                    destination = %destination,
                    consumer_idx = idx,
                    "JMS consumer cancelled"
                );
                break;
            }
            _ = ctx.cancelled() => {
                info!(
                    broker = %broker_name,
                    destination = %destination,
                    consumer_idx = idx,
                    "JMS consumer context cancelled"
                );
                break;
            }
            result = await_ready_channel(pool, broker_name, selector_required(endpoint_config)) => {
                match result {
                    Ok(channel) => channel,
                    Err(WaitReadyError::Terminal(e)) => {
                        // A terminal slot state (Parked/Stopped) can never
                        // become Ready: fail the route now with the operator
                        // guidance instead of retrying `await_ready_channel`
                        // forever under an unlimited reconnect budget
                        // (Finding 1).
                        warn!(
                            broker = %broker_name,
                            destination = %destination,
                            consumer_idx = idx,
                            error = %e,
                            "JMS consumer readiness permanently failed; terminating without retry"
                        );
                        return Err(e);
                    }
                    Err(WaitReadyError::Transient(e)) => {
                        warn!(
                            broker = %broker_name,
                            destination = %destination,
                            consumer_idx = idx,
                            error = %e,
                            "JMS consumer waiting for ready bridge failed"
                        );
                        attempt += 1;
                        if !reconnect.should_retry(attempt) {
                            warn!(
                                broker = %broker_name,
                                destination = %destination,
                                consumer_idx = idx,
                                attempts = attempt,
                                "JMS consumer max reconnect attempts reached; terminating"
                            );
                            return Err(CamelError::ProcessorError(format!(
                                "JMS consumer reconnect attempts exhausted waiting for a ready \
                                 bridge after {} attempts",
                                attempt
                            )));
                        }
                        let delay = reconnect.delay_for(attempt - 1);
                        tokio::select! {
                            _ = cancel.cancelled() => break,
                            _ = ctx.cancelled() => break,
                            _ = tokio::time::sleep(delay) => {}
                        }
                        continue;
                    }
                }
            }
        };

        // The subscription id is retained for the lifetime of the stream so a
        // terminal `Complete` can name the exact subscription it settles.
        let subscription_id = Uuid::new_v4().to_string();
        let mut client = bridge_service_client(channel);
        // The subscribe RPC is raced against cancellation so a requested
        // shutdown is never blocked behind an unresponsive bridge (task 4.5).
        let mut stream = match tokio::select! {
            _ = cancel.cancelled() => {
                info!(
                    broker = %broker_name,
                    destination = %destination,
                    consumer_idx = idx,
                    "JMS consumer cancelled before subscribe"
                );
                return Ok(());
            }
            _ = ctx.cancelled() => {
                info!(
                    broker = %broker_name,
                    destination = %destination,
                    consumer_idx = idx,
                    "JMS consumer context cancelled before subscribe"
                );
                return Ok(());
            }
            result = client.subscribe(SubscribeRequest {
                destination: destination.clone(),
                subscription_id: subscription_id.clone(),
                message_selector: endpoint_config.message_selector.clone().unwrap_or_default(),
            }) => result,
        } {
            Ok(resp) => {
                consecutive_transport_failures = 0;
                attempt = 0;
                info!(
                    broker = %broker_name,
                    destination = %destination,
                    consumer_idx = idx,
                    "JMS consumer subscribed successfully"
                );
                resp.into_inner()
            }
            Err(status) => {
                if is_permanent_status(&status) {
                    // A subscribe-time permanent failure (lock, replay,
                    // capacity, or an invalid journal configuration) fails only
                    // this route: terminate without reconnecting and never park
                    // or restart the broker slot, which stays Ready for other
                    // routes and producers (task 4.4). The route failure is
                    // surfaced through the supervised handle by task 4.5.
                    warn!(
                        broker = %broker_name,
                        destination = %destination,
                        consumer_idx = idx,
                        operation = "subscribe",
                        grpc_code = ?status.code(),
                        "JMS subscribe permanent failure; failing the route without restarting the broker slot"
                    );
                    // A destination permanent failure is a route-only Err:
                    // the supervised handle fails the route (task 4.5) while
                    // the broker slot stays Ready for other routes. The error
                    // names the operator action and drops the remote status
                    // detail so no payload/header bytes leak (task 4.7).
                    return Err(permanent_destination_error(
                        broker_name,
                        "subscribe",
                        &destination,
                        &status,
                    ));
                }
                // Task 8.1: when the broker handle carries a classified
                // connection failure, the detail is the local rendering (never
                // the remote status text). The transport prefix is preserved,
                // so retry classification is unchanged.
                let e = CamelError::ProcessorError(format!(
                    "{BRIDGE_TRANSPORT_ERROR_PREFIX}subscribe error: {}",
                    connection_failure_detail(pool, broker_name, &status.to_string())
                ));
                if is_bridge_transport_error(&e) {
                    consecutive_transport_failures += 1;
                    if consecutive_transport_failures >= 2 {
                        warn!(
                            broker = %broker_name,
                            destination = %destination,
                            consumer_idx = idx,
                            failures = consecutive_transport_failures,
                            "JMS subscribe transport failures exceeded threshold; refreshing channel"
                        );
                        // Shared with the stream-error site: race the channel
                        // refresh against cancellation so a requested shutdown
                        // is never blocked (task 4.5), then wake the monitor
                        // only when the refresh failed (task 2.5).
                        match refresh_channel_or_request_probe(
                            pool,
                            broker_name,
                            &destination,
                            idx as usize,
                            &cancel,
                            ctx,
                        )
                        .await
                        {
                            RecoveryOutcome::Cancelled => return Ok(()),
                            RecoveryOutcome::Continue => {}
                        }
                        consecutive_transport_failures = 0;
                    }
                } else {
                    consecutive_transport_failures = 0;
                }
                warn!(
                    broker = %broker_name,
                    destination = %destination,
                    consumer_idx = idx,
                    error = %e,
                    "JMS subscribe failed; retrying"
                );
                attempt += 1;
                if !reconnect.should_retry(attempt) {
                    warn!(
                        broker = %broker_name,
                        destination = %destination,
                        consumer_idx = idx,
                        attempts = attempt,
                        "JMS consumer max subscribe attempts reached; terminating"
                    );
                    return Err(CamelError::ProcessorError(format!(
                        "JMS consumer subscribe attempts exhausted after {} attempts",
                        attempt
                    )));
                }
                let delay = reconnect.delay_for(attempt - 1);
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = ctx.cancelled() => break,
                    _ = tokio::time::sleep(delay) => {}
                }
                continue;
            }
        };

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!(
                        broker = %broker_name,
                        destination = %destination,
                        consumer_idx = idx,
                        "JMS consumer cancelled"
                    );
                    return Ok(());
                }
                _ = ctx.cancelled() => {
                    info!(
                        broker = %broker_name,
                        destination = %destination,
                        consumer_idx = idx,
                        "JMS consumer context cancelled"
                    );
                    return Ok(());
                }
                msg = stream.message() => {
                    match msg {
                        Ok(Some(jms_msg)) => {
                            consecutive_transport_failures = 0;
                            attempt = 0;
                            let delivery_token = jms_msg.delivery_token.clone();
                            let subscription_generation = jms_msg.subscription_generation;
                            let exchange = build_exchange(&jms_msg, map_headers);

                            // Wait for the pipeline result, racing shutdown.
                            // Cancellation before the owner thread's terminal
                            // decision sends no completion: the owner closes
                            // without acknowledging and the broker redelivers.
                            let pipeline_result = tokio::select! {
                                _ = cancel.cancelled() => {
                                    info!(
                                        broker = %broker_name,
                                        destination = %destination,
                                        consumer_idx = idx,
                                        "JMS consumer cancelled before settlement"
                                    );
                                    return Ok(());
                                }
                                _ = ctx.cancelled() => {
                                    info!(
                                        broker = %broker_name,
                                        destination = %destination,
                                        consumer_idx = idx,
                                        "JMS consumer context cancelled before settlement"
                                    );
                                    return Ok(());
                                }
                                result = ctx.send_and_wait(exchange) => result,
                            };

                            // `Ok` (Completed, Stopped, or a handled error)
                            // commits; any `Err`, including `ChannelClosed`,
                            // rolls back or drops the stream and never commits.
                            let outcome = match &pipeline_result {
                                Ok(_) => CompletionOutcome::Commit,
                                Err(e) => {
                                    runtime.metrics().increment_errors(
                                        ctx.route_id(),
                                        "b-prime:jms:consumer-send",
                                    );
                                    // log-policy: outside-contract
                                    error!(
                                        broker = %broker_name,
                                        consumer_idx = idx,
                                        "JMS consumer route error: {e}"
                                    );
                                    CompletionOutcome::Rollback
                                }
                            };
                            let pipeline_channel_closed =
                                matches!(&pipeline_result, Err(CamelError::ChannelClosed));

                            // Hand the one terminal decision to the owner thread,
                            // racing shutdown. The completion is never retried:
                            // an uncertain response drops the stream, and a
                            // completion already accepted after the decision
                            // stands (no speculative second acknowledgement).
                            let completion = tokio::select! {
                                _ = cancel.cancelled() => {
                                    info!(
                                        broker = %broker_name,
                                        destination = %destination,
                                        consumer_idx = idx,
                                        "JMS consumer cancelled before completion"
                                    );
                                    return Ok(());
                                }
                                _ = ctx.cancelled() => {
                                    info!(
                                        broker = %broker_name,
                                        destination = %destination,
                                        consumer_idx = idx,
                                        "JMS consumer context cancelled before completion"
                                    );
                                    return Ok(());
                                }
                                result = client.complete(CompletionRequest {
                                    subscription_id: subscription_id.clone(),
                                    subscription_generation,
                                    delivery_token,
                                    outcome: outcome as i32,
                                }) => result,
                            };

                            let accepted = match completion {
                                Ok(response) => response.into_inner().accepted,
                                Err(e) => {
                                    // A completion RPC failure is uncertain:
                                    // drop the stream and never retry against
                                    // the stale token.
                                    warn!(
                                        broker = %broker_name,
                                        destination = %destination,
                                        consumer_idx = idx,
                                        error = %e,
                                        "JMS completion RPC failed; dropping stream"
                                    );
                                    false
                                }
                            };
                            if !accepted {
                                warn!(
                                    broker = %broker_name,
                                    destination = %destination,
                                    consumer_idx = idx,
                                    "JMS completion not accepted; dropping stream"
                                );
                                break;
                            }
                            if pipeline_channel_closed {
                                // The pipeline channel is dead: submitting
                                // another delivery is futile, so drop the
                                // stream. The rollback above lets the broker
                                // redeliver the reservation.
                                info!(
                                    broker = %broker_name,
                                    destination = %destination,
                                    consumer_idx = idx,
                                    "JMS pipeline channel closed; dropping stream"
                                );
                                break;
                            }
                        }
                        Ok(None) => {
                            info!(
                                broker = %broker_name,
                                destination = %destination,
                                consumer_idx = idx,
                                "JMS stream ended; reconnecting"
                            );
                            break;
                        }
                        Err(status) => {
                            if is_permanent_status(&status) {
                                // A consumption-time permanent failure (replay
                                // or capacity) fails only this route: terminate
                                // without reconnecting, leaving the broker slot
                                // Ready (task 4.4). The route failure is
                                // surfaced through the supervised handle by
                                // task 4.5.
                                warn!(
                                    broker = %broker_name,
                                    destination = %destination,
                                    consumer_idx = idx,
                                    operation = "stream",
                                    grpc_code = ?status.code(),
                                    "JMS stream permanent failure; failing the route without restarting the broker slot"
                                );
                                // Route-only Err: fail the supervised handle
                                // without restarting the broker slot (4.5).
                                // The error names the operator action and drops
                                // the remote status detail so no payload/header
                                // bytes leak (task 4.7).
                                return Err(permanent_destination_error(
                                    broker_name,
                                    "stream",
                                    &destination,
                                    &status,
                                ));
                            }
                            // Task 8.1: same local rendering as the
                            // subscribe site — a classified connection
                            // failure replaces the remote status detail; the
                            // transport prefix keeps the retry classification.
                            let subscribe_err = CamelError::ProcessorError(format!(
                                "{BRIDGE_TRANSPORT_ERROR_PREFIX}subscribe error: {}",
                                connection_failure_detail(pool, broker_name, &status.to_string())
                            ));
                            if is_bridge_transport_error(&subscribe_err) {
                                consecutive_transport_failures += 1;
                                // Invariant (rc-5vfz3): a successful subscribe
                                // resets `consecutive_transport_failures` to
                                // zero, so the first stream error reaches one
                                // and `break`s the stream loop before a second
                                // error can arrive. This threshold-two branch is
                                // therefore unreachable in the natural stream
                                // path; its shared recovery action is tested
                                // directly and its wiring statically, without
                                // claiming end-to-end coverage of this branch.
                                if consecutive_transport_failures >= 2 {
                                    warn!(
                                        broker = %broker_name,
                                        destination = %destination,
                                        consumer_idx = idx,
                                        failures = consecutive_transport_failures,
                                        "JMS stream transport failures exceeded threshold; refreshing channel"
                                    );
                                    // Shared refresh/cancel/wake action; a
                                    // requested shutdown is never blocked
                                    // (4.5).
                                    match refresh_channel_or_request_probe(
                                        pool,
                                        broker_name,
                                        &destination,
                                        idx as usize,
                                        &cancel,
                                        ctx,
                                    )
                                    .await
                                    {
                                        RecoveryOutcome::Cancelled => return Ok(()),
                                        RecoveryOutcome::Continue => {}
                                    }
                                    consecutive_transport_failures = 0;
                                }
                            } else {
                                consecutive_transport_failures = 0;
                            }
                            warn!(
                                broker = %broker_name,
                                destination = %destination,
                                consumer_idx = idx,
                                error = %subscribe_err,
                                "JMS stream error; reconnecting"
                            );
                            break;
                        }
                    }
                }
            }
        }

        attempt += 1;
        if !reconnect.should_retry(attempt) {
            warn!(
                broker = %broker_name,
                destination = %destination,
                consumer_idx = idx,
                attempts = attempt,
                "JMS consumer max reconnect attempts reached; terminating"
            );
            return Err(CamelError::ProcessorError(format!(
                "JMS consumer reconnect attempts exhausted after {} attempts",
                attempt
            )));
        }
        let delay = reconnect.delay_for(attempt - 1);
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = ctx.cancelled() => break,
            _ = tokio::time::sleep(delay) => {}
        }
    }

    // Every `break` above is a requested cancellation, which is normal.
    Ok(())
}

#[async_trait]
impl Consumer for JmsConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        // Reject double-start (JMS-006)
        if self.cancel_token.is_some() {
            return Err(CamelError::EndpointCreationFailed(
                "JMS consumer already started".into(),
            ));
        }

        // JMS-012: session transactions are unsupported; the bridge manages
        // settlement itself. Name that effective behaviour instead of implying
        // a silent fallback to AUTO_ACKNOWLEDGE / "None".
        if self.endpoint_config.transaction_mode == JmsTransactionMode::Session {
            warn!(
                "JMS session transaction mode is not supported by this consumer; the bridge \
                 manages settlement with CLIENT_ACKNOWLEDGE and per-delivery completion decisions"
            );
        }

        // JMS-005: warn when InOut pattern requested but not implemented
        if self.endpoint_config.exchange_pattern == ExchangePattern::InOut {
            warn!("JMS InOut pattern not yet implemented");
        }

        // JMS-017: Pre-flight check — fail fast when bridge is unavailable.
        // Probe the bridge slot before spawning the background loop so that
        // missing bridge binaries or unreachable brokers are reported immediately
        // instead of looping silently.
        {
            let slot = self
                .pool
                .get_or_create_slot(&self.broker_name, selector_required(&self.endpoint_config))
                .await?;
            match &*slot.state_rx.borrow() {
                BridgeState::Ready { .. } => {} // bridge is up — proceed
                // A concurrent caller observes Starting while the slot
                // creator boots the bridge (it holds bridge_create_lock).
                // Proceed: consumer_loop's await_ready_channel waits for
                // Ready (cancel-raced) and fails terminally on Parked/Stopped.
                BridgeState::Starting => {}
                BridgeState::Degraded(reason) => {
                    return Err(CamelError::ProcessorError(format!(
                        "JMS bridge not available: {}",
                        reason
                    )));
                }
                BridgeState::Parked(reason) => {
                    // Fail closed: the pre-flight must not spawn a consumer
                    // for a terminal slot (tasks 4.3/4.7).
                    return Err(parked_operator_error(&self.broker_name, reason));
                }
                other => {
                    return Err(CamelError::ProcessorError(format!(
                        "JMS bridge not available: {:?}",
                        other
                    )));
                }
            }
        }

        let pool = Arc::clone(&self.pool);
        let broker_name = self.broker_name.clone();
        let endpoint_config = self.endpoint_config.clone();
        let reconnect = self.reconnect.clone();
        // Requested-shutdown token: a child of the route cancel token, so a
        // context cancellation marks a requested shutdown, and `stop()` marks
        // one explicitly by cancelling it. The shared sibling token is a child
        // of it, so a requested shutdown cascades to every worker, while a
        // failure cancels only the group (never the requested token) (4.5).
        let requested = ctx.cancel_token().child_token();
        let shared = requested.child_token();
        self.cancel_token = Some(requested.clone());
        self.sibling_token = Some(shared.clone());
        self.requested_token = Some(requested);

        // JMS-011: spawn concurrent consumer tasks
        let consumer_count = endpoint_config.concurrent_consumers;
        let runtime = self.runtime.clone();
        // allow-unwrap: consumer_count validated >= 1 in from_uri
        let handles: Vec<JoinHandle<Result<(), CamelError>>> = (0..consumer_count)
            .map(|idx| {
                let pool = Arc::clone(&pool);
                let broker_name = broker_name.clone();
                let endpoint_config = endpoint_config.clone();
                let cancel = shared.clone();
                let ctx = ctx.clone();
                let reconnect = reconnect.clone();
                let runtime = runtime.clone();

                tokio::spawn(async move {
                    consumer_loop(
                        &pool,
                        &broker_name,
                        &endpoint_config,
                        &reconnect,
                        cancel,
                        &ctx,
                        idx,
                        runtime,
                    )
                    .await
                })
            })
            .collect();

        // Retain abort authority over every worker so stop() can always clean
        // them up, even after the supervised handle is handed to the runtime.
        self.worker_aborts = handles.iter().map(JoinHandle::abort_handle).collect();
        // Store all handles so stop() can abort/join every concurrent consumer.
        self.task_handles = handles;

        // NOTE: The actual JMS connection (consumer_loop with SubscribeRequest)
        // happens inside each spawned task. mark_ready is best-effort — the
        // pre-flight above accepted a ready or starting slot, so bridge
        // readiness is not guaranteed yet (the background loop waits for it),
        // and individual subscriptions are not confirmed.
        ctx.mark_ready();

        Ok(())
    }

    fn startup_mode(&self) -> ConsumerStartupMode {
        ConsumerStartupMode::Explicit
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        // Mark a requested shutdown FIRST: cancelling the requested-shutdown
        // token cascades to the shared sibling token, so every worker exits
        // cooperatively and the supervisor classifies the exits (including any
        // later abort) as normal rather than an unexpected abort. The global
        // route/context token is never cancelled here.
        if let Some(cancel) = self.cancel_token.take() {
            cancel.cancel();
        }
        // Join workers we still own (the supervised handle was never handed
        // off), preserving the bounded cooperative grace period.
        let handles = std::mem::take(&mut self.task_handles);
        for mut handle in handles {
            if tokio::time::timeout(Duration::from_secs(5), &mut handle)
                .await
                .is_err()
            {
                handle.abort();
                let _ = handle.await;
                warn!("JMS consumer task did not stop in 5s; aborted");
            }
        }
        // Fallback abort authority: if the runtime took the supervised handle
        // and aborted the supervisor, workers are aborted by its RAII guard;
        // this is a belt-and-braces guarantee that no worker is left detached.
        for abort in std::mem::take(&mut self.worker_aborts) {
            abort.abort();
        }
        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Sequential
    }

    fn background_task_handle(
        &mut self,
    ) -> Option<tokio::task::JoinHandle<Result<(), CamelError>>> {
        // The runtime observes the whole concurrent group, never a single
        // task: move every consumer handle into one supervisor task. The
        // returned handle resolves to the first failure (which cancels its
        // siblings) or `Ok(())` after a requested cancellation (task 4.5). The
        // supervisor owns an abort-on-drop guard and `stop()` retains
        // `worker_aborts`, so aborting the supervisor never detaches workers.
        let handles = std::mem::take(&mut self.task_handles);
        if handles.is_empty() {
            return None;
        }
        let shared = self.sibling_token.clone().unwrap_or_default();
        let requested = self.requested_token.clone().unwrap_or_default();
        Some(spawn_supervisor(handles, shared, requested))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BrokerType;
    use crate::bridge_client_test::{
        TestBridgeFixture, env_guard, next_refresh_attempt, ready_stub_process, refresh_attempts,
        replacement_attempts, spawn_scripted_health, stop_slot_process,
    };
    use crate::component::{BridgeSlot, ProbeOutcome, StartBridgeError};
    use crate::config::{JmsPoolConfig, jms_reconnect_default};
    use crate::diagnostics::evaluate_startup_selector_gate;
    use crate::proto::bridge_service_server::{BridgeService, BridgeServiceServer};
    use crate::proto::{
        CompletionOutcome, CompletionRequest, CompletionResponse, HealthRequest, HealthResponse,
        SendRequest, SendResponse,
    };
    use camel_bridge::process::BridgeProcessConfig;
    use camel_component_api::ExchangeEnvelope;
    use camel_component_api::test_support::{
        PanicRuntimeObservability, RecordingRuntimeObservability, timeout_wait,
    };
    use futures::Stream;
    use std::collections::VecDeque;
    use std::fmt::Write as _;
    use std::pin::Pin;
    use std::sync::Mutex;
    use tokio::sync::mpsc;
    use tonic::{Request, Response, Status};
    use tracing_subscriber::layer::SubscriberExt as _;

    fn rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(PanicRuntimeObservability)
    }

    #[test]
    fn build_exchange_text_body() {
        let msg = JmsMessage {
            message_id: "ID:1".to_string(),
            body: b"hello world".to_vec(),
            content_type: "text/plain".to_string(),
            ..Default::default()
        };
        let ex = build_exchange(&msg, true);
        assert!(matches!(ex.input.body, Body::Text(_)));
    }

    #[test]
    fn build_exchange_binary_body() {
        let msg = JmsMessage {
            message_id: "ID:2".to_string(),
            body: vec![0x00, 0x01, 0x02],
            content_type: "application/octet-stream".to_string(),
            ..Default::default()
        };
        let ex = build_exchange(&msg, true);
        assert!(matches!(ex.input.body, Body::Bytes(_)));
    }

    #[test]
    fn build_exchange_json_body() {
        let msg = JmsMessage {
            message_id: "ID:json".to_string(),
            body: br#"{"ok":true}"#.to_vec(),
            content_type: "application/json".to_string(),
            ..Default::default()
        };
        let ex = build_exchange(&msg, true);
        assert!(matches!(ex.input.body, Body::Json(_)));
    }

    #[test]
    fn build_exchange_empty_body() {
        let msg = JmsMessage {
            message_id: "ID:3".to_string(),
            body: vec![],
            content_type: "".to_string(),
            ..Default::default()
        };
        let ex = build_exchange(&msg, true);
        assert!(matches!(ex.input.body, Body::Empty));
    }

    #[test]
    fn build_exchange_without_header_mapping() {
        // JMS-018: when map_jms_headers is false, JMS headers must not be set
        let msg = JmsMessage {
            message_id: "ID:header-test".to_string(),
            correlation_id: "CORR:123".to_string(),
            timestamp: 1700000000,
            destination: "queue:orders".to_string(),
            body: b"hello".to_vec(),
            headers: Default::default(),
            content_type: "text/plain".to_string(),
            ..Default::default()
        };
        let ex = build_exchange(&msg, false);
        assert!(ex.input.header("JMSMessageID").is_none());
        assert!(ex.input.header("JMSCorrelationID").is_none());
        assert!(ex.input.header("JMSTimestamp").is_none());
        assert!(ex.input.header("JMSDestination").is_none());
        assert!(ex.input.header("Content-Type").is_none());
        assert_eq!(
            ex.property("CamelJmsReservationCount"),
            Some(&serde_json::json!(0))
        );
    }

    #[test]
    fn build_exchange_sets_reservation_count_property() {
        // The reservation count is bridge metadata: it must surface on the
        // exchange as a property, never on the inbound header map.
        let msg = JmsMessage {
            reservation_count: 4,
            ..Default::default()
        };
        let ex = build_exchange(&msg, true);
        assert_eq!(
            ex.property("CamelJmsReservationCount"),
            Some(&serde_json::json!(4)),
            "reservation_count must land in the CamelJmsReservationCount property"
        );
        assert!(
            ex.input.header("CamelJmsReservationCount").is_none(),
            "reservation count must not leak into the header map"
        );
    }

    /// Task 3.1: the generated proto module exposes the completion message and
    /// enum symbols used by the settlement RPC.
    #[test]
    fn proto_completion_types_exist() {
        let request = CompletionRequest::default();
        assert_eq!(
            request.outcome,
            CompletionOutcome::Unspecified as i32,
            "default outcome must be UNSPECIFIED"
        );
        assert_eq!(CompletionOutcome::Commit as i32, 1);
        assert_eq!(CompletionOutcome::Rollback as i32, 2);
        let response = CompletionResponse { accepted: true };
        assert!(response.accepted);
    }

    /// Task 3.1: the `RecordingBridge` mock also implements the new `Complete`
    /// RPC and accepts completions.
    #[tokio::test]
    async fn mock_services_implement_complete() {
        let bridge = RecordingBridge {
            recorded: Arc::new(Mutex::new(Vec::new())),
        };
        let response = BridgeService::complete(&bridge, Request::new(CompletionRequest::default()))
            .await
            .expect("recording bridge must complete");
        assert!(
            response.into_inner().accepted,
            "recording bridge must accept the completion"
        );
    }

    #[tokio::test]
    async fn stop_without_start_is_noop() {
        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::Generic,
            ))
            .unwrap(),
        );
        let endpoint_cfg = crate::config::JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut consumer = JmsConsumer::new(
            pool,
            "default".to_string(),
            endpoint_cfg,
            jms_reconnect_default(),
            rt(),
        );
        assert!(consumer.stop().await.is_ok());
    }

    // ── Task 4.3: parked waiters fail fast ───────────────────────────────────

    /// Task 4.3: `await_ready_channel` on a parked slot must return `Err`
    /// within a bounded time, naming the operator repair, instead of waiting
    /// for a state change that can never come.
    #[tokio::test]
    async fn await_ready_on_parked_returns_err() {
        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::Generic,
            ))
            .unwrap(),
        );
        let (state_tx, state_rx) =
            tokio::sync::watch::channel(BridgeState::Parked("corrupt journal".to_string()));
        let slot = Arc::new(BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::Generic,
            credentials: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        pool.slots.insert("default".to_string(), slot);

        let result = tokio::time::timeout(
            Duration::from_secs(2),
            await_ready_channel(&pool, "default", false),
        )
        .await
        .expect("await_ready_channel must not hang on a parked slot");

        let err = result
            .expect_err("a parked slot must fail fast")
            .to_string();
        assert!(
            err.contains("corrupt journal"),
            "error must carry the park reason: {err}"
        );
        assert!(
            err.contains("repair") && err.contains("restart the context"),
            "error must name the operator repair: {err}"
        );
    }

    /// Task 4.3: the consumer pre-flight probe treats `Parked` as a hard
    /// error — `start` must not spawn a consumer for a terminal slot.
    #[tokio::test]
    async fn consumer_preflight_parked_fails() {
        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::Generic,
            ))
            .unwrap(),
        );
        let (state_tx, state_rx) =
            tokio::sync::watch::channel(BridgeState::Parked("corrupt journal".to_string()));
        let slot = Arc::new(BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::Generic,
            credentials: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        pool.slots.insert("default".to_string(), slot);

        let endpoint_cfg = crate::config::JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut consumer = JmsConsumer::new(
            pool,
            "default".to_string(),
            endpoint_cfg,
            jms_reconnect_default(),
            rt(),
        );
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "jms-test-parked-preflight".to_string(),
        );

        let result = tokio::time::timeout(Duration::from_secs(2), consumer.start(ctx))
            .await
            .expect("start must fail fast on a parked slot");
        let err = result.expect_err("parked pre-flight must fail").to_string();
        assert!(
            err.contains("corrupt journal"),
            "error must carry the park reason: {err}"
        );
        assert!(
            err.contains("repair") && err.contains("restart the context"),
            "error must name the operator repair: {err}"
        );
    }

    /// camel-112: a concurrent caller of `get_or_create_slot` can observe the
    /// slot in `Starting` while the creator boots the bridge. The pre-flight
    /// must accept it — `consumer_loop`'s `await_ready_channel` already waits
    /// for `Ready` (cancel-raced) and fails terminally on a parked slot
    /// (mirrors the camel-cxf consumer pre-flight).
    #[tokio::test]
    async fn consumer_preflight_starting_spawns_and_waits() {
        // Identical pool + hand-built BridgeSlot shape as
        // consumer_preflight_parked_fails, but the watch starts at Starting.
        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::Generic,
            ))
            .unwrap(),
        );
        let (state_tx, state_rx) = tokio::sync::watch::channel(BridgeState::Starting);
        let slot = Arc::new(BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::Generic,
            credentials: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        // Retain the Arc so the test can reach state_tx/state_rx after the pool
        // is moved into the consumer (matches the component.rs slot fixtures).
        pool.slots.insert("default".to_string(), Arc::clone(&slot));

        let endpoint_cfg = crate::config::JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut consumer = JmsConsumer::new(
            pool,
            "default".to_string(),
            endpoint_cfg,
            jms_reconnect_default(),
            rt(),
        );
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "jms-test-starting-preflight".to_string(),
        );

        // Pre-flight accepts Starting: start() must return Ok (before the fix it
        // returns Err "JMS bridge not available: Starting"). Bounded timeout, no
        // sleeps.
        let result = tokio::time::timeout(Duration::from_secs(2), consumer.start(ctx))
            .await
            .expect("start must not hang in the pre-flight");
        result.expect("a Starting bridge must be accepted by the pre-flight");

        // The spawned workers await readiness via await_ready_channel, raced
        // against cancellation — but start() does not wait for them, so they
        // may not have polled yet. Grab the supervised group handle.
        let handle = consumer
            .background_task_handle()
            .expect("supervised group handle");

        // Park the slot: the terminal state must fail the workers with the
        // operator-repair error — whether a worker first observes Starting or
        // goes straight to Parked — with no time-based sync and no spurious
        // reconnect/restart.
        slot.state_tx
            .send(BridgeState::Parked("corrupt journal".to_string()))
            .expect("publish Parked");

        let inner = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("a terminal parked slot must terminate the background loop")
            .expect("supervisor task must not panic");
        let err = inner
            .expect_err("a parked slot must surface Err from the background loop")
            .to_string();
        assert!(
            err.contains("repair") && err.contains("restart the context"),
            "the loop error must carry the operator repair guidance, not a retry error: {err}"
        );
        assert!(
            matches!(*slot.state_rx.borrow(), BridgeState::Parked(_)),
            "the slot must stay Parked (no spurious restart)"
        );
    }

    // ── Task 4.7: route-only permanent errors ────────────────────────────────

    /// Task 4.7: a route-only permanent failure (gRPC `FAILED_PRECONDITION`)
    /// must name the operator action, yet must NOT echo the raw remote status
    /// detail — that detail is remote-controlled and may embed arbitrary
    /// payload or header bytes. The stable gRPC classification is preserved.
    #[test]
    fn permanent_route_error_omits_remote_status_detail() {
        let sentinel = "SECRET-HEADER-VALUE";
        let status = Status::failed_precondition(format!("replay failed for header {sentinel}"));
        assert!(
            is_permanent_status(&status),
            "fixture must classify as a permanent status"
        );

        let err =
            permanent_destination_error("default", "stream", "queue:orders", &status).to_string();
        for needle in [
            "stop the route",
            "repair or remove",
            "unkeyable source",
            "restart the context",
        ] {
            assert!(
                err.contains(needle),
                "route permanent error must name '{needle}': {err}"
            );
        }
        assert!(
            !err.contains(sentinel),
            "route permanent error must not echo the remote status detail: {err}"
        );
    }

    // ── JMS-006: Consumer double-start guard ──────────────────────────────────

    #[tokio::test]
    async fn consumer_double_start_returns_error() {
        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::Generic,
            ))
            .unwrap(),
        );
        let endpoint_cfg = crate::config::JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut consumer = JmsConsumer::new(
            pool,
            "default".to_string(),
            endpoint_cfg,
            jms_reconnect_default(),
            rt(),
        );

        // Simulate an already-started state by setting a cancel token directly.
        consumer.cancel_token = Some(CancellationToken::new());

        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "jms-test-route".to_string(),
        );
        let result = consumer.start(ctx).await;
        assert!(result.is_err(), "second start must return an error");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("already started"),
            "error must mention already started: {}",
            msg
        );
    }

    // ── JMS-002: Consumer stop joins task handle ─────────────────────────────

    #[tokio::test]
    async fn test_jms_consumer_stop_joins() {
        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::Generic,
            ))
            .unwrap(),
        );
        let endpoint_cfg = crate::config::JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut consumer = JmsConsumer::new(
            pool,
            "default".to_string(),
            endpoint_cfg,
            jms_reconnect_default(),
            rt(),
        );

        // Simulate a started consumer with a task that respects cancellation.
        let cancel = CancellationToken::new();
        consumer.cancel_token = Some(cancel.clone());
        consumer.task_handles = vec![tokio::spawn({
            let cancel = cancel.clone();
            async move {
                // Task waits for cancellation then exits cleanly.
                cancel.cancelled().await;
                Ok(())
            }
        })];

        // stop() must not panic and must join the task.
        let result = consumer.stop().await;
        assert!(result.is_ok(), "stop must succeed: {:?}", result.err());
        // Verify the handles were taken (tasks were joined).
        assert!(
            consumer.task_handles.is_empty(),
            "task_handles must be cleared after stop"
        );
    }

    // ── JMS-002: Consumer task panic is absorbed (Pattern S) ─────────────────

    #[tokio::test]
    async fn stop_absorbs_consumer_task_panic() {
        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::Generic,
            ))
            .unwrap(),
        );
        let endpoint_cfg = crate::config::JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut consumer = JmsConsumer::new(
            pool,
            "default".to_string(),
            endpoint_cfg,
            jms_reconnect_default(),
            rt(),
        );

        // Manually set a task handle that will panic.
        consumer.task_handles = vec![tokio::spawn(async {
            panic!("simulated consumer panic");
        })];
        // Give the panic time to materialize.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Pattern S: panic is absorbed, stop returns Ok.
        let result = consumer.stop().await;
        assert!(
            result.is_ok(),
            "stop must absorb panic and return Ok: {:?}",
            result.err()
        );
    }

    // ── JMS-002: stop joins tasks awaiting the context token ────────────────

    #[tokio::test]
    async fn stop_joins_tasks_awaiting_context_token() {
        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::Generic,
            ))
            .unwrap(),
        );
        let endpoint_cfg = crate::config::JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut consumer = JmsConsumer::new(
            pool,
            "default".to_string(),
            endpoint_cfg,
            jms_reconnect_default(),
            rt(),
        );

        let (tx, _rx) = mpsc::channel(16);
        let t = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, t.clone(), "test-route".to_string());
        consumer.cancel_token = Some(ctx.cancel_token());
        consumer.task_handles = vec![tokio::spawn({
            let t = t.clone();
            async move {
                t.cancelled().await;
                Ok(())
            }
        })];

        let result = tokio::time::timeout(Duration::from_secs(1), consumer.stop()).await;
        assert!(result.is_ok(), "stop must complete within 1s");
        assert!(
            result.unwrap().is_ok(),
            "stop must return Ok after joining tasks"
        );
        assert!(
            consumer.task_handles.is_empty(),
            "task_handles must be cleared after stop"
        );
    }

    // ── JMS-017: Missing bridge returns Err immediately ───────────────────────

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn missing_bridge_returns_err_within_1s() {
        // Serialize against other tests mutating CAMEL_JMS_BRIDGE_BINARY_PATH (rc-alwn).
        let _env_lock_guard = crate::BRIDGE_ENV_LOCK.lock().unwrap();

        let env_key = "CAMEL_JMS_BRIDGE_BINARY_PATH";
        let _guard = env_guard(env_key);
        unsafe { std::env::set_var(env_key, "/bin/false") };

        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig {
                brokers: std::collections::HashMap::from([(
                    "default".to_string(),
                    crate::config::BrokerConfig {
                        broker_url: "tcp://localhost:61616".to_string(),
                        broker_type: BrokerType::ActiveMq,
                        username: None,
                        password: None,
                        tls: None,
                    },
                )]),
                bridge_start_timeout_ms: 100,
                ..JmsPoolConfig::default()
            })
            .unwrap(),
        );
        let endpoint_cfg = crate::config::JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut consumer = JmsConsumer::new(
            pool,
            "default".to_string(),
            endpoint_cfg,
            jms_reconnect_default(),
            rt(),
        );

        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "jms-test-route-2".to_string(),
        );

        let start = std::time::Instant::now();
        let result = consumer.start(ctx).await;
        let elapsed = start.elapsed();

        assert!(result.is_err(), "expected Err when bridge missing, got Ok");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("JMS bridge not available"),
            "error must mention bridge unavailability: {}",
            msg
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "must fail within 1s, took {:?}",
            elapsed
        );

        // Cleanup: stop the health monitors etc.
        let _ = consumer.stop().await;
    }

    /// Regression: max_attempts=N → exactly N invocations (caught OpenSearch off-by-one 1f5c4c2a).
    /// Replicates the exact retry loop from `consumer_loop` (consumer.rs:166-177):
    ///   attempt starts at 0, incremented on error, !should_retry(attempt), delay_for(attempt-1)
    #[tokio::test]
    async fn retry_loop_invokes_operation_exactly_max_attempts_times() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::time::Duration;

        let policy = NetworkRetryPolicy {
            max_attempts: 3,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
            multiplier: 1.0,
            ..NetworkRetryPolicy::default()
        };

        let calls = Arc::new(AtomicU32::new(0));
        let calls_clone = Arc::clone(&calls);
        let mut attempt: u32 = 0;

        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                calls_clone.fetch_add(1, Ordering::SeqCst);
                let result: Result<(), ()> = Err(());
                match result {
                    Ok(_) => {
                        break;
                    }
                    Err(_) => {
                        attempt += 1;
                        if !policy.should_retry(attempt) {
                            break;
                        }
                        let delay = policy.delay_for(attempt - 1);
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                }
            }
        })
        .await
        .expect("retry loop must finish within 30s");

        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "max_attempts=3 must yield exactly 3 invocations"
        );
    }

    // ── rc-5e4l: subscription_id uniqueness ──────────────────────────────────

    /// In-process bridge mock that records every outgoing SubscribeRequest
    /// (mirrors `spawn_mock_bridge` in bridge_client_test.rs).
    #[derive(Clone)]
    struct RecordingBridge {
        recorded: Arc<Mutex<Vec<SubscribeRequest>>>,
    }

    #[async_trait]
    impl BridgeService for RecordingBridge {
        async fn send(
            &self,
            _request: Request<SendRequest>,
        ) -> Result<Response<SendResponse>, Status> {
            Err(Status::unimplemented("send not exercised here"))
        }

        type SubscribeStream = Pin<Box<dyn Stream<Item = Result<JmsMessage, Status>> + Send>>;

        async fn subscribe(
            &self,
            request: Request<SubscribeRequest>,
        ) -> Result<Response<Self::SubscribeStream>, Status> {
            self.recorded
                .lock()
                .expect("recorder mutex poisoned")
                .push(request.into_inner());
            // Empty stream: consumer_loop sees Ok(None) immediately and, with
            // max_attempts=1, terminates after exactly one subscribe.
            Ok(Response::new(Box::pin(futures::stream::empty::<
                Result<JmsMessage, Status>,
            >())))
        }

        async fn health(
            &self,
            _request: Request<HealthRequest>,
        ) -> Result<Response<HealthResponse>, Status> {
            // Task 1.2: model a modern, selector-capable bridge so the
            // fail-closed capability gate lets selector routes reach their
            // Subscribe (the legacy empty answer parks them).
            Ok(Response::new(HealthResponse {
                healthy: true,
                broker_connected: true,
                message: "ok".to_string(),
                status: 0,
                age_ms: 0,
                capabilities: vec![
                    "broker-tls-v1".to_string(),
                    "message-selector-v1".to_string(),
                ],
                ..Default::default()
            }))
        }

        async fn complete(
            &self,
            _request: Request<CompletionRequest>,
        ) -> Result<Response<CompletionResponse>, Status> {
            // Task 3.1: accept every completion so the mock service satisfies
            // the new RPC surface; real semantics land in task 3.2.
            Ok(Response::new(CompletionResponse { accepted: true }))
        }
    }

    async fn spawn_recording_bridge(
        recorded: Arc<Mutex<Vec<SubscribeRequest>>>,
    ) -> std::io::Result<u16> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(BridgeServiceServer::new(RecordingBridge { recorded }))
                .serve_with_incoming(incoming)
                .await;
        });

        // Give the spawned accept loop a moment before the client dials.
        tokio::time::sleep(Duration::from_millis(50)).await;

        Ok(port)
    }

    /// Task 2.5 fixture: every subscribe fails with `UNAVAILABLE`; health
    /// succeeds and signals each call so producer/consumer wakes are observed
    /// through the real monitor.
    #[derive(Clone)]
    struct AlwaysUnavailableSubscribeBridge {
        subscribes: Arc<std::sync::atomic::AtomicUsize>,
        health_calls: Arc<std::sync::atomic::AtomicUsize>,
        subscribe_events: mpsc::UnboundedSender<()>,
        health_events: mpsc::UnboundedSender<()>,
    }

    #[async_trait]
    impl BridgeService for AlwaysUnavailableSubscribeBridge {
        async fn send(
            &self,
            _request: Request<SendRequest>,
        ) -> Result<Response<SendResponse>, Status> {
            Err(Status::unimplemented("send not exercised here"))
        }

        type SubscribeStream = Pin<Box<dyn Stream<Item = Result<JmsMessage, Status>> + Send>>;

        async fn subscribe(
            &self,
            _request: Request<SubscribeRequest>,
        ) -> Result<Response<Self::SubscribeStream>, Status> {
            self.subscribes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _ = self.subscribe_events.send(());
            Err(Status::unavailable("subscribe always unavailable"))
        }

        async fn health(
            &self,
            _request: Request<HealthRequest>,
        ) -> Result<Response<HealthResponse>, Status> {
            self.health_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _ = self.health_events.send(());
            // Legacy bridge answer: no explicit status, UNSPECIFIED with zero age.
            Ok(Response::new(HealthResponse {
                healthy: true,
                broker_connected: true,
                message: "ok".to_string(),
                status: 0,
                age_ms: 0,
                ..Default::default()
            }))
        }

        async fn complete(
            &self,
            _request: Request<CompletionRequest>,
        ) -> Result<Response<CompletionResponse>, Status> {
            Ok(Response::new(CompletionResponse { accepted: true }))
        }
    }

    struct AlwaysUnavailableBridgeHandle {
        port: u16,
        subscribes: Arc<std::sync::atomic::AtomicUsize>,
        health_calls: Arc<std::sync::atomic::AtomicUsize>,
        subscribe_events: mpsc::UnboundedReceiver<()>,
        health_events: mpsc::UnboundedReceiver<()>,
        task: tokio::task::JoinHandle<()>,
    }

    impl AlwaysUnavailableBridgeHandle {
        async fn next_subscribe(&mut self) {
            tokio::time::timeout(Duration::from_secs(5), self.subscribe_events.recv())
                .await
                .expect("subscribe attempt within 5s")
                .expect("subscribe event channel open");
        }

        async fn next_health(&mut self) {
            tokio::time::timeout(Duration::from_secs(5), self.health_events.recv())
                .await
                .expect("health call within 5s")
                .expect("health event channel open");
        }

        async fn shutdown(self) {
            self.task.abort();
            let _ = self.task.await;
        }
    }

    async fn spawn_always_unavailable_subscribe_bridge() -> AlwaysUnavailableBridgeHandle {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind always-unavailable listener");
        let port = listener.local_addr().expect("local addr").port();
        let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
        let subscribes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let health_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (subscribe_events_tx, subscribe_events) = mpsc::unbounded_channel();
        let (health_events_tx, health_events) = mpsc::unbounded_channel();
        let service = AlwaysUnavailableSubscribeBridge {
            subscribes: Arc::clone(&subscribes),
            health_calls: Arc::clone(&health_calls),
            subscribe_events: subscribe_events_tx,
            health_events: health_events_tx,
        };
        let task = tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(BridgeServiceServer::new(service))
                .serve_with_incoming(incoming)
                .await;
        });
        AlwaysUnavailableBridgeHandle {
            port,
            subscribes,
            health_calls,
            subscribe_events,
            health_events,
            task,
        }
    }

    /// Task 2.5 fixture: subscribe 0 succeeds with a stream that immediately
    /// errors `UNAVAILABLE`; subscribes 1 and 2 are gated and then fail
    /// `UNAVAILABLE`, producing two separated threshold-two refresh windows so
    /// the test can install a second real refresh target between them. Later
    /// subscribes stay pending, keeping the consumer alive for cancellation.
    #[derive(Clone)]
    struct GatedTransportFailuresBridge {
        subscribes: Arc<std::sync::atomic::AtomicUsize>,
        health_calls: Arc<std::sync::atomic::AtomicUsize>,
        subscribe_events: mpsc::UnboundedSender<()>,
        health_events: mpsc::UnboundedSender<()>,
        first_gate: Arc<tokio::sync::Notify>,
        second_gate: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl BridgeService for GatedTransportFailuresBridge {
        async fn send(
            &self,
            _request: Request<SendRequest>,
        ) -> Result<Response<SendResponse>, Status> {
            Err(Status::unimplemented("send not exercised here"))
        }

        type SubscribeStream = Pin<Box<dyn Stream<Item = Result<JmsMessage, Status>> + Send>>;

        async fn subscribe(
            &self,
            _request: Request<SubscribeRequest>,
        ) -> Result<Response<Self::SubscribeStream>, Status> {
            let n = self
                .subscribes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _ = self.subscribe_events.send(());
            match n {
                0 => Ok(Response::new(Box::pin(futures::stream::once(async move {
                    Err::<JmsMessage, Status>(Status::unavailable("stream unavailable"))
                })))),
                1 => {
                    self.first_gate.notified().await;
                    Err(Status::unavailable(
                        "subscribe unavailable after first stream error",
                    ))
                }
                2 => {
                    self.second_gate.notified().await;
                    Err(Status::unavailable(
                        "subscribe unavailable after first refresh",
                    ))
                }
                3 => Err(Status::unavailable(
                    "subscribe unavailable after second retry",
                )),
                _ => Ok(Response::new(Box::pin(futures::stream::pending()))),
            }
        }

        async fn health(
            &self,
            _request: Request<HealthRequest>,
        ) -> Result<Response<HealthResponse>, Status> {
            self.health_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _ = self.health_events.send(());
            // Legacy bridge answer: no explicit status, UNSPECIFIED with zero age.
            Ok(Response::new(HealthResponse {
                healthy: true,
                broker_connected: true,
                message: "ok".to_string(),
                status: 0,
                age_ms: 0,
                ..Default::default()
            }))
        }

        async fn complete(
            &self,
            _request: Request<CompletionRequest>,
        ) -> Result<Response<CompletionResponse>, Status> {
            Ok(Response::new(CompletionResponse { accepted: true }))
        }
    }

    async fn spawn_gated_transport_failures_bridge() -> (
        AlwaysUnavailableBridgeHandle,
        Arc<tokio::sync::Notify>,
        Arc<tokio::sync::Notify>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind gated transport-failure listener");
        let port = listener.local_addr().expect("local addr").port();
        let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
        let subscribes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let health_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (subscribe_events_tx, subscribe_events) = mpsc::unbounded_channel();
        let (health_events_tx, health_events) = mpsc::unbounded_channel();
        let first_gate = Arc::new(tokio::sync::Notify::new());
        let second_gate = Arc::new(tokio::sync::Notify::new());
        let service = GatedTransportFailuresBridge {
            subscribes: Arc::clone(&subscribes),
            health_calls: Arc::clone(&health_calls),
            subscribe_events: subscribe_events_tx,
            health_events: health_events_tx,
            first_gate: Arc::clone(&first_gate),
            second_gate: Arc::clone(&second_gate),
        };
        let task = tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(BridgeServiceServer::new(service))
                .serve_with_incoming(incoming)
                .await;
        });
        (
            AlwaysUnavailableBridgeHandle {
                port,
                subscribes,
                health_calls,
                subscribe_events,
                health_events,
                task,
            },
            first_gate,
            second_gate,
        )
    }

    /// Two ways a gRPC `FAILED_PRECONDITION` reaches the consumer: immediate
    /// subscribe failure, or a successful subscribe followed by a stream error.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum PermanentBridgeMode {
        /// `subscribe` itself fails with `FAILED_PRECONDITION`.
        Subscribe,
        /// `subscribe` succeeds, then the stream's first item is an `Err`.
        Stream,
    }

    /// In-process bridge mock that surfaces a permanent gRPC
    /// `FAILED_PRECONDITION` in `mode` and records every subscribe attempt.
    /// Proves a destination permanent failure is route-only (task 4.4).
    #[derive(Clone)]
    struct PermanentFailureBridge {
        mode: PermanentBridgeMode,
        attempts: Arc<Mutex<Vec<String>>>,
        /// Remote-controlled status message. Tests inject a sentinel here to
        /// prove the consumer never echoes it into a log or a returned error.
        status_message: String,
    }

    #[async_trait]
    impl BridgeService for PermanentFailureBridge {
        async fn send(
            &self,
            _request: Request<SendRequest>,
        ) -> Result<Response<SendResponse>, Status> {
            Err(Status::unimplemented("send not exercised here"))
        }

        type SubscribeStream = Pin<Box<dyn Stream<Item = Result<JmsMessage, Status>> + Send>>;

        async fn subscribe(
            &self,
            request: Request<SubscribeRequest>,
        ) -> Result<Response<Self::SubscribeStream>, Status> {
            self.attempts
                .lock()
                .expect("attempts mutex poisoned")
                .push(request.into_inner().subscription_id);
            let permanent = Status::failed_precondition(self.status_message.clone());
            match self.mode {
                PermanentBridgeMode::Subscribe => Err(permanent),
                PermanentBridgeMode::Stream => {
                    let stream =
                        futures::stream::once(async move { Err::<JmsMessage, Status>(permanent) });
                    Ok(Response::new(Box::pin(stream)))
                }
            }
        }

        async fn health(
            &self,
            _request: Request<HealthRequest>,
        ) -> Result<Response<HealthResponse>, Status> {
            // Legacy bridge answer: no explicit status, UNSPECIFIED with zero age.
            Ok(Response::new(HealthResponse {
                healthy: true,
                broker_connected: true,
                message: "ok".to_string(),
                status: 0,
                age_ms: 0,
                ..Default::default()
            }))
        }

        async fn complete(
            &self,
            _request: Request<CompletionRequest>,
        ) -> Result<Response<CompletionResponse>, Status> {
            Ok(Response::new(CompletionResponse { accepted: true }))
        }
    }

    async fn spawn_permanent_failure_bridge(
        mode: PermanentBridgeMode,
        attempts: Arc<Mutex<Vec<String>>>,
    ) -> std::io::Result<u16> {
        spawn_permanent_failure_bridge_with_message(
            mode,
            attempts,
            "corrupt journal: replay failed (permanent)".to_string(),
        )
        .await
    }

    async fn spawn_permanent_failure_bridge_with_message(
        mode: PermanentBridgeMode,
        attempts: Arc<Mutex<Vec<String>>>,
        status_message: String,
    ) -> std::io::Result<u16> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(BridgeServiceServer::new(PermanentFailureBridge {
                    mode,
                    attempts,
                    status_message,
                }))
                .serve_with_incoming(incoming)
                .await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        Ok(port)
    }

    /// Task 4.5 group-supervision mock: the first subscribe fails permanently,
    /// later subscribes hand back a stream that stays pending. Without sibling
    /// cancellation the pending consumer would keep the group alive forever.
    #[derive(Clone)]
    struct FirstPermanentBridge {
        subscribes: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl BridgeService for FirstPermanentBridge {
        async fn send(
            &self,
            _request: Request<SendRequest>,
        ) -> Result<Response<SendResponse>, Status> {
            Err(Status::unimplemented("send not exercised here"))
        }

        type SubscribeStream = Pin<Box<dyn Stream<Item = Result<JmsMessage, Status>> + Send>>;

        async fn subscribe(
            &self,
            _request: Request<SubscribeRequest>,
        ) -> Result<Response<Self::SubscribeStream>, Status> {
            let n = self
                .subscribes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                Err(Status::failed_precondition("corrupt journal (permanent)"))
            } else {
                Ok(Response::new(Box::pin(futures::stream::pending())))
            }
        }

        async fn health(
            &self,
            _request: Request<HealthRequest>,
        ) -> Result<Response<HealthResponse>, Status> {
            // Legacy bridge answer: no explicit status, UNSPECIFIED with zero age.
            Ok(Response::new(HealthResponse {
                healthy: true,
                broker_connected: true,
                message: "ok".to_string(),
                status: 0,
                age_ms: 0,
                ..Default::default()
            }))
        }

        async fn complete(
            &self,
            _request: Request<CompletionRequest>,
        ) -> Result<Response<CompletionResponse>, Status> {
            Ok(Response::new(CompletionResponse { accepted: true }))
        }
    }

    async fn spawn_first_permanent_bridge(
        subscribes: Arc<std::sync::atomic::AtomicUsize>,
    ) -> std::io::Result<u16> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(BridgeServiceServer::new(FirstPermanentBridge {
                    subscribes,
                }))
                .serve_with_incoming(incoming)
                .await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        Ok(port)
    }

    /// Task 4.5 mock: `subscribe` never returns, so a worker can only stop if
    /// its subscribe wait is raced against cancellation. With `fail_first`, the
    /// first subscribe fails permanently and every later one hangs.
    #[derive(Clone)]
    struct HangingSubscribeBridge {
        fail_first: bool,
        subscribes: Arc<std::sync::atomic::AtomicUsize>,
        subscribed: mpsc::UnboundedSender<()>,
    }

    #[async_trait]
    impl BridgeService for HangingSubscribeBridge {
        async fn send(
            &self,
            _request: Request<SendRequest>,
        ) -> Result<Response<SendResponse>, Status> {
            Err(Status::unimplemented("send not exercised here"))
        }

        type SubscribeStream = Pin<Box<dyn Stream<Item = Result<JmsMessage, Status>> + Send>>;

        async fn subscribe(
            &self,
            _request: Request<SubscribeRequest>,
        ) -> Result<Response<Self::SubscribeStream>, Status> {
            let n = self
                .subscribes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _ = self.subscribed.send(());
            if self.fail_first && n == 0 {
                return Err(Status::failed_precondition("corrupt journal (permanent)"));
            }
            std::future::pending::<()>().await;
            unreachable!("hanging subscribe never returns")
        }

        async fn health(
            &self,
            _request: Request<HealthRequest>,
        ) -> Result<Response<HealthResponse>, Status> {
            // Legacy bridge answer: no explicit status, UNSPECIFIED with zero age.
            Ok(Response::new(HealthResponse {
                healthy: true,
                broker_connected: true,
                message: "ok".to_string(),
                status: 0,
                age_ms: 0,
                ..Default::default()
            }))
        }

        async fn complete(
            &self,
            _request: Request<CompletionRequest>,
        ) -> Result<Response<CompletionResponse>, Status> {
            Ok(Response::new(CompletionResponse { accepted: true }))
        }
    }

    async fn spawn_hanging_subscribe_bridge(
        fail_first: bool,
        subscribed: mpsc::UnboundedSender<()>,
    ) -> std::io::Result<u16> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(BridgeServiceServer::new(HangingSubscribeBridge {
                    fail_first,
                    subscribes: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                    subscribed,
                }))
                .serve_with_incoming(incoming)
                .await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        Ok(port)
    }

    /// Task 4.6 transient mock: the first subscribe hands back a stream whose
    /// first item is a transient `UNAVAILABLE` error; every later subscribe
    /// parks on a pending stream (so the reconnected consumer stays open instead
    /// of tripping the channel-refresh path). Records every subscribe attempt
    /// and signals each one so the test can await the reconnect barrier.
    #[derive(Clone)]
    struct TransientOnceBridge {
        subscribes: Arc<std::sync::atomic::AtomicUsize>,
        subscribed: mpsc::UnboundedSender<()>,
    }

    #[async_trait]
    impl BridgeService for TransientOnceBridge {
        async fn send(
            &self,
            _request: Request<SendRequest>,
        ) -> Result<Response<SendResponse>, Status> {
            Err(Status::unimplemented("send not exercised here"))
        }

        type SubscribeStream = Pin<Box<dyn Stream<Item = Result<JmsMessage, Status>> + Send>>;

        async fn subscribe(
            &self,
            _request: Request<SubscribeRequest>,
        ) -> Result<Response<Self::SubscribeStream>, Status> {
            let n = self
                .subscribes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _ = self.subscribed.send(());
            if n == 0 {
                let stream = futures::stream::once(async move {
                    Err::<JmsMessage, Status>(Status::unavailable("transient transport failure"))
                });
                Ok(Response::new(Box::pin(stream)))
            } else {
                Ok(Response::new(Box::pin(futures::stream::pending())))
            }
        }

        async fn health(
            &self,
            _request: Request<HealthRequest>,
        ) -> Result<Response<HealthResponse>, Status> {
            // Legacy bridge answer: no explicit status, UNSPECIFIED with zero age.
            Ok(Response::new(HealthResponse {
                healthy: true,
                broker_connected: true,
                message: "ok".to_string(),
                status: 0,
                age_ms: 0,
                ..Default::default()
            }))
        }

        async fn complete(
            &self,
            _request: Request<CompletionRequest>,
        ) -> Result<Response<CompletionResponse>, Status> {
            Ok(Response::new(CompletionResponse { accepted: true }))
        }
    }

    async fn spawn_transient_once_bridge(
        subscribes: Arc<std::sync::atomic::AtomicUsize>,
        subscribed: mpsc::UnboundedSender<()>,
    ) -> std::io::Result<u16> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(BridgeServiceServer::new(TransientOnceBridge {
                    subscribes,
                    subscribed,
                }))
                .serve_with_incoming(incoming)
                .await;
        });

        // The listener is already bound; the caller's `pool_with_mock_channel`
        // connect is bounded by its own handshake timeout, so no readiness
        // sleep is needed.
        Ok(port)
    }

    /// Pins consumer.rs:213 — `subscription_id: Uuid::new_v4().to_string()` —
    /// against a future refactor to a static/shared id. Two subscribe flows
    /// through the same consumer path must mint DISTINCT, non-empty,
    /// uuid-shaped ids (not a test of uuid randomness itself).
    #[tokio::test]
    async fn two_subscribes_mint_distinct_subscription_ids() {
        let recorded: Arc<Mutex<Vec<SubscribeRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let port = spawn_recording_bridge(Arc::clone(&recorded))
            .await
            .expect("spawn recording bridge");

        let channel = timeout_wait(
            "bridge connect",
            Duration::from_secs(30),
            tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
                .expect("valid endpoint uri")
                .connect(),
        )
        .await
        .expect("connect to mock bridge");

        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::Generic,
            ))
            .unwrap(),
        );
        // Manually insert a Ready slot pointing at the in-process mock, the
        // same shape `get_or_create_slot` produces after a bridge starts.
        let (state_tx, state_rx) = tokio::sync::watch::channel(BridgeState::Ready { channel });
        let slot = Arc::new(BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::Generic,
            credentials: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        pool.slots.insert("default".to_string(), Arc::clone(&slot));

        let endpoint_cfg = crate::config::JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        // max_attempts=1 → each consumer_loop invocation performs exactly one
        // subscribe, then terminates when the empty stream ends.
        let reconnect = NetworkRetryPolicy {
            max_attempts: 1,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
            multiplier: 1.0,
            ..NetworkRetryPolicy::default()
        };

        for idx in 0..2u32 {
            let (route_tx, _route_rx) = mpsc::channel(16);
            let ctx = ConsumerContext::new(
                route_tx,
                CancellationToken::new(),
                format!("jms-distinct-id-{idx}"),
            );
            let _ = consumer_loop(
                &pool,
                "default",
                &endpoint_cfg,
                &reconnect,
                CancellationToken::new(),
                &ctx,
                idx,
                rt(),
            )
            .await;
        }

        let requests = recorded.lock().expect("recorder mutex poisoned");
        assert_eq!(
            requests.len(),
            2,
            "two subscribe flows must each mint exactly one subscription_id"
        );
        let ids: Vec<&str> = requests
            .iter()
            .map(|r| r.subscription_id.as_str())
            .collect();
        assert!(
            !ids[0].is_empty() && !ids[1].is_empty(),
            "subscription_ids must be non-empty"
        );
        assert_ne!(
            ids[0], ids[1],
            "two subscribes must never share a subscription_id"
        );
        for id in ids.iter() {
            assert_eq!(id.len(), 36, "uuid-shaped subscription_id: {id}");
            assert_eq!(
                id.matches('-').count(),
                4,
                "uuid-shaped subscription_id: {id}"
            );
        }
    }

    /// Task 1.1 (JMS-010): the decoded `message_selector` configured on the
    /// endpoint URI must reach the bridge `SubscribeRequest` — an encoded
    /// selector reaching the broker would filter on the wrong expression.
    #[tokio::test]
    async fn selector_reaches_subscribe_request() {
        let recorded: Arc<Mutex<Vec<SubscribeRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let port = spawn_recording_bridge(Arc::clone(&recorded))
            .await
            .expect("spawn recording bridge");
        let pool = pool_with_mock_channel(port).await;
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "jms-selector".to_string(),
        );
        let endpoint_cfg =
            JmsEndpointConfig::from_uri("jms:queue:orders?message_selector=priority%20%3E%205")
                .expect("uri with selector must parse");
        // max_attempts=1 → exactly one subscribe, then the empty stream ends
        // the loop (same shape as `two_subscribes_mint_distinct_subscription_ids`).
        let _ = consumer_loop(
            &pool,
            "default",
            &endpoint_cfg,
            &reconnect_after(1),
            CancellationToken::new(),
            &ctx,
            0,
            rt(),
        )
        .await;

        let captured = recorded.lock().expect("recorder mutex poisoned");
        assert_eq!(captured.len(), 1, "exactly one subscribe request");
        assert_eq!(
            captured[0].message_selector, "priority > 5",
            "encoded selector must reach the bridge decoded"
        );
        assert_eq!(captured[0].destination, "queue:orders");
    }

    /// Task 1.1 (JMS-010): without a `message_selector` option the outgoing
    /// `SubscribeRequest` is unchanged — empty selector, and destination /
    /// uuid-shaped `subscription_id` equal what the pre-change assertions
    /// capture.
    #[tokio::test]
    async fn no_selector_request_unchanged() {
        let recorded: Arc<Mutex<Vec<SubscribeRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let port = spawn_recording_bridge(Arc::clone(&recorded))
            .await
            .expect("spawn recording bridge");
        let pool = pool_with_mock_channel(port).await;
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "jms-no-selector".to_string(),
        );
        let endpoint_cfg = JmsEndpointConfig::from_uri("jms:queue:orders")
            .expect("uri without selector must parse");
        // max_attempts=1 → exactly one subscribe, then the empty stream ends
        // the loop (same shape as `two_subscribes_mint_distinct_subscription_ids`).
        let _ = consumer_loop(
            &pool,
            "default",
            &endpoint_cfg,
            &reconnect_after(1),
            CancellationToken::new(),
            &ctx,
            0,
            rt(),
        )
        .await;

        let captured = recorded.lock().expect("recorder mutex poisoned");
        assert_eq!(captured.len(), 1, "exactly one subscribe request");
        assert!(
            captured[0].message_selector.is_empty(),
            "absent selector must serialize as empty"
        );
        assert_eq!(captured[0].destination, "queue:orders");
        let id = &captured[0].subscription_id;
        assert!(!id.is_empty(), "subscription_id must be non-empty");
        assert_eq!(id.len(), 36, "uuid-shaped subscription_id: {id}");
        assert_eq!(
            id.matches('-').count(),
            4,
            "uuid-shaped subscription_id: {id}"
        );
    }

    // ── Task 1.2: fail-closed message-selector-v1 capability gate ────────────

    use crate::bridge_client_test::{
        connect_scripted, selector_monitor_fixture, shutdown_test_slot, spawn_selector_gate_bridge,
        wait_parked,
    };

    /// Writes an executable stub bridge child announcing `port` on the ready
    /// line (task 1.2 launch-path tests: the gate window's channel is
    /// substituted, so the announced port is inert). Returns the stub path and
    /// its pidfile.
    #[cfg(unix)]
    fn ready_line_stub(
        dir: &std::path::Path,
        tag: &str,
        port: u16,
    ) -> (std::path::PathBuf, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let pidfile = dir.join(format!("{tag}.pid"));
        let stub = dir.join(format!("{tag}.sh"));
        std::fs::write(
            &stub,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nprintf '%s\\n' '{{\"status\":\"ready\",\"port\":{port}}}'\nexec sleep 30\n",
                pidfile.display()
            ),
        )
        .expect("write ready stub");
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755))
            .expect("ready stub exec bit");
        (stub, pidfile)
    }

    /// Bounded wait until `pid` is gone (task 1.2 kill assertions; mirrors the
    /// `wait_pid_gone` helper in the component test module).
    #[cfg(unix)]
    async fn wait_pid_gone(pid: u32) -> bool {
        for _ in 0..100 {
            let alive = tokio::process::Command::new("kill")
                .arg("-0")
                .arg(pid.to_string())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await
                .map(|s| s.success())
                .unwrap_or(false);
            if !alive {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    /// Diagnostic contract (task 1.2): the park/error text names the missing
    /// capability, the required bridge version, and the pinned bridge version.
    fn assert_selector_park_text(text: &str) {
        for needle in ["message-selector-v1", crate::BRIDGE_VERSION] {
            assert!(
                text.contains(needle),
                "text must contain '{needle}': {text}"
            );
        }
    }

    /// Spawns `consumer_loop` for one route; the token requests a normal
    /// shutdown once the test's barrier is reached.
    fn spawn_route_consumer(
        pool: Arc<JmsBridgePool>,
        endpoint_cfg: JmsEndpointConfig,
        route_id: &str,
    ) -> (JoinHandle<Result<(), CamelError>>, CancellationToken) {
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let route_id = route_id.to_string();
        let handle = tokio::spawn(async move {
            let (route_tx, _route_rx) = mpsc::channel(16);
            let ctx = ConsumerContext::new(route_tx, CancellationToken::new(), route_id);
            consumer_loop(
                &pool,
                "default",
                &endpoint_cfg,
                &reconnect_after(3),
                worker_cancel,
                &ctx,
                0,
                rt(),
            )
            .await
        });
        (handle, cancel)
    }

    /// Task 1.2: the STARTUP selector gate. The shared decision seam rejects
    /// empty evidence when a selector is required (three-fragment error), and
    /// the REAL launch path (`start_bridge_process_with_config`) invokes it:
    /// the launch is rejected permanently, the gated child is killed, and the
    /// launch-fixture path parks the slot with EXACTLY that error before any
    /// Subscribe reaches the bridge.
    #[cfg(unix)]
    #[tokio::test]
    async fn startup_parks_when_selector_configured_and_capability_missing() {
        // Unit level: the shared startup seam itself.
        let seam_err = evaluate_startup_selector_gate(&[], true).expect_err(
            "empty capabilities must fail the startup gate when a selector is required",
        );
        assert_selector_park_text(&seam_err.to_string());
        assert!(
            evaluate_startup_selector_gate(&[], false).is_ok(),
            "without a selector requirement the startup gate passes trivially"
        );

        // Launch path: Health evidence `capabilities: vec![]`, the requirement
        // armed exactly as a selector route arms it.
        let bridge = spawn_selector_gate_bridge(&[]).await;
        let channel = connect_scripted(bridge.port).await;
        let dir = tempfile::tempdir().expect("stub tempdir");
        let (stub, pidfile) = ready_line_stub(dir.path(), "startup-gate", 1);
        let config = BridgeProcessConfig::jms(
            stub,
            "tcp://localhost:61616".to_string(),
            BrokerType::ActiveMq,
            None,
            None,
            2_000,
            30_000,
            30_000,
            Some("/var/lib/state/rust-camel/jms-bridge".to_string()),
            Some("0123456789abcdef0123456789abcdef".to_string()),
            None,
        );
        let result = JmsBridgePool::start_bridge_process_with_config(
            &config,
            2_000,
            false,
            &std::sync::atomic::AtomicBool::new(true),
            Some(channel),
        )
        .await;
        let reason = match result {
            Err(StartBridgeError::Permanent(reason)) => reason,
            Err(StartBridgeError::Other(e)) => {
                panic!("the startup gate must classify permanently: {e}")
            }
            Ok(_) => panic!("the startup selector gate must reject the launch"),
        };
        assert_selector_park_text(&reason);
        // Mirrors the TLS precedent (startup_gate_kills_bridge_and_fails_permanently):
        // the gated child is killed, not leaked.
        let pid: u32 = std::fs::read_to_string(&pidfile)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        assert!(pid > 0, "the stub must have written its pid");
        assert!(
            wait_pid_gone(pid).await,
            "the gated child {pid} must be killed, not leaked"
        );

        // The launch-fixture path parks with EXACTLY the startup gate error.
        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::Generic,
            ))
            .expect("startup gate pool config must be valid"),
        );
        let (state_tx, state_rx) = tokio::sync::watch::channel(BridgeState::Starting);
        let slot = Arc::new(BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::Generic,
            credentials: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        pool.slots.insert("default".to_string(), Arc::clone(&slot));
        JmsBridgePool::apply_initial_start_result(
            &slot,
            Err(StartBridgeError::Permanent(reason.clone())),
        )
        .await;
        match &*slot.state_rx.borrow() {
            BridgeState::Parked(parked) => assert_eq!(
                parked, &reason,
                "the launch-fixture path must park with exactly the startup gate error"
            ),
            other => panic!("the slot must park permanently: {other:?}"),
        }

        // ...before any Subscribe: a selector route is stopped by the parked
        // slot and the bridge mock records ZERO Subscribe calls.
        let endpoint_cfg =
            JmsEndpointConfig::from_uri("jms:queue:orders?message_selector=priority%20%3E%205")
                .expect("uri with selector must parse");
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "jms-selector-startup-park".to_string(),
        );
        let err = tokio::time::timeout(
            Duration::from_secs(10),
            consumer_loop(
                &pool,
                "default",
                &endpoint_cfg,
                &reconnect_after(1),
                CancellationToken::new(),
                &ctx,
                0,
                rt(),
            ),
        )
        .await
        .expect("a parked slot must fail fast")
        .expect_err("a selector route on an old bridge must fail");
        assert_selector_park_text(&err.to_string());
        assert_eq!(
            bridge.subscribe_count(),
            0,
            "the startup gate parked before any Subscribe"
        );
        bridge.shutdown().await;
    }

    /// Task 1.2: a selector-configured route on a bridge advertising
    /// `message-selector-v1` passes the gate and the Subscribe call carries the
    /// decoded selector.
    #[tokio::test]
    async fn startup_subscribes_when_selector_capability_present() {
        let mut bridge =
            spawn_selector_gate_bridge(&["broker-tls-v1", "message-selector-v1"]).await;
        let (pool, slot) = selector_monitor_fixture(bridge.port).await;

        let endpoint_cfg =
            JmsEndpointConfig::from_uri("jms:queue:orders?message_selector=priority%20%3E%205")
                .expect("uri with selector must parse");
        let (handle, cancel) =
            spawn_route_consumer(Arc::clone(&pool), endpoint_cfg, "jms-selector-capable");

        bridge.next_subscribe().await;
        cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("consumer exits on requested shutdown");

        {
            let captured = bridge.subscribes.lock().expect("subscribes mutex poisoned");
            assert_eq!(captured.len(), 1, "exactly one subscribe request");
            assert_eq!(
                captured[0].message_selector, "priority > 5",
                "the selector must reach the bridge decoded"
            );
            assert_eq!(captured[0].destination, "queue:orders");
        }
        shutdown_test_slot(&slot).await;
        bridge.shutdown().await;
    }

    /// Task 1.2: a NO-selector route on an old bridge (empty capabilities) is
    /// unaffected — the gate passes trivially and the subscription proceeds.
    #[tokio::test]
    async fn no_selector_old_bridge_unaffected() {
        let mut bridge = spawn_selector_gate_bridge(&[]).await;
        let (pool, slot) = selector_monitor_fixture(bridge.port).await;

        let endpoint_cfg = JmsEndpointConfig::from_uri("jms:queue:orders")
            .expect("uri without selector must parse");
        let (handle, cancel) =
            spawn_route_consumer(Arc::clone(&pool), endpoint_cfg, "jms-no-selector-old");

        bridge.next_subscribe().await;
        cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("consumer exits on requested shutdown");

        {
            let captured = bridge.subscribes.lock().expect("subscribes mutex poisoned");
            assert_eq!(captured.len(), 1, "the subscription must proceed");
            assert!(
                captured[0].message_selector.is_empty(),
                "no selector must serialize as empty"
            );
        }
        shutdown_test_slot(&slot).await;
        bridge.shutdown().await;
    }

    /// Task 1.2: capability evidence is authoritative regardless of broker
    /// health status — a Health response with `status: STALE` advertising
    /// `message-selector-v1` passes the gate on the capability alone and the
    /// Subscribe call proceeds.
    #[tokio::test]
    async fn startup_gate_ignores_broker_health_status() {
        let mut bridge = spawn_selector_gate_bridge(&["message-selector-v1"]).await;
        // Raw `HealthStatus.STALE` = 2: unverified broker status must neither
        // bypass nor weaken the capability gate.
        *bridge.status.lock().expect("status mutex poisoned") = 2;
        let (pool, slot) = selector_monitor_fixture(bridge.port).await;

        let endpoint_cfg =
            JmsEndpointConfig::from_uri("jms:queue:orders?message_selector=priority%20%3E%205")
                .expect("uri with selector must parse");
        let (handle, cancel) =
            spawn_route_consumer(Arc::clone(&pool), endpoint_cfg, "jms-selector-stale");

        bridge.next_subscribe().await;
        cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("consumer exits on requested shutdown");

        {
            let captured = bridge.subscribes.lock().expect("subscribes mutex poisoned");
            assert_eq!(
                captured.len(),
                1,
                "the gate must pass on the capability alone and the Subscribe proceed"
            );
        }
        shutdown_test_slot(&slot).await;
        bridge.shutdown().await;
    }

    /// Task 1.2: runtime capability loss parks permanently and stops
    /// resubscription. The later Health responses (no capability) model the
    /// replacement instance; the shared flag registered at consumer attach
    /// arms the runtime gate, so the slot parks and no further Subscribe calls
    /// are sent.
    #[tokio::test]
    async fn runtime_capability_loss_parks_permanently() {
        let mut bridge = spawn_selector_gate_bridge(&["message-selector-v1"]).await;
        let (pool, slot) = selector_monitor_fixture(bridge.port).await;
        let endpoint_cfg =
            JmsEndpointConfig::from_uri("jms:queue:orders?message_selector=priority%20%3E%205")
                .expect("uri with selector must parse");
        let (handle, _cancel) =
            spawn_route_consumer(Arc::clone(&pool), endpoint_cfg, "jms-selector-runtime-loss");

        // First responses include the capability: the attach gate passes and
        // exactly one Subscribe goes out.
        bridge.next_subscribe().await;
        assert_eq!(bridge.subscribe_count(), 1, "one Subscribe before the loss");

        // The replaced (older) instance advertises nothing.
        bridge.set_capabilities(&[]);
        let reason = wait_parked(&slot).await;
        assert_selector_park_text(&reason);

        // Release the stream: the consumer re-attaches — or would resubscribe
        // if the park had not stopped it terminally.
        bridge.end_streams();
        let result = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("consumer must terminate after the park")
            .expect("consumer task must not panic");
        let err = result.expect_err("the parked slot must fail the consumer terminally");
        assert_selector_park_text(&err.to_string());
        assert_eq!(
            bridge.subscribe_count(),
            1,
            "no further Subscribe calls after capability loss"
        );

        shutdown_test_slot(&slot).await;
        bridge.shutdown().await;
    }

    /// Task 1.2: a selector consumer attaching to an already-running slot
    /// whose bridge never advertised `message-selector-v1` (the startup gate
    /// was bypassed before the selector route existed) is stopped by the
    /// attach-path fresh Health check BEFORE its first Subscribe — the
    /// Subscribe-call count does not increase and the consumer fails with the
    /// three-fragment error.
    #[tokio::test]
    async fn selector_on_already_running_incapable_slot_parks_before_subscribe() {
        let mut bridge = spawn_selector_gate_bridge(&[]).await;
        let (pool, slot) = selector_monitor_fixture(bridge.port).await;

        // A NON-selector consumer keeps running against the incapable bridge
        // (slot startup saw no selector requirement).
        let no_selector_cfg = JmsEndpointConfig::from_uri("jms:queue:orders")
            .expect("uri without selector must parse");
        let (first_handle, _first_cancel) =
            spawn_route_consumer(Arc::clone(&pool), no_selector_cfg, "jms-incapable-plain");
        bridge.next_subscribe().await;
        assert_eq!(
            bridge.subscribe_count(),
            1,
            "baseline non-selector Subscribe"
        );

        // The selector route attaches late.
        let selector_cfg =
            JmsEndpointConfig::from_uri("jms:queue:orders?message_selector=priority%20%3E%205")
                .expect("uri with selector must parse");
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "jms-incapable-selector".to_string(),
        );
        let err = tokio::time::timeout(
            Duration::from_secs(10),
            consumer_loop(
                &pool,
                "default",
                &selector_cfg,
                &reconnect_after(1),
                CancellationToken::new(),
                &ctx,
                1,
                rt(),
            ),
        )
        .await
        .expect("the attach gate must fail fast")
        .expect_err("a selector consumer on an incapable bridge must fail");
        assert_selector_park_text(&err.to_string());
        assert_eq!(
            bridge.subscribe_count(),
            1,
            "the attach gate must park before the selector consumer's Subscribe"
        );

        bridge.end_streams();
        let _ = tokio::time::timeout(Duration::from_secs(5), first_handle)
            .await
            .expect("first consumer terminates");
        shutdown_test_slot(&slot).await;
        bridge.shutdown().await;
    }

    /// Task 1.2: a selector consumer attaching LATE to a capable running slot
    /// passes the fresh attach check and subscribes; when the shared flag (via
    /// that registration) is armed and the bridge stops advertising the
    /// capability, the runtime gate parks permanently with the three-fragment
    /// error and no further Subscribe calls are sent.
    #[tokio::test]
    async fn late_selector_on_capable_slot_then_capability_loss_parks() {
        let mut bridge = spawn_selector_gate_bridge(&["message-selector-v1"]).await;
        let (pool, slot) = selector_monitor_fixture(bridge.port).await;

        // NON-selector consumer already running.
        let no_selector_cfg = JmsEndpointConfig::from_uri("jms:queue:orders")
            .expect("uri without selector must parse");
        let (plain_handle, _plain_cancel) =
            spawn_route_consumer(Arc::clone(&pool), no_selector_cfg, "jms-late-plain");
        bridge.next_subscribe().await;

        // The selector consumer attaches late: the fresh check passes and its
        // Subscribe (carrying the selector) proceeds.
        let selector_cfg =
            JmsEndpointConfig::from_uri("jms:queue:orders?message_selector=priority%20%3E%205")
                .expect("uri with selector must parse");
        let (selector_handle, _selector_cancel) =
            spawn_route_consumer(Arc::clone(&pool), selector_cfg, "jms-late-selector");
        bridge.next_subscribe().await;
        assert_eq!(bridge.subscribe_count(), 2, "both consumers subscribed");
        {
            let captured = bridge.subscribes.lock().expect("subscribes mutex poisoned");
            assert_eq!(
                captured[1].message_selector, "priority > 5",
                "the late selector consumer's Subscribe must carry the selector"
            );
        }

        // The bridge stops advertising the capability: the shared flag armed
        // by the late registration trips the runtime gate.
        bridge.set_capabilities(&[]);
        let reason = wait_parked(&slot).await;
        assert_selector_park_text(&reason);

        bridge.end_streams();
        for handle in [plain_handle, selector_handle] {
            let result = tokio::time::timeout(Duration::from_secs(5), handle)
                .await
                .expect("consumer must terminate after the park")
                .expect("consumer task must not panic");
            let err = result.expect_err("the parked slot must fail consumers terminally");
            assert_selector_park_text(&err.to_string());
        }
        assert_eq!(
            bridge.subscribe_count(),
            2,
            "no further Subscribe calls after capability loss"
        );

        shutdown_test_slot(&slot).await;
        bridge.shutdown().await;
    }

    // ── Task 3.5: send_and_wait settlement harness ───────────────────────────
    //
    // These tests drive the real `consumer_loop` against an in-process tonic
    // mock bridge. The mock delivers one JMS message per subscription and
    // records every `Complete(…)` request. The harness is barrier-based: the
    // test pulls the route envelope off the pipeline channel (proving the
    // message reached the route), completes its `reply_tx`, and then awaits the
    // recorded completion. No sleeps or fixed durations gate synchronization.

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct CompletionRecord {
        subscription_id: String,
        subscription_generation: u64,
        delivery_token: String,
        outcome: i32,
    }

    /// Completion-RPC disposition the mock returns for every request.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum CompleteMode {
        Accept,
        Reject,
        Error,
    }

    /// In-process bridge mock that records completion requests and serves a
    /// finite queue of deliveries.
    #[derive(Clone)]
    struct SettlementBridge {
        messages: Arc<Mutex<VecDeque<JmsMessage>>>,
        completions: Arc<Mutex<Vec<CompletionRecord>>>,
        completion_tx: mpsc::UnboundedSender<CompletionRecord>,
        subscribe_tx: mpsc::UnboundedSender<String>,
        mode: CompleteMode,
    }

    #[async_trait]
    impl BridgeService for SettlementBridge {
        async fn send(
            &self,
            _request: Request<SendRequest>,
        ) -> Result<Response<SendResponse>, Status> {
            Err(Status::unimplemented("send not exercised here"))
        }

        type SubscribeStream = Pin<Box<dyn Stream<Item = Result<JmsMessage, Status>> + Send>>;

        async fn subscribe(
            &self,
            request: Request<SubscribeRequest>,
        ) -> Result<Response<Self::SubscribeStream>, Status> {
            let sub_id = request.into_inner().subscription_id;
            let _ = self.subscribe_tx.send(sub_id);
            let message = self.messages.lock().expect("messages mutex").pop_front();
            let stream: Self::SubscribeStream = match message {
                Some(msg) => Box::pin(
                    futures::stream::once(async move { Ok(msg) })
                        .chain(futures::stream::pending::<Result<JmsMessage, Status>>()),
                ),
                // No prepared delivery: keep the stream open so the consumer
                // parks instead of reconnecting.
                None => Box::pin(futures::stream::pending()),
            };
            Ok(Response::new(stream))
        }

        async fn health(
            &self,
            _request: Request<HealthRequest>,
        ) -> Result<Response<HealthResponse>, Status> {
            // Legacy bridge answer: no explicit status, UNSPECIFIED with zero age.
            Ok(Response::new(HealthResponse {
                healthy: true,
                broker_connected: true,
                message: "ok".to_string(),
                status: 0,
                age_ms: 0,
                ..Default::default()
            }))
        }

        async fn complete(
            &self,
            request: Request<CompletionRequest>,
        ) -> Result<Response<CompletionResponse>, Status> {
            let req = request.into_inner();
            let record = CompletionRecord {
                subscription_id: req.subscription_id,
                subscription_generation: req.subscription_generation,
                delivery_token: req.delivery_token,
                outcome: req.outcome,
            };
            self.completions
                .lock()
                .expect("completions mutex")
                .push(record.clone());
            let _ = self.completion_tx.send(record);
            match self.mode {
                CompleteMode::Accept => Ok(Response::new(CompletionResponse { accepted: true })),
                CompleteMode::Reject => Ok(Response::new(CompletionResponse { accepted: false })),
                CompleteMode::Error => Err(Status::internal("completion failed")),
            }
        }
    }

    struct SettlementMock {
        port: u16,
        completions: Arc<Mutex<Vec<CompletionRecord>>>,
        completion_rx: mpsc::UnboundedReceiver<CompletionRecord>,
        subscribe_rx: mpsc::UnboundedReceiver<String>,
    }

    async fn spawn_settlement_bridge(
        messages: Vec<JmsMessage>,
        mode: CompleteMode,
    ) -> std::io::Result<SettlementMock> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

        let completions = Arc::new(Mutex::new(Vec::new()));
        let (completion_tx, completion_rx) = mpsc::unbounded_channel();
        let (subscribe_tx, subscribe_rx) = mpsc::unbounded_channel();
        let bridge = SettlementBridge {
            messages: Arc::new(Mutex::new(VecDeque::from(messages))),
            completions: Arc::clone(&completions),
            completion_tx,
            subscribe_tx,
            mode,
        };

        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(BridgeServiceServer::new(bridge))
                .serve_with_incoming(incoming)
                .await;
        });

        Ok(SettlementMock {
            port,
            completions,
            completion_rx,
            subscribe_rx,
        })
    }

    /// Insert a `Ready` bridge slot whose channel dials the in-process mock.
    async fn pool_with_mock_channel(port: u16) -> Arc<JmsBridgePool> {
        let endpoint = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
            .expect("valid endpoint uri");
        let channel = tokio::time::timeout(Duration::from_secs(5), endpoint.connect())
            .await
            .expect("connect must not time out")
            .expect("connect to mock bridge");

        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::Generic,
            ))
            .unwrap(),
        );
        let (state_tx, state_rx) = tokio::sync::watch::channel(BridgeState::Ready { channel });
        let slot = Arc::new(BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::Generic,
            credentials: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        pool.slots.insert("default".to_string(), slot);
        pool
    }

    /// Insert a terminal `Parked` bridge slot (a startup journal failure), with
    /// no process and no channel: the shape `get_or_create_slot` publishes when
    /// the bridge reports a permanent startup failure (tasks 4.1/4.2).
    async fn pool_with_parked_slot() -> Arc<JmsBridgePool> {
        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::Generic,
            ))
            .unwrap(),
        );
        let (state_tx, state_rx) =
            tokio::sync::watch::channel(BridgeState::Parked("corrupt journal".to_string()));
        let slot = Arc::new(BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::Generic,
            credentials: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        pool.slots.insert("default".to_string(), slot);
        pool
    }

    fn consumer_liveness_pool(interval_ms: u64) -> Arc<JmsBridgePool> {
        Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig {
                health_check_interval_ms: interval_ms,
                health_timeout_ms: 250,
                health_failure_threshold: 1,
                ..JmsPoolConfig::default()
            })
            .expect("consumer liveness pool config must be valid"),
        )
    }

    fn test_slot_consumer(name: &str) -> Arc<BridgeSlot> {
        let (state_tx, state_rx) = tokio::sync::watch::channel(BridgeState::Starting);
        Arc::new(BridgeSlot {
            name: name.to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::Generic,
            credentials: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }

    async fn insert_ready_process_slot(
        pool: &Arc<JmsBridgePool>,
        channel: tonic::transport::Channel,
        process: camel_bridge::process::BridgeProcess,
    ) -> Arc<BridgeSlot> {
        let (state_tx, state_rx) = tokio::sync::watch::channel(BridgeState::Ready { channel });
        let slot = Arc::new(BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::Generic,
            credentials: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(Some(process))),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        pool.slots.insert("default".to_string(), Arc::clone(&slot));
        slot
    }

    async fn stop_test_monitor(slot: &Arc<BridgeSlot>) {
        let _ = slot.state_tx.send_replace(BridgeState::Stopped);
        let mut monitor =
            tokio::time::timeout(Duration::from_secs(2), slot.health_monitor_handle.lock())
                .await
                .expect("monitor handle lock within 2s")
                .take();
        if let Some(monitor) = monitor.as_mut()
            && tokio::time::timeout(Duration::from_secs(2), &mut *monitor)
                .await
                .is_err()
        {
            monitor.abort();
            let _ = monitor.await;
        }
    }

    fn spawn_jms_loop(
        pool: &Arc<JmsBridgePool>,
        ctx: &ConsumerContext,
        cancel: CancellationToken,
        reconnect: NetworkRetryPolicy,
        runtime: Arc<dyn RuntimeObservability>,
    ) -> tokio::task::JoinHandle<Result<(), CamelError>> {
        let pool = Arc::clone(pool);
        let ctx = ctx.clone();
        let endpoint_cfg = JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        tokio::spawn(async move {
            consumer_loop(
                &pool,
                "default",
                &endpoint_cfg,
                &reconnect,
                cancel,
                &ctx,
                0,
                runtime,
            )
            .await
        })
    }

    async fn recv_envelope(rx: &mut mpsc::Receiver<ExchangeEnvelope>) -> ExchangeEnvelope {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("envelope within 5s")
            .expect("pipeline channel open")
    }

    async fn recv_completion(
        rx: &mut mpsc::UnboundedReceiver<CompletionRecord>,
    ) -> CompletionRecord {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("completion within 5s")
            .expect("completion channel open")
    }

    async fn recv_subscribe(rx: &mut mpsc::UnboundedReceiver<String>) -> String {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("subscribe within 5s")
            .expect("subscribe channel open")
    }

    fn reconnect_after(attempts: u32) -> NetworkRetryPolicy {
        NetworkRetryPolicy {
            max_attempts: attempts,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
            multiplier: 1.0,
            ..NetworkRetryPolicy::default()
        }
    }

    fn delivered_message(token: &str, generation: u64) -> JmsMessage {
        JmsMessage {
            message_id: token.to_string(),
            body: b"payload".to_vec(),
            content_type: "text/plain".to_string(),
            delivery_token: token.to_string(),
            subscription_generation: generation,
            reservation_count: 1,
            ..Default::default()
        }
    }

    /// `send_and_wait` returning `Ok` must report exactly one `COMMIT` carrying
    /// the wire subscription id, delivery token, and generation.
    #[tokio::test]
    async fn send_and_wait_ok_reports_commit() {
        let mut mock = spawn_settlement_bridge(
            vec![delivered_message("tok-commit", 11)],
            CompleteMode::Accept,
        )
        .await
        .expect("spawn settlement bridge");
        let pool = pool_with_mock_channel(mock.port).await;
        let (route_tx, mut route_rx) = mpsc::channel(16);
        let ctx =
            ConsumerContext::new(route_tx, CancellationToken::new(), "jms-commit".to_string());
        let cancel = CancellationToken::new();
        let handle = spawn_jms_loop(&pool, &ctx, cancel.clone(), reconnect_after(1), rt());

        let sub_id = recv_subscribe(&mut mock.subscribe_rx).await;
        let envelope = recv_envelope(&mut route_rx).await;
        let reply_tx = envelope
            .reply_tx
            .expect("consumer must use send_and_wait (reply_tx present)");
        assert!(
            mock.completions
                .lock()
                .expect("completions lock")
                .is_empty(),
            "no completion may be sent while the reply is pending"
        );

        reply_tx
            .send(Ok(Exchange::new(Message::new(Body::Empty))))
            .expect("reply delivered");
        let record = recv_completion(&mut mock.completion_rx).await;
        assert_eq!(
            record.outcome,
            CompletionOutcome::Commit as i32,
            "Ok must map to COMMIT"
        );
        assert_eq!(record.delivery_token, "tok-commit");
        assert_eq!(record.subscription_generation, 11);
        assert_eq!(
            record.subscription_id, sub_id,
            "completion must retain the wire subscription_id"
        );

        cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
    }

    /// A propagated route error must report `ROLLBACK` and keep the stream for
    /// broker redelivery, while emitting the protected consumer-send metric.
    #[tokio::test]
    async fn send_and_wait_err_reports_rollback() {
        let mut mock = spawn_settlement_bridge(
            vec![delivered_message("tok-rollback", 12)],
            CompleteMode::Accept,
        )
        .await
        .expect("spawn settlement bridge");
        let pool = pool_with_mock_channel(mock.port).await;
        let runtime = RecordingRuntimeObservability::new(false);
        let runtime_dyn: Arc<dyn RuntimeObservability> = runtime.clone();
        let (route_tx, mut route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "jms-rollback".to_string(),
        );
        let cancel = CancellationToken::new();
        let handle = spawn_jms_loop(&pool, &ctx, cancel.clone(), reconnect_after(1), runtime_dyn);

        let envelope = recv_envelope(&mut route_rx).await;
        envelope
            .reply_tx
            .expect("send_and_wait")
            .send(Err(CamelError::ProcessorError("route failed".to_string())))
            .expect("reply delivered");
        let record = recv_completion(&mut mock.completion_rx).await;
        assert_eq!(record.outcome, CompletionOutcome::Rollback as i32);
        assert_eq!(
            runtime.errors(),
            vec![(
                "jms-rollback".to_string(),
                "b-prime:jms:consumer-send".to_string()
            )],
            "Err must emit the b-prime consumer-send metric"
        );
        // A healthy route failure keeps the same stream so the broker can
        // redeliver; no new subscription is opened.
        let first_sub = recv_subscribe(&mut mock.subscribe_rx).await;
        assert_eq!(first_sub.len(), 36);
        assert!(
            mock.subscribe_rx.try_recv().is_err(),
            "rollback must keep the consumer stream for broker redelivery"
        );

        cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
    }

    /// `ChannelClosed` must never `COMMIT`; the metric path is still used and
    /// the stream is dropped because the pipeline channel is dead.
    #[tokio::test]
    async fn channel_closed_never_commits() {
        let mut mock = spawn_settlement_bridge(
            vec![delivered_message("tok-closed", 13)],
            CompleteMode::Accept,
        )
        .await
        .expect("spawn settlement bridge");
        let pool = pool_with_mock_channel(mock.port).await;
        let runtime = RecordingRuntimeObservability::new(false);
        let runtime_dyn: Arc<dyn RuntimeObservability> = runtime.clone();
        let (route_tx, mut route_rx) = mpsc::channel(16);
        let ctx =
            ConsumerContext::new(route_tx, CancellationToken::new(), "jms-closed".to_string());
        let cancel = CancellationToken::new();
        let handle = spawn_jms_loop(&pool, &ctx, cancel.clone(), reconnect_after(1), runtime_dyn);

        let envelope = recv_envelope(&mut route_rx).await;
        envelope
            .reply_tx
            .expect("send_and_wait")
            .send(Err(CamelError::ChannelClosed))
            .expect("reply delivered");
        let record = recv_completion(&mut mock.completion_rx).await;
        assert_ne!(
            record.outcome,
            CompletionOutcome::Commit as i32,
            "ChannelClosed must never COMMIT"
        );
        assert_eq!(record.outcome, CompletionOutcome::Rollback as i32);
        assert_eq!(
            runtime.errors(),
            vec![(
                "jms-closed".to_string(),
                "b-prime:jms:consumer-send".to_string()
            )]
        );

        let joined = tokio::time::timeout(Duration::from_secs(5), handle).await;
        assert!(
            joined.is_ok(),
            "a dead pipeline channel must drop the stream and stop the consumer"
        );
        assert_eq!(
            mock.completions.lock().expect("completions lock").len(),
            1,
            "no completion may be retried"
        );
    }

    /// ADR-0024: `Completed`, `Stopped`, and handled errors all arrive at the
    /// reply boundary as `Ok` and must `COMMIT`.
    #[tokio::test]
    async fn handled_and_stop_map_to_ok() {
        let mut mock = spawn_settlement_bridge(
            vec![delivered_message("tok-stop", 14)],
            CompleteMode::Accept,
        )
        .await
        .expect("spawn settlement bridge");
        let pool = pool_with_mock_channel(mock.port).await;
        let (route_tx, mut route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(route_tx, CancellationToken::new(), "jms-stop".to_string());
        let cancel = CancellationToken::new();
        let handle = spawn_jms_loop(&pool, &ctx, cancel.clone(), reconnect_after(1), rt());

        let envelope = recv_envelope(&mut route_rx).await;
        let mut stopped = Exchange::new(Message::new(Body::Empty));
        stopped.set_property(camel_api::CAMEL_STOP, serde_json::Value::Bool(true));
        envelope
            .reply_tx
            .expect("send_and_wait")
            .send(Ok(stopped))
            .expect("reply delivered");
        let record = recv_completion(&mut mock.completion_rx).await;
        assert_eq!(
            record.outcome,
            CompletionOutcome::Commit as i32,
            "Stopped is Ok at the reply boundary and must COMMIT"
        );

        cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
    }

    /// A completion RPC error must drop the stream and terminate; the
    /// completion is never retried.
    #[tokio::test]
    async fn completion_failure_drops_stream() {
        let mut mock =
            spawn_settlement_bridge(vec![delivered_message("tok-fail", 15)], CompleteMode::Error)
                .await
                .expect("spawn settlement bridge");
        let pool = pool_with_mock_channel(mock.port).await;
        let (route_tx, mut route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(route_tx, CancellationToken::new(), "jms-fail".to_string());
        let cancel = CancellationToken::new();
        let handle = spawn_jms_loop(&pool, &ctx, cancel.clone(), reconnect_after(1), rt());

        let envelope = recv_envelope(&mut route_rx).await;
        envelope
            .reply_tx
            .expect("send_and_wait")
            .send(Ok(Exchange::new(Message::new(Body::Empty))))
            .expect("reply delivered");
        let record = recv_completion(&mut mock.completion_rx).await;
        assert_eq!(record.outcome, CompletionOutcome::Commit as i32);

        let joined = tokio::time::timeout(Duration::from_secs(5), handle).await;
        assert!(
            joined.is_ok(),
            "a completion RPC failure must drop the stream and stop the consumer"
        );
        assert_eq!(
            mock.completions.lock().expect("completions lock").len(),
            1,
            "the completion must not be retried"
        );
        cancel.cancel();
    }

    /// `accepted == false` is an uncertain completion: drop the stream, never
    /// retry against the stale token.
    #[tokio::test]
    async fn completion_rejected_drops_stream_without_retry() {
        let mut mock = spawn_settlement_bridge(
            vec![delivered_message("tok-reject", 16)],
            CompleteMode::Reject,
        )
        .await
        .expect("spawn settlement bridge");
        let pool = pool_with_mock_channel(mock.port).await;
        let (route_tx, mut route_rx) = mpsc::channel(16);
        let ctx =
            ConsumerContext::new(route_tx, CancellationToken::new(), "jms-reject".to_string());
        let cancel = CancellationToken::new();
        let handle = spawn_jms_loop(&pool, &ctx, cancel.clone(), reconnect_after(1), rt());

        let envelope = recv_envelope(&mut route_rx).await;
        envelope
            .reply_tx
            .expect("send_and_wait")
            .send(Ok(Exchange::new(Message::new(Body::Empty))))
            .expect("reply delivered");
        let record = recv_completion(&mut mock.completion_rx).await;
        assert_eq!(record.delivery_token, "tok-reject");

        let joined = tokio::time::timeout(Duration::from_secs(5), handle).await;
        assert!(
            joined.is_ok(),
            "accepted=false must drop the stream and stop the consumer"
        );
        assert_eq!(
            mock.completions.lock().expect("completions lock").len(),
            1,
            "a rejected completion must not be retried"
        );
        cancel.cancel();
    }

    /// After a completion failure the stream is dropped and a new subscription
    /// opens, but the stale wire token is never re-completed.
    #[tokio::test]
    async fn completion_failure_does_not_retry_stale_token() {
        let mut mock = spawn_settlement_bridge(
            vec![
                delivered_message("tok-stale-1", 21),
                delivered_message("tok-stale-2", 22),
            ],
            CompleteMode::Error,
        )
        .await
        .expect("spawn settlement bridge");
        let pool = pool_with_mock_channel(mock.port).await;
        let (route_tx, mut route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(route_tx, CancellationToken::new(), "jms-stale".to_string());
        let cancel = CancellationToken::new();
        let handle = spawn_jms_loop(&pool, &ctx, cancel.clone(), reconnect_after(2), rt());

        let first_sub = recv_subscribe(&mut mock.subscribe_rx).await;
        let first = recv_envelope(&mut route_rx).await;
        first
            .reply_tx
            .expect("send_and_wait")
            .send(Ok(Exchange::new(Message::new(Body::Empty))))
            .expect("reply delivered");
        let first_record = recv_completion(&mut mock.completion_rx).await;
        assert_eq!(first_record.delivery_token, "tok-stale-1");

        // The failed completion dropped the stream; a fresh subscription opens.
        let second_sub = recv_subscribe(&mut mock.subscribe_rx).await;
        assert_ne!(first_sub, second_sub, "drop must open a new subscription");
        let _second = recv_envelope(&mut route_rx).await;

        assert_eq!(
            mock.completions.lock().expect("completions lock").len(),
            1,
            "the stale token must never be completed twice"
        );
        cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
    }

    /// Cancel delivered through the consumer-owned token while the reply is
    /// pending must wake the loop and send no completion.
    #[tokio::test]
    async fn held_reply_cancel_consumer_token_sends_no_complete() {
        let mock = spawn_settlement_bridge(
            vec![delivered_message("tok-cancel-a", 31)],
            CompleteMode::Accept,
        )
        .await
        .expect("spawn settlement bridge");
        let pool = pool_with_mock_channel(mock.port).await;
        let (route_tx, mut route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "jms-cancel-a".to_string(),
        );
        let cancel = CancellationToken::new();
        let handle = spawn_jms_loop(&pool, &ctx, cancel.clone(), reconnect_after(1), rt());

        let envelope = recv_envelope(&mut route_rx).await;
        assert!(
            envelope.reply_tx.is_some(),
            "consumer must be parked on the reply"
        );
        cancel.cancel();
        let joined = tokio::time::timeout(Duration::from_secs(5), handle).await;
        assert!(joined.is_ok(), "cancellation must wake the consumer");
        assert!(
            mock.completions
                .lock()
                .expect("completions lock")
                .is_empty(),
            "cancel before the decision must send no completion"
        );
    }

    /// Cancel delivered through the shared `ConsumerContext` token (distinct
    /// from the consumer token) must equally send no completion.
    #[tokio::test]
    async fn held_reply_context_cancel_sends_no_complete() {
        let mock = spawn_settlement_bridge(
            vec![delivered_message("tok-cancel-b", 32)],
            CompleteMode::Accept,
        )
        .await
        .expect("spawn settlement bridge");
        let pool = pool_with_mock_channel(mock.port).await;
        let ctx_token = CancellationToken::new();
        let (route_tx, mut route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(route_tx, ctx_token.clone(), "jms-cancel-b".to_string());
        let cancel = CancellationToken::new();
        let handle = spawn_jms_loop(&pool, &ctx, cancel, reconnect_after(1), rt());

        let envelope = recv_envelope(&mut route_rx).await;
        assert!(envelope.reply_tx.is_some());
        ctx_token.cancel();
        let joined = tokio::time::timeout(Duration::from_secs(5), handle).await;
        assert!(
            joined.is_ok(),
            "context cancellation must wake the consumer"
        );
        assert!(
            mock.completions
                .lock()
                .expect("completions lock")
                .is_empty(),
            "context cancel before the decision must send no completion"
        );
    }

    /// Task 4.4/4.5: a destination permanent failure (gRPC
    /// `FAILED_PRECONDITION`) must fail only the affected route in both
    /// delivery modes — an immediate subscribe failure and a successful
    /// subscribe followed by a stream error. For each mode: the consumer task
    /// returns `Err` through the supervised handle (never a panic), exactly one
    /// subscribe attempt is made, and the broker slot stays `Ready` for other
    /// routes and producers.
    #[tokio::test]
    async fn destination_failure_is_route_only() {
        for mode in [PermanentBridgeMode::Subscribe, PermanentBridgeMode::Stream] {
            let attempts: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
            let port = spawn_permanent_failure_bridge(mode, Arc::clone(&attempts))
                .await
                .expect("spawn permanent-failure bridge");
            let pool = pool_with_mock_channel(port).await;
            let (route_tx, _route_rx) = mpsc::channel(16);
            let ctx = ConsumerContext::new(
                route_tx,
                CancellationToken::new(),
                format!("jms-permanent-{mode:?}"),
            );
            let cancel = CancellationToken::new();
            // max_attempts=3: a transient failure would reconnect three times.
            let handle = spawn_jms_loop(&pool, &ctx, cancel.clone(), reconnect_after(3), rt());

            let joined = tokio::time::timeout(Duration::from_secs(5), handle)
                .await
                .unwrap_or_else(|_| {
                    panic!(
                        "{mode:?}: a permanent failure must terminate the route consumer, \
                         not reconnect"
                    )
                });
            // The task must surface the permanent failure as an inner `Err`; a
            // panic or abort would surface here as a JoinError and must be
            // distinguished from the expected route failure.
            let inner = joined
                .unwrap_or_else(|e| panic!("{mode:?}: the consumer task must not panic: {e}"));
            assert!(
                inner.is_err(),
                "{mode:?}: a permanent destination failure must surface Err, got Ok"
            );

            let seen = attempts.lock().expect("attempts lock").clone();
            assert_eq!(
                seen.len(),
                1,
                "{mode:?}: a permanent failure must not retry (route-only, no hot loop): {seen:?}"
            );

            // The slot is never parked or restarted: it stays Ready for other
            // routes and producers sharing it.
            let slot = pool
                .get_or_create_slot("default", false)
                .await
                .expect("slot");
            assert!(
                matches!(*slot.state_rx.borrow(), BridgeState::Ready { .. }),
                "{mode:?}: a destination permanent failure must leave the broker slot Ready"
            );

            cancel.cancel();
        }
    }

    /// Task 4.4: only gRPC `FAILED_PRECONDITION` is permanent. The classifier
    /// runs on the raw status before conversion erases the code; transport
    /// codes stay transient and reconnectable.
    #[test]
    fn permanent_status_classifier() {
        assert!(
            is_permanent_status(&Status::failed_precondition("corrupt journal")),
            "FAILED_PRECONDITION must classify as permanent"
        );
        assert!(
            !is_permanent_status(&Status::unavailable("transport down")),
            "UNAVAILABLE must stay transient and reconnectable"
        );
    }

    /// The full `JmsConsumer::start` path must deliver through `send_and_wait`
    /// and settle `Ok` as `COMMIT`.
    #[tokio::test]
    async fn full_consumer_start_settles_via_send_and_wait() {
        let mut mock = spawn_settlement_bridge(
            vec![delivered_message("tok-full", 41)],
            CompleteMode::Accept,
        )
        .await
        .expect("spawn settlement bridge");
        let pool = pool_with_mock_channel(mock.port).await;
        let (route_tx, mut route_rx) = mpsc::channel(16);
        let ctx_token = CancellationToken::new();
        let ctx = ConsumerContext::new(route_tx, ctx_token.clone(), "jms-full".to_string());
        let endpoint_cfg = JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut consumer = JmsConsumer::new(
            pool,
            "default".to_string(),
            endpoint_cfg,
            reconnect_after(1),
            rt(),
        );
        consumer.start(ctx).await.expect("consumer start");

        let envelope = recv_envelope(&mut route_rx).await;
        let reply_tx = envelope
            .reply_tx
            .expect("full consumer path must use send_and_wait");
        assert!(
            mock.completions
                .lock()
                .expect("completions lock")
                .is_empty(),
            "no completion may be sent while the reply is pending"
        );
        reply_tx
            .send(Ok(Exchange::new(Message::new(Body::Empty))))
            .expect("reply delivered");
        let record = recv_completion(&mut mock.completion_rx).await;
        assert_eq!(record.outcome, CompletionOutcome::Commit as i32);
        assert_eq!(record.subscription_id.len(), 36);

        consumer.stop().await.expect("stop");
    }

    // ── Task 4.5: all-task supervision ───────────────────────────────────────

    /// Task 4.5: with two concurrent consumers, one permanent failure must
    /// cancel its sibling (whose stream stays pending) and surface `Err`
    /// through the supervised background-task handle. If the sibling were not
    /// cancelled, the pending stream would keep the group alive until timeout.
    #[tokio::test]
    async fn any_task_failure_cancels_siblings_and_returns_err() {
        let subscribes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let port = spawn_first_permanent_bridge(Arc::clone(&subscribes))
            .await
            .expect("spawn first-permanent bridge");
        let pool = pool_with_mock_channel(port).await;
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "jms-supervised".to_string(),
        );
        let endpoint_cfg =
            JmsEndpointConfig::from_uri("jms:queue:test?concurrentConsumers=2").unwrap();
        let mut consumer = JmsConsumer::new(
            pool,
            "default".to_string(),
            endpoint_cfg,
            reconnect_after(3),
            rt(),
        );
        consumer.start(ctx).await.expect("consumer start");

        let handle = consumer
            .background_task_handle()
            .expect("supervised group handle");
        let joined = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("a permanent sibling failure must cancel the pending sibling, not hang");
        let inner = joined.expect("supervisor task must not panic");
        assert!(
            inner.is_err(),
            "the supervised group must surface the permanent failure as Err"
        );
        assert!(
            subscribes.load(std::sync::atomic::Ordering::SeqCst) >= 1,
            "the permanent subscribe must be attempted"
        );
    }

    /// Task 4.5: a panicking consumer task must be surfaced as `Err` and must
    /// cancel its siblings; it is never converted to a normal exit. The sibling
    /// task waits on the shared token, so resolving proves cancellation.
    #[tokio::test]
    async fn panic_is_surfaced() {
        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::Generic,
            ))
            .unwrap(),
        );
        let endpoint_cfg = JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut consumer = JmsConsumer::new(
            pool,
            "default".to_string(),
            endpoint_cfg,
            reconnect_after(1),
            rt(),
        );

        let shared = CancellationToken::new();
        consumer.cancel_token = Some(shared.clone());
        consumer.sibling_token = Some(shared.clone());
        let sibling = shared.clone();
        consumer.task_handles = vec![
            tokio::spawn(async { panic!("simulated consumer panic") }),
            tokio::spawn(async move {
                sibling.cancelled().await;
                Ok(())
            }),
        ];

        let handle = consumer
            .background_task_handle()
            .expect("supervised group handle");
        let inner = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("a panicking task must cancel its siblings, not hang")
            .expect("supervisor task must not panic");
        assert!(
            inner.is_err(),
            "a panicking consumer task must surface Err, not be swallowed"
        );
    }

    /// Task 4.5: reconnect exhaustion must surface `Err` through the supervised
    /// handle, not a silent normal exit.
    #[tokio::test]
    async fn reconnect_exhaustion_is_surfaced() {
        let recorded: Arc<Mutex<Vec<SubscribeRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let port = spawn_recording_bridge(Arc::clone(&recorded))
            .await
            .expect("spawn recording bridge");
        let pool = pool_with_mock_channel(port).await;
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "jms-exhausted".to_string(),
        );
        let endpoint_cfg = JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        // max_attempts=1: the empty stream ends and the reconnect budget is
        // spent immediately.
        let mut consumer = JmsConsumer::new(
            pool,
            "default".to_string(),
            endpoint_cfg,
            reconnect_after(1),
            rt(),
        );
        consumer.start(ctx).await.expect("consumer start");

        let handle = consumer
            .background_task_handle()
            .expect("supervised group handle");
        let inner = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("reconnect exhaustion must terminate, not hang")
            .expect("supervisor task must not panic");
        assert!(inner.is_err(), "reconnect exhaustion must surface Err");
    }

    /// Task 4.5: a requested cancellation is normal. Every sibling stops
    /// cooperatively and the supervised handle resolves to `Ok(())`.
    #[tokio::test]
    async fn requested_cancellation_is_normal() {
        // No prepared deliveries: both consumers park on a pending stream.
        let mut mock = spawn_settlement_bridge(Vec::new(), CompleteMode::Accept)
            .await
            .expect("spawn settlement bridge");
        let pool = pool_with_mock_channel(mock.port).await;
        let (route_tx, _route_rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();
        let ctx = ConsumerContext::new(route_tx, cancel.clone(), "jms-cancel-normal".to_string());
        let endpoint_cfg =
            JmsEndpointConfig::from_uri("jms:queue:test?concurrentConsumers=2").unwrap();
        let mut consumer = JmsConsumer::new(
            pool,
            "default".to_string(),
            endpoint_cfg,
            reconnect_after(1),
            rt(),
        );
        consumer.start(ctx).await.expect("consumer start");

        // Both concurrent consumers must be subscribed and parked before the
        // requested cancellation is delivered.
        let _first = recv_subscribe(&mut mock.subscribe_rx).await;
        let _second = recv_subscribe(&mut mock.subscribe_rx).await;

        let handle = consumer
            .background_task_handle()
            .expect("supervised group handle");
        cancel.cancel();
        let inner = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("requested cancellation must stop siblings promptly")
            .expect("supervisor task must not panic");
        assert!(
            inner.is_ok(),
            "a requested cancellation must resolve to Ok, not Err"
        );
    }

    /// Builds a consumer whose supervised workers are plain pending tasks, with
    /// the requested-shutdown and shared tokens wired exactly as `start()` wires
    /// them. The caller supplies the worker handles and retains their abort
    /// handles; the returned token is the route/context token.
    fn consumer_with_supervised_workers(
        handles: Vec<tokio::task::JoinHandle<Result<(), CamelError>>>,
    ) -> (
        JmsConsumer,
        CancellationToken,
        Vec<tokio::task::AbortHandle>,
    ) {
        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::Generic,
            ))
            .unwrap(),
        );
        let endpoint_cfg = JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut consumer = JmsConsumer::new(
            pool,
            "default".to_string(),
            endpoint_cfg,
            reconnect_after(1),
            rt(),
        );
        let route = CancellationToken::new();
        // Mirror `start()`: route -> requested-shutdown -> shared sibling.
        let requested = route.child_token();
        let shared = requested.child_token();
        consumer.cancel_token = Some(requested.clone());
        consumer.sibling_token = Some(shared.clone());
        consumer.requested_token = Some(requested);
        let aborts: Vec<tokio::task::AbortHandle> =
            handles.iter().map(|h| h.abort_handle()).collect();
        consumer.worker_aborts = aborts.clone();
        consumer.task_handles = handles;
        (consumer, route, aborts)
    }

    /// A worker that stores a oneshot sender; the sender drops only when the
    /// task future is dropped, so the paired receiver resolves exactly when the
    /// worker is cleaned up (aborted), never while it is detached.
    fn pending_worker() -> (
        tokio::task::JoinHandle<Result<(), CamelError>>,
        tokio::sync::oneshot::Receiver<()>,
    ) {
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(async move {
            let _keep = tx;
            std::future::pending::<()>().await;
            Ok(())
        });
        (handle, rx)
    }

    /// Task 4.5 (reviewer): a task aborted WITHOUT a requested cancellation is
    /// a failure and must surface `Err`; it is never mistaken for a normal
    /// exit. Classification happens before siblings are cancelled.
    #[tokio::test]
    async fn unexpected_abort_is_surfaced() {
        let worker = tokio::spawn(async {
            std::future::pending::<()>().await;
            Ok(())
        });
        let abort = worker.abort_handle();
        let (mut consumer, _route, _aborts) = consumer_with_supervised_workers(vec![worker]);
        let handle = consumer
            .background_task_handle()
            .expect("supervised group handle");

        // Abort without cancelling the route token: unexpected.
        abort.abort();
        let inner = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("an unexpected abort must terminate, not hang")
            .expect("supervisor task must not panic");
        assert!(
            inner.is_err(),
            "an unexpected abort without a requested cancellation must surface Err"
        );
    }

    /// Task 4.5 (reviewer): an abort delivered as part of a *requested*
    /// cancellation is normal and must surface `Ok`, not a failure.
    #[tokio::test]
    async fn requested_cancel_abort_is_normal() {
        let worker = tokio::spawn(async {
            std::future::pending::<()>().await;
            Ok(())
        });
        let abort = worker.abort_handle();
        let (mut consumer, route, _aborts) = consumer_with_supervised_workers(vec![worker]);
        let handle = consumer
            .background_task_handle()
            .expect("supervised group handle");

        route.cancel();
        abort.abort();
        let inner = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("a requested-cancel abort must terminate, not hang")
            .expect("supervisor task must not panic");
        assert!(
            inner.is_ok(),
            "an abort during a requested cancellation must be normal (Ok)"
        );
    }

    /// Task 4.5 (reviewer, Finding 1): runtime-style shutdown cancels the route
    /// token and aborts the supervised handle BEFORE `stop()`. Aborting the
    /// supervisor must abort its workers (RAII guard), never detach them, and
    /// `stop()` must still complete with retained abort authority.
    #[tokio::test]
    async fn supervisor_abort_does_not_detach_workers() {
        let workers: Vec<_> = (0..2).map(|_| pending_worker()).collect();
        let mut receivers = Vec::new();
        let mut handles = Vec::new();
        for (handle, rx) in workers {
            handles.push(handle);
            receivers.push(rx);
        }
        let (mut consumer, route, _aborts) = consumer_with_supervised_workers(handles);
        let handle = consumer
            .background_task_handle()
            .expect("supervised group handle");

        // Runtime-style shutdown: cancel then abort the supervisor.
        route.cancel();
        handle.abort();
        let _ = handle.await;

        // Before stop() runs, every worker must already be cleaned up (the
        // oneshot sender drops when the task is dropped). A detached worker
        // would keep its sender alive and time out here.
        let cleaned = tokio::time::timeout(Duration::from_secs(2), async {
            for rx in receivers {
                let _ = rx.await;
            }
        })
        .await;
        assert!(
            cleaned.is_ok(),
            "aborting the supervisor must abort its workers, not detach them"
        );

        // stop() must still succeed with the retained abort authority.
        consumer.stop().await.expect("stop after supervisor abort");
    }

    /// Task 4.5 (reviewer, direct-stop cross-path): an explicit `stop()` with
    /// NO prior context cancellation is a requested shutdown. After the
    /// supervised handle is taken, `stop()` must mark that shutdown and the
    /// handle must resolve to `Ok(())` — never an unexpected-abort `Err`. This
    /// drives the real `start()` path.
    #[tokio::test]
    async fn explicit_stop_is_requested_shutdown() {
        // No prepared deliveries: both consumers park on a pending stream.
        let mut mock = spawn_settlement_bridge(Vec::new(), CompleteMode::Accept)
            .await
            .expect("spawn settlement bridge");
        let pool = pool_with_mock_channel(mock.port).await;
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "jms-explicit-stop".to_string(),
        );
        let endpoint_cfg =
            JmsEndpointConfig::from_uri("jms:queue:test?concurrentConsumers=2").unwrap();
        let mut consumer = JmsConsumer::new(
            pool,
            "default".to_string(),
            endpoint_cfg,
            reconnect_after(1),
            rt(),
        );
        consumer.start(ctx).await.expect("consumer start");

        // Both concurrent consumers must be subscribed and parked first.
        let _first = recv_subscribe(&mut mock.subscribe_rx).await;
        let _second = recv_subscribe(&mut mock.subscribe_rx).await;

        let handle = consumer
            .background_task_handle()
            .expect("supervised group handle");

        // Explicit component stop WITHOUT cancelling the route/context token.
        consumer.stop().await.expect("explicit stop");

        let inner = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("explicit stop must terminate the group, not hang")
            .expect("supervisor task must not panic");
        assert!(
            inner.is_ok(),
            "an explicit component stop is a requested shutdown, not an unexpected abort"
        );
    }

    /// Task 4.5 (reviewer, Finding 1): a requested cancellation must interrupt
    /// a subscribe that never returns, rather than waiting for the bounded
    /// sibling drain. The worker is confirmed to be inside the hanging
    /// subscribe before the token is cancelled.
    #[tokio::test]
    async fn cancel_during_hanging_subscribe_is_prompt() {
        let (subscribed_tx, mut subscribed_rx) = mpsc::unbounded_channel();
        let port = spawn_hanging_subscribe_bridge(false, subscribed_tx)
            .await
            .expect("spawn hanging-subscribe bridge");
        let pool = pool_with_mock_channel(port).await;
        let (route_tx, _route_rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();
        let ctx = ConsumerContext::new(route_tx, cancel.clone(), "jms-hanging-sub".to_string());
        let endpoint_cfg = JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut consumer = JmsConsumer::new(
            pool,
            "default".to_string(),
            endpoint_cfg,
            reconnect_after(3),
            rt(),
        );
        consumer.start(ctx).await.expect("consumer start");
        let handle = consumer
            .background_task_handle()
            .expect("supervised group handle");

        // Wait until the worker is actually blocked in the hanging subscribe.
        tokio::time::timeout(Duration::from_secs(2), subscribed_rx.recv())
            .await
            .expect("subscribe must be entered within 2s")
            .expect("subscribe signal channel open");

        cancel.cancel();
        // Well under SIBLING_DRAIN_TIMEOUT (3s): only the cancellation race can
        // stop a subscribe that never returns.
        let inner = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("cancellation must interrupt the hanging subscribe, not wait for the drain")
            .expect("supervisor task must not panic");
        assert!(
            inner.is_ok(),
            "a requested cancellation during subscribe must be normal (Ok)"
        );
    }

    /// Task 4.5 (reviewer, Finding 1): one permanent subscribe failure must not
    /// be suppressed by a sibling stuck in a subscribe that never returns; the
    /// group resolves to `Err` within the bounded drain (aborting the straggler
    /// if necessary).
    #[tokio::test]
    async fn failure_with_pending_subscribe_is_bounded() {
        let (subscribed_tx, _subscribed_rx) = mpsc::unbounded_channel();
        let port = spawn_hanging_subscribe_bridge(true, subscribed_tx)
            .await
            .expect("spawn hanging-subscribe bridge");
        let pool = pool_with_mock_channel(port).await;
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "jms-pending-sub".to_string(),
        );
        let endpoint_cfg =
            JmsEndpointConfig::from_uri("jms:queue:test?concurrentConsumers=2").unwrap();
        let mut consumer = JmsConsumer::new(
            pool,
            "default".to_string(),
            endpoint_cfg,
            reconnect_after(3),
            rt(),
        );
        consumer.start(ctx).await.expect("consumer start");
        let handle = consumer
            .background_task_handle()
            .expect("supervised group handle");

        let inner = tokio::time::timeout(Duration::from_secs(6), handle)
            .await
            .expect("a sibling stuck in subscribe must not suppress the failure")
            .expect("supervisor task must not panic");
        assert!(
            inner.is_err(),
            "the permanent subscribe failure must be surfaced as Err"
        );
    }

    /// Task 4.5 (reviewer, Finding 1): a sibling that ignores the shared token
    /// (stuck) must not suppress the first failure; the bounded drain aborts
    /// the straggler and the group still resolves to `Err`.
    #[tokio::test]
    async fn failure_with_stuck_sibling_is_bounded() {
        let failing =
            tokio::spawn(async { Err(CamelError::ProcessorError("permanent".to_string())) });
        let stuck = tokio::spawn(async {
            std::future::pending::<()>().await;
            Ok(())
        });
        let (mut consumer, _route, _aborts) =
            consumer_with_supervised_workers(vec![failing, stuck]);
        let handle = consumer
            .background_task_handle()
            .expect("supervised group handle");

        let inner = tokio::time::timeout(Duration::from_secs(6), handle)
            .await
            .expect("a stuck sibling must not suppress the failure past the bounded drain")
            .expect("supervisor task must not panic");
        assert!(inner.is_err(), "the first failure must be preserved");
    }

    // ── Task 4.6: restart no hot-loop and transient reconnect ────────────────

    /// Task 4.6: a supervision restart against a still-broken journal must fail
    /// again via the pre-flight slot probe or a subscribe-time
    /// `FAILED_PRECONDITION`, without hot-looping.
    ///
    /// Phase A — a startup journal failure parks the slot; two restart attempts
    /// each fail at the pre-flight probe (no consumer is spawned).
    /// Phase B — a subscribe-time permanent failure leaves the slot `Ready`; two
    /// restart attempts each make exactly one subscribe and surface `Err`
    /// through the supervised handle, so the reconnect loop never fires.
    #[tokio::test]
    async fn restart_broken_journal_no_hot_loop() {
        let endpoint_cfg = JmsEndpointConfig::from_uri("jms:queue:test").unwrap();

        // Phase A: startup-broken journal → Parked slot → pre-flight fail-fast.
        let parked_pool = pool_with_parked_slot().await;
        for restart in 0..2u32 {
            let (route_tx, _route_rx) = mpsc::channel(16);
            let ctx = ConsumerContext::new(
                route_tx,
                CancellationToken::new(),
                format!("jms-parked-restart-{restart}"),
            );
            let mut consumer = JmsConsumer::new(
                Arc::clone(&parked_pool),
                "default".to_string(),
                endpoint_cfg.clone(),
                reconnect_after(3),
                rt(),
            );
            let err = tokio::time::timeout(Duration::from_secs(5), consumer.start(ctx))
                .await
                .expect(
                    "a restart against a parked slot must fail the pre-flight probe promptly, \
                     not hang",
                )
                .expect_err(
                    "a restart against a parked (broken journal) slot must fail at the pre-flight probe",
                );
            assert!(
                err.to_string().contains("parked"),
                "restart {restart}: the pre-flight error must name the parked slot: {err}"
            );
        }

        // Phase B: subscribe-time FAILED_PRECONDITION → Ready slot → each
        // restart subscribes exactly once, then fails the route.
        let attempts: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let port =
            spawn_permanent_failure_bridge(PermanentBridgeMode::Subscribe, Arc::clone(&attempts))
                .await
                .expect("spawn permanent-failure bridge");
        let ready_pool = pool_with_mock_channel(port).await;
        for restart in 0..2u32 {
            let (route_tx, _route_rx) = mpsc::channel(16);
            let ctx = ConsumerContext::new(
                route_tx,
                CancellationToken::new(),
                format!("jms-subscribe-restart-{restart}"),
            );
            let mut consumer = JmsConsumer::new(
                Arc::clone(&ready_pool),
                "default".to_string(),
                endpoint_cfg.clone(),
                reconnect_after(3),
                rt(),
            );
            // The pre-flight passes on a Ready slot; the restart fails at
            // subscribe time with FAILED_PRECONDITION.
            consumer
                .start(ctx)
                .await
                .expect("pre-flight must pass on a Ready slot");
            let handle = consumer
                .background_task_handle()
                .expect("supervised group handle");
            let joined = tokio::time::timeout(Duration::from_secs(5), handle)
                .await
                .expect("a restart against a broken journal must terminate, not hot-loop")
                .expect("supervisor task must not panic");
            assert!(
                joined.is_err(),
                "restart {restart} against a broken journal must fail the route"
            );
        }

        let seen = attempts.lock().expect("attempts lock").clone();
        assert_eq!(
            seen.len(),
            2,
            "two restarts must make exactly one permanent subscribe each, \
             never reconnect in a hot loop: {seen:?}"
        );

        let slot = ready_pool
            .get_or_create_slot("default", false)
            .await
            .expect("slot");
        assert!(
            matches!(*slot.state_rx.borrow(), BridgeState::Ready { .. }),
            "a subscribe-time permanent failure must leave the broker slot Ready"
        );
    }

    /// Task 4.6: a transient stream error must still take the bounded reconnect
    /// path. The first subscribe's stream errors with `UNAVAILABLE`; the
    /// consumer must reconnect and subscribe again instead of terminating after
    /// one attempt (which a permanent misclassification would do).
    #[tokio::test]
    async fn transient_error_still_reconnects() {
        let subscribes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (subscribed_tx, mut subscribed_rx) = mpsc::unbounded_channel();
        let port = spawn_transient_once_bridge(Arc::clone(&subscribes), subscribed_tx)
            .await
            .expect("spawn transient-once bridge");
        let pool = pool_with_mock_channel(port).await;
        let (route_tx, _route_rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();
        let ctx = ConsumerContext::new(route_tx, cancel.clone(), "jms-transient".to_string());
        let handle = spawn_jms_loop(&pool, &ctx, cancel.clone(), reconnect_after(3), rt());

        // First subscribe: the stream errors transiently. Second subscribe: the
        // consumer has reconnected and parks. Awaiting both barriers proves the
        // reconnect path ran, with no fixed sleep.
        tokio::time::timeout(Duration::from_secs(2), subscribed_rx.recv())
            .await
            .expect("first subscribe within 2s")
            .expect("subscribe signal channel open");
        tokio::time::timeout(Duration::from_secs(3), subscribed_rx.recv())
            .await
            .expect("a transient stream error must trigger a reconnect (second subscribe)")
            .expect("subscribe signal channel open");
        assert_eq!(
            subscribes.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "the transient error must reconnect exactly once, then park"
        );

        // A requested cancellation after the reconnect is normal.
        cancel.cancel();
        let inner = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("cancellation must stop the reconnected consumer")
            .expect("consumer task must not panic");
        assert!(
            inner.is_ok(),
            "a requested cancellation after reconnect must be normal (Ok)"
        );
    }

    // ── Phase 4 interphase findings: typed readiness + status-free logs ──────

    /// A tracing `Layer` that captures every event's formatted fields into a
    /// shared buffer (mirrors `camel-component-api`'s `CollectingLayer`). Lets a
    /// test assert on ACTUAL production log events, never just a formatter in
    /// isolation.
    struct CapturingLayer {
        events: Arc<Mutex<Vec<String>>>,
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CapturingLayer {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut buf = String::new();
            let mut visitor = CapturingVisitor { fields: &mut buf };
            event.record(&mut visitor);
            if let Ok(mut events) = self.events.lock() {
                events.push(buf);
            }
        }
    }

    struct CapturingVisitor<'a> {
        fields: &'a mut String,
    }

    impl CapturingVisitor<'_> {
        fn record_field(&mut self, name: &str, value: &str) {
            write!(self.fields, " {name}={value}").ok();
        }
    }

    impl tracing::field::Visit for CapturingVisitor<'_> {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.record_field(field.name(), value);
        }
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.record_field(field.name(), &format!("{value:?}"));
        }
        fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
            self.record_field(field.name(), &value.to_string());
        }
    }

    /// Install a bare registry as the process-global tracing default, once per
    /// test binary. Guards the capture test against callsite-interest poisoning:
    /// `tracing` caches each callsite's `Interest` from its first execution, so
    /// a plain test reaching the shared permanent-failure `warn!` callsite first
    /// would otherwise silence the capture layer (bd rc-zushg).
    fn ensure_global_tracing_default() {
        static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        if INIT.set(()).is_ok() {
            let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
        }
    }

    async fn recv_subscribed(rx: &mut mpsc::UnboundedReceiver<()>) {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("subscribe signal within 5s")
            .expect("subscribe signal channel open");
    }

    /// Finding-1 mock: `subscribe` #1 signals, then its stream's first item (a
    /// transient `UNAVAILABLE`) is gated on `release`. The test parks the slot
    /// while the consumer is still inside subscribe #1, so the reconnect that
    /// follows observes the terminal slot deterministically (no sleep race).
    /// Any second subscribe hands back a pending stream: if the terminal-slot
    /// bug regressed, the group would hang instead of resolving `Err`.
    #[derive(Clone)]
    struct ParkDuringReconnectBridge {
        subscribes: Arc<std::sync::atomic::AtomicUsize>,
        subscribed: mpsc::UnboundedSender<()>,
        release: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl BridgeService for ParkDuringReconnectBridge {
        async fn send(
            &self,
            _request: Request<SendRequest>,
        ) -> Result<Response<SendResponse>, Status> {
            Err(Status::unimplemented("send not exercised here"))
        }

        type SubscribeStream = Pin<Box<dyn Stream<Item = Result<JmsMessage, Status>> + Send>>;

        async fn subscribe(
            &self,
            _request: Request<SubscribeRequest>,
        ) -> Result<Response<Self::SubscribeStream>, Status> {
            let n = self
                .subscribes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _ = self.subscribed.send(());
            if n == 0 {
                let release = Arc::clone(&self.release);
                let stream = futures::stream::once(async move {
                    release.notified().await;
                    Err::<JmsMessage, Status>(Status::unavailable("transient transport failure"))
                });
                Ok(Response::new(Box::pin(stream)))
            } else {
                Ok(Response::new(Box::pin(futures::stream::pending())))
            }
        }

        async fn health(
            &self,
            _request: Request<HealthRequest>,
        ) -> Result<Response<HealthResponse>, Status> {
            // Legacy bridge answer: no explicit status, UNSPECIFIED with zero age.
            Ok(Response::new(HealthResponse {
                healthy: true,
                broker_connected: true,
                message: "ok".to_string(),
                status: 0,
                age_ms: 0,
                ..Default::default()
            }))
        }

        async fn complete(
            &self,
            _request: Request<CompletionRequest>,
        ) -> Result<Response<CompletionResponse>, Status> {
            Ok(Response::new(CompletionResponse { accepted: true }))
        }
    }

    async fn spawn_park_during_reconnect_bridge(
        subscribes: Arc<std::sync::atomic::AtomicUsize>,
        subscribed: mpsc::UnboundedSender<()>,
        release: Arc<tokio::sync::Notify>,
    ) -> std::io::Result<u16> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(BridgeServiceServer::new(ParkDuringReconnectBridge {
                    subscribes,
                    subscribed,
                    release,
                }))
                .serve_with_incoming(incoming)
                .await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        Ok(port)
    }

    /// Phase 4 interphase Finding 1 regression: a slot that becomes terminal
    /// (`Parked`) DURING a reconnect must fail the route immediately, even under
    /// the DEFAULT unlimited reconnect budget (`max_attempts = 0`). The old code
    /// treated every `await_ready_channel` error as transient and retried
    /// forever. Drives the real loop plus the supervised group, so the permanent
    /// readiness outcome must surface through the handle with the operator
    /// repair guidance and no spurious reconnect.
    #[tokio::test]
    async fn parked_during_reconnect_fails_route_without_retry() {
        let subscribes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (subscribed_tx, mut subscribed_rx) = mpsc::unbounded_channel();
        let release = Arc::new(tokio::sync::Notify::new());
        let port = spawn_park_during_reconnect_bridge(
            Arc::clone(&subscribes),
            subscribed_tx,
            Arc::clone(&release),
        )
        .await
        .expect("spawn park-during-reconnect bridge");
        let pool = pool_with_mock_channel(port).await;
        let slot = pool
            .get_or_create_slot("default", false)
            .await
            .expect("slot");
        let state_tx = slot.state_tx.clone();

        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "jms-park-during-reconnect".to_string(),
        );
        let endpoint_cfg = JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        // DEFAULT unlimited budget (`max_attempts = 0`); only the backoff is
        // shortened so the regression is fast. The property under test is the
        // unlimited retry budget, not the 5s default delay.
        let reconnect = NetworkRetryPolicy {
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
            multiplier: 1.0,
            ..jms_reconnect_default()
        };
        assert_eq!(
            reconnect.max_attempts, 0,
            "regression must run under the default unlimited reconnect budget"
        );

        let mut consumer = JmsConsumer::new(
            pool.clone(),
            "default".to_string(),
            endpoint_cfg,
            reconnect,
            rt(),
        );
        consumer
            .start(ctx)
            .await
            .expect("pre-flight passes on a Ready slot");
        let handle = consumer
            .background_task_handle()
            .expect("supervised group handle");

        // The consumer is inside the first subscribe; its stream is gated.
        recv_subscribed(&mut subscribed_rx).await;
        // Park the slot DURING the reconnect window, then release the transient
        // stream error so the loop runs its reconnect against the terminal slot.
        state_tx
            .send(BridgeState::Parked("corrupt journal".to_string()))
            .expect("publish Parked");
        release.notify_one();

        let inner = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("a terminal slot during reconnect must terminate the route, not retry forever")
            .expect("supervisor task must not panic");
        let err = inner
            .expect_err("a Parked slot during reconnect must surface Err")
            .to_string();
        assert!(
            err.contains("repair") && err.contains("restart the context"),
            "the route error must carry the operator repair guidance: {err}"
        );
        assert_eq!(
            subscribes.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a terminal Parked slot must not trigger a second subscribe (no spurious reconnect)"
        );
        assert!(
            matches!(*slot.state_rx.borrow(), BridgeState::Parked(_)),
            "the terminal slot must stay Parked (no spurious restart)"
        );
    }

    /// Phase 4 interphase Finding 2 regression: the permanent-failure paths must
    /// not echo the remote gRPC status message (arbitrary remote bytes) into the
    /// operator log or the returned error. Captures the ACTUAL tracing events
    /// from the real production loop for BOTH the subscribe-time and stream-time
    /// `FAILED_PRECONDITION`, with a sentinel embedded in the remote status
    /// message.
    #[tokio::test]
    async fn permanent_failure_logs_and_error_omit_remote_status_detail() {
        ensure_global_tracing_default();
        const SENTINEL: &str = "SECRET-REMOTE-STATUS-DETAIL";

        for mode in [PermanentBridgeMode::Subscribe, PermanentBridgeMode::Stream] {
            let events = Arc::new(Mutex::new(Vec::new()));
            let subscriber = tracing_subscriber::registry().with(CapturingLayer {
                events: Arc::clone(&events),
            });
            let _guard = tracing::subscriber::set_default(subscriber);

            let attempts: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
            let port = spawn_permanent_failure_bridge_with_message(
                mode,
                Arc::clone(&attempts),
                format!("corrupt journal: replay failed for header {SENTINEL} (permanent)"),
            )
            .await
            .expect("spawn permanent-failure bridge");
            let pool = pool_with_mock_channel(port).await;
            let (route_tx, _route_rx) = mpsc::channel(16);
            let ctx = ConsumerContext::new(
                route_tx,
                CancellationToken::new(),
                format!("jms-permanent-log-{mode:?}"),
            );
            let cancel = CancellationToken::new();
            let handle = spawn_jms_loop(&pool, &ctx, cancel.clone(), reconnect_after(3), rt());

            let inner = tokio::time::timeout(Duration::from_secs(5), handle)
                .await
                .expect("a permanent failure must terminate the route")
                .expect("consumer task must not panic");
            let err = inner
                .expect_err("a permanent failure must surface Err")
                .to_string();

            let captured = events.lock().expect("events lock").join("\n");
            assert!(
                captured.contains("permanent failure"),
                "{mode:?}: the permanent failure must be logged: {captured}"
            );
            assert!(
                captured.contains("grpc_code=FailedPrecondition"),
                "{mode:?}: the log must carry only the stable gRPC code: {captured}"
            );
            assert!(
                !captured.contains(SENTINEL),
                "{mode:?}: the log must not echo the remote status detail: {captured}"
            );
            assert!(
                !err.contains(SENTINEL),
                "{mode:?}: the returned error must not echo the remote status detail: {err}"
            );
            assert!(
                err.contains("repair") && err.contains("restart the context"),
                "{mode:?}: the returned error must name the operator repair: {err}"
            );
            cancel.cancel();
        }
    }

    /// Task 2.5: the subscribe-retry refresh itself fails against the real
    /// installed dead process endpoint, then wakes the real monitor within
    /// 500 ms of that observation; the slot stays Ready and cancellation
    /// remains bounded.
    #[cfg(unix)]
    #[tokio::test]
    async fn consumer_subscribe_retry_notifies_probe_wake() {
        let dir = tempfile::tempdir().expect("stub tempdir");
        let dead_endpoint = 2;
        let process = ready_stub_process(dir.path(), "consumer-dead", dead_endpoint).await;

        let mut bridge = spawn_always_unavailable_subscribe_bridge().await;
        let channel = crate::bridge_client_test::connect_scripted(bridge.port).await;
        let pool = consumer_liveness_pool(60_000);
        let slot = insert_ready_process_slot(&pool, channel, process).await;
        pool.spawn_health_monitor(Arc::clone(&slot)).await;

        let (route_tx, _route_rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();
        let ctx = ConsumerContext::new(route_tx, cancel.clone(), "jms-consumer-wake".to_string());
        let handle = spawn_jms_loop(&pool, &ctx, cancel.clone(), reconnect_after(10), rt());

        bridge.next_subscribe().await;
        bridge.next_subscribe().await;
        assert!(bridge.subscribes.load(std::sync::atomic::Ordering::SeqCst) >= 2);

        // The actual failed refresh is observed before the wake deadline
        // starts; the refresh observation is bounded separately by three
        // seconds.
        let attempt = next_refresh_attempt(&pool, 0).await;
        assert_eq!(attempt.endpoint, Some(dead_endpoint), "{attempt:?}");
        assert!(attempt.failed, "{attempt:?}");
        assert!(!attempt.timed_out, "{attempt:?}");

        tokio::time::timeout(Duration::from_millis(500), bridge.next_health())
            .await
            .expect("the failed refresh must wake the monitor within 500 ms");
        assert!(
            bridge
                .health_calls
                .load(std::sync::atomic::Ordering::SeqCst)
                >= 1
        );
        assert!(
            matches!(*slot.state_rx.borrow(), BridgeState::Ready { .. }),
            "a liveness wake must preserve Ready"
        );
        assert_eq!(replacement_attempts(&pool), 0);

        cancel.cancel();
        let inner = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("cancellation must complete within 5s")
            .expect("consumer task must not panic");
        assert!(inner.is_ok(), "requested cancellation is normal: {inner:?}");

        bridge.shutdown().await;
        stop_test_monitor(&slot).await;
        stop_slot_process(&slot).await;
    }

    /// Task 2.5: gated consumer reconnects produce two separated refresh
    /// failures. The second observation names the second real installed dead
    /// process endpoint, waking the monitor within 500 ms while the slot stays
    /// Ready.
    #[cfg(unix)]
    #[tokio::test]
    async fn consumer_channel_refresh_retry_notifies_probe_wake() {
        let dir = tempfile::tempdir().expect("stub tempdir");
        let first_endpoint = 2;
        let second_endpoint = 3;
        let first_process = ready_stub_process(dir.path(), "consumer-first", first_endpoint).await;
        let second_process =
            ready_stub_process(dir.path(), "consumer-second", second_endpoint).await;

        let (mut bridge, first_gate, second_gate) = spawn_gated_transport_failures_bridge().await;
        let channel = crate::bridge_client_test::connect_scripted(bridge.port).await;
        let pool = consumer_liveness_pool(60_000);
        let slot = insert_ready_process_slot(&pool, channel, first_process).await;
        pool.spawn_health_monitor(Arc::clone(&slot)).await;

        let (route_tx, _route_rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();
        let ctx = ConsumerContext::new(
            route_tx,
            cancel.clone(),
            "jms-consumer-refresh-wake".to_string(),
        );
        let handle = spawn_jms_loop(&pool, &ctx, cancel.clone(), reconnect_after(10), rt());

        // Subscribe 0 is the immediate stream error; subscribe 1 is the first
        // gated failure that reaches the threshold.
        bridge.next_subscribe().await;
        bridge.next_subscribe().await;
        first_gate.notify_one();

        let first_attempt = next_refresh_attempt(&pool, 0).await;
        assert_eq!(
            first_attempt.endpoint,
            Some(first_endpoint),
            "{first_attempt:?}"
        );
        assert!(first_attempt.failed, "{first_attempt:?}");
        assert!(!first_attempt.timed_out, "{first_attempt:?}");
        tokio::time::timeout(Duration::from_millis(500), bridge.next_health())
            .await
            .expect("the first failed refresh must wake the monitor");

        // Install the second real dead target before releasing the next retry.
        let old = tokio::time::timeout(Duration::from_secs(2), slot.process.lock())
            .await
            .expect("process lock within 2s")
            .replace(second_process);
        if let Some(old) = old {
            let _ = tokio::time::timeout(Duration::from_secs(2), old.stop()).await;
        }
        second_gate.notify_one();

        let second_attempt = next_refresh_attempt(&pool, 1).await;
        assert_eq!(
            second_attempt.endpoint,
            Some(second_endpoint),
            "the second refresh must name the second installed endpoint: {second_attempt:?}"
        );
        assert!(second_attempt.failed, "{second_attempt:?}");
        assert!(!second_attempt.timed_out, "{second_attempt:?}");

        tokio::time::timeout(Duration::from_millis(500), bridge.next_health())
            .await
            .expect("the second failed refresh must wake the monitor within 500 ms");
        assert!(
            matches!(*slot.state_rx.borrow(), BridgeState::Ready { .. }),
            "a liveness wake must preserve Ready"
        );
        assert_eq!(replacement_attempts(&pool), 0);

        cancel.cancel();
        let inner = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("cancellation must complete within 5s")
            .expect("consumer task must not panic");
        assert!(inner.is_ok(), "requested cancellation is normal: {inner:?}");

        bridge.shutdown().await;
        stop_test_monitor(&slot).await;
        stop_slot_process(&slot).await;
    }

    /// Task 2.5: the shared stream-site recovery action is exercised directly
    /// (the natural stream-error threshold branch is unreachable by design).
    /// Its real refresh fails against the installed dead process endpoint and
    /// wakes the monitor within 500 ms of the recorded failure.
    #[cfg(unix)]
    #[tokio::test]
    async fn stream_site_recovery_helper_notifies_probe_wake() {
        let dir = tempfile::tempdir().expect("stub tempdir");
        let dead_endpoint = 4;
        let process = ready_stub_process(dir.path(), "helper-dead", dead_endpoint).await;
        let server = spawn_scripted_health(Vec::new()).await;
        let pool = consumer_liveness_pool(60_000);
        let slot = test_slot_consumer("default");
        pool.slots.insert("default".to_string(), Arc::clone(&slot));
        let mut fx =
            TestBridgeFixture::start(Arc::clone(&pool), Arc::clone(&slot), server, Some(process))
                .await;

        let (route_tx, _route_rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();
        let ctx = ConsumerContext::new(route_tx, cancel.clone(), "jms-stream-helper".to_string());

        let outcome = tokio::time::timeout(
            Duration::from_secs(3),
            refresh_channel_or_request_probe(&pool, "default", "queue:test", 0, &cancel, &ctx),
        )
        .await
        .expect("the recovery helper must return within three seconds");
        assert_eq!(outcome, RecoveryOutcome::Continue);

        let attempt = next_refresh_attempt(&pool, 0).await;
        assert_eq!(attempt.endpoint, Some(dead_endpoint), "{attempt:?}");
        assert!(attempt.failed, "{attempt:?}");
        assert!(!attempt.timed_out, "{attempt:?}");

        let ack = tokio::time::timeout(Duration::from_millis(500), fx.next_probe())
            .await
            .expect("the helper must wake the monitor within 500 ms of the failure");
        assert!(matches!(ack.outcome, ProbeOutcome::Success), "{ack:?}");
        assert!(matches!(ack.state, BridgeState::Ready { .. }), "{ack:?}");
        assert_eq!(ack.attempts, 0, "{ack:?}");
        assert_eq!(replacement_attempts(&pool), 0);

        fx.shutdown().await;
    }

    /// Task 2.5: cancellation wins over an endpoint-selected connector future,
    /// returns `Cancelled`, records no refresh attempt, requests no probe or
    /// replacement, and drops the connector future.
    #[cfg(unix)]
    #[tokio::test]
    async fn stream_site_recovery_helper_returns_cancelled() {
        use std::sync::atomic::{AtomicBool, Ordering};

        struct DropSignal(Arc<AtomicBool>);
        impl Drop for DropSignal {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let dir = tempfile::tempdir().expect("stub tempdir");
        let process = ready_stub_process(dir.path(), "helper-cancel", 5).await;
        let server = spawn_scripted_health(Vec::new()).await;
        let pool = consumer_liveness_pool(60_000);
        let slot = test_slot_consumer("default");
        pool.slots.insert("default".to_string(), Arc::clone(&slot));
        let mut fx =
            TestBridgeFixture::start(Arc::clone(&pool), Arc::clone(&slot), server, Some(process))
                .await;

        let entered = Arc::new(tokio::sync::Notify::new());
        let dropped = Arc::new(AtomicBool::new(false));
        pool.set_refresh_connector_override({
            let entered = Arc::clone(&entered);
            let dropped = Arc::clone(&dropped);
            move |_endpoint| {
                let entered = Arc::clone(&entered);
                let dropped = Arc::clone(&dropped);
                Box::pin(async move {
                    let _drop_signal = DropSignal(dropped);
                    entered.notify_one();
                    futures::future::pending::<
                        Result<tonic::transport::Channel, camel_bridge::process::BridgeError>,
                    >()
                    .await
                })
                    as Pin<
                        Box<
                            dyn std::future::Future<
                                    Output = Result<
                                        tonic::transport::Channel,
                                        camel_bridge::process::BridgeError,
                                    >,
                                > + Send,
                        >,
                    >
            }
        });

        let (route_tx, _route_rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();
        let ctx = ConsumerContext::new(route_tx, cancel.clone(), "jms-stream-cancel".to_string());
        let helper = tokio::spawn({
            let pool = Arc::clone(&pool);
            let cancel = cancel.clone();
            let ctx = ctx.clone();
            async move {
                refresh_channel_or_request_probe(&pool, "default", "queue:test", 0, &cancel, &ctx)
                    .await
            }
        });

        tokio::time::timeout(Duration::from_secs(5), entered.notified())
            .await
            .expect("the connector future must start after endpoint selection");
        cancel.cancel();

        let outcome = tokio::time::timeout(Duration::from_secs(5), helper)
            .await
            .expect("cancellation must return within five seconds")
            .expect("helper task must not panic");
        assert_eq!(outcome, RecoveryOutcome::Cancelled);
        assert!(
            dropped.load(Ordering::SeqCst),
            "cancellation must drop the selected connector future"
        );
        assert!(
            refresh_attempts(&pool).is_empty(),
            "a cancelled refresh records no completion"
        );
        assert_eq!(replacement_attempts(&pool), 0);
        assert!(
            tokio::time::timeout(Duration::from_millis(300), fx.next_probe())
                .await
                .is_err(),
            "cancellation must not request a probe"
        );
        assert!(
            matches!(*slot.state_rx.borrow(), BridgeState::Ready { .. }),
            "cancellation must preserve Ready"
        );

        fx.shutdown().await;
    }

    /// Task 2.5: the single absolute production deadline covers process-lock
    /// acquisition plus the whole selected connector future. Progress every
    /// 25 ms never renews it; the timeout observation names the real process
    /// endpoint and the helper wakes the live monitor without any restart.
    #[cfg(unix)]
    #[tokio::test]
    async fn slow_progressing_connector_has_fixed_budget_and_never_restarts_live_bridge() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        struct DropSignal(Arc<AtomicBool>);
        impl Drop for DropSignal {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let dir = tempfile::tempdir().expect("stub tempdir");
        let selected_endpoint = 6;
        let process = ready_stub_process(dir.path(), "helper-slow", selected_endpoint).await;
        let server = spawn_scripted_health(Vec::new()).await;
        let pool = consumer_liveness_pool(60_000);
        let slot = test_slot_consumer("default");
        pool.slots.insert("default".to_string(), Arc::clone(&slot));
        let mut fx =
            TestBridgeFixture::start(Arc::clone(&pool), Arc::clone(&slot), server, Some(process))
                .await;

        let progress = Arc::new(AtomicUsize::new(0));
        let completed = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        pool.set_refresh_connector_override({
            let progress = Arc::clone(&progress);
            let completed = Arc::clone(&completed);
            let dropped = Arc::clone(&dropped);
            move |_endpoint| {
                let progress = Arc::clone(&progress);
                let completed = Arc::clone(&completed);
                let dropped = Arc::clone(&dropped);
                Box::pin(async move {
                    let _drop_signal = DropSignal(dropped);
                    // The progress loop lives inside the connector future, not
                    // in the test body. It is finite (5 s) and must be
                    // cancelled by the production two-second deadline long
                    // before it can finish.
                    let mut ticker = tokio::time::interval(Duration::from_millis(25));
                    for _ in 0..200 {
                        ticker.tick().await;
                        progress.fetch_add(1, Ordering::SeqCst);
                    }
                    completed.store(true, Ordering::SeqCst);
                    Err(camel_bridge::process::BridgeError::Transport(
                        "slow connector finished".to_string(),
                    ))
                })
                    as Pin<
                        Box<
                            dyn std::future::Future<
                                    Output = Result<
                                        tonic::transport::Channel,
                                        camel_bridge::process::BridgeError,
                                    >,
                                > + Send,
                        >,
                    >
            }
        });

        let (route_tx, _route_rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();
        let ctx = ConsumerContext::new(route_tx, cancel.clone(), "jms-stream-slow".to_string());

        let start = std::time::Instant::now();
        let outcome = tokio::time::timeout(
            Duration::from_secs(3),
            refresh_channel_or_request_probe(&pool, "default", "queue:test", 0, &cancel, &ctx),
        )
        .await
        .expect("the helper must return within the two-second deadline plus one second margin");
        let elapsed = start.elapsed();
        assert_eq!(outcome, RecoveryOutcome::Continue);
        assert!(
            elapsed >= Duration::from_secs(2),
            "the production deadline must not be shortened: {elapsed:?}"
        );
        assert!(
            elapsed <= Duration::from_secs(3),
            "one recovery operation must stay bounded: {elapsed:?}"
        );
        assert!(
            progress.load(Ordering::SeqCst) >= 10,
            "the connector must have signalled repeated progress"
        );
        assert!(
            !completed.load(Ordering::SeqCst),
            "the five-second connector loop must be cancelled, not completed"
        );
        assert!(
            dropped.load(Ordering::SeqCst),
            "timeout must drop the connector future"
        );

        let attempt = next_refresh_attempt(&pool, 0).await;
        assert_eq!(attempt.endpoint, Some(selected_endpoint), "{attempt:?}");
        assert!(attempt.failed, "{attempt:?}");
        assert!(attempt.timed_out, "{attempt:?}");

        let ack = tokio::time::timeout(Duration::from_millis(500), fx.next_probe())
            .await
            .expect("the timeout must wake the live monitor within 500 ms");
        assert!(matches!(ack.outcome, ProbeOutcome::Success), "{ack:?}");
        assert!(matches!(ack.state, BridgeState::Ready { .. }), "{ack:?}");
        assert_eq!(ack.attempts, 0, "{ack:?}");

        // Control: a connector that completes within the budget succeeds.
        let ready_channel = match &*slot.state_rx.borrow() {
            BridgeState::Ready { channel } => channel.clone(),
            other => panic!("slot must stay Ready: {other:?}"),
        };
        pool.set_refresh_connector_override(move |_endpoint| {
            let channel = ready_channel.clone();
            Box::pin(async move { Ok(channel) })
                as Pin<
                    Box<
                        dyn std::future::Future<
                                Output = Result<
                                    tonic::transport::Channel,
                                    camel_bridge::process::BridgeError,
                                >,
                            > + Send,
                    >,
                >
        });
        let outcome = tokio::time::timeout(
            Duration::from_secs(1),
            refresh_channel_or_request_probe(&pool, "default", "queue:test", 1, &cancel, &ctx),
        )
        .await
        .expect("a promptly completing connector must finish within the budget");
        assert_eq!(outcome, RecoveryOutcome::Continue);

        let control = next_refresh_attempt(&pool, 1).await;
        assert_eq!(control.endpoint, Some(selected_endpoint), "{control:?}");
        assert!(
            !control.failed,
            "the control refresh must succeed: {control:?}"
        );
        assert!(!control.timed_out, "{control:?}");
        assert!(
            matches!(*slot.state_rx.borrow(), BridgeState::Ready { .. }),
            "the control refresh must keep Ready"
        );
        assert_eq!(replacement_attempts(&pool), 0);

        fx.shutdown().await;
    }
}
