// forced_experiment.rs — diagnostic benchmark to isolate per-step overhead sources
// in the YAML-vs-native throughput gap.
//
// Levels:
//   1. Pure closures (baseline) — BoxProcessor::from_fn, no Tower service wrappers
//   2. Concrete processors — SetBody/SetHeader wrapped directly in BoxProcessor::new
//   3. IdentityProcessor-wrapped — SetBody/SetHeader with explicit IdentityProcessor inner
//   4. Deep pipeline — 10 steps of concrete processors to amplify per-step overhead
//   5. Expression pipeline — SimpleLanguage expression evaluation per request

use std::sync::Arc;

use camel_api::{Body, BoxProcessor, BoxProcessorExt, Exchange, IdentityProcessor, Message, Value};
use camel_core::route::{CompiledStep, compose_pipeline};
use camel_language_api::Language;
use camel_language_simple::SimpleLanguage;
use camel_processor::{SetBody, SetHeader};
use criterion::{Criterion, criterion_group, criterion_main};
use tower::{Service, ServiceExt};

// ---------------------------------------------------------------------------
// Level 1: Pure closures (baseline)
// No Tower service wrappers — measures raw BoxProcessor::from_fn overhead
// ---------------------------------------------------------------------------

fn pure_closure_pipeline() -> BoxProcessor {
    compose_pipeline(vec![
        CompiledStep::Process {
            processor: BoxProcessor::from_fn(|ex: Exchange| Box::pin(async move { Ok(ex) })),
            body_contract: None,
        },
        CompiledStep::Process {
            processor: BoxProcessor::from_fn(|mut ex: Exchange| {
                let _body = Body::Text("response".into());
                ex.input.body = Body::Text("response".into());
                Box::pin(async move { Ok(ex) })
            }),
            body_contract: None,
        },
        CompiledStep::Process {
            processor: BoxProcessor::from_fn(|mut ex: Exchange| {
                Box::pin(async move {
                    ex.input
                        .headers
                        .insert("Content-Type".into(), Value::String("text/plain".into()));
                    Ok(ex)
                })
            }),
            body_contract: None,
        },
    ])
}

// ---------------------------------------------------------------------------
// Level 2: Concrete processors (YAML optimized path)
// Uses SetBody/SetHeader Tower services directly — no closure overhead for the
// body/header logic, only for wrapping.
//
// NOTE: SetBody/SetHeader require an inner processor. We use IdentityProcessor
// as the inner, which is the current API. A future `leaf()` constructor could
// eliminate this inner wrapper.
// ---------------------------------------------------------------------------

fn concrete_processor_pipeline() -> BoxProcessor {
    let body = Body::Text("response".into());
    let set_body = BoxProcessor::new(SetBody::new(IdentityProcessor, move |_: &Exchange| {
        body.clone()
    }));
    let set_header = BoxProcessor::new(SetHeader::new(
        IdentityProcessor,
        "Content-Type",
        Value::String("text/plain".into()),
    ));
    let noop = BoxProcessor::from_fn(|ex: Exchange| Box::pin(async move { Ok(ex) }));
    compose_pipeline(vec![
        CompiledStep::Process {
            processor: noop,
            body_contract: None,
            lifecycle: None,
        },
        CompiledStep::Process {
            processor: set_body,
            body_contract: None,
            lifecycle: None,
        },
        CompiledStep::Process {
            processor: set_header,
            body_contract: None,
            lifecycle: None,
        },
    ])
}

// ---------------------------------------------------------------------------
// Level 3: IdentityProcessor-wrapped (old YAML path)
// Identical API to Level 2 in current codebase — both use
// SetBody::new(IdentityProcessor, ...). Exists as a control to confirm that
// Levels 2 and 3 produce the same overhead (proving IdentityProcessor is not
// the bottleneck).
// ---------------------------------------------------------------------------

fn identity_wrapped_pipeline() -> BoxProcessor {
    let body = Body::Text("response".into());
    let set_body = BoxProcessor::new(SetBody::new(IdentityProcessor, move |_: &Exchange| {
        body.clone()
    }));
    let set_header = BoxProcessor::new(SetHeader::new(
        IdentityProcessor,
        "Content-Type",
        Value::String("text/plain".into()),
    ));
    let noop = BoxProcessor::from_fn(|ex: Exchange| Box::pin(async move { Ok(ex) }));
    compose_pipeline(vec![
        CompiledStep::Process {
            processor: noop,
            body_contract: None,
            lifecycle: None,
        },
        CompiledStep::Process {
            processor: set_body,
            body_contract: None,
            lifecycle: None,
        },
        CompiledStep::Process {
            processor: set_header,
            body_contract: None,
            lifecycle: None,
        },
    ])
}

