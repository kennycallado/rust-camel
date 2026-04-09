use camel_api::{BoxProcessor, BoxProcessorExt, Exchange, Message};
use camel_core::DetailLevel;
use camel_core::route::{compose_pipeline, compose_traced_pipeline};
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use tower::Service;

fn pass_through() -> BoxProcessor {
    BoxProcessor::from_fn(|ex: Exchange| Box::pin(async move { Ok(ex) }))
}

fn bench_tracing_overhead(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("pipeline/tracing_overhead");
    group.throughput(Throughput::Elements(1));

    let steps: Vec<BoxProcessor> = (0..10).map(|_| pass_through()).collect();
    let mut plain = compose_pipeline(steps.clone());
    let mut traced = compose_traced_pipeline(steps, "bench-route", true, DetailLevel::Full, None);

    group.bench_function("plain_10_steps", |b| {
        b.to_async(&rt).iter(|| {
            let ex = Exchange::new(Message::new("hello"));
            plain.call(ex)
        });
    });

    group.bench_function("traced_10_steps", |b| {
        b.to_async(&rt).iter(|| {
            let ex = Exchange::new(Message::new("hello"));
            traced.call(ex)
        });
    });

    group.finish();
}

criterion_group!(benches, bench_tracing_overhead);
criterion_main!(benches);
