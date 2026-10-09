//! RabbitMQ producer: maps an [`Exchange`] onto an AMQP `basic.publish`
//! with publisher confirms always on, and fails bounded while the broker is
//! disconnected.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use camel_api::{Body, ExchangePattern, Message};
use camel_component_api::{CamelError, Exchange, RuntimeObservability};
use lapin::BasicProperties;
use lapin::options::{BasicPublishOptions, ConfirmSelectOptions};
use lapin::types::ShortString;
use tokio::sync::OwnedMutexGuard;
use tower::Service;
use tracing::debug;

use crate::config::RabbitEndpointConfig;
use crate::connection::{PUBLISH_DISCONNECTED_BOUND, RabbitConnectionManager};
use crate::error::RabbitError;
use crate::reply::{Admission, DIRECT_REPLY_TO, ReplySession, admit, await_reply};

/// Bound on the best-effort channel close after a confirm timeout: the broker
/// may be unresponsive (which is why the confirm timed out), so the close must
/// never stall the publish path (task 3.1).
pub(crate) const CHANNEL_CLOSE_BOUND: Duration = Duration::from_secs(1);

/// Single preparation deadline for a fresh publisher channel
/// (`channel.open` + `confirm.select`). A live connection answers both
/// promptly; the bound only fires on a half-dead or stalled connection, and it
/// is applied OUTSIDE the channel cache lock so a slow preparation can never
/// block a concurrent publish's cache invalidation (task 3.1 review fix).
pub(crate) const PRODUCER_OPEN_BOUND: Duration = Duration::from_secs(10);

/// Outcome of one publish, mapped onto the component-operations metric.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PublishOutcome {
    /// The broker confirmed the publish.
    Success,
    /// The publish failed (routing, confirm nack, or a dead channel).
    Failure,
}

/// Build the AMQP basic properties for one publish.
///
/// Delegates the reserved/free-form mapping to [`crate::headers::outbound`]
/// (the single two-way source): reserved names map onto `BasicProperties`,
/// everything else rides the free-form `FieldTable` as a `LongString`.
/// `delivery_mode` is 2 when `persistent`, else 1, and `content_type` comes
/// from the URI option first, then the exchange's `contentType` header.
pub(crate) fn build_properties(
    persistent: bool,
    content_type: Option<&str>,
    headers: &camel_api::Headers,
) -> BasicProperties {
    let (props, table) = crate::headers::outbound(headers);

    let resolved_content_type = content_type.map(str::to_string).or_else(|| {
        props
            .content_type()
            .as_ref()
            .map(|value| value.as_str().to_string())
    });

    let mut properties = props
        .with_delivery_mode(if persistent { 2 } else { 1 })
        .with_headers(table);
    if let Some(value) = resolved_content_type.and_then(|value| ShortString::try_new(value).ok()) {
        properties = properties.with_content_type(value);
    }
    properties
}

/// Emit one `camel_component_operations_total{component="rabbitmq",
/// operation="publish"}` observation through `rt`.
pub(crate) fn record_publish(rt: &dyn RuntimeObservability, outcome: PublishOutcome) {
    rt.component_metrics()
        .observe("rabbitmq", "publish", outcome == PublishOutcome::Failure);
}

/// Map a camel [`Body`] onto the AMQP payload bytes (same shape as the JMS
/// producer's body mapping). A body that cannot be materialized is a publish
/// failure (task 3.5 taxonomy). Shared with the consumer's reply publisher
/// (task 4.3) so both directions use ONE body converter.
pub(crate) fn body_to_bytes(body: &Body) -> Result<Vec<u8>, RabbitError> {
    match body {
        Body::Text(text) => Ok(text.as_bytes().to_vec()),
        Body::Xml(xml) => Ok(xml.as_bytes().to_vec()),
        Body::Bytes(bytes) => Ok(bytes.to_vec()),
        Body::Json(json) => serde_json::to_vec(json)
            .map_err(|error| RabbitError::PublishFailed(format!("JSON error: {error}"))),
        Body::Empty => Ok(Vec::new()),
        Body::Stream(_) => Err(RabbitError::PublishFailed(
            "Body::Stream must be materialized before sending to RabbitMQ".to_string(),
        )),
        _ => Err(RabbitError::PublishFailed(
            "unsupported body type for RabbitMQ producer".to_string(),
        )),
    }
}

