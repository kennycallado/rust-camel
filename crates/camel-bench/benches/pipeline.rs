use camel_api::{
    AggregationStrategy, Body, BoxProcessor, Exchange, FilterPredicate, Message, OutcomePipeline,
    OutcomeSegment, PipelineOutcome, SplitSource, Value, split_body,
};
use camel_api::{PredicateSource, SpanKindHint};
use camel_core::route::{CompiledStep, PipelineRuntimeCtx, compose_pipeline};
use camel_processor::{
    ChoiceSegment, FilterSegment, LogLevel, LogProcessor, SplitSegment, WhenClauseSegment,
};
use criterion::{Criterion, criterion_group, criterion_main};
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

fn build_pipeline() -> BoxProcessor {
    let filter = CompiledStep::Segment {
        segment: OutcomeSegment::new(Box::new(FilterSegment {
            predicate: PredicateSource::Sync(FilterPredicate::new(|ex: &Exchange| {
                ex.input.header("active").is_some()
            })),
            body: passthrough_body(),
        })),
        body_contract: None,
        lifecycle: None,
        label: None,
    };

    let choice = CompiledStep::Segment {
        segment: OutcomeSegment::new(Box::new(ChoiceSegment {
            clauses: vec![WhenClauseSegment {
                predicate: PredicateSource::Sync(FilterPredicate::new(|ex: &Exchange| {
                    matches!(
                        ex.input.header("type"),
                        Some(v) if v == &Value::String("important".to_string())
                    )
                })),
                body: passthrough_body(),
            }],
            otherwise: Some(passthrough_body()),
        })),
        body_contract: None,
        lifecycle: None,
        label: None,
    };

    let splitter = CompiledStep::Segment {
        segment: OutcomeSegment::new(Box::new(SplitSegment {
            splitter: SplitSource::Sync(split_body(|body: &Body| match body {
                Body::Text(text) => text
                    .split(',')
                    .map(|s| Body::Text(s.trim().to_string()))
                    .collect(),
                _ => vec![],
            })),
            body: passthrough_body(),
            parallel: false,
            parallel_limit: None,
            stop_on_exception: true,
            max_fragments: camel_api::splitter::DEFAULT_MAX_FRAGMENTS,
            aggregation: AggregationStrategy::CollectAll,
        })),
        body_contract: None,
        lifecycle: None,
        label: None,
    };

    let log = BoxProcessor::new(LogProcessor::new(LogLevel::Info, "bench-log".to_string()));

    compose_pipeline(
        vec![
            filter,
            choice,
            splitter,
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

/// Benchmarks a 4-stage pipeline (filter → choice → splitter → log)
/// to measure end-to-end latency of composed EIP patterns with headers.
fn bench_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("integration/pipeline");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let pipeline = build_pipeline();

    group.bench_function("filter_choice_splitter_log", |b| {
        b.to_async(&rt).iter(|| {
            let pipeline = pipeline.clone();
            let mut ex = Exchange::new(Message::new("a,b,c"));
            ex.input
                .set_header("active", Value::String("yes".to_string()));
            ex.input
                .set_header("type", Value::String("important".to_string()));
            async move { pipeline.oneshot(ex).await.unwrap() }
        })
    });

    group.finish();
}

criterion_group!(benches, bench_pipeline);
criterion_main!(benches);
