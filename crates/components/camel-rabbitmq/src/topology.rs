//! Consumer-start topology checks (tasks 3.3/3.4).
//!
//! The consumer start runs exactly ONE topology check on a dedicated
//! short-lived probe channel, created through the manager's bounded
//! `consumer_channel` helper and dropped after the check:
//!
//! - `autoDeclare=false` (default, task 3.3): a passive `queue_declare` asks
//!   the broker whether the queue exists. A missing queue fails fast.
//! - `autoDeclare=true` (task 3.4): the exchange, queue, and binding are
//!   actively declared. When the endpoint exchange is the default exchange
//!   (empty path) the exchange declare and the bind are SKIPPED — the reserved
//!   default exchange cannot be declared or bound (the broker answers 403) —
//!   but the queue is still declared with its durability and x-args.
//!
//! A soft AMQP error (404/405/406) closes ONLY the probe channel: the
//! connection-scoped error listener classifies it as channel-local, so the
//! shared connection/generation is never demoted and sibling consumers or
//! producers keep consuming. Task 3.5 classifies the broker reply by its typed
//! AMQP code: a passive-probe 404 is [`RabbitError::MissingQueue`] and an
//! active-declare 405/406 is [`RabbitError::DeclareConflict`] carrying the
//! broker's verbatim `PRECONDITION_FAILED` text, so a conflict fails the route
//! start (fail closed). Every other probe failure is a neutral
//! [`RabbitError::PublishFailed`] that still names the queue/target.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use lapin::ExchangeKind;
use lapin::options::{ExchangeDeclareOptions, QueueBindOptions, QueueDeclareOptions};
use lapin::types::{AMQPValue, FieldTable, LongString, ShortString};

use camel_component_api::CamelError;

use crate::config::RabbitEndpointConfig;
use crate::connection::RabbitConnectionManager;
use crate::error::RabbitError;

/// Bound on a single topology RPC (passive declare, exchange declare, queue
/// declare, or bind).
///
/// A live broker answers each immediately; this only fires on a half-dead
/// connection and keeps every op within a sensible order of magnitude. It
/// matches the manager's own `channel.open` bound, so start never hangs on an
/// absent queue.
pub(crate) const PASSIVE_CHECK_BOUND: Duration = Duration::from_secs(10);

/// Bound on the probe channel's explicit close.
///
/// Best-effort: when it does not resolve the channel drops and lapin's
/// `ChannelCloser` completes (or aborts) the close on the last clone — no task
/// is spawned by us. A soft error that failed the probe already closed the
/// channel broker-side, so the wait normally resolves at once.
const PROBE_CLOSE_BOUND: Duration = Duration::from_secs(1);

/// Run the consumer-start topology check on a dedicated short-lived probe
/// channel (tasks 3.3/3.4).
///
/// Opening the probe is bounded by the manager helper; the probe itself is
/// closed bounded on both the success and failure paths. A missing queue or a
/// conflicting active declare is a soft channel-local error, so the shared
/// connection stays healthy for siblings.
pub(crate) async fn topology_check(
    manager: &Arc<RabbitConnectionManager>,
    config: &RabbitEndpointConfig,
) -> Result<(), CamelError> {
    // A channel-open failure is classified against the captured origin inside
    // the helper, so it demotes only an actually dead connection.
    let (probe, _generation, _origin) = manager.consumer_channel().await?;

    let result = if config.auto_declare {
        declare_topology(&probe, config).await
    } else {
        let queue = config.queue.clone().unwrap_or_default();
        passive_queue_check(&probe, &queue).await
    };

    close_probe_channel(probe).await;
    result
}

/// Passive queue-exists probe: a `queue_declare` with `passive = true`.
///
/// The task 3.3 fault is a soft 404 that closes only the probe channel.
async fn passive_queue_check(channel: &lapin::Channel, queue: &str) -> Result<(), CamelError> {
    let declare = tokio::time::timeout(
        PASSIVE_CHECK_BOUND,
        channel.queue_declare(
            ShortString::from(queue),
            QueueDeclareOptions {
                passive: true,
                ..QueueDeclareOptions::default()
            },
            FieldTable::default(),
        ),
    )
    .await;

    match declare {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) => {
            // The queue name is explicit and the field is not a redaction
            // identifier; no URL or credential is ever rendered here.
            tracing::warn!(
                queue = %queue,
                broker_error = %error,
                "rabbitmq consumer passive queue check failed; consumer will not start"
            );
            Err(RabbitError::for_queue_error(queue, &error).into())
        }
        Err(_) => {
            tracing::warn!(
                queue = %queue,
                "rabbitmq consumer passive queue check timed out; consumer will not start"
            );
            Err(RabbitError::PublishFailed(format!(
                "rabbitmq consumer passive queue check failed for queue '{queue}': \
                 timed out after {PASSIVE_CHECK_BOUND:?}"
            ))
            .into())
        }
    }
}

