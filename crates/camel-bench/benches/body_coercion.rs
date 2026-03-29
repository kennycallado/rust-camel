use camel_api::{Body, BodyType, BoxProcessor, BoxProcessorExt, Exchange, Message, Value};
use camel_core::route::compose_pipeline_with_contracts;
use criterion::{Criterion, criterion_group, criterion_main};
use tower::ServiceExt;

fn noop() -> BoxProcessor {
    BoxProcessor::from_fn(|ex: Exchange| Box::pin(async move { Ok(ex) }))
}

fn bench_body_coercion(c: &mut Criterion) {
    let mut group = c.benchmark_group("integration/body_coercion");
    let rt = tokio::runtime::Runtime::new().unwrap();

    let single_contract = compose_pipeline_with_contracts(vec![(noop(), Some(BodyType::Text))]);
    group.bench_function("coerce_json_to_text_single_step", |b| {
        b.to_async(&rt).iter(|| {
            let pipeline = single_contract.clone();
            let ex = Exchange::new(Message::new(Body::Json(Value::String("hello".into()))));
            async move { pipeline.oneshot(ex).await.unwrap() }
        })
    });

    let no_contracts = compose_pipeline_with_contracts(vec![(noop(), None)]);
    group.bench_function("pipeline_no_contracts", |b| {
        b.to_async(&rt).iter(|| {
            let pipeline = no_contracts.clone();
            let ex = Exchange::new(Message::new(Body::Json(Value::String("hello".into()))));
            async move { pipeline.oneshot(ex).await.unwrap() }
        })
    });

    let mixed_contracts = compose_pipeline_with_contracts(vec![
        (noop(), Some(BodyType::Text)),
        (noop(), None),
        (noop(), None),
    ]);
    group.bench_function("pipeline_mixed_contracts", |b| {
        b.to_async(&rt).iter(|| {
            let pipeline = mixed_contracts.clone();
            let ex = Exchange::new(Message::new(Body::Json(Value::String("hello".into()))));
            async move { pipeline.oneshot(ex).await.unwrap() }
        })
    });

    group.finish();
}

criterion_group!(benches, bench_body_coercion);
criterion_main!(benches);
