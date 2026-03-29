use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
use std::time::Duration;

use camel_api::{
    BoxProcessor, BoxProcessorExt, CamelError, CircuitBreakerConfig, Exchange, Message,
};
use camel_processor::circuit_breaker::CircuitBreakerLayer;
use criterion::{Criterion, criterion_group, criterion_main};
use tower::{Layer, ServiceExt};

fn pass_through() -> BoxProcessor {
    BoxProcessor::from_fn(|ex: Exchange| Box::pin(async move { Ok(ex) }))
}

fn failing_service() -> BoxProcessor {
    BoxProcessor::from_fn(|_| {
        Box::pin(async move { Err(CamelError::ProcessorError("fail".into())) })
    })
}

fn new_config() -> CircuitBreakerConfig {
    CircuitBreakerConfig::new()
        .failure_threshold(3)
        .open_duration(Duration::from_secs(60))
}

fn fast_cycle_config() -> CircuitBreakerConfig {
    CircuitBreakerConfig::new()
        .failure_threshold(3)
        .open_duration(Duration::from_millis(2))
}

fn ex() -> Exchange {
    Exchange::new(Message::new("msg"))
}

fn bench_circuit_breaker_states(c: &mut Criterion) {
    let mut group = c.benchmark_group("circuit_breaker");
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        tokio::time::pause();
    });

    group.bench_function("closed_passthrough", |b| {
        b.to_async(&rt).iter(|| {
            let config = new_config();
            let layer = CircuitBreakerLayer::new(config);
            let svc = layer.layer(pass_through());
            async move {
                svc.oneshot(ex()).await.unwrap();
            }
        })
    });

    group.bench_function("closed_to_open_cycle", |b| {
        b.to_async(&rt).iter(|| {
            let config = new_config();
            let layer = CircuitBreakerLayer::new(config.clone());
            let svc = layer.layer(failing_service());
            async move {
                for _ in 0..config.failure_threshold {
                    let _ = svc.clone().oneshot(ex()).await;
                }
                let _ = svc.clone().oneshot(ex()).await;
            }
        })
    });

    group.bench_function("half_open_recovery", |b| {
        b.to_async(&rt).iter(|| {
            let config = fast_cycle_config();
            let threshold = config.failure_threshold;
            let failures = Arc::new(AtomicU32::new(0));
            let fail_then_recover = BoxProcessor::from_fn(move |ex: Exchange| {
                let failures = Arc::clone(&failures);
                Box::pin(async move {
                    let seen = failures.fetch_add(1, Ordering::Relaxed);
                    if seen < threshold {
                        Err(CamelError::ProcessorError("fail".into()))
                    } else {
                        Ok(ex)
                    }
                })
            });
            let layer = CircuitBreakerLayer::new(config.clone());
            let svc = layer.layer(fail_then_recover);
            async move {
                for _ in 0..threshold {
                    let _ = svc.clone().oneshot(ex()).await;
                }
                tokio::time::advance(config.open_duration + Duration::from_millis(1)).await;
                svc.clone().oneshot(ex()).await.unwrap();
            }
        })
    });

    group.bench_function("repeated_cycles", |b| {
        b.to_async(&rt).iter(|| {
            let config = fast_cycle_config();
            let threshold = config.failure_threshold;
            let cycle_calls = threshold + 1;
            let calls = Arc::new(AtomicU32::new(0));
            let cycling_service = BoxProcessor::from_fn(move |ex: Exchange| {
                let calls = Arc::clone(&calls);
                Box::pin(async move {
                    let idx = calls.fetch_add(1, Ordering::Relaxed);
                    if idx % cycle_calls < threshold {
                        Err(CamelError::ProcessorError("fail".into()))
                    } else {
                        Ok(ex)
                    }
                })
            });
            let layer = CircuitBreakerLayer::new(config.clone());
            let svc = layer.layer(cycling_service);
            async move {
                for _ in 0..3 {
                    for _ in 0..threshold {
                        let _ = svc.clone().oneshot(ex()).await;
                    }
                    tokio::time::advance(config.open_duration + Duration::from_millis(1)).await;
                    svc.clone().oneshot(ex()).await.unwrap();
                }
            }
        })
    });

    group.finish();
}

criterion_group!(benches, bench_circuit_breaker_states);
criterion_main!(benches);