/// Cached confirm channel plus the connection generation it was created on.
///
/// The cache is a `std::sync::Mutex` so its critical sections are tiny,
/// synchronous snapshots/conditional updates: the guard is NEVER held across an
/// `.await`. A previous `tokio::sync::Mutex` was held across the unbounded
/// `channel.open`/`confirm.select` network awaits, which let a stalled
/// preparation wedge every concurrent publish's cache access (task 3.1 review
/// fix).
type ChannelCache = std::sync::Mutex<Option<(lapin::Channel, u64)>>;

/// Lazily filled slot for the producer's shared InOut reply state.
///
/// Like [`ChannelCache`], a `std::sync::Mutex` guards a tiny, synchronous
/// snapshot/conditional update; the guard is NEVER held across an `.await`.
/// The connection may be down at `new`, so the state is built on the first
/// InOut call.
type ReplySlot = std::sync::Mutex<Option<Arc<ReplySession>>>;

/// Tower producer publishing each [`Exchange`] to the endpoint's target.
///
/// The lapin channel is created once per producer, has publisher confirms
/// enabled once, and is reused across messages (`Clone` shares the same cached
/// channel). A dead channel is dropped so the next publish recreates it.
#[derive(Clone)]
pub struct RabbitProducer {
    config: RabbitEndpointConfig,
    target: (String, String),
    manager: Arc<RabbitConnectionManager>,
    rt: Arc<dyn RuntimeObservability>,
    channel: Arc<ChannelCache>,
    reply: Arc<ReplySlot>,
}

impl RabbitProducer {
    /// Build a producer for one endpoint target.
    pub(crate) fn new(
        config: RabbitEndpointConfig,
        manager: Arc<RabbitConnectionManager>,
        rt: Arc<dyn RuntimeObservability>,
    ) -> Self {
        let target = config.target();
        Self {
            config,
            target,
            manager,
            rt,
            channel: Arc::new(ChannelCache::new(None)),
            reply: Arc::new(ReplySlot::new(None)),
        }
    }

    /// The `(exchange, routing_key)` this producer publishes to.
    ///
    /// Exercised by the `producer_uses_config_target` parity re-assert; the
    /// publish path reads the stored `target` field directly.
    #[cfg(test)]
    pub(crate) fn target(&self) -> (String, String) {
        self.target.clone()
    }
}

impl Service<Exchange> for RabbitProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let config = self.config.clone();
        let target = self.target.clone();
        let manager = Arc::clone(&self.manager);
        let rt = Arc::clone(&self.rt);
        let channel = Arc::clone(&self.channel);
        let reply = Arc::clone(&self.reply);

        Box::pin(async move {
            // InOut rides the shared direct reply-to channel (same channel for
            // request publish and reply consume); InOnly keeps the 3.1 cached
            // channel and 3.2 mandatory leases unchanged.
            let result = if exchange.pattern == ExchangePattern::InOut {
                publish_in_out(&config, &target, &manager, &reply, exchange).await
            } else {
                publish(&config, &target, &manager, &channel, &exchange)
                    .await
                    .map(|()| exchange)
            };
            let outcome = if result.is_ok() {
                PublishOutcome::Success
            } else {
                PublishOutcome::Failure
            };
            record_publish(&*rt, outcome);
            result
        })
    }
}

