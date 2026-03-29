use camel_api::{AggregatorConfig, Exchange, Message, Value};
use camel_processor::aggregator::AggregatorService;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tower::ServiceExt;

fn bench_aggregator_sizes(c: &mut Criterion) {
    let mut group = c.benchmark_group("aggregator/messages");
    let rt = tokio::runtime::Runtime::new().unwrap();

    for size in [10, 100, 1000] {
        group.throughput(Throughput::Elements(size as u64));
        let payloads: Vec<String> = (0..size).map(|i| format!("msg-{i}")).collect();
        group.bench_with_input(BenchmarkId::new("aggregate", size), &size, |b, &size| {
            b.to_async(&rt).iter(|| {
                let config = AggregatorConfig::correlate_by("corr-id")
                    .complete_when_size(size)
                    .build();
                let svc = AggregatorService::new(config);
                let payloads = payloads.clone();
                async move {
                    for payload in payloads {
                        let mut ex = Exchange::new(Message::new(payload));
                        ex.input
                            .set_header("corr-id", Value::String("bucket-1".into()));
                        svc.clone().oneshot(ex).await.unwrap();
                    }
                }
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench_aggregator_sizes);
criterion_main!(benches);
