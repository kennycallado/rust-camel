// Shared test helpers for producer tests.
// All items are `pub(super)` — visible to the parent `producer_tests` module
// and its children (sibling test modules).

use std::sync::Arc;
use std::time::Duration;

use camel_api::{Body, Exchange, Message};
use camel_component_api::NetworkRetryPolicy;
use futures::StreamExt;
use tokio::sync::Semaphore;

use crate::config::LlmOperation;
use crate::cost::PricingTable;
use crate::producer::LlmProducer;
use crate::provider::LlmProvider;
use crate::provider::mock::{MockMode, MockProvider};

/// Create a producer with an optional retry policy (materialized default).
pub(super) fn make_producer_with_retry(
    provider: Arc<dyn LlmProvider>,
    stream: bool,
    retry: Option<NetworkRetryPolicy>,
) -> LlmProducer {
    use crate::LlmEndpointConfig;
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream,
        ..Default::default()
    };
    LlmProducer::new(config, provider, 32768, "test-route".into())
        .with_retry(retry)
        .build()
}

/// Helper: create a producer with both timeout and retry configured.
pub(super) fn make_producer_with_timeout_and_retry(
    provider: Arc<dyn LlmProvider>,
    stream: bool,
    timeout: Duration,
    retry: Option<NetworkRetryPolicy>,
) -> LlmProducer {
    use crate::LlmEndpointConfig;
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream,
        ..Default::default()
    };
    LlmProducer::new(config, provider, 32768, "test-route".into())
        .with_timeout(Some(timeout))
        .with_retry(retry)
        .build()
}

/// Helper: create a producer with both semaphore and retry configured.
pub(super) fn make_producer_with_concurrency_and_retry(
    provider: Arc<dyn LlmProvider>,
    max_concurrency: usize,
    retry: Option<NetworkRetryPolicy>,
) -> LlmProducer {
    use crate::LlmEndpointConfig;
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream: false,
        ..Default::default()
    };
    let semaphore = Some(Arc::new(Semaphore::new(max_concurrency)));
    LlmProducer::new(config, provider, 32768, "test-route".into())
        .with_semaphore(semaphore)
        .with_retry(retry)
        .build()
}

pub(super) fn make_producer(stream: bool, operation: LlmOperation) -> LlmProducer {
    use crate::LlmEndpointConfig;
    let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hello".into())));
    let config = LlmEndpointConfig {
        operation,
        stream,
        ..Default::default()
    };
    LlmProducer::new(config, provider, 32768, "test-route".into()).build()
}

pub(super) fn make_exchange(body: Body) -> Exchange {
    Exchange::new(Message::new(body))
}

/// Helper: create a producer with pricing configured for materialized mode.
pub(super) fn make_producer_with_pricing(
    provider: Arc<dyn LlmProvider>,
    input_price: f64,
    output_price: f64,
) -> LlmProducer {
    use crate::LlmEndpointConfig;
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream: false,
        ..Default::default()
    };
    let pricing = Arc::new(PricingTable {
        input_per_1k_tokens: input_price,
        output_per_1k_tokens: output_price,
    });
    LlmProducer::new(config, provider, 32768, "test-route".into())
        .with_pricing(Some(pricing))
        .build()
}

/// Helper: create a producer with a given timeout.
pub(super) fn make_producer_with_timeout(
    provider: Arc<dyn LlmProvider>,
    stream: bool,
    timeout: Duration,
) -> LlmProducer {
    use crate::LlmEndpointConfig;
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream,
        ..Default::default()
    };
    LlmProducer::new(config, provider, 32768, "test-route".into())
        .with_timeout(Some(timeout))
        .build()
}

/// A helper that creates a producer with a semaphore for concurrency tests.
pub(super) fn make_producer_with_semaphore(
    provider: Arc<dyn LlmProvider>,
    stream: bool,
    max_concurrency: usize,
) -> LlmProducer {
    use crate::LlmEndpointConfig;
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream,
        ..Default::default()
    };
    let semaphore = Some(Arc::new(Semaphore::new(max_concurrency)));
    LlmProducer::new(config, provider, 32768, "test-route".into())
        .with_semaphore(semaphore)
        .build()
}

/// Drain a streaming body to completion (consume entire stream) and
/// clear the inner stream option so the `PermitStream` (and its
/// semaphore permit) is dropped.
pub(super) async fn drain_stream(out: &mut Exchange) {
    if let Body::Stream(sb) = &out.input.body {
        let mut guard = sb.stream.lock().await;
        if let Some(stream) = guard.as_mut() {
            while stream.next().await.is_some() {}
        }
        // Clear the stream — this drops the PermitStream and releases
        // the semaphore permit (permit lives in the stream, see ADR-0021).
        *guard = None;
    }
}

/// Pull exactly one event from a streaming body (to start it) without
/// draining it fully.
pub(super) async fn drain_one_chunk(out: &mut Exchange) {
    if let Body::Stream(sb) = &out.input.body {
        let mut guard = sb.stream.lock().await;
        if let Some(stream) = guard.as_mut() {
            let _ = stream.next().await;
        }
    }
}
