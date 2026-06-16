use std::sync::Arc;
use std::time::Duration;

use camel_api::{Body, Exchange, Message};
use camel_component_llm::config::LlmEndpointConfig;
use camel_component_llm::cost::PricingTable;
use camel_component_llm::headers::*;
use camel_component_llm::producer::LlmProducer;
use camel_component_llm::producer_cache::ProducerCache;
use camel_component_llm::provider::LlmProvider;
use camel_component_llm::provider::mock::{MockMode, MockProvider};
use tokio::sync::Semaphore;
use tower::Service;

fn make_exchange(body: Body) -> Exchange {
    Exchange::new(Message::new(body))
}

/// Build a producer with cache enabled for materialized mode.
fn make_producer_with_cache(provider: Arc<dyn LlmProvider>, cache_ttl: Duration) -> LlmProducer {
    let config = LlmEndpointConfig {
        operation: camel_component_llm::config::LlmOperation::Chat,
        stream: false,
        ..Default::default()
    };
    let cache = Arc::new(ProducerCache::new(cache_ttl, None));
    LlmProducer::new(config, provider, 32768, "test-route".into())
        .with_cache(Some(cache))
        .build()
}

/// Build a producer with cache + pricing + timeout enabled.
fn make_producer_with_cache_pricing_timeout(
    provider: Arc<dyn LlmProvider>,
    cache_ttl: Duration,
    input_price: f64,
    output_price: f64,
    timeout: Option<Duration>,
    max_concurrency: Option<usize>,
) -> LlmProducer {
    let config = LlmEndpointConfig {
        operation: camel_component_llm::config::LlmOperation::Chat,
        stream: false,
        ..Default::default()
    };
    let cache = Arc::new(ProducerCache::new(cache_ttl, None));
    let pricing = Arc::new(PricingTable {
        input_per_1k_tokens: input_price,
        output_per_1k_tokens: output_price,
    });
    let semaphore = max_concurrency.map(|n| Arc::new(Semaphore::new(n)));
    LlmProducer::new(config, provider, 32768, "test-route".into())
        .with_semaphore(semaphore)
        .with_timeout(timeout)
        .with_pricing(Some(pricing))
        .with_cache(Some(cache))
        .build()
}

/// If the exchange body is Text, strip the wrapping quotes if present
/// (for consistent assertion).
fn body_text(out: &Exchange) -> String {
    match &out.input.body {
        Body::Text(s) => s.clone(),
        other => panic!("expected Text body, got {other:?}"),
    }
}

// -----------------------------------------------------------------------
// Cache hit tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn cache_hit_skips_provider() {
    let mock = Arc::new(MockProvider::new("t", MockMode::Fixed("hello".into())));
    let provider = Arc::clone(&mock) as Arc<dyn LlmProvider>;
    let mut producer = make_producer_with_cache(provider, Duration::from_secs(60));

    // First call: materialized, goes to provider
    let out1 = producer
        .call(make_exchange(Body::Text("hello".into())))
        .await
        .expect("first call ok");
    assert_eq!(body_text(&out1), "hello");
    assert_eq!(mock.call_count(), 1);

    // Second call: identical prompt, should hit cache
    let out2 = producer
        .call(make_exchange(Body::Text("hello".into())))
        .await
        .expect("second call ok");
    assert_eq!(body_text(&out2), "hello");
    // Provider NOT called again
    assert_eq!(mock.call_count(), 1, "cache hit must skip provider call");
}

