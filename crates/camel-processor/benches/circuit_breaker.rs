use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
use std::time::Duration;

use camel_api::{
    BoxProcessor, BoxProcessorExt, CamelError, CircuitBreakerConfig, Exchange, Message,
};
use camel_processor::circuit_breaker::CircuitBreakerDecision;
use camel_processor::circuit_breaker::CircuitBreakerGate;
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
    // The breaker measures open-state expiry with `std::time::Instant`
    // (real monotonic time), so these benches must sleep for real instead
    // of advancing a paused tokio clock.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    group.bench_function("closed_passthrough", |b| {
        b.to_async(&rt).iter(|| {
            let config = new_config();
            let layer = CircuitBreakerLayer::new(config, Arc::from("bench"), None);
            let svc = layer.layer(pass_through());
            async move {
                svc.oneshot(ex()).await.unwrap();
            }
        })
    });

    group.bench_function("closed_to_open_cycle", |b| {
        b.to_async(&rt).iter(|| {
            let config = new_config();
            let layer = CircuitBreakerLayer::new(config.clone(), Arc::from("bench"), None);
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
            let layer = CircuitBreakerLayer::new(config.clone(), Arc::from("bench"), None);
            let svc = layer.layer(fail_then_recover);
            async move {
                for _ in 0..threshold {
                    let _ = svc.clone().oneshot(ex()).await;
                }
                tokio::time::sleep(config.open_duration + Duration::from_millis(10)).await;
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
            let layer = CircuitBreakerLayer::new(config.clone(), Arc::from("bench"), None);
            let svc = layer.layer(cycling_service);
            async move {
                for _ in 0..3 {
                    for _ in 0..threshold {
                        let _ = svc.clone().oneshot(ex()).await;
                    }
                    tokio::time::sleep(config.open_duration + Duration::from_millis(10)).await;
                    svc.clone().oneshot(ex()).await.unwrap();
                }
            }
        })
    });

    group.finish();
}

/// Calls per task in the gate microbench.
const GATE_ITERATIONS: usize = 2_000;

fn bench_gate_multi(c: &mut Criterion) {
    let mut group = c.benchmark_group("gate_multi");
    // Dedicated multi-thread runtime: the tasks must actually run in
    // parallel so the gate's mutex is contended. Tokio time is NOT paused
    // here — the breaker measures expiry with `std::time::Instant`, and
    // `open_duration` is 1h, so the gate stays Closed for the sweep.
    let multi_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build multi-thread runtime");

    for tasks in [1usize, 8, 16] {
        // One gate shared by every task: the point is the contended hot
        // state, not per-task copies.
        let gate = Arc::new(CircuitBreakerGate::new(
            CircuitBreakerConfig::new()
                .failure_threshold(100_000)
                .open_duration(Duration::from_secs(3600)),
            Arc::from("bench"),
            None,
        ));

        group.bench_function(format!("gate_closed_success_tasks_{tasks}"), |b| {
            b.to_async(&multi_rt).iter(|| {
                let gate = Arc::clone(&gate);
                async move {
                    let barrier = Arc::new(tokio::sync::Barrier::new(tasks));
                    let mut handles = Vec::with_capacity(tasks);
                    for _ in 0..tasks {
                        let gate = Arc::clone(&gate);
                        let barrier = Arc::clone(&barrier);
                        handles.push(tokio::spawn(async move {
                            barrier.wait().await;
                            // `Exchange` is not `Clone`, and rebuilding it
                            // per call would swamp the mutex cost, so build
                            // the `Ok` result once and reuse its reference.
                            let ok: Result<Exchange, CamelError> =
                                Ok(Exchange::new(Message::new("msg")));
                            for _ in 0..GATE_ITERATIONS {
                                let decision = gate.before_call();
                                if !matches!(&decision, CircuitBreakerDecision::Allow) {
                                    panic!("gate left the closed success path");
                                }
                                std::hint::black_box(&decision);
                                gate.after_result(&ok);
                            }
                        }));
                    }
                    for handle in handles {
                        handle.await.expect("gate bench task panicked");
                    }
                }
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_circuit_breaker_states, bench_gate_multi);
criterion_main!(benches);
