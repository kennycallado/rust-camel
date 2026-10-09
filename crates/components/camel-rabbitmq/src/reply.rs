//! Request/reply correlation for the RabbitMQ InOut producer half (task 4.1).
//!
//! RabbitMQ direct reply-to requires the request PUBLISH and the
//! `amq.rabbitmq.reply-to` CONSUME to share one channel (RabbitMQ 3.13
//! "Caveats and Limitations"). One [`ReplySession`] therefore pairs a single
//! confirm-enabled channel that both consumes replies (no-ack) and publishes
//! requests with a transport-free [`ReplyState`] that owns the
//! [`CorrelationTable`] mapping each request's UUID onto a oneshot sender
//! resolved by the reply loop.
//!
//! The reply loop captures only an `Arc<CorrelationTable>` and an
//! `Arc<AtomicBool>` (never the state), so a retired state cannot form a
//! reference cycle and the loop is reaped by [`ReplyState`]'s `Drop` /
//! retirement.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bytes::Bytes;
use dashmap::DashMap;
use futures::StreamExt;
use lapin::options::BasicConsumeOptions;
use lapin::types::{FieldTable, ShortString};
use tokio::sync::{Mutex, OwnedMutexGuard, oneshot};
use tokio::task::AbortHandle;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use camel_component_api::CamelError;

use crate::connection::{PUBLISH_DISCONNECTED_BOUND, RabbitConnectionManager};
use crate::error::RabbitError;
use crate::producer::{CHANNEL_CLOSE_BOUND, PRODUCER_OPEN_BOUND, prepare_channel};

/// The broker's direct reply-to pseudo-queue name.
pub(crate) const DIRECT_REPLY_TO: &str = "amq.rabbitmq.reply-to";

/// One direct reply-to response, decoupled from lapin.
pub(crate) struct ReplyPayload {
    /// The reply body bytes.
    pub body: Bytes,
    /// The reply's inbound header mapping (`headers::inbound`), carrying the
    /// delivery's actual `redelivered` flag (a direct reply-to delivery is
    /// normally first-delivery, but the flag is mapped as received).
    pub headers: camel_api::Headers,
}

/// Correlates an outstanding InOut request id with its reply sender.
#[derive(Default)]
pub(crate) struct CorrelationTable {
    inner: DashMap<String, oneshot::Sender<ReplyPayload>>,
}

impl CorrelationTable {
    /// Register `correlation_id` BEFORE publishing, returning the receiver.
    pub(crate) fn register(&self, correlation_id: &str) -> oneshot::Receiver<ReplyPayload> {
        let (sender, receiver) = oneshot::channel();
        self.inner.insert(correlation_id.to_string(), sender);
        receiver
    }

    /// Resolve `correlation_id` with `payload`; `false` means late/dropped.
    ///
    /// The entry is removed atomically, so a reply can resolve at most once and
    /// a late reply after timeout/cancellation finds nothing.
    pub(crate) fn resolve(&self, correlation_id: &str, payload: ReplyPayload) -> bool {
        match self.inner.remove(correlation_id) {
            Some((_, sender)) => sender.send(payload).is_ok(),
            None => false,
        }
    }

    /// Remove `correlation_id`; `true` when an entry was present.
    pub(crate) fn remove(&self, correlation_id: &str) -> bool {
        self.inner.remove(correlation_id).is_some()
    }

    /// Number of outstanding correlations (in-crate test accessor).
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.inner.len()
    }

    /// Drop every sender, closing all outstanding receivers.
    fn clear(&self) {
        self.inner.clear();
    }
}

/// Outcome of the InOut admission step: mandatory-lane acquisition plus
/// correlation registration.
pub(crate) enum Admission {
    /// The lane (when mandatory) was acquired and the correlation registered.
    Ready {
        /// Held mandatory lane guard, released before the reply wait.
        lane: Option<OwnedMutexGuard<()>>,
        /// Receiver for the correlated reply.
        rx: oneshot::Receiver<ReplyPayload>,
    },
    /// The mandatory lane was not free within `confirmTimeout`.
    LaneTimeout,
    /// The state was retired before the entry landed; the entry was removed.
    Retired,
}