/// Active declare of the endpoint topology (task 3.4).
///
/// Order: exchange (named endpoints only), queue, then bind (named endpoints
/// only). The exchange is always declared durable; `durableQueue` governs the
/// queue only. The default exchange (empty path) cannot be declared or bound
/// (the broker answers 403), so both are skipped.
pub(crate) async fn declare_topology(
    channel: &lapin::Channel,
    config: &RabbitEndpointConfig,
) -> Result<(), CamelError> {
    let queue = config.queue.clone().unwrap_or_default();
    let exchange = config.exchange.as_str();
    // An explicit routing key wins; the bind uses the publish target's key.
    let routing_key = config.target().1;

    if !exchange.is_empty() {
        rpc_bounded(
            "exchange",
            exchange,
            channel.exchange_declare(
                ShortString::from(exchange),
                exchange_kind(&config.exchange_type),
                ExchangeDeclareOptions {
                    durable: true,
                    ..ExchangeDeclareOptions::default()
                },
                FieldTable::default(),
            ),
        )
        .await?;
    }

    rpc_bounded(
        "queue",
        &queue,
        channel.queue_declare(
            ShortString::from(queue.as_str()),
            QueueDeclareOptions {
                durable: config.durable_queue,
                ..QueueDeclareOptions::default()
            },
            queue_arguments_field_table(&config.queue_arguments),
        ),
    )
    .await?;

    if !exchange.is_empty() {
        rpc_bounded(
            "binding",
            &queue,
            channel.queue_bind(
                ShortString::from(queue.as_str()),
                ShortString::from(exchange),
                ShortString::from(routing_key.as_str()),
                QueueBindOptions::default(),
                FieldTable::default(),
            ),
        )
        .await?;
    }

    Ok(())
}

/// Run one topology RPC under [`PASSIVE_CHECK_BOUND`], mapping both a broker
/// error and a local timeout onto the start's error path.
async fn rpc_bounded<T>(
    op: &str,
    target: &str,
    fut: impl std::future::Future<Output = Result<T, lapin::Error>>,
) -> Result<T, CamelError> {
    match tokio::time::timeout(PASSIVE_CHECK_BOUND, fut).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => {
            tracing::warn!(
                op = %op,
                topology_target = %target,
                broker_error = %error,
                "rabbitmq consumer auto-declare failed; consumer will not start"
            );
            Err(RabbitError::for_declare_error(op, target, &error).into())
        }
        Err(_) => {
            tracing::warn!(
                op = %op,
                topology_target = %target,
                "rabbitmq consumer auto-declare timed out; consumer will not start"
            );
            Err(RabbitError::PublishFailed(format!(
                "rabbitmq consumer auto-declare {op} failed for '{target}': \
                 timed out after {PASSIVE_CHECK_BOUND:?}"
            ))
            .into())
        }
    }
}

/// Map an `exchangeType` string onto lapin's [`ExchangeKind`].
///
/// The four standard AMQP kinds are mapped exactly; any other string is passed
/// through as [`ExchangeKind::Custom`], so a broker plugin type works and a
/// broker-rejected type fails the route start (fail closed).
fn exchange_kind(exchange_type: &str) -> ExchangeKind {
    match exchange_type {
        "direct" => ExchangeKind::Direct,
        "fanout" => ExchangeKind::Fanout,
        "topic" => ExchangeKind::Topic,
        "headers" => ExchangeKind::Headers,
        custom => ExchangeKind::Custom(custom.to_string()),
    }
}

/// Convert the parsed `queueArguments` map into long-string `FieldTable`
/// entries (e.g. `x-dead-letter-exchange`).
fn queue_arguments_field_table(arguments: &HashMap<String, String>) -> FieldTable {
    let mut table = FieldTable::default();
    for (key, value) in arguments {
        table.insert(
            ShortString::from(key.as_str()),
            AMQPValue::LongString(LongString::from(value.as_str())),
        );
    }
    table
}

/// Close the short-lived probe channel under a bounded wait.
///
/// Best-effort, on both the success and failure paths. If the wait does not
/// resolve the channel is dropped and lapin's `ChannelCloser::drop` sends
/// `CloseChannel` (when the id is non-zero and the connection still lives); the
/// probe channel is never reused.
async fn close_probe_channel(channel: lapin::Channel) {
    let _ = tokio::time::timeout(
        PROBE_CLOSE_BOUND,
        channel.close(200, ShortString::from("rabbitmq topology probe complete")),
    )
    .await;
}
