use camel_api::{
    BoxProcessor, Exchange, FilterPredicate, Message, OutcomePipeline, OutcomeSegment,
    PipelineOutcome, PredicateSource, SpanKindHint,
};
use camel_core::route::{CompiledStep, PipelineRuntimeCtx, compose_pipeline};
use camel_processor::{ChoiceSegment, FilterSegment, LogLevel, LogProcessor, WhenClauseSegment};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tower::ServiceExt;

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

fn build_throughput_pipeline() -> BoxProcessor {
    let filter = CompiledStep::Segment {
        segment: OutcomeSegment::new(Box::new(FilterSegment {
            predicate: PredicateSource::Sync(FilterPredicate::new(|_: &Exchange| true)),
            body: passthrough_body(),
        })),
        body_contract: None,
        lifecycle: None,
        label: None,
    };
    let choice = CompiledStep::Segment {
        segment: OutcomeSegment::new(Box::new(ChoiceSegment {
            clauses: vec![WhenClauseSegment {
                predicate: PredicateSource::Sync(FilterPredicate::new(|_: &Exchange| true)),
                body: passthrough_body(),
            }],
            otherwise: Some(passthrough_body()),
        })),
        body_contract: None,
        lifecycle: None,
        label: None,
    };
    let log = BoxProcessor::new(LogProcessor::new(
        LogLevel::Info,
        "throughput-log".to_string(),
    ));

    compose_pipeline(
        vec![
            filter,
            choice,
            CompiledStep::Process {
                kind_hint: SpanKindHint::Internal,
                processor: log,
                body_contract: None,
                lifecycle: None,
                label: None,
                to_uri: None,
            },
        ],
        PipelineRuntimeCtx::compile_time(),
    )
}

/// Benchmarks sustained throughput of a simple pipeline (filter → choice → log)
/// at two message counts (100, 1000) to measure messages-per-second.
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