/// Acquire the mandatory lane first, then register the correlation.
///
/// Ordering is the leak guard: a caller cancelled while waiting for the lane
/// (bounded by `bound`) has not registered anything, so no entry can leak.
/// Registration, the retirement re-check, and the caller's `ConfirmGuard` arm
/// happen with no `.await` in between. A retired state is signalled early with
/// [`Admission::Retired`] so a reply wait never starts on a dead state.
pub(crate) async fn admit(
    table: &CorrelationTable,
    retired: &AtomicBool,
    lane: Option<&Arc<Mutex<()>>>,
    id: &str,
    bound: Duration,
) -> Admission {
    let lane = match lane {
        Some(lane) => match tokio::time::timeout(bound, Arc::clone(lane).lock_owned()).await {
            Ok(guard) => Some(guard),
            Err(_) => return Admission::LaneTimeout,
        },
        None => None,
    };

    // Synchronous from here: no `.await` between registration, the retirement
    // re-check, and the caller arming its guard.
    let rx = table.register(id);
    if retired.load(Ordering::Acquire) {
        table.remove(id);
        return Admission::Retired;
    }
    Admission::Ready { lane, rx }
}

/// Remove a correlation entry when dropped unless disarmed.
///
/// `await_reply` succeeds only because `resolve` already removed the entry; on
/// timeout, receiver closure, or caller cancellation (the future is dropped)
/// this guard removes it, so a caller-cancelled InOut never leaks an entry.
struct RemoveOnDrop<'a> {
    table: &'a CorrelationTable,
    id: &'a str,
    armed: bool,
}

impl RemoveOnDrop<'_> {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RemoveOnDrop<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.table.remove(self.id);
        }
    }
}

/// Await one reply bounded by `timeout`, removing the entry on the way out.
///
/// A timed-out reply maps to [`RabbitError::ReplyTimeout`]; a receiver closed
/// because the reply state was drained maps to [`RabbitError::Disconnected`]
/// (never `ReplyTimeout`, so a channel death is not mistaken for a slow reply).
pub(crate) async fn await_reply(
    table: &CorrelationTable,
    id: &str,
    rx: oneshot::Receiver<ReplyPayload>,
    timeout: Duration,
) -> Result<ReplyPayload, RabbitError> {
    let mut guard = RemoveOnDrop {
        table,
        id,
        armed: true,
    };
    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(payload)) => {
            guard.disarm();
            Ok(payload)
        }
        // The sender was dropped: the reply state was retired or drained.
        Ok(Err(_closed)) => Err(RabbitError::Disconnected),
        Err(_elapsed) => Err(RabbitError::ReplyTimeout {
            timeout_ms: timeout.as_millis() as u64,
        }),
    }
}

/// The logical, transport-free core of one producer's InOut reply session.
///
/// It owns the [`CorrelationTable`], the mandatory-publish lane, the retirement
/// flag, and the reply loop's cancel token and abort handle. `publish_lane`
/// serializes mandatory publishes only (lapin's `basic.return` is FIFO per
/// confirmation with no per-publish correlation, so concurrent mandatory
/// publishes on one channel could swap a return); it is held from
/// `basic_publish` through the confirm verdict, never across the reply wait.
///
/// The lapin channel lives in [`ReplySession`], so this core's `Drop` cleanup
/// is unit testable without a broker.
pub(crate) struct ReplyState {
    /// Outstanding request correlations.
    pub(crate) table: Arc<CorrelationTable>,
    /// Mandatory-publish serialization lane (task 4.1 G-3).
    pub(crate) publish_lane: Arc<Mutex<()>>,
    /// Shared with the reply loop; `true` once the state must not be reused.
    pub(crate) retired: Arc<AtomicBool>,
    /// Cancels the reply loop on a runtime stop (child of the shutdown token).
    loop_cancel: Option<CancellationToken>,
    /// Controls the reply loop task.
    loop_abort: AbortHandle,
}

impl ReplyState {
    /// Spawn the reply loop over a delivery stream and return the logical core
    /// that owns it.
    ///
    /// Transport-free: the lapin channel is owned by [`ReplySession`], so the
    /// `Drop`/retirement cleanup is testable without a broker. The loop captures
    /// only clones of the table and the retirement flag (never `Self`), so no
    /// reference cycle can form.
    fn spawn<S>(stream: S, cancel: Option<CancellationToken>) -> Self
    where
        S: futures::Stream<Item = (Option<String>, ReplyPayload)> + Send + 'static,
    {
        let table = Arc::new(CorrelationTable::default());
        let retired = Arc::new(AtomicBool::new(false));
        let loop_abort = tokio::spawn(reply_loop(
            stream,
            Arc::clone(&table),
            Arc::clone(&retired),
            cancel.clone(),
        ))
        .abort_handle();

        Self {
            table,
            publish_lane: Arc::new(Mutex::new(())),
            retired,
            loop_cancel: cancel,
            loop_abort,
        }
    }

    /// Whether the reply loop is still live (the channel connectivity half is
    /// checked by [`ReplySession::is_usable`]).
    fn is_alive(&self) -> bool {
        !self.retired.load(Ordering::Acquire) && !self.loop_abort.is_finished()
    }

