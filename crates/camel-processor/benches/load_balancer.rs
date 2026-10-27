use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use camel_api::{
    Exchange, LoadBalanceStrategy, Message, OutcomePipeline, OutcomeSegment, PipelineOutcome,
};
use camel_processor::LoadBalanceSegment;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

/// OutcomePipeline body that passes the exchange through unchanged.
#[derive(Clone)]
struct Passthrough;

impl OutcomePipeline for Passthrough {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }
    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PipelineOutcome> + Send + 'a>> {
        Box::pin(async move { PipelineOutcome::Completed(exchange) })
    }
}

fn passthrough_body() -> OutcomeSegment {
    OutcomeSegment::new(Box::new(Passthrough))
}

fn bench_load_balancer_destinations(c: &mut Criterion) {
    let mut group = c.benchmark_group("load_balancer/destinations");
    let rt = tokio::runtime::Runtime::new().unwrap();

    for count in [2, 5, 10] {
        let seg = LoadBalanceSegment {
            destinations: (0..count).map(|_| passthrough_body()).collect(),
            strategy: LoadBalanceStrategy::RoundRobin,
            round_robin_index: Arc::new(AtomicUsize::new(0)),
        };
        let payloads: Vec<String> = (0..100).map(|i| format!("msg-{i}")).collect();
        group.throughput(Throughput::Elements(100));

        group.bench_with_input(BenchmarkId::new("round_robin", count), &count, |b, _| {
            b.to_async(&rt).iter(|| {
                let seg = seg.clone();
                let payloads = payloads.clone();
                async move {
                    for payload in payloads {
                        let mut seg = seg.clone();
                        let ex = Exchange::new(Message::new(payload));
                        match OutcomePipeline::run(&mut seg, ex).await {
                            PipelineOutcome::Completed(_) => {}
                            other => panic!("load balance segment did not complete: {other:?}"),
                        }
                    }
                }
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench_load_balancer_destinations);
criterion_main!(benches);
