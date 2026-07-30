//! Shared test harness for the `template:` scheme integration tests
//! (ADR-0047 Stage 2, Phase 4 / 5).
//!
//! Provides three entry points used by both `template_render_integration.rs`
//! and `template_reload_integration.rs`:
//!
//! - [`start_template_route`] — build a `CamelContext` with a
//!   `direct:in → template:<uri>` route.
//! - [`send_title`] — drive a `title=<title>` exchange through the route's
//!   `direct:in` producer.
//! - [`body_text`] — extract the rendered body as a string.
//!
//! # `direct:in` consumer-registration race
//!
//! `CamelContext::start()` returns after each route's `start_route` resolves,
//! which in turn awaits the `DirectConsumer::start()` call. `start()` inserts
//! the consumer's `mpsc::Sender` into the shared `DirectRegistry` SYNCHRONOUSLY
//! before returning. In principle the registry entry is therefore visible by
//! the time `ctx.start()` returns.
//!
//! In practice, under the full-suite parallel `cargo test -p camel-template`,
//! the `direct` producer's `poll_ready` has been observed to hit
//! `EndpointCreationFailed("direct endpoint 'in' not registered")` on a small
//! fraction of runs (the reviewer observed ~25% failures). The same code
//! passes deterministically with `--test-threads=1` and per isolated
//! `--test` invocation. `send_title` therefore retries on the
//! `"…not registered"` error for a bounded window so the integration suite
//! stays stable across CI reruns without touching production code.
//!
//! The retry only fires for the specific registration-race error; any other
//! failure (a real missing consumer at the deadline, a render error, etc.)
//! propagates immediately and panics the test.

use std::sync::Arc;
use std::time::Duration;

use camel_api::{Body, CamelError, Exchange, Message, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::{NoOpComponentContext, RuntimeObservability};
use camel_component_direct::DirectComponent;
use camel_core::CamelContext;
use camel_template::TemplateComponent;
use tower::ServiceExt;

/// Maximum wall-clock time [`send_title`] will keep retrying on a transient
/// `direct endpoint 'in' not registered` error. Chosen long enough to absorb
/// the slow parallel-suite startup that surfaces the race, short enough that
/// a real missing consumer fails the test promptly.
const RACE_RETRY_TIMEOUT: Duration = Duration::from_secs(2);
/// Poll interval between retries. Small enough to keep the test snappy when
/// the consumer is mid-registration, large enough to avoid a tight spin.
const RACE_RETRY_POLL: Duration = Duration::from_millis(5);

/// `true` iff `err` is the specific registration-race error the `direct`
/// producer emits when its `poll_ready` check runs before the consumer has
/// finished inserting itself into the per-instance `DirectRegistry`.
fn is_direct_not_registered(err: &CamelError) -> bool {
    matches!(
        err,
        CamelError::EndpointCreationFailed(msg)
            if msg.starts_with("direct endpoint '") && msg.ends_with("' not registered")
    )
}

/// Build and start a context with one route: `direct:in → template:<uri>`.
pub async fn start_template_route(
    template_uri: &str,
    route_id: &str,
) -> Result<CamelContext, CamelError> {
    let mut ctx = CamelContext::builder()
        .build()
        .await
        .expect("context build");
    ctx.register_component(DirectComponent::new());
    ctx.register_component(TemplateComponent::default());
    let route = RouteBuilder::from("direct:in")
        .route_id(route_id)
        .to(template_uri.to_string())
        .build()
        .expect("route build");
    ctx.add_route_definition(route)
        .await
        .expect("add_route_definition");
    ctx.start().await?;
    Ok(ctx)
}

/// Drive one exchange with `title=<title>` through the route's `direct:in`
/// producer and return the processed exchange. Retries on the
/// consumer-registration race; panics on any other error.
pub async fn send_title(ctx: &CamelContext, title: &str) -> Exchange {
    let deadline = tokio::time::Instant::now() + RACE_RETRY_TIMEOUT;
    loop {
        let outcome = send_title_once(ctx, title).await;
        match outcome {
            Ok(exchange) => return exchange,
            Err(err)
                if is_direct_not_registered(&err) && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(RACE_RETRY_POLL).await;
            }
            Err(err) => panic!("render request failed: {err}"),
        }
    }
}

async fn send_title_once(ctx: &CamelContext, title: &str) -> Result<Exchange, CamelError> {
    let producer = {
        let producer_ctx = ctx.producer_context();
        let registry = ctx.registry();
        let component = registry
            .get("direct")
            .expect("direct component not registered")
            .clone();
        let endpoint = component
            .create_endpoint("direct:in", ctx)
            .expect("direct endpoint");
        let rt: Arc<dyn RuntimeObservability> = Arc::new(NoOpComponentContext);
        endpoint
            .create_producer(rt, &producer_ctx)
            .expect("direct producer")
    };

    let mut msg = Message::new(Body::Empty);
    msg.headers
        .insert("title".to_string(), Value::String(title.to_string()));
    producer.oneshot(Exchange::new(msg)).await
}

/// Extract the rendered body as a string (the producer writes `Body::Text`).
pub fn body_text(ex: &Exchange) -> String {
    match &ex.input.body {
        Body::Text(s) => s.clone(),
        Body::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        other => panic!("expected rendered text body, got: {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_not_registered_matcher_is_specific() {
        assert!(is_direct_not_registered(
            &CamelError::EndpointCreationFailed("direct endpoint 'in' not registered".to_string())
        ));
        assert!(!is_direct_not_registered(
            &CamelError::EndpointCreationFailed("direct endpoint 'in' channel closed".to_string())
        ));
        assert!(!is_direct_not_registered(&CamelError::ProcessorError(
            "direct endpoint 'in' not registered".to_string()
        )));
    }

    /// Sanity check: a closure that fails with the race error for the first
    /// two attempts and succeeds on the third must converge inside
    /// `RACE_RETRY_TIMEOUT`. Wraps `send_title`-style retry logic in a
    /// controllable op to keep the unit test hermetic.
    #[tokio::test]
    async fn retry_loop_eventually_succeeds() {
        use std::sync::Mutex;

        let attempts = std::sync::Arc::new(Mutex::new(0_u8));
        let deadline = tokio::time::Instant::now() + RACE_RETRY_TIMEOUT;
        let result: Result<(), CamelError> = loop {
            let attempts_for_op = attempts.clone();
            let outcome: Result<(), CamelError> = async move {
                let mut guard = attempts_for_op.lock().expect("attempts lock");
                *guard += 1;
                if *guard < 3 {
                    Err(CamelError::EndpointCreationFailed(
                        "direct endpoint 'in' not registered".to_string(),
                    ))
                } else {
                    Ok(())
                }
            }
            .await;
            match outcome {
                Ok(()) => break Ok(()),
                Err(err)
                    if is_direct_not_registered(&err) && tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(RACE_RETRY_POLL).await;
                }
                Err(err) => break Err(err),
            }
        };
        assert!(result.is_ok());
        assert_eq!(*attempts.lock().expect("attempts lock"), 3);
    }
}
