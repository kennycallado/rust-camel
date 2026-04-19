use std::sync::{Arc, Mutex};

use camel_component_timer::TimerComponent;
use camel_core::{DefaultRouteController, Registry, spawn_controller_actor};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

fn bench_lock_contention(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("lock_contention/controller");

    let mut registry = Registry::new();
    registry.register(std::sync::Arc::new(TimerComponent::new()));
    let registry = Arc::new(Mutex::new(registry));

    let (handle, actor_join) = rt.block_on(async {
        let controller = DefaultRouteController::new(
            Arc::clone(&registry),
            Arc::new(camel_api::NoopLeaderElector),
        );
        spawn_controller_actor(controller)
    });
    let shared_handle = Arc::new(handle.clone());

    for workers in [1_u64, 4, 8, 16] {
        group.throughput(Throughput::Elements(workers * 100));
        group.bench_with_input(
            BenchmarkId::new("route_ids_concurrent", workers),
            &workers,
            |b, &workers| {
                let shared_handle = Arc::clone(&shared_handle);
                b.to_async(&rt).iter(|| {
                    let shared_handle = Arc::clone(&shared_handle);
                    async move {
                        let mut tasks = Vec::with_capacity(workers as usize);
                        for _ in 0..workers {
                            let handle = Arc::clone(&shared_handle);
                            tasks.push(tokio::spawn(async move {
                                for _ in 0..100 {
                                    let _ = handle.route_ids().await;
                                }
                            }));
                        }

                        for task in tasks {
                            task.await.expect("worker task panicked");
                        }
                    }
                });
            },
        );
    }

    rt.block_on(async {
        handle.shutdown().await.expect("shutdown actor");
        actor_join.await.expect("join controller actor");
    });

    group.finish();
}

criterion_group!(benches, bench_lock_contention);
criterion_main!(benches);
