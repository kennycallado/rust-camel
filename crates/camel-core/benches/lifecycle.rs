use camel_api::{BoxProcessor, BoxProcessorExt, Exchange};
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_core::route::{BuilderStep, RouteDefinition};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

fn pass_through() -> BoxProcessor {
    BoxProcessor::from_fn(|ex: Exchange| Box::pin(async move { Ok(ex) }))
}

fn bench_lifecycle(c: &mut Criterion) {
    let mut group = c.benchmark_group("lifecycle/route_start_stop");
    let rt = tokio::runtime::Runtime::new().unwrap();

    for count in [1, 5, 20] {
        group.bench_with_input(
            BenchmarkId::new("add_start_stop", count),
            &count,
            |b, &count| {
                b.to_async(&rt).iter(|| async move {
                    let mut ctx = CamelContext::new();
                    ctx.register_component(TimerComponent::new());

                    for i in 0..count {
                        let route = RouteDefinition::new(
                            format!("timer:tick-{i}?period=1000"),
                            vec![BuilderStep::Processor(pass_through())],
                        )
                        .with_route_id(format!("bench-lifecycle-{i}"));
                        ctx.add_route_definition(route).await.unwrap();
                    }

                    ctx.start().await.unwrap();
                    ctx.stop().await.unwrap();
                })
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_lifecycle);
criterion_main!(benches);
