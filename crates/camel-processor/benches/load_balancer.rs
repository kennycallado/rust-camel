use camel_api::{BoxProcessor, BoxProcessorExt, Exchange, LoadBalancerConfig, Message};
use camel_processor::load_balancer::LoadBalancerService;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tower::ServiceExt;

fn pass_through() -> BoxProcessor {
    BoxProcessor::from_fn(|ex: Exchange| Box::pin(async move { Ok(ex) }))
}

fn bench_load_balancer_destinations(c: &mut Criterion) {
    let mut group = c.benchmark_group("load_balancer/destinations");
    let rt = tokio::runtime::Runtime::new().unwrap();

    for count in [2, 5, 10] {
        let endpoints: Vec<BoxProcessor> = (0..count).map(|_| pass_through()).collect();
        let config = LoadBalancerConfig::round_robin();
        let svc = LoadBalancerService::new(endpoints, config);
        let payloads: Vec<String> = (0..100).map(|i| format!("msg-{i}")).collect();
        group.throughput(Throughput::Elements(100));

        group.bench_with_input(BenchmarkId::new("round_robin", count), &count, |b, _| {
            b.to_async(&rt).iter(|| {
                let svc = svc.clone();
                let payloads = payloads.clone();
                async move {
                    for payload in payloads {
                        let ex = Exchange::new(Message::new(payload));
                        svc.clone().oneshot(ex).await.unwrap();
                    }
                }
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench_load_balancer_destinations);
criterion_main!(benches);
