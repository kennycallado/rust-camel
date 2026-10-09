//! Consumer-side direct reply-to publishing (task 4.3).
//!
//! The decision (whether a reply is warranted, and what it carries) is split
//! from the I/O (one `basic_publish` on a plain channel) per the P4 G-10
//! guidance: [`maybe_reply`] is pure and unit-tested with a recorder fake;
//! [`send_reply`] performs the publish through the [`ReplyPublisher`] seam.
//!
//! A replier may publish on ANY channel/connection — the RabbitMQ direct
//! reply-to same-channel rule binds only the requester — so production opens a
//! plain, non-confirm channel from the manager per reply and closes it bounded.
//! The shared connection is never reset; a reply publish failure is an
//! infrastructure side effect, not a business route failure.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use camel_api::Headers;
use camel_component_api::{CamelError, Exchange};
use lapin::BasicProperties;
use lapin::options::BasicPublishOptions;
use lapin::types::ShortString;

use crate::connection::RabbitConnectionManager;
use crate::error::RabbitError;

/// Bound on one reply publish (`basic_publish` write plus its no-confirm
/// completion). A stalled broker must never wedge the consumer engine; it
/// follows the producer's 10 s channel-preparation order of magnitude.
pub(crate) const REPLY_SEND_BOUND: Duration = Duration::from_secs(10);

/// One decided reply publish.
pub(crate) struct ReplyIntent {
    /// The inbound `replyTo`: the routing key on the default exchange.
    pub(crate) reply_to: String,
    /// The inbound `correlationId`, enforced on the outgoing properties.
    pub(crate) correlation_id: String,
    /// The route OUT body (falling back to the IN body).
    pub(crate) body: Vec<u8>,
    /// The route OUT headers (falling back to the IN headers).
    pub(crate) headers: Headers,
}

/// Publish one direct reply-to reply.
///
/// Implemented for `lapin::Channel`; unit fakes implement the same trait, so
/// the 4.3 tests need no broker.
#[async_trait]
pub(crate) trait ReplyPublisher: Send + Sync {
    async fn publish_reply(
        &self,
        reply_to: &str,
        correlation_id: &str,
        body: Vec<u8>,
        props: BasicProperties,
    ) -> Result<(), CamelError>;
}

#[async_trait]
impl ReplyPublisher for lapin::Channel {
    async fn publish_reply(
        &self,
        reply_to: &str,
        correlation_id: &str,
        body: Vec<u8>,
        props: BasicProperties,
    ) -> Result<(), CamelError> {
        // The request's own correlation id wins over any route-set property so
        // a reply can never be paired with another request's UUID.
        let props = force_correlation_id(props, correlation_id)?;
        let routing_key = ShortString::try_new(reply_to).map_err(|_| {
            RabbitError::PublishFailed(
                "rabbitmq replyTo exceeds the AMQP short-string limit".to_string(),
            )
        })?;
        let confirm = self
            .basic_publish(
                // Default exchange: the reply-to name IS the routing key.
                ShortString::default(),
                routing_key,
                BasicPublishOptions {
                    // Replies are never requested unroutable; the default
                    // exchange never 404s, so no `basic.return` is needed.
                    mandatory: false,
                    ..BasicPublishOptions::default()
                },
                &body,
                props,
            )
            .await
            .map_err(|error| reply_publish_failed(&error))?;
        // No publisher confirm is requested on a plain channel: the returned
        // future resolves to `NotRequested` once the frame is written. Await it
        // so the bounded channel close cannot race the reply; there is no
        // broker ack wait (the direct reply-to limitation).
        confirm
            .await
            .map_err(|error| reply_publish_failed(&error))?;
        Ok(())
    }
}

/// Force the ORIGINAL request `correlation_id` onto the outgoing properties,
/// AFTER [`crate::headers::outbound`] mapped the route headers: a route-set
/// `correlationId` must never override it.
fn force_correlation_id(
    props: BasicProperties,
    correlation_id: &str,
) -> Result<BasicProperties, CamelError> {
    let value = ShortString::try_new(correlation_id).map_err(|_| {
        RabbitError::PublishFailed(
            "rabbitmq reply correlationId exceeds the AMQP short-string limit".to_string(),
        )
    })?;
    Ok(props.with_correlation_id(value))
}

