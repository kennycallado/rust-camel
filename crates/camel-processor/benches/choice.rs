use camel_api::{Body, BoxProcessor, BoxProcessorExt, Exchange, Message, Value};
use camel_processor::choice::{ChoiceService, WhenClause};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::sync::Arc;
use tower::ServiceExt;

fn append_body(suffix: &str) -> BoxProcessor {
    let suffix = suffix.to_string();
    BoxProcessor::from_fn(move |mut ex: Exchange| {
        let suffix = suffix.clone();
        Box::pin(async move {
            if let Some(text) = ex.input.body.as_text() {
                let mut new_body = text.to_string();
                new_body.push_str(&suffix);
                ex.input.body = Body::Text(new_body);
            }
            Ok(ex)
        })
    })
}

fn bench_choice_predicates(c: &mut Criterion) {
    let mut group = c.benchmark_group("choice/predicates");
    let rt = tokio::runtime::Runtime::new().unwrap();

    for count in [3, 10, 50] {
        let whens: Vec<WhenClause> = (0..count)
            .map(|i| WhenClause {
                predicate: Arc::new(move |ex: &Exchange| {
                    ex.input.header(&format!("match-{i}")).is_some()
                }),
                pipeline: append_body(&format!("-matched-{i}")),
            })
            .collect();

        let svc = ChoiceService::new(whens.clone(), Some(append_body("-else")));

        group.bench_with_input(BenchmarkId::new("no_match", count), &count, |b, _| {
            b.to_async(&rt).iter(|| {
                let svc = svc.clone();
                let ex = Exchange::new(Message::new("input"));
                async move { svc.oneshot(ex).await.unwrap() }
            })
        });

        let svc_match = ChoiceService::new(whens.clone(), Some(append_body("-else")));
        group.bench_with_input(BenchmarkId::new("first_match", count), &count, |b, _| {
            b.to_async(&rt).iter(|| {
                let svc = svc_match.clone();
                let mut ex = Exchange::new(Message::new("input"));
                ex.input.set_header("match-0", Value::String("yes".into()));
                async move { svc.oneshot(ex).await.unwrap() }
            })
        });

        let svc_last = ChoiceService::new(whens.clone(), Some(append_body("-else")));
        group.bench_with_input(BenchmarkId::new("last_match", count), &count, |b, _| {
            b.to_async(&rt).iter(|| {
                let svc = svc_last.clone();
                let mut ex = Exchange::new(Message::new("input"));
                let last = count - 1;
                ex.input
                    .set_header(format!("match-{last}"), Value::String("yes".into()));
                async move { svc.oneshot(ex).await.unwrap() }
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench_choice_predicates);
criterion_main!(benches);
