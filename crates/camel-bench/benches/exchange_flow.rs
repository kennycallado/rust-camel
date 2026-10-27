use camel_api::{
    Body, BoxProcessor, BoxProcessorExt, Exchange, FilterPredicate, Message, OutcomePipeline,
    OutcomeSegment, PipelineOutcome, PredicateSource, SpanKindHint, Value,
};
use camel_core::route::{CompiledStep, PipelineRuntimeCtx, compose_pipeline};
use camel_processor::{ChoiceSegment, FilterSegment, LogLevel, LogProcessor, WhenClauseSegment};
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

fn header_step(key: &'static str, value: &'static str) -> BoxProcessor {
    BoxProcessor::from_fn(move |mut ex: Exchange| {
        Box::pin(async move {
            ex.input.set_header(key, Value::String(value.to_string()));
            Ok(ex)
        })
    })
}

fn body_step(value: &'static str) -> BoxProcessor {
    BoxProcessor::from_fn(move |mut ex: Exchange| {
        Box::pin(async move {
            ex.input.body = Body::Text(value.to_string());
            Ok(ex)
        })
    })
}

fn build_exchange_flow() -> BoxProcessor {
    let step1 = header_step("step", "1");
    let step2 = CompiledStep::Segment {
        segment: OutcomeSegment::new(Box::new(FilterSegment {
            predicate: PredicateSource::Sync(FilterPredicate::new(|_: &Exchange| true)),
            body: passthrough_body(),
        })),
        body_contract: None,
        lifecycle: None,
        label: None,
    };
    let step3 = CompiledStep::Segment {
        segment: OutcomeSegment::new(Box::new(ChoiceSegment {
            clauses: vec![WhenClauseSegment {
                predicate: PredicateSource::Sync(FilterPredicate::new(|ex: &Exchange| {
                    ex.input.header("step").is_some()
                })),
                body: passthrough_body(),
            }],
            otherwise: Some(passthrough_body()),
        })),
        body_contract: None,
        lifecycle: None,
        label: None,
    };
    let step4 = BoxProcessor::new(LogProcessor::new(LogLevel::Info, "flow-log".to_string()));
    let step5 = body_step("final");

    compose_pipeline(
        vec![
            CompiledStep::Process {
                kind_hint: SpanKindHint::Internal,
                processor: step1,
                body_contract: None,
                lifecycle: None,
                label: None,
                to_uri: None,
            },
            step2,
            step3,
            CompiledStep::Process {
                kind_hint: SpanKindHint::Internal,
                processor: step4,
                body_contract: None,
                lifecycle: None,
                label: None,
                to_uri: None,
            },
            CompiledStep::Process {
                kind_hint: SpanKindHint::Internal,
                processor: step5,
                body_contract: None,
                lifecycle: None,
                label: None,
                to_uri: None,
            },
        ],
        PipelineRuntimeCtx::compile_time(),
    )
}

/// Benchmarks a 5-processor exchange flow (set-header → filter → choice → log → set-body)
/// to measure overhead of sequential header/body mutations across a pipeline.
fn bench_exchange_flow(c: &mut Criterion) {
    let mut group = c.benchmark_group("integration/exchange_flow");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let pipeline = build_exchange_flow();

    group.bench_function("five_processors", |b| {
        b.to_async(&rt).iter(|| {
            let pipeline = pipeline.clone();
            let ex = Exchange::new(Message::new("input"));
            async move { pipeline.oneshot(ex).await.unwrap() }
        })
    });

    group.finish();
}

criterion_group!(benches, bench_exchange_flow);
criterion_main!(benches);
