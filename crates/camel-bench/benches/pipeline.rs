use std::sync::Arc;

use camel_api::{
    AggregationStrategy, Body, BoxProcessor, BoxProcessorExt, Exchange, Message, SplitterConfig,
    Value, split_body,
};
use camel_core::route::compose_pipeline;
use camel_processor::{
    ChoiceService, FilterService, LogLevel, LogProcessor, SplitterService, WhenClause,
};
use criterion::{Criterion, criterion_group, criterion_main};
use tower::ServiceExt;

fn noop() -> BoxProcessor {
    BoxProcessor::from_fn(|ex: Exchange| Box::pin(async move { Ok(ex) }))
}

fn build_pipeline() -> BoxProcessor {
    let filter = BoxProcessor::new(FilterService::new(
        |ex: &Exchange| ex.input.header("active").is_some(),
        noop(),
    ));

    let choice = BoxProcessor::new(ChoiceService::new(
        vec![WhenClause {
            predicate: Arc::new(|ex: &Exchange| {
                matches!(
                    ex.input.header("type"),
                    Some(v) if v == &Value::String("important".to_string())
                )
            }),
            pipeline: noop(),
        }],
        Some(noop()),
    ));

    let splitter = BoxProcessor::new(SplitterService::new(
        SplitterConfig::new(split_body(|body: &Body| match body {
            Body::Text(text) => text
                .split(',')
                .map(|s| Body::Text(s.trim().to_string()))
                .collect(),
            _ => vec![],
        }))
        .aggregation(AggregationStrategy::CollectAll),
        noop(),
    ));

    let log = BoxProcessor::new(LogProcessor::new(LogLevel::Info, "bench-log".to_string()));

    compose_pipeline(vec![filter, choice, splitter, log])
}

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
