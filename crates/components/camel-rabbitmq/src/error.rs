//! Consolidated RabbitMQ component error taxonomy (task 3.5).
//!
//! The error constructors scattered across tasks 1.3–3.4 now funnel through
//! [`RabbitError`]. Every variant maps onto [`CamelError::ProcessorError`] (the
//! component-error-semantics mapping the kafka component uses) through the
//! [`From`] impl, so route processing keeps classifying RabbitMQ failures as
//! processor errors and never panics.
//!
//! The kafka component does not preserve a source chain
//! (`ProcessorErrorWithSource` is not used there), so this taxonomy converts to
//! the plain `ProcessorError` and adds no dependency: `thiserror` was already a
//! crate dependency.
//!
//! `#[non_exhaustive]` lets Phase 4 add `ReplyTimeout { timeout_ms }` (task
//! 4.1) without a breaking change.

use camel_component_api::CamelError;
use thiserror::Error;

/// AMQP reply code for `NOT-FOUND`: the broker's proof a passively checked
/// queue does not exist.
const AMQP_NOT_FOUND: u16 = 404;

/// AMQP reply code for `RESOURCE-LOCKED`.
const AMQP_RESOURCE_LOCKED: u16 = 405;

/// AMQP reply code for `PRECONDITION-FAILED`: a conflicting active declare.
const AMQP_PRECONDITION_FAILED: u16 = 406;

/// Consolidated RabbitMQ failure taxonomy.
///
/// `#[non_exhaustive]`: Phase 4 appends `ReplyTimeout` (task 4.1).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum RabbitError {
    /// The manager has no live broker connection within the bounded wait.
    #[error("rabbitmq connection disconnected")]
    Disconnected,

    /// The broker did not confirm a publish within `confirmTimeout`.
    #[error(
        "rabbitmq publish confirm timed out awaiting the broker confirm for exchange \
         '{exchange}' routing key '{routing_key}'"
    )]
    ConfirmTimeout {
        /// Publish target exchange.
        exchange: String,
        /// Publish target routing key.
        routing_key: String,
    },

    /// The broker nacked a publish without returning a message.
    #[error(
        "rabbitmq publish to exchange '{exchange}' routing key '{routing_key}' was nacked \
         by the broker"
    )]
    ConfirmNacked {
        /// Publish target exchange.
        exchange: String,
        /// Publish target routing key.
        routing_key: String,
    },

    /// A mandatory publish was returned by the broker as unroutable.
    #[error(
        "rabbitmq publish to exchange '{exchange}' routing key '{routing_key}' was returned \
         by the broker as unroutable"
    )]
    Unroutable {
        /// Publish target exchange.
        exchange: String,
        /// Publish target routing key.
        routing_key: String,
    },

    /// The passive queue probe got a broker `404 NOT_FOUND`: the queue is
    /// absent.
    #[error("rabbitmq queue '{0}' does not exist")]
    MissingQueue(String),

    /// An active topology declare failed with a `405`/`406` conflict. The
    /// payload is the broker's verbatim protocol text (it carries
    /// `PRECONDITION-FAILED` and the queue/exchange names).
    #[error("rabbitmq topology declare conflict: {0}")]
    DeclareConflict(String),

    /// An InOut request got no direct reply-to response within `replyTimeout`
    /// (task 4.1). The payload is the configured bound in milliseconds.
    #[error("rabbitmq reply timed out after {timeout_ms} ms awaiting the direct reply-to response")]
    ReplyTimeout {
        /// The configured `replyTimeout` bound in milliseconds.
        timeout_ms: u64,
    },

    /// Any other broker, channel, or infrastructure failure. The generic
    /// `publish failed` label is a deliberate catch-all: the taxonomy has no
    /// dedicated variant for a connect/start bound.
    #[error("rabbitmq publish failed: {0}")]
    PublishFailed(String),
}

impl From<RabbitError> for CamelError {
    fn from(error: RabbitError) -> Self {
        CamelError::ProcessorError(error.to_string())
    }
}

impl RabbitError {
    /// Classify a broker error from the passive queue probe.
    ///
    /// Only a broker `404 NOT_FOUND` proves absence ([`Self::MissingQueue`]).
    /// A timeout, a permission failure, or any other reply code is neutral
    /// ([`Self::PublishFailed`]) and still names the queue, so absence is never
    /// inferred from an unrelated failure.
    pub(crate) fn for_queue_error(queue: &str, error: &lapin::Error) -> Self {
        if amqp_reply_code(error) == Some(AMQP_NOT_FOUND) {
            Self::MissingQueue(queue.to_string())
        } else {
            Self::PublishFailed(format!(
                "rabbitmq consumer passive queue check failed for queue '{queue}': {error}"
            ))
        }
    }