#[tokio::test]
async fn cache_miss_for_streaming_same_prompt() {
    let mock = Arc::new(MockProvider::new("t", MockMode::Fixed("hello".into())));
    let provider = Arc::clone(&mock) as Arc<dyn LlmProvider>;
    let cache = Arc::new(ProducerCache::new(Duration::from_secs(60), None));

    // Materialized call — caches the result
    let cfg_mat = LlmEndpointConfig {
        operation: camel_component_llm::config::LlmOperation::Chat,
        stream: false,
        ..Default::default()
    };
    let mut mat_producer =
        LlmProducer::new(cfg_mat, Arc::clone(&provider), 32768, "test-route".into())
            .with_cache(Some(Arc::clone(&cache)))
            .build();
    let out1 = mat_producer
        .call(make_exchange(Body::Text("hello".into())))
        .await
        .expect("materialized ok");
    assert_eq!(body_text(&out1), "hello");

    // Streaming call with same prompt — must NOT hit cache
    let cfg_stream = LlmEndpointConfig {
        operation: camel_component_llm::config::LlmOperation::Chat,
        stream: true,
        ..Default::default()
    };
    let mut stream_producer = LlmProducer::new(cfg_stream, provider, 32768, "test-route".into())
        .with_cache(Some(cache))
        .build();
    let out2 = stream_producer
        .call(make_exchange(Body::Text("hello".into())))
        .await
        .expect("streaming ok");
    // Streaming body — must have called provider again
    assert!(matches!(out2.input.body, Body::Stream(_)));
    assert_eq!(
        mock.call_count(),
        2,
        "streaming with same prompt must NOT hit materialized cache entry"
    );
}

// -----------------------------------------------------------------------
// Single-flight tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn single_flight_one_provider_call_under_concurrency() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("result".into()))
            .with_delay(Duration::from_millis(100)),
    );
    let provider = Arc::clone(&mock) as Arc<dyn LlmProvider>;
    let producer = make_producer_with_cache(provider, Duration::from_secs(60));

    let mut handles = vec![];
    for _ in 0..5 {
        let mut p = producer.clone();
        handles.push(tokio::spawn(async move {
            p.call(make_exchange(Body::Text("same prompt".into())))
                .await
        }));
    }
    for h in handles {
        let out = h.await.expect("join ok").expect("call ok");
        assert_eq!(body_text(&out), "result");
    }
    // Exactly 1 provider call despite 5 concurrent requests
    assert_eq!(
        mock.call_count(),
        1,
        "single-flight must coalesce N concurrent identical requests into 1 provider call"
    );
}

#[tokio::test]
async fn single_flight_error_fans_out_to_all_waiters() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_delay(Duration::from_millis(80))
            .with_fail_after(
                1,
                camel_component_llm::error::LlmError::Network("boom".into()),
            ),
    );
    let provider = Arc::clone(&mock) as Arc<dyn LlmProvider>;
    let producer = make_producer_with_cache(provider, Duration::from_secs(60));

    let mut handles = vec![];
    for _ in 0..3 {
        let mut p = producer.clone();
        handles.push(tokio::spawn(async move {
            p.call(make_exchange(Body::Text("x".into()))).await
        }));
    }
    for h in handles {
        let result = h.await.expect("join ok");
        // The inner Result<Exchange, CamelError> must be an error
        assert!(
            result.is_err(),
            "all waiters must receive an error when the leader fails"
        );
    }
    // Exactly 1 provider call
    assert_eq!(mock.call_count(), 1);
}

