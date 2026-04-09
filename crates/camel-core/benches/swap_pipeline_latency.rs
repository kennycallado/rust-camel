use std::sync::{Arc, Mutex};

use camel_api::{BoxProcessor, BoxProcessorExt, Exchange};
use camel_component_timer::TimerComponent;
use camel_core::route::{BuilderStep, RouteDefinition};
use camel_core::{DefaultRouteController, Registry, spawn_controller_actor};
use criterion::{Criterion, criterion_group, criterion_main};

fn pass_through() -> BoxProcessor {
    BoxProcessor::from_fn(|ex: Exchange| Box::pin(async move { Ok(ex) }))
}

fn bench_swap_pipeline_latency(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("controller/swap_pipeline_latency");

    let mut registry = Registry::new();
    registry.register(TimerComponent::new());
    let registry = Arc::new(Mutex::new(registry));

    let (handle, actor_join) = rt.block_on(async {
        let controller = DefaultRouteController::new(Arc::clone(&registry));
        spawn_controller_actor(controller)
    });

    rt.block_on(async {
        let route = RouteDefinition::new(
            "timer:swap?period=1000",
            vec![BuilderStep::Processor(pass_through())],
        )
        .with_route_id("bench-swap");

        handle.add_route(route).await.expect("add route");
        handle.start_route("bench-swap").await.expect("start route");
    });

    group.bench_function("swap_pipeline", |b| {
        let handle = handle.clone();
        b.to_async(&rt).iter(|| async {
            handle
                .swap_pipeline("bench-swap", pass_through())
                .await
                .expect("swap pipeline");
        });
    });

    rt.block_on(async {
        handle.stop_route("bench-swap").await.expect("stop route");
        handle.shutdown().await.expect("shutdown actor");
        actor_join.await.expect("join controller actor");
    });

    group.finish();
}

criterion_group!(benches, bench_swap_pipeline_latency);
criterion_main!(benches);