    /// Retire the state without awaiting: drain the table (closing receivers
    /// with the `Disconnected` signal), cancel the loop token, and abort the
    /// loop task. Idempotent.
    pub(crate) fn retire_sync(&self) {
        if self.retired.swap(true, Ordering::AcqRel) {
            return;
        }
        self.table.clear();
        if let Some(cancel) = &self.loop_cancel {
            cancel.cancel();
        }
        self.loop_abort.abort();
    }
}

impl Drop for ReplyState {
    fn drop(&mut self) {
        // Cancel + abort the loop so no task is leaked; never block a runtime
        // thread joining it (mqtt `DriverHandle` shape). The channel in
        // `ReplySession` closes via lapin's `ChannelCloser` when the last clone
        // (this session plus any consumer-held reference) drops; each in-flight
        // call holds an `Arc<ReplySession>`, so that is bounded by
        // `replyTimeout`.
        self.retire_sync();
    }
}

/// A producer-owned InOut reply session: the logical [`ReplyState`] plus the
/// single confirm+consume channel direct reply-to requires.
///
/// The channel is both the request publisher (confirm enabled) and the
/// `amq.rabbitmq.reply-to` consumer (no-ack), as direct reply-to demands.
/// Splitting it out keeps [`ReplyState`] transport-free so its `Drop` cleanup
/// is unit testable.
pub(crate) struct ReplySession {
    /// The logical core; its `Drop` cancels and reaps the reply loop.
    state: ReplyState,
    /// The shared confirm + consume channel.
    channel: lapin::Channel,
}

impl ReplySession {
    /// Open the reply channel, start `amq.rabbitmq.reply-to` consumption, and
    /// spawn the reply loop.
    ///
    /// Ordering follows the direct reply-to rule (G-1): `prepare_channel`
    /// (open + confirm_select) -> `basic_consume(no_ack)` -> spawn the loop.
    /// The channel is returned only after all three succeed.
    pub(crate) async fn new(
        manager: &Arc<RabbitConnectionManager>,
    ) -> Result<Arc<Self>, CamelError> {
        let (connection, _generation) = manager
            .connection_within(PUBLISH_DISCONNECTED_BOUND)
            .await?;
        let channel = prepare_channel(&connection).await?;
        let consumer = open_reply_consumer(&channel).await?;
        let cancel = manager.shutdown_child();
        let state = ReplyState::spawn(reply_stream(consumer), cancel);

        Ok(Arc::new(Self { state, channel }))
    }

    /// The shared confirm + consume channel for InOut publishes.
    pub(crate) fn channel(&self) -> &lapin::Channel {
        &self.channel
    }

    /// The correlation table shared with the reply loop.
    pub(crate) fn table(&self) -> &Arc<CorrelationTable> {
        &self.state.table
    }

    /// The shared retirement flag.
    pub(crate) fn retired(&self) -> &Arc<AtomicBool> {
        &self.state.retired
    }

    /// The mandatory-publish serialization lane.
    pub(crate) fn publish_lane(&self) -> &Arc<Mutex<()>> {
        &self.state.publish_lane
    }

    /// Whether this session can still serve a fresh InOut request.
    pub(crate) fn is_usable(&self) -> bool {
        self.state.is_alive() && self.channel.status().connected()
    }

    /// Retire the session without awaiting (see [`ReplyState::retire_sync`]).
    pub(crate) fn retire_sync(&self) {
        self.state.retire_sync();
    }

    /// Retire and best-effort close the channel bounded (used when a freshly
    /// built session loses the slot race or replaces a dead predecessor).
    pub(crate) async fn retire_bounded(&self) {
        self.state.retire_sync();
        let _ = tokio::time::timeout(
            CHANNEL_CLOSE_BOUND,
            self.channel
                .close(200, ShortString::from("reply state retired")),
        )
        .await;
    }
}

/// Open the `amq.rabbitmq.reply-to` consumer, bounded by [`PRODUCER_OPEN_BOUND`].
async fn open_reply_consumer(channel: &lapin::Channel) -> Result<lapin::Consumer, CamelError> {
    tokio::time::timeout(
        PRODUCER_OPEN_BOUND,
        channel.basic_consume(
            ShortString::from(DIRECT_REPLY_TO),
            ShortString::default(),
            BasicConsumeOptions {
                no_ack: true,
                ..BasicConsumeOptions::default()
            },
            FieldTable::default(),
        ),
    )
    .await
    .map_err(|_| {
        RabbitError::PublishFailed(format!(
            "rabbitmq reply consume on '{DIRECT_REPLY_TO}' timed out after {PRODUCER_OPEN_BOUND:?}"
        ))
    })?
    .map_err(|error| {
        RabbitError::PublishFailed(format!(
            "rabbitmq reply consume on '{DIRECT_REPLY_TO}' failed: {error}"
        ))
    })
    .map_err(Into::into)
}

