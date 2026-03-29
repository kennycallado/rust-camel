use std::sync::Arc;

use camel_api::{BoxProcessor, BoxProcessorExt, Exchange, Message};
use camel_core::route::compose_pipeline;
use camel_processor::{ChoiceService, FilterService, LogLevel, LogProcessor, WhenClause};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tower::ServiceExt;

fn noop() -> BoxProcessor {
    BoxProcessor::from_fn(|ex: Exchange| Box::pin(async move { Ok(ex) }))
}

fn build_throughput_pipeline() -> BoxProcessor {
    let filter = BoxProcessor::new(FilterService::new(|_: &Exchange| true, noop()));
    let choice = BoxProcessor::new(ChoiceService::new(
        vec![WhenClause {
            predicate: Arc::new(|_: &Exchange| true),
            pipeline: noop(),
        }],
        Some(noop()),
    ));
    let log = BoxProcessor::new(LogProcessor::new(
        LogLevel::Info,
        "throughput-log".to_string(),
    ));

    compose_pipeline(vec![filter, choice, log])
}

fn bench_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("integration/throughput");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let pipeline = build_throughput_pipeline();

    for msg_count in [100usize, 1000usize] {
        let payloads: Vec<String> = (0..msg_count).map(|i| format!("msg-{i}")).collect();
        group.throughput(Throughput::Elements(msg_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(msg_count),
            &msg_count,
            |b, &count| {
                b.to_async(&rt).iter(|| {
                    let pipeline = pipeline.clone();
                    let payloads = payloads.clone();
                    async move {
                        for payload in payloads.into_iter().take(count) {
                            let ex = Exchange::new(Message::new(payload));
                            pipeline.clone().oneshot(ex).await.unwrap();
                        }
                    }
                })
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_throughput);
criterion_main!(benches);