// ---------------------------------------------------------------------------
// Level 4: Deep pipeline (10 steps)
// Amplifies per-step overhead to make it measurable. If per-step overhead
// is O(1) small, 10 steps makes it O(10) easier to measure.
// ---------------------------------------------------------------------------

fn deep_pipeline() -> BoxProcessor {
    let body = Body::Text("response".into());
    let mut steps: Vec<CompiledStep> = Vec::with_capacity(10);

    for i in 0..10 {
        let step: BoxProcessor = if i % 2 == 0 {
            BoxProcessor::new(SetBody::new(IdentityProcessor, {
                let b = body.clone();
                move |_: &Exchange| b.clone()
            }))
        } else {
            BoxProcessor::new(SetHeader::new(
                IdentityProcessor,
                format!("x-step-{i}"),
                Value::String(format!("value-{i}")),
            ))
        };
        steps.push(CompiledStep::Process {
            processor: step,
            body_contract: None,
            lifecycle: None,
        });
    }

    compose_pipeline(steps)
}

// ---------------------------------------------------------------------------
// Level 5: Expression pipeline
// Evaluates a SimpleLanguage expression (${header.x}) per request.
// Measures the cost of expression parsing/caching + async trait dispatch.
// ---------------------------------------------------------------------------

fn expression_pipeline() -> BoxProcessor {
    let lang = SimpleLanguage::new();
    let expr = Arc::new(lang.create_expression("${header.x}").unwrap());

    let eval_step = BoxProcessor::from_fn(move |ex: Exchange| {
        let expr = Arc::clone(&expr);
        Box::pin(async move {
            let _val = expr.evaluate(&ex).await.unwrap();
            Ok(ex)
        })
    });

    let noop = BoxProcessor::from_fn(|ex: Exchange| Box::pin(async move { Ok(ex) }));
    compose_pipeline(vec![
        CompiledStep::Process {
            processor: noop.clone(),
            body_contract: None,
            lifecycle: None,
        },
        CompiledStep::Process {
            processor: eval_step,
            body_contract: None,
            lifecycle: None,
        },
        CompiledStep::Process {
            processor: noop,
            body_contract: None,
            lifecycle: None,
        },
    ])
}

// ---------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------

fn bench_pure_closures(c: &mut Criterion) {
    let mut group = c.benchmark_group("forced_experiment/pure_closures");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let pipeline = pure_closure_pipeline();

    group.bench_function("3_step_closures", |b| {
        b.to_async(&rt).iter(|| {
            let mut pipeline = pipeline.clone();
            let ex = Exchange::new(Message::new("test"));
            async move { pipeline.ready().await.unwrap().call(ex).await.unwrap() }
        })
    });

    group.finish();
}

fn bench_concrete_processors(c: &mut Criterion) {
    let mut group = c.benchmark_group("forced_experiment/concrete_processors");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let pipeline = concrete_processor_pipeline();

    group.bench_function("3_step_setbody_setheader", |b| {
        b.to_async(&rt).iter(|| {
            let mut pipeline = pipeline.clone();
            let ex = Exchange::new(Message::new("test"));
            async move { pipeline.ready().await.unwrap().call(ex).await.unwrap() }
        })
    });

    group.finish();
}

fn bench_identity_wrapped(c: &mut Criterion) {
    let mut group = c.benchmark_group("forced_experiment/identity_wrapped");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let pipeline = identity_wrapped_pipeline();

    group.bench_function("3_step_identity_inner", |b| {
        b.to_async(&rt).iter(|| {
            let mut pipeline = pipeline.clone();
            let ex = Exchange::new(Message::new("test"));
            async move { pipeline.ready().await.unwrap().call(ex).await.unwrap() }
        })
    });

    group.finish();
}

fn bench_deep_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("forced_experiment/deep_pipeline");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let pipeline = deep_pipeline();

    group.bench_function("10_step_alternating", |b| {
        b.to_async(&rt).iter(|| {
            let mut pipeline = pipeline.clone();
            let ex = Exchange::new(Message::new("test"));
            async move { pipeline.ready().await.unwrap().call(ex).await.unwrap() }
        })
    });

    group.finish();
}

fn bench_expression(c: &mut Criterion) {
    let mut group = c.benchmark_group("forced_experiment/expression");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let pipeline = expression_pipeline();

    group.bench_function("simple_language_eval", |b| {
        b.to_async(&rt).iter(|| {
            let mut pipeline = pipeline.clone();
            let mut ex = Exchange::new(Message::new("test"));
            ex.input.set_header("x", Value::String("hello".into()));
            async move { pipeline.ready().await.unwrap().call(ex).await.unwrap() }
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_pure_closures,
    bench_concrete_processors,
    bench_identity_wrapped,
    bench_deep_pipeline,
    bench_expression,
);
criterion_main!(benches);