    /// Classify a broker error from an active topology declare RPC.
    ///
    /// A `405 RESOURCE_LOCKED` or `406 PRECONDITION_FAILED` is a declare
    /// conflict ([`Self::DeclareConflict`]) carrying the broker's verbatim text
    /// (so `PRECONDITION` and the queue/exchange names survive); any other code
    /// is neutral.
    pub(crate) fn for_declare_error(op: &str, target: &str, error: &lapin::Error) -> Self {
        let detail = format!("rabbitmq consumer auto-declare {op} failed for '{target}': {error}");
        match amqp_reply_code(error) {
            Some(AMQP_RESOURCE_LOCKED) | Some(AMQP_PRECONDITION_FAILED) => {
                Self::DeclareConflict(detail)
            }
            _ => Self::PublishFailed(detail),
        }
    }
}

/// The AMQP reply code of a broker protocol error, if the error is one.
///
/// Reads the typed `AMQPError` reply identifier (`u16`) instead of parsing the
/// rendered message, so classification never depends on broker wording.
fn amqp_reply_code(error: &lapin::Error) -> Option<u16> {
    match error.kind() {
        lapin::ErrorKind::ProtocolError(amqp) => Some(amqp.get_id()),
        _ => None,
    }
}

/// Test-only constructor for a lapin protocol error with a specific AMQP reply
/// code, without a broker. Uses the public `AMQPError::new` +
/// `From<ErrorKind> for Error` seam; no third-party test hook is added to
/// lapin.
#[cfg(test)]
pub(crate) fn protocol_error(code: u16, message: &str) -> lapin::Error {
    use lapin::protocol::{AMQPError, AMQPErrorKind, AMQPSoftError};
    use lapin::types::ShortString;

    let kind = AMQPErrorKind::Soft(match code {
        404 => AMQPSoftError::NOTFOUND,
        405 => AMQPSoftError::RESOURCELOCKED,
        406 => AMQPSoftError::PRECONDITIONFAILED,
        other => panic!("unsupported test reply code {other}"),
    });
    lapin::Error::from(lapin::ErrorKind::ProtocolError(AMQPError::new(
        kind,
        ShortString::from(message),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Task 3.5: every variant's `Display` names its required token —
    /// exchange, routing key, queue name, `PRECONDITION`, and `disconnected` —
    /// and the typed broker classification only calls a `404` a missing queue
    /// and a `405`/`406` a declare conflict.
    #[test]
    fn error_display_names_targets() {
        let cases: Vec<(RabbitError, &[&str])> = vec![
            (RabbitError::Disconnected, &["disconnected"]),
            (
                RabbitError::ConfirmTimeout {
                    exchange: "orders.exchange".to_string(),
                    routing_key: "rk-42".to_string(),
                },
                &["confirm", "orders.exchange", "rk-42"],
            ),
            (
                RabbitError::ConfirmNacked {
                    exchange: "orders.exchange".to_string(),
                    routing_key: "rk-42".to_string(),
                },
                &["nacked", "orders.exchange", "rk-42"],
            ),
            (
                RabbitError::Unroutable {
                    exchange: "orders.exchange".to_string(),
                    routing_key: "rk-42".to_string(),
                },
                &["unroutable", "orders.exchange", "rk-42"],
            ),
            (
                RabbitError::ReplyTimeout { timeout_ms: 2000 },
                &["reply", "2000"],
            ),
            (
                RabbitError::MissingQueue("orders-missing".to_string()),
                &["orders-missing"],
            ),
            (
                RabbitError::DeclareConflict(
                    "PRECONDITION_FAILED - inequivalent arg for queue 'orders'".to_string(),
                ),
                &["PRECONDITION", "orders"],
            ),
            (
                RabbitError::PublishFailed("broker detail".to_string()),
                &["broker detail"],
            ),
        ];

        for (error, tokens) in cases {
            let message = error.to_string();
            for token in tokens {
                assert!(
                    message.contains(token),
                    "`{error:?}` Display must contain `{token}`, got: {message}"
                );
            }
        }

        // Typed classification by AMQP reply code, not message text.
        let not_found = protocol_error(404, "NOT_FOUND - no queue 'orders' in vhost '/'");
        assert_eq!(
            RabbitError::for_queue_error("orders", &not_found),
            RabbitError::MissingQueue("orders".to_string())
        );
        let io = lapin::Error::from(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "passive probe stalled",
        ));
        assert!(matches!(
            RabbitError::for_queue_error("orders", &io),
            RabbitError::PublishFailed(detail) if detail.contains("orders")
        ));

        let conflict = protocol_error(406, "PRECONDITION_FAILED - queue 'orders' mismatch");
        assert!(matches!(
            RabbitError::for_declare_error("queue", "orders", &conflict),
            RabbitError::DeclareConflict(detail)
                if detail.contains("PRECONDITION") && detail.contains("orders")
        ));
        assert!(matches!(
            RabbitError::for_declare_error("queue", "orders", &io),
            RabbitError::PublishFailed(_)
        ));

        // The taxonomy converts to a plain processor error (kafka mapping),
        // never a panic.
        let converted: CamelError = RabbitError::Disconnected.into();
        assert!(matches!(converted, CamelError::ProcessorError(_)));
        assert!(converted.to_string().contains("disconnected"));
    }
}
