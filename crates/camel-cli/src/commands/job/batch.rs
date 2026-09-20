//! Batch drain for `mode: batch` jobs: poll
//! [`CamelContext::total_in_flight()`] until it reads zero.
//!
//! Drain contract:
//!
//! - The verdict is a SINGLE linearizable read of the context-global
//!   accepted-not-completed counter. Every exchange accepted through a
//!   counted path (seda enqueue, `ConsumerContext::send` /
//!   `send_and_wait` dispatch, inline dispatch) holds an RAII
//!   `InFlightClaim` across its whole lifecycle — seda queue residency,
//!   dispatch, and route-pipeline residency — so a zero read states
//!   that no counted exchange is mid-lifecycle. The raw-sender fast
//!   path (`sender()`) stays uncounted; that exception is documented in
//!   the observability spec.
//! - No timed samples, queue-depth labels, or quiescence window: unlike
//!   the deleted seda-gauge streak heuristic, sampling gaps cannot
//!   manufacture a false zero because claims cover the whole exchange
//!   lifecycle, including the route-pipeline residency the gauge never
//!   saw.
//! - Deadline discipline: poll at most every 100 ms and never past
//!   `deadline`, so a queue that never drains deterministically hits
//!   the overall deadline (`Timeout` verdict).

use std::time::Duration;

use camel_core::CamelContext;

/// Wait until `ctx.total_in_flight()` reads zero, napping at most
/// 100 ms at a time and never past `deadline`. Returns false once the
/// deadline has passed.
pub(crate) async fn drain_until_settled(
    ctx: &CamelContext,
    deadline: tokio::time::Instant,
) -> bool {
    loop {
        if ctx.total_in_flight() == 0 {
            return true;
        }
        let now = tokio::time::Instant::now();
        let nap = deadline
            .saturating_duration_since(now)
            .min(Duration::from_millis(100));
        if nap.is_zero() {
            return false;
        }
        tokio::time::sleep(nap).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use camel_api::{Exchange, Message};
    use camel_component_api::{ComponentContext, ExchangeEnvelope, InFlightClaim};

    #[tokio::test]
    async fn drain_until_settled_zero_completes() {
        let ctx = camel_core::CamelContext::builder().build().await.unwrap();
        let started = tokio::time::Instant::now();
        let deadline = started + Duration::from_secs(5);
        assert!(drain_until_settled(&ctx, deadline).await);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "an idle context must settle on the first zero read, with no \
             quiescence window; took {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn drain_until_settled_waits_for_live_claim() {
        let ctx = camel_core::CamelContext::builder().build().await.unwrap();
        let counter = ctx
            .in_flight_counter()
            .expect("core context installs the in-flight counter");
        let envelope = ExchangeEnvelope {
            exchange: Exchange::new(Message::new("parked")),
            reply_tx: None,
            in_flight_claim: Some(InFlightClaim::attach(&counter)),
        };
        assert_eq!(ctx.total_in_flight(), 1, "the attached claim is live");

        let short_deadline = tokio::time::Instant::now() + Duration::from_millis(250);
        assert!(
            !drain_until_settled(&ctx, short_deadline).await,
            "a live claim must hold the drain until the deadline"
        );

        drop(envelope);
        assert_eq!(ctx.total_in_flight(), 0, "the dropped claim is released");
        let started = tokio::time::Instant::now();
        let generous_deadline = started + Duration::from_secs(5);
        assert!(drain_until_settled(&ctx, generous_deadline).await);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "dropping the envelope must settle the drain promptly; took {:?}",
            started.elapsed()
        );
    }
}