#[tokio::test]
async fn single_flight_waiter_timeout_abandons_leader_keeps_running() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into())).with_delay(Duration::from_millis(200)),
    );
    let provider = Arc::clone(&mock) as Arc<dyn LlmProvider>;
    let cache = Arc::new(ProducerCache::new(Duration::from_secs(60), None));

    // Leader: no timeout so it can complete and cache the result.
    let config_leader = LlmEndpointConfig {
        operation: camel_component_llm::config::LlmOperation::Chat,
        stream: false,
        ..Default::default()
    };
    let mut leader_producer = LlmProducer::new(
        config_leader,
        Arc::clone(&provider),
        32768,
        "test-route".into(),
    )
    .with_cache(Some(Arc::clone(&cache)))
    .build();

    // Spawn the leader first
    let leader_task = tokio::spawn(async move {
        leader_producer
            .call(make_exchange(Body::Text("x".into())))
            .await
    });

    // Give the leader time to start and acquire the in-flight slot
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Waiter: short timeout so it abandons before the leader finishes
    let config_waiter = LlmEndpointConfig {
        operation: camel_component_llm::config::LlmOperation::Chat,
        stream: false,
        ..Default::default()
    };
    let mut waiter_producer = LlmProducer::new(
        config_waiter,
        Arc::clone(&provider),
        32768,
        "test-route".into(),
    )
    .with_timeout(Some(Duration::from_millis(50))) // short timeout — waiter abandons
    .with_cache(Some(Arc::clone(&cache)))
    .build();
    let waiter_task = tokio::spawn(async move {
        waiter_producer
            .call(make_exchange(Body::Text("x".into())))
            .await
    });

    // Waiter must time out within 100ms (50ms timeout + scheduling margin)
    let waiter_result = tokio::time::timeout(Duration::from_millis(150), waiter_task)
        .await
        .expect("waiter must resolve within 150ms")
        .expect("waiter join ok");
    assert!(
        waiter_result.is_err(),
        "waiter must time out before the leader finishes"
    );

    // Wait for the leader to finish and cache the result
    let leader_result = leader_task.await.expect("leader join ok");
    assert!(leader_result.is_ok(), "leader should succeed");

    // Now a third call with no timeout should get a cached hit
    let config_hit = LlmEndpointConfig {
        operation: camel_component_llm::config::LlmOperation::Chat,
        stream: false,
        ..Default::default()
    };
    let mut hit_producer = LlmProducer::new(
        config_hit,
        // Dummy provider — won't be called on cache hit
        Arc::new(MockProvider::new("t", MockMode::Fixed("irrelevant".into())))
            as Arc<dyn LlmProvider>,
        32768,
        "test-route".into(),
    )
    .with_cache(Some(Arc::clone(&cache)))
    .build();
    let out3 = hit_producer
        .call(make_exchange(Body::Text("x".into())))
        .await
        .expect("third call must hit cached entry");
    assert_eq!(body_text(&out3), "ok");
}

#[tokio::test]
async fn leader_drop_does_not_strand_waiters() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into())).with_delay(Duration::from_millis(500)),
    );
    let provider = Arc::clone(&mock) as Arc<dyn LlmProvider>;
    let cache = Arc::new(ProducerCache::new(Duration::from_secs(60), None));
    let config = LlmEndpointConfig {
        operation: camel_component_llm::config::LlmOperation::Chat,
        stream: false,
        ..Default::default()
    };

    // Leader: NO timeout, provider takes 500ms → stays in-flight until aborted.
    let mut leader_producer = LlmProducer::new(
        config.clone(),
        Arc::clone(&provider),
        32768,
        "test-route".into(),
    )
    .with_cache(Some(Arc::clone(&cache)))
    .build();

    // Spawn the leader
    let leader_task = tokio::spawn(async move {
        leader_producer
            .call(make_exchange(Body::Text("x".into())))
            .await
    });

    // Give the leader time to start and acquire the in-flight slot
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Spawn a waiter — should register as waiter while leader is still running
    let mut waiter_producer = LlmProducer::new(
        config,
        // Dummy provider (won't be called)
        Arc::new(MockProvider::new("t", MockMode::Fixed("irrelevant".into())))
            as Arc<dyn LlmProvider>,
        32768,
        "test-route".into(),
    )
    .with_timeout(Some(Duration::from_millis(500))) // enough time to receive the cancellation error
    .with_cache(Some(Arc::clone(&cache)))
    .build();
    let waiter_task = tokio::spawn(async move {
        waiter_producer
            .call(make_exchange(Body::Text("x".into())))
            .await
    });

    // Give the waiter time to register as a waiter
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Abort the leader — this drops the task, which drops the LeaderHandle
    // (the `complete()` call is never reached), firing the Drop guard.
    leader_task.abort();

    // Waiter must resolve with an error (not hang) when the leader is aborted.
    let waiter_result = tokio::time::timeout(Duration::from_millis(300), waiter_task)
        .await
        .expect("waiter must not hang — it must resolve within 300ms")
        .expect("waiter join ok");
    assert!(
        waiter_result.is_err(),
        "waiter must receive an error when the leader is aborted without completing"
    );
}