/// A channel leased for exactly one publish call.
///
/// `dedicated == true` is a fresh, confirm-enabled channel created for a single
/// `mandatory = true` publish and never cached or shared, so exactly one
/// publish is ever outstanding on it. That is required because lapin pops a
/// mandatory `basic.return` FIFO per confirmation
/// (`ReturnedMessages::get_waiting_message`, driven by
/// `acknowledgement.rs::complete_pending`) with NO per-publish correlation: on
/// a shared channel with several in-flight confirms, an out-of-order ack can
/// pair one publish's return with another publish's confirm (a returned
/// message would be read as a clean ack, silently losing the return). One
/// channel ⇒ one outstanding confirm ⇒ any return can only belong to this
/// publish.
///
/// Lifecycle: on the normal path `publish` closes a dedicated channel
/// explicitly under `CHANNEL_CLOSE_BOUND`. If the future is dropped
/// (cancellation) the lease drops instead and lapin's built-in closer runs: the
/// last `lapin::Channel` clone's `ChannelCloser::drop` sends `CloseChannel`
/// when the channel id is non-zero and still connected. No task is spawned by
/// us, and a dedicated channel is never reused, so an abandoned one cannot
/// poison a successor.
struct ChannelLease {
    channel: lapin::Channel,
    generation: u64,
    dedicated: bool,
}

/// Acquire the channel for one publish call.
///
/// `mandatory = true` gets its OWN fresh confirm channel on the shared
/// connection (never cached, never reused across calls), so a mandatory publish
/// never shares a channel with a concurrent one and the lapin FIFO return can
/// not be misattributed. `mandatory = false` keeps the task 3.1 shared cached
/// channel and its concurrency.
async fn acquire_channel(
    mandatory: bool,
    manager: &Arc<RabbitConnectionManager>,
    cache: &ChannelCache,
) -> Result<ChannelLease, CamelError> {
    if mandatory {
        let (connection, generation) = manager
            .connection_within(PUBLISH_DISCONNECTED_BOUND)
            .await?;
        let channel = prepare_channel(&connection).await?;
        Ok(ChannelLease {
            channel,
            generation,
            dedicated: true,
        })
    } else {
        let (channel, generation) = ensure_channel(manager, cache).await?;
        Ok(ChannelLease {
            channel,
            generation,
            dedicated: false,
        })
    }
}

/// Open one confirm-enabled channel on `connection`, bounded by
/// [`PRODUCER_OPEN_BOUND`]. The bound is applied with no cache lock held, so a
/// stalled preparation can never block a concurrent publish.
pub(crate) async fn prepare_channel(
    connection: &lapin::Connection,
) -> Result<lapin::Channel, RabbitError> {
    tokio::time::timeout(PRODUCER_OPEN_BOUND, async {
        let channel = connection.create_channel().await?;
        channel
            .confirm_select(ConfirmSelectOptions::default())
            .await?;
        Ok::<_, lapin::Error>(channel)
    })
    .await
    .map_err(|_| {
        RabbitError::PublishFailed(format!(
            "rabbitmq channel preparation timed out after {PRODUCER_OPEN_BOUND:?}"
        ))
    })?
    .map_err(|error| {
        RabbitError::PublishFailed(format!("rabbitmq channel preparation failed: {error}"))
    })
}

/// Publish one exchange, returning `Ok(())` once the broker confirms.
async fn publish(
    config: &RabbitEndpointConfig,
    target: &(String, String),
    manager: &Arc<RabbitConnectionManager>,
    cache: &ChannelCache,
    exchange: &Exchange,
) -> Result<(), CamelError> {
    let payload = body_to_bytes(&exchange.input.body)?;
    let properties = build_properties(
        config.persistent,
        config.content_type.as_deref(),
        &exchange.input.headers,
    );

    let lease = acquire_channel(config.mandatory, manager, cache).await?;
    let result = deliver(config, target, manager, cache, &lease, &payload, properties).await;
    if lease.dedicated {
        // Own channel for exactly this publish: close it bounded now that the
        // confirm (or its bounded timeout) resolved. A cancelled future skips
        // this and relies on lapin's `ChannelCloser` on lease drop.
        let _ = tokio::time::timeout(
            CHANNEL_CLOSE_BOUND,
            lease
                .channel
                .close(200, ShortString::from("mandatory publish complete")),
        )
        .await;
    }
    result
}

