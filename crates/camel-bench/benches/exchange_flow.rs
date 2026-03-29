use std::sync::Arc;

use camel_api::{Body, BoxProcessor, BoxProcessorExt, Exchange, Message, Value};
use camel_core::route::compose_pipeline;
use camel_processor::{ChoiceService, FilterService, LogLevel, LogProcessor, WhenClause};
use criterion::{Criterion, criterion_group, criterion_main};
use tower::ServiceExt;

fn noop() -> BoxProcessor {
    BoxProcessor::from_fn(|ex: Exchange| Box::pin(async move { Ok(ex) }))
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
    let step2 = BoxProcessor::new(FilterService::new(|_: &Exchange| true, noop()));
    let step3 = BoxProcessor::new(ChoiceService::new(
        vec![WhenClause {
            predicate: Arc::new(|ex: &Exchange| ex.input.header("step").is_some()),
            pipeline: noop(),
        }],
        Some(noop()),
    ));
    let step4 = BoxProcessor::new(LogProcessor::new(LogLevel::Info, "flow-log".to_string()));
    let step5 = body_step("final");

    compose_pipeline(vec![step1, step2, step3, step4, step5])
}

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
