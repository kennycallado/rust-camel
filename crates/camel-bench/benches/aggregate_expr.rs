//! Route-level benchmark for the Aggregator EIP with expression-based
//! correlation and completion configuration.
//!
//! The production route compiler builds one `AggregatorService`, wraps it in a
//! `BoxProcessor`, and composes it into a pipeline
//! (`crates/camel-core/src/lifecycle/adapters/step_compilers/splitting.rs`,
//! `BuilderStep::Aggregate`). This bench reproduces that shape and constructs
//! the pipeline ONCE per input size (setup outside the measured loop), so the
//! numbers isolate per-exchange cost of the compiled route rather than service
//! reconstruction — unlike `crates/camel-processor/benches/aggregator.rs`,
//! which rebuilds the service on every iteration.
//!
//! Every exchange pays two expression evaluations: the correlation expression
//! extracts the bucket key and the completion predicate evaluates to false.
//! `Size(N)` is the condition that actually completes the bucket.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use camel_api::aggregator::{
    AggregatorConfig, CompletionCondition, CompletionMode, CorrelationStrategy,
};
use camel_api::{BoxProcessor, Exchange, Message, SpanKindHint, Value};
use camel_core::route::{CompiledStep, PipelineRuntimeCtx, compose_pipeline};
use camel_language_api::Language;
use camel_language_simple::SimpleLanguage;
use camel_processor::AggregatorService;
use camel_processor::aggregator::SharedLanguageRegistry;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

/// Registry populated with the `simple` language under the name it advertises,
/// so both the correlation expression and the completion predicate resolve.
fn language_registry() -> SharedLanguageRegistry {
    let simple = SimpleLanguage::new();
    let mut languages: HashMap<String, Arc<dyn Language>> = HashMap::new();
    languages.insert(simple.name().to_string(), Arc::new(simple));
    Arc::new(Mutex::new(languages))
}

/// Correlate by expression and complete on `Size(size)` while a
/// legal-but-false predicate condition is also configured. The predicate must
/// evaluate on every exchange (cost paid), but never completes a bucket, so
/// each batch of `size` exchanges completes exactly one bucket.
fn aggregator_config(size: usize) -> AggregatorConfig {
    let mut config = AggregatorConfig::correlate_by("corr-id")
        .complete_when_size(size)
        .build()
        .expect("size completion is a valid aggregator config");
    config.correlation = CorrelationStrategy::Expression {
        expr: "${header.corr-id}".into(),
        language: "simple".into(),
    };
    config.completion = CompletionMode::Any(vec![
        CompletionCondition::Size(size),
        CompletionCondition::PredicateExpr {
            expr: "${header.corr-id} == 'never-matches'".into(),
            language: "simple".into(),
        },
    ]);
    config
}

/// Compile the aggregator into the same composed-pipeline shape the route
/// compiler emits. Called once per input size, outside the measured loop.
fn build_pipeline(size: usize) -> BoxProcessor {
    let (late_tx, _late_rx) = mpsc::channel(1024);
    let service = AggregatorService::new(
        aggregator_config(size),
        late_tx,
        language_registry(),
        CancellationToken::new(),
    );
    let aggregator_step = CompiledStep::Process {
        kind_hint: SpanKindHint::Internal,
        processor: BoxProcessor::new(service),
        body_contract: None,
        lifecycle: None,
        label: None,
        to_uri: None,
    };
    compose_pipeline(vec![aggregator_step], PipelineRuntimeCtx::compile_time())
}

fn bench_aggregate_expr(c: &mut Criterion) {
    let mut group = c.benchmark_group("integration/aggregate_expr");
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime builds");

    for size in [10usize, 100, 1000] {
        let pipeline = build_pipeline(size);
        let payloads: Vec<String> = (0..size).map(|i| format!("msg-{i}")).collect();

        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("aggregate", size), &size, |b, _| {
            b.to_async(&rt).iter(|| {
                let pipeline = pipeline.clone();
                let payloads = payloads.clone();
                async move {
                    for payload in payloads {
                        let mut exchange = Exchange::new(Message::new(payload));
                        exchange
                            .input
                            .set_header("corr-id", Value::String("bucket-1".into()));
                        pipeline
                            .clone()
                            .oneshot(exchange)
                            .await
                            .expect("aggregator pipeline call succeeds");
                    }
                }
            })
        });
    }

    group.finish();
}

criterion_group!(benches, bench_aggregate_expr);
criterion_main!(benches);
