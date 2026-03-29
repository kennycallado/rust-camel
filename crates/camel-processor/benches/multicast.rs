use camel_api::{
    BoxProcessor, BoxProcessorExt, Exchange, Message, MulticastConfig, MulticastStrategy,
};
use camel_processor::multicast::MulticastService;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use tower::ServiceExt;

fn pass_through() -> BoxProcessor {
    BoxProcessor::from_fn(|ex: Exchange| Box::pin(async move { Ok(ex) }))
}

fn bench_multicast_endpoints(c: &mut Criterion) {
    let mut group = c.benchmark_group("multicast/endpoints");
    let rt = tokio::runtime::Runtime::new().unwrap();

    for count in [1, 5, 10, 20] {
        let endpoints: Vec<BoxProcessor> = (0..count).map(|_| pass_through()).collect();
        let config = MulticastConfig::new().aggregation(MulticastStrategy::CollectAll);
        let svc = MulticastService::new(endpoints, config);

        group.bench_with_input(BenchmarkId::new("fanout", count), &count, |b, _| {
            b.to_async(&rt).iter(|| {
                let svc = svc.clone();
                let ex = Exchange::new(Message::new("payload"));
                async move { svc.oneshot(ex).await.unwrap() }
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench_multicast_endpoints);
criterion_main!(benches);
