use camel_api::{BoxProcessor, BoxProcessorExt, Exchange, Message};
use camel_core::route::compose_pipeline;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tower::Service;

fn pass_through() -> BoxProcessor {
    BoxProcessor::from_fn(|ex: Exchange| Box::pin(async move { Ok(ex) }))
}

fn bench_pipeline_throughput(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("pipeline/throughput");
    group.throughput(Throughput::Elements(1));

    for step_count in [1, 5, 20] {
        let steps: Vec<BoxProcessor> = (0..step_count).map(|_| pass_through()).collect();
        let mut pipeline = compose_pipeline(steps.clone());

        group.bench_with_input(
            BenchmarkId::new("steps", step_count),
            &step_count,
            |b, _| {
                b.to_async(&rt).iter(|| {
                    let ex = Exchange::new(Message::new("hello"));
                    pipeline.call(ex)
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_pipeline_throughput);
criterion_main!(benches);