// -----------------------------------------------------------------------
// Cache hit restores usage for cost
// -----------------------------------------------------------------------

#[tokio::test]
async fn cache_hit_restores_usage_for_cost() {
    let mock = Arc::new(MockProvider::new("t", MockMode::Fixed("hello".into())));
    let provider = Arc::clone(&mock) as Arc<dyn LlmProvider>;
    let mut producer = make_producer_with_cache_pricing_timeout(
        provider,
        Duration::from_secs(60),
        0.0025, // input price
        0.01,   // output price
        None,   // no timeout
        None,   // unbounded concurrency
    );

    // First call: caches the result
    let out1 = producer
        .call(make_exchange(Body::Text("hello".into())))
        .await
        .expect("first call ok");
    assert!(out1.input.headers.contains_key(CAMEL_LLM_TOKENS_IN));
    assert!(out1.input.headers.contains_key(CAMEL_LLM_TOKENS_OUT));
    assert!(
        out1.input
            .headers
            .contains_key(CAMEL_LLM_ESTIMATED_COST_USD)
    );

    // Second call: cache hit, must restore token & cost headers
    let out2 = producer
        .call(make_exchange(Body::Text("hello".into())))
        .await
        .expect("second call ok");
    assert_eq!(
        mock.call_count(),
        1,
        "must be a cache hit — provider not called"
    );
    assert!(
        out2.input.headers.contains_key(CAMEL_LLM_TOKENS_IN),
        "cached hit must set tokens_in header"
    );
    assert!(
        out2.input.headers.contains_key(CAMEL_LLM_TOKENS_OUT),
        "cached hit must set tokens_out header"
    );
    assert!(
        out2.input
            .headers
            .contains_key(CAMEL_LLM_ESTIMATED_COST_USD),
        "cached hit must set estimated cost header"
    );
    assert_eq!(
        out1.input.headers.get(CAMEL_LLM_TOKENS_IN),
        out2.input.headers.get(CAMEL_LLM_TOKENS_IN),
        "token counts must match between cached and fresh responses"
    );
}

// -----------------------------------------------------------------------
// High-load single-flight stress test
// -----------------------------------------------------------------------

/// Verifies that 100 concurrent identical requests coalesce into exactly 1
/// provider call, all 100 waiters receive the same cached response, and
/// there are no panics or deadlocks.
#[tokio::test]
async fn single_flight_high_load_coalesces() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("cached response".into()))
            .with_delay(Duration::from_millis(100)),
    );
    let provider = Arc::clone(&mock) as Arc<dyn LlmProvider>;
    let producer = make_producer_with_cache(provider, Duration::from_secs(60));

    let n = 100;
    let mut handles = Vec::with_capacity(n);
    for _ in 0..n {
        let mut p = producer.clone();
        handles.push(tokio::spawn(async move {
            p.call(make_exchange(Body::Text("identical prompt".into())))
                .await
        }));
    }

    let mut results = Vec::with_capacity(n);
    for h in handles {
        let out = h.await.expect("join ok").expect("call ok");
        assert_eq!(body_text(&out), "cached response");
        results.push(out);
    }

    // Exactly 1 provider call despite 100 concurrent requests
    assert_eq!(
        mock.call_count(),
        1,
        "single-flight must coalesce 100 concurrent requests into 1 provider call"
    );

    // All 100 results must have identical token counts (same cached entry)
    let tokens_in = results[0].input.headers.get(CAMEL_LLM_TOKENS_IN).cloned();
    for (i, out) in results.iter().enumerate() {
        assert_eq!(
            out.input.headers.get(CAMEL_LLM_TOKENS_IN),
            tokens_in.as_ref(),
            "all results must have identical tokens_in at index {i}"
        );
    }
}