/// Publish on `lease`'s channel and map the broker verdict onto the result.
async fn deliver(
    config: &RabbitEndpointConfig,
    target: &(String, String),
    manager: &Arc<RabbitConnectionManager>,
    cache: &ChannelCache,
    lease: &ChannelLease,
    payload: &[u8],
    properties: BasicProperties,
) -> Result<(), CamelError> {
    let channel = &lease.channel;
    let channel_id = channel.id();
    let generation = lease.generation;
    let (exchange_name, routing_key) = target;
    let confirm_timeout = config.confirm_timeout;

    let confirm = match channel
        .basic_publish(
            ShortString::from(exchange_name.as_str()),
            ShortString::from(routing_key.as_str()),
            BasicPublishOptions {
                mandatory: config.mandatory,
                ..BasicPublishOptions::default()
            },
            payload,
            properties,
        )
        .await
    {
        Ok(confirm) => confirm,
        Err(error) => {
            mark_channel_dead(manager, cache, channel_id, generation, &error);
            return Err(RabbitError::PublishFailed(format!(
                "rabbitmq publish to exchange '{}' routing key '{}' failed: {error}",
                target.0, target.1
            ))
            .into());
        }
    };

    // lapin 4.12: `basic_publish` returns the `PublisherConfirm` future, which
    // is awaited a second time for the broker's verdict.
    let confirmation = match tokio::time::timeout(confirm_timeout, confirm).await {
        Ok(Ok(confirmation)) => confirmation,
        Ok(Err(error)) => {
            mark_channel_dead(manager, cache, channel_id, generation, &error);
            return Err(RabbitError::PublishFailed(format!(
                "rabbitmq publish to exchange '{}' routing key '{}' failed: {error}",
                target.0, target.1
            ))
            .into());
        }
        Err(_elapsed) => {
            if !lease.dedicated {
                // Cached path only: this channel's confirm accounting is now
                // uncertain, so invalidate the exact origin and close bounded
                // (task 3.1). A dedicated channel has no cache entry and is
                // closed by `publish` after `deliver` returns.
                invalidate_cached(cache, channel_id, generation);
                let _ = tokio::time::timeout(
                    CHANNEL_CLOSE_BOUND,
                    channel.close(200, ShortString::from("confirm timeout")),
                )
                .await;
            }
            return Err(RabbitError::ConfirmTimeout {
                exchange: target.0.clone(),
                routing_key: target.1.clone(),
            }
            .into());
        }
    };

    let result = map_confirmation(target, confirmation);
    if result.is_ok() {
        debug!(
            exchange = %exchange_name,
            routing_key = %routing_key,
            "rabbitmq publish confirmed"
        );
    }
    result
}

/// Map the broker's confirm for ONE publish onto the publish result.
///
/// lapin carries a mandatory `basic.return` inside the confirming publish's
/// [`lapin::Confirmation`] (an `Ack` or a `Nack`), but it does NOT correlate
/// the return to a specific in-flight publish: `ReturnedMessages` is a FIFO
/// popped once per completed confirmation (`acknowledgement.rs::
/// complete_pending`). That is unambiguous ONLY because a `mandatory = true`
/// publish runs on its own dedicated channel with exactly one outstanding
/// confirm (`acquire_channel`); on a shared channel concurrent returns could be
/// mispaired, which is why the cached channel is used only for
/// `mandatory = false`. A present returned message therefore means THIS message
/// was unroutable and the exchange must fail naming the target; the shared
/// connection and channel stay healthy (only the confirm-accounting-uncertain
/// timeout path invalidates a cached channel).
///
/// - `Ack(None)` / `NotRequested` — confirmed (or confirms disabled): `Ok`.
/// - `Ack(Some(..))` / `Nack(Some(..))` — unroutable and returned:
///   [`RabbitError::Unroutable`].
/// - `Nack(None)` — broker nacked without a return:
///   [`RabbitError::ConfirmNacked`].
fn map_confirmation(
    target: &(String, String),
    confirmation: lapin::Confirmation,
) -> Result<(), CamelError> {
    match confirmation {
        lapin::Confirmation::Ack(None) | lapin::Confirmation::NotRequested => Ok(()),
        lapin::Confirmation::Ack(Some(_)) | lapin::Confirmation::Nack(Some(_)) => {
            Err(RabbitError::Unroutable {
                exchange: target.0.clone(),
                routing_key: target.1.clone(),
            }
            .into())
        }
        lapin::Confirmation::Nack(None) => Err(RabbitError::ConfirmNacked {
            exchange: target.0.clone(),
            routing_key: target.1.clone(),
        }
        .into()),
    }
}