/// Map a lapin reply consumer onto `(correlation_id, payload)` pairs.
///
/// A delivery error is logged and skipped; the loop ends when the consumer
/// stream itself terminates (channel/broker death), matching the consumer
/// engine's stream-end handling.
fn reply_stream(
    consumer: lapin::Consumer,
) -> impl futures::Stream<Item = (Option<String>, ReplyPayload)> + Send {
    consumer.filter_map(|item| async move {
        match item {
            Ok(delivery) => {
                let correlation_id = delivery
                    .properties
                    .correlation_id()
                    .as_ref()
                    .map(|value| value.as_str().to_string());
                let empty = FieldTable::default();
                let headers = crate::headers::inbound(
                    delivery.properties.headers().as_ref().unwrap_or(&empty),
                    &delivery.properties,
                    delivery.redelivered,
                );
                Some((
                    correlation_id,
                    ReplyPayload {
                        body: Bytes::from(delivery.data),
                        headers,
                    },
                ))
            }
            Err(error) => {
                debug!(error = %error, "rabbitmq reply consumer delivery failed");
                None
            }
        }
    })
}

/// The reply loop: resolve each delivery against the correlation table.
///
/// Generic over the delivery stream so unit tests drive it without a broker.
/// On stream end (or cancellation) it drains the table so pending waiters fail
/// with `Disconnected` instead of hanging.
async fn reply_loop<S>(
    stream: S,
    table: Arc<CorrelationTable>,
    retired: Arc<AtomicBool>,
    cancel: Option<CancellationToken>,
) where
    S: futures::Stream<Item = (Option<String>, ReplyPayload)> + Send + 'static,
{
    let mut stream = std::pin::pin!(stream);
    loop {
        let next = tokio::select! {
            biased;
            _ = wait_cancelled(&cancel) => break,
            next = stream.next() => next,
        };
        match next {
            Some((Some(correlation_id), payload)) => {
                if !table.resolve(&correlation_id, payload) {
                    debug!(correlation_id = %correlation_id, "rabbitmq late reply dropped");
                }
            }
            Some((None, _payload)) => {
                debug!("rabbitmq reply without a correlation id dropped");
            }
            None => break,
        }
    }
    retired.store(true, Ordering::Release);
    table.clear();
}