fn reply_publish_failed(error: &lapin::Error) -> CamelError {
    RabbitError::PublishFailed(format!("rabbitmq reply publish failed: {error}")).into()
}

/// Decide whether the consumer must publish a reply for `route_result`.
///
/// A reply is warranted ONLY when the route completed `Ok` AND the ORIGINAL
/// request headers (snapshotted before the route ran) carry both a string
/// `replyTo` and a string `correlationId`. The reply body is materialized ONLY
/// after that test passes, so an InOnly route without reply headers allocates
/// nothing. The reply message is selected exactly once — the route OUT message
/// when present, otherwise the IN message — and BOTH the body and the headers
/// come from that same single message. The body uses the producer's shared
/// [`crate::producer::body_to_bytes`]; a body it cannot materialize yields no
/// intent (same behavior as before, no new error policy).
pub(crate) fn maybe_reply(
    route_result: &Result<Exchange, CamelError>,
    original_headers: &Headers,
) -> Option<ReplyIntent> {
    let exchange = route_result.as_ref().ok()?;
    let reply_to = string_header(original_headers, "replyTo")?.to_string();
    let correlation_id = string_header(original_headers, "correlationId")?.to_string();
    let message = exchange.output.as_ref().unwrap_or(&exchange.input);
    let body = crate::producer::body_to_bytes(&message.body).ok()?;
    Some(ReplyIntent {
        reply_to,
        correlation_id,
        body,
        headers: message.headers.clone(),
    })
}

fn string_header<'a>(headers: &'a Headers, name: &str) -> Option<&'a str> {
    headers.get(name)?.as_str()
}

/// Build the transient reply properties from the OUT headers and publish
/// through `publisher`, bounded by [`REPLY_SEND_BOUND`].
///
/// The properties are mapped by the single two-way [`crate::headers::outbound`]
/// source, then `delivery_mode = 1` (replies are transient) and the ORIGINAL
/// `correlationId` are enforced after that mapping.
pub(crate) async fn send_reply(
    intent: &ReplyIntent,
    publisher: &dyn ReplyPublisher,
) -> Result<(), CamelError> {
    let (props, table) = crate::headers::outbound(&intent.headers);
    let props = force_correlation_id(
        props.with_delivery_mode(1).with_headers(table),
        &intent.correlation_id,
    )?;
    tokio::time::timeout(
        REPLY_SEND_BOUND,
        publisher.publish_reply(
            &intent.reply_to,
            &intent.correlation_id,
            intent.body.clone(),
            props,
        ),
    )
    .await
    .map_err(|_: tokio::time::error::Elapsed| -> CamelError {
        RabbitError::PublishFailed(format!(
            "rabbitmq reply send timed out after {REPLY_SEND_BOUND:?}"
        ))
        .into()
    })?
}

/// Acquire a plain (non-confirm) channel from `manager`, publish `intent`, and
/// close that channel bounded.
///
/// [`RabbitConnectionManager::consumer_channel`] opens a plain channel without
/// `confirm_select` (the reply is fire-and-forget, outside the cached producer
/// confirm path) bounded by its open bound, capturing the origin connection
/// BEFORE the open await so a channel-open failure only demotes a dead origin.
/// A failure here is an infrastructure side effect: the caller records it
/// (b-prime) and warns, but the route's own disposition is unchanged (G-10).
/// Only this reply's own channel is closed — the shared connection is never
/// reset (if the connection is actually dead the ack fails on its own and the
/// existing stale-generation drop lets the broker redeliver).
pub(crate) async fn send_reply_on_managed_channel(
    intent: &ReplyIntent,
    manager: &Arc<RabbitConnectionManager>,
) -> Result<(), CamelError> {
    let (channel, _generation, _origin) = manager.consumer_channel().await?;
    let result = send_reply(intent, &channel).await;
    let _ = tokio::time::timeout(
        crate::producer::CHANNEL_CLOSE_BOUND,
        channel.close(200, ShortString::from("rabbitmq reply complete")),
    )
    .await;
    result
}