/// Publish one InOut exchange on the shared direct reply-to channel and await
/// the correlated reply.
///
/// The channel is BOTH the request publisher (confirm enabled) and the
/// `amq.rabbitmq.reply-to` consumer (no-ack), as the RabbitMQ direct reply-to
/// caveat requires. The request is published with
/// `reply_to = amq.rabbitmq.reply-to` and a fresh UUID `correlation_id`,
/// registered before the publish so an early reply is not lost.
async fn publish_in_out(
    config: &RabbitEndpointConfig,
    target: &(String, String),
    manager: &Arc<RabbitConnectionManager>,
    reply: &ReplySlot,
    mut exchange: Exchange,
) -> Result<Exchange, CamelError> {
    let payload = body_to_bytes(&exchange.input.body)?;
    let properties = build_properties(
        config.persistent,
        config.content_type.as_deref(),
        &exchange.input.headers,
    );

    let state = ensure_reply_state(manager, reply).await?;

    // G-1/G-3: acquire the bounded mandatory lane FIRST, then register the
    // correlation (still before `basic_publish`, so an early reply is not
    // lost). A caller cancelled while waiting for the lane has not registered
    // anything, so nothing can leak; `admit` registers, re-checks retirement,
    // and returns with no `.await` gap before the guard is armed below.
    let correlation_id = uuid::Uuid::new_v4().to_string();
    let lane_ref = config.mandatory.then_some(state.publish_lane());
    let (lane, rx) = match admit(
        state.table(),
        state.retired(),
        lane_ref,
        &correlation_id,
        config.confirm_timeout,
    )
    .await
    {
        Admission::Ready { lane, rx } => (lane, rx),
        // The lane was busy for the whole bound: no entry was registered, so
        // there is nothing to clean up.
        Admission::LaneTimeout => return Err(confirm_timeout(target).into()),
        Admission::Retired => return Err(RabbitError::Disconnected.into()),
    };

    let properties = properties
        .with_reply_to(ShortString::from(DIRECT_REPLY_TO))
        .with_correlation_id(ShortString::from(correlation_id.as_str()));

    // Armed until a clean confirm verdict: a cancellation or channel-level
    // failure before then retires the state so a stale return cannot pair with
    // a successor's confirm on this channel.
    let mut guard = ConfirmGuard {
        state: Arc::clone(&state),
        _lane: lane,
        complete: false,
    };

    let confirm = match state
        .channel()
        .basic_publish(
            ShortString::from(target.0.as_str()),
            ShortString::from(target.1.as_str()),
            BasicPublishOptions {
                mandatory: config.mandatory,
                ..BasicPublishOptions::default()
            },
            &payload,
            properties,
        )
        .await
    {
        Ok(confirm) => confirm,
        // Channel-level publish error: the guard retires on drop.
        Err(error) => return Err(publish_failed(target, &error)),
    };

    let confirmation = match tokio::time::timeout(config.confirm_timeout, confirm).await {
        Ok(Ok(confirmation)) => confirmation,
        Ok(Err(error)) => return Err(publish_failed(target, &error)),
        // Confirm timeout: accounting is uncertain; retire (guard drop).
        Err(_) => return Err(confirm_timeout(target).into()),
    };

    // A clean verdict (Ack or Nack) leaves the channel healthy; an unroutable
    // or nacked publish is still a definite verdict, so the state is not
    // retired, only this request's correlation entry is removed.
    let confirmed = map_confirmation(target, confirmation);
    guard.complete();
    drop(guard);

    if let Err(error) = confirmed {
        state.table().remove(&correlation_id);
        return Err(error);
    }

    // The reply wait is bounded by replyTimeout; a plain timeout leaves the
    // shared state usable so a late reply is simply dropped by `resolve`.
    let reply_payload =
        await_reply(state.table(), &correlation_id, rx, config.reply_timeout).await?;

    let mut message = Message::new(Body::Bytes(reply_payload.body));
    message.headers = reply_payload.headers;
    exchange.output = Some(message);
    Ok(exchange)
}