/// Resolve when no shutdown token is bound, so the loop still parks on the
/// stream instead of spinning.
async fn wait_cancelled(cancel: &Option<CancellationToken>) {
    match cancel {
        Some(token) => token.cancelled().await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(text: &str) -> ReplyPayload {
        ReplyPayload {
            body: Bytes::copy_from_slice(text.as_bytes()),
            headers: camel_api::Headers::new(),
        }
    }

    #[tokio::test]
    async fn correlation_register_resolve_roundtrip() {
        let table = CorrelationTable::default();
        let rx = table.register("a");
        assert!(
            table.resolve("a", payload("x")),
            "resolve must find the registered entry"
        );
        assert_eq!(rx.await.unwrap().body, Bytes::from_static(b"x"));
        assert_eq!(table.len(), 0, "resolve must remove the entry");
    }

    #[tokio::test]
    async fn correlation_resolve_after_remove_is_late() {
        let table = CorrelationTable::default();
        let _rx = table.register("a");
        assert!(table.remove("a"), "remove must find the registered entry");
        assert!(
            !table.resolve("a", payload("x")),
            "a reply to a removed entry must be late"
        );
        assert_eq!(table.len(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn reply_timeout_fails_within_bound() {
        let table = CorrelationTable::default();
        let rx = table.register("a");
        let start = tokio::time::Instant::now();
        let result = await_reply(&table, "a", rx, Duration::from_millis(50)).await;
        assert!(
            matches!(result, Err(RabbitError::ReplyTimeout { timeout_ms: 50 })),
            "a missing reply must surface ReplyTimeout{{timeout_ms: 50}}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "the reply timeout must fire within its bound"
        );
        assert_eq!(table.len(), 0, "the timeout must remove the entry");
    }

    /// A caller cancelled while waiting for the mandatory publish lane must
    /// leave no correlation entry behind (the lane is acquired before
    /// registration), and a state retired before registration must reject the
    /// request without a reply wait.
    #[tokio::test]
    async fn mandatory_admission_cancel_leaves_table_empty() {
        use futures::FutureExt;

        let table = CorrelationTable::default();
        let retired = AtomicBool::new(false);
        let lane = Arc::new(Mutex::new(()));
        let held = Arc::clone(&lane).lock_owned().await;

        // Poll the admission once: it parks on the held lane, then the future
        // is dropped (caller cancellation). Registration must not have happened.
        let waiting = admit(
            &table,
            &retired,
            Some(&lane),
            "leak-probe",
            Duration::from_secs(30),
        );
        assert!(
            waiting.now_or_never().is_none(),
            "admission must park on the held lane"
        );
        assert_eq!(
            table.len(),
            0,
            "a cancelled lane admission must not leak a correlation entry"
        );

        // The table stays usable once the lane frees.
        drop(held);
        let Admission::Ready { lane: _lane, rx } = admit(
            &table,
            &retired,
            Some(&lane),
            "leak-probe",
            Duration::from_secs(30),
        )
        .await
        else {
            panic!("admission must succeed after the lane frees");
        };
        assert!(table.resolve("leak-probe", payload("ok")));
        assert_eq!(rx.await.unwrap().body, Bytes::from_static(b"ok"));
        assert_eq!(table.len(), 0);

        // Retired-before-registration is rejected and the entry removed.
        retired.store(true, Ordering::Release);
        let outcome = admit(
            &table,
            &retired,
            None,
            "late-probe",
            Duration::from_secs(30),
        )
        .await;
        assert!(matches!(outcome, Admission::Retired));
        assert_eq!(
            table.len(),
            0,
            "a retired admission must remove its registration"
        );
    }

    /// 4.2 step 3: the timeout removes the entry when the timer fires (not at
    /// the later resolve attempt), so a late reply finds nothing and is dropped.
    #[tokio::test(start_paused = true)]
    async fn late_reply_dropped_and_table_empty() {
        let table = CorrelationTable::default();
        let rx = table.register("late");
        let result = await_reply(&table, "late", rx, Duration::from_millis(50)).await;
        assert!(
            matches!(result, Err(RabbitError::ReplyTimeout { timeout_ms: 50 })),
            "a missing reply must time out before any resolve"
        );
        assert!(
            !table.resolve("late", payload("stale")),
            "a reply that arrives after the timeout must be dropped"
        );
        assert_eq!(table.len(), 0, "the timed-out entry must be gone");
    }

    /// 4.2 step 2: dropping the state cancels and reaps the reply loop, drains
    /// the table, and closes every outstanding receiver.
    #[tokio::test]
    async fn reply_state_drop_drains_table() {
        // Transport-free core: the reply loop runs over a never-yielding stream
        // with no shutdown token, so only `Drop` drives the cleanup.
        let state = ReplyState::spawn(
            futures::stream::pending::<(Option<String>, ReplyPayload)>(),
            None,
        );
        let table = Arc::clone(&state.table);
        let abort = state.loop_abort.clone();

        let receivers = ["a", "b", "c"].map(|id| table.register(id));
        assert_eq!(table.len(), 3, "three correlations must be outstanding");

        drop(state);

        for rx in receivers {
            let drained = tokio::time::timeout(Duration::from_secs(1), rx).await;
            assert!(
                matches!(drained, Ok(Err(_))),
                "Drop must close every outstanding receiver"
            );
        }
        assert_eq!(table.len(), 0, "Drop must drain the correlation table");

        let reaped = tokio::time::timeout(Duration::from_secs(1), async {
            while !abort.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(reaped.is_ok(), "the reply loop must be reaped after drop");
    }

    /// 4.2 / G-7: a caller that drops its `await_reply` future must remove its
    /// entry, so a cancelled InOut never leaks a correlation.
    #[tokio::test(start_paused = true)]
    async fn await_reply_cancel_removes_entry() {
        let table = CorrelationTable::default();
        let rx = table.register("cancel");
        let mut pending = Box::pin(await_reply(&table, "cancel", rx, Duration::from_millis(50)));
        assert!(
            futures::poll!(pending.as_mut()).is_pending(),
            "await_reply must park while the reply is outstanding"
        );
        assert_eq!(table.len(), 1, "the entry is registered while awaited");

        // Dispose the boxed future (not just a `Pin` reference): the
        // `RemoveOnDrop` guard must remove the entry on cancellation.
        drop(pending);

        assert_eq!(
            table.len(),
            0,
            "a cancelled await_reply must remove its entry"
        );
        assert!(
            !table.resolve("cancel", payload("late")),
            "a reply after cancellation must be dropped"
        );
    }
}
