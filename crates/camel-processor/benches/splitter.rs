use camel_api::{
    AggregationStrategy, Body, BoxProcessor, BoxProcessorExt, Exchange, Message, SplitterConfig,
    split_body,
};
use camel_processor::splitter::SplitterService;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use tower::ServiceExt;

fn pass_through() -> BoxProcessor {
    BoxProcessor::from_fn(|ex: Exchange| Box::pin(async move { Ok(ex) }))
}

fn make_lines_body(count: usize) -> String {
    (0..count)
        .map(|i| format!("line-{i}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn bench_splitter_fragments(c: &mut Criterion) {
    let mut group = c.benchmark_group("splitter/fragments");
    let rt = tokio::runtime::Runtime::new().unwrap();

    for count in [1, 10, 100, 1000] {
        let config = SplitterConfig::new(split_body(|body: &Body| match body {
            Body::Text(text) => text
                .split('\n')
                .map(|s| Body::Text(s.to_string()))
                .collect(),
            _ => Vec::new(),
        }))
        .aggregation(AggregationStrategy::CollectAll);

        let svc = SplitterService::new(config, pass_through());
        let body = make_lines_body(count);

        group.bench_with_input(BenchmarkId::new("split", count), &count, |b, _| {
            b.to_async(&rt).iter(|| {
                let svc = svc.clone();
                let ex = Exchange::new(Message::new(body.clone()));
                async move { svc.oneshot(ex).await.unwrap() }
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench_splitter_fragments);
criterion_main!(benches);