/// Ensure the producer's shared InOut reply state exists, building it lazily
/// and resolving the first-use race by identity.
///
/// Mirrors the [`ChannelCache`] discipline: build outside the lock, publish
/// only if still vacant, and retire the loser bounded. A retired or dead
/// predecessor is displaced.
async fn ensure_reply_state(
    manager: &Arc<RabbitConnectionManager>,
    slot: &ReplySlot,
) -> Result<Arc<ReplySession>, CamelError> {
    if let Some(state) = snapshot_reply(slot) {
        return Ok(state);
    }

    // The connection may be down at `new`; build the state on first InOut use,
    // outside the lock (network setup awaits).
    let state = ReplySession::new(manager).await?;

    let mut replaced: Option<Arc<ReplySession>> = None;
    let winner = {
        let mut guard = lock_reply(slot);
        match guard.as_ref() {
            Some(existing) if existing.is_usable() => Some(Arc::clone(existing)),
            _ => {
                replaced = guard.replace(Arc::clone(&state));
                None
            }
        }
    };

    match winner {
        // Lost the race: retire the freshly built loser bounded.
        Some(existing) => {
            state.retire_bounded().await;
            Ok(existing)
        }
        None => {
            // Displaced a dead/retired predecessor: retire it bounded.
            if let Some(old) = replaced {
                old.retire_bounded().await;
            }
            Ok(state)
        }
    }
}

/// Guards the mandatory publish lane and retires the reply state when the
/// caller is cancelled (or a channel-level failure occurs) before a clean
/// confirm verdict.
struct ConfirmGuard {
    state: Arc<ReplySession>,
    _lane: Option<OwnedMutexGuard<()>>,
    complete: bool,
}

impl ConfirmGuard {
    fn complete(&mut self) {
        self.complete = true;
    }
}

impl Drop for ConfirmGuard {
    fn drop(&mut self) {
        if !self.complete {
            self.state.retire_sync();
        }
    }
}

/// Lock the reply slot, recovering a poisoned mutex (plain snapshot value).
fn lock_reply(slot: &ReplySlot) -> std::sync::MutexGuard<'_, Option<Arc<ReplySession>>> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Clone out the slot's reply state when it is still usable. The lock is
/// released before returning, so no caller holds it across an `.await`.
fn snapshot_reply(slot: &ReplySlot) -> Option<Arc<ReplySession>> {
    let guard = lock_reply(slot);
    guard.as_ref().filter(|state| state.is_usable()).cloned()
}

/// Map a channel-level publish error onto the taxonomy naming the target.
fn publish_failed(target: &(String, String), error: &lapin::Error) -> CamelError {
    RabbitError::PublishFailed(format!(
        "rabbitmq publish to exchange '{}' routing key '{}' failed: {error}",
        target.0, target.1
    ))
    .into()
}

/// The confirm-timeout failure for `target`.
fn confirm_timeout(target: &(String, String)) -> RabbitError {
    RabbitError::ConfirmTimeout {
        exchange: target.0.clone(),
        routing_key: target.1.clone(),
    }
}

/// Lock the channel cache, recovering a poisoned mutex.
///
/// The guarded value is a plain `Option` snapshot; a panic in a holder cannot
/// leave it half-written, so poisoning is irrelevant and recovery is correct.
fn lock_cache(cache: &ChannelCache) -> std::sync::MutexGuard<'_, Option<(lapin::Channel, u64)>> {
    cache.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Clone out the cached channel when it is still connected. The lock is
/// released before returning, so no caller holds it across an `.await`.
fn snapshot_connected(cache: &ChannelCache) -> Option<(lapin::Channel, u64)> {
    let guard = lock_cache(cache);
    match guard.as_ref() {
        Some((channel, generation)) if channel.status().connected() => {
            Some((channel.clone(), *generation))
        }
        _ => None,
    }
}

/// Remove the cached channel ONLY when it is still the exact origin identified
/// by `channel_id`/`generation`. A concurrent publish may already have
/// replaced it with a healthy channel; clearing that successor would discard a
/// working channel and defeat the cache's identity invariant.
fn invalidate_cached(cache: &ChannelCache, channel_id: u16, generation: u64) {
    let mut guard = lock_cache(cache);
    let is_origin = guard
        .as_ref()
        .is_some_and(|(cached, cached_gen)| cached.id() == channel_id && *cached_gen == generation);
    if is_origin {
        *guard = None;
    }
}

/// Return the cached channel and the connection generation it was created on,
/// creating the channel (and enabling confirms once) on first use or after a
/// channel death.
///
/// The connection wait is bounded by [`PUBLISH_DISCONNECTED_BOUND`] and the
/// fresh-channel preparation by [`PRODUCER_OPEN_BOUND`]; neither bound is
/// applied while the cache lock is held, so a stalled preparation can never
/// block a concurrent publish's cache access or invalidation (task 3.1 review
/// fix).
async fn ensure_channel(
    manager: &Arc<RabbitConnectionManager>,
    cache: &ChannelCache,
) -> Result<(lapin::Channel, u64), CamelError> {
    if let Some((channel, generation)) = snapshot_connected(cache) {
        return Ok((channel, generation));
    }

    let (connection, generation) = manager
        .connection_within(PUBLISH_DISCONNECTED_BOUND)
        .await?;

    // One preparation deadline for `channel.open` + `confirm.select`, applied
    // with the cache lock released.
    let channel = prepare_channel(&connection).await?;

    // Publish into the cache only if it is still vacant: a concurrent creator
    // may have won the race. Never overwrite a healthy cached replacement. The
    // loser is closed bounded after the lock is released, so it is never left
    // reusable.
    let winner = {
        let mut guard = lock_cache(cache);
        match guard.as_ref() {
            Some((cached, cached_gen)) if cached.status().connected() => {
                Some((cached.clone(), *cached_gen))
            }
            _ => {
                *guard = Some((channel.clone(), generation));
                None
            }
        }
    };

    match winner {
        Some((cached, cached_gen)) => {
            let _ = tokio::time::timeout(
                CHANNEL_CLOSE_BOUND,
                channel.close(200, ShortString::from("superseded")),
            )
            .await;
            Ok((cached, cached_gen))
        }
        None => Ok((channel, generation)),
    }
}

/// Drop the cached channel when it is still the origin this publish used. A
/// channel-scoped error (for example a 404 NOT_FOUND on a missing exchange)
/// closes only this channel: the connection stays healthy, so recovery is
/// limited to recreating the channel on the next publish. Only a
/// connection-scoped error hands recovery to the manager's reconnect loop,
/// qualified with the generation of the connection the channel was created on
/// so a late failure cannot demote a replacement.
fn mark_channel_dead(
    manager: &Arc<RabbitConnectionManager>,
    cache: &ChannelCache,
    channel_id: u16,
    generation: u64,
    error: &lapin::Error,
) {
    invalidate_cached(cache, channel_id, generation);
    if crate::connection::is_connection_level_error(error) {
        manager.note_failure_for_generation(generation);
    }
}

#[cfg(test)]
mod tests;
