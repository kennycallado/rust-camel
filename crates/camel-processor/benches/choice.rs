use std::hint::black_box;

use camel_api::{
    Body, Exchange, FilterPredicate, Message, OutcomePipeline, OutcomeSegment, PipelineOutcome,
    PredicateSource, Value,
};
use camel_processor::{ChoiceSegment, WhenClauseSegment};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

#[derive(Clone)]
struct AppendBody {
    suffix: String,
}

impl OutcomePipeline for AppendBody {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }
    fn run<'a>(
        &'a mut self,
        mut exchange: Exchange,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PipelineOutcome> + Send + 'a>> {
        if let Some(text) = exchange.input.body.as_text() {
            let mut new_body = text.to_string();
            new_body.push_str(&self.suffix);
            exchange.input.body = Body::Text(new_body);
        }
        Box::pin(async move { PipelineOutcome::Completed(exchange) })
    }
}

fn append_body(suffix: &str) -> OutcomeSegment {
    OutcomeSegment::new(Box::new(AppendBody {
        suffix: suffix.to_string(),
    }))
}

fn choice_segment(count: usize) -> ChoiceSegment {
    let clauses = (0..count)
        .map(|i| WhenClauseSegment {
            predicate: PredicateSource::Sync(FilterPredicate::new(move |ex: &Exchange| {
                ex.input.header(&format!("match-{i}")).is_some()
            })),
            body: append_body(&format!("-matched-{i}")),
        })
        .collect();

    ChoiceSegment {
        clauses,
        otherwise: Some(append_body("-else")),
    }
}

fn bench_choice_predicates(c: &mut Criterion) {
    let mut group = c.benchmark_group("choice/predicates");
    let rt = tokio::runtime::Runtime::new().unwrap();

    for count in [3, 10, 50] {
        let segment = choice_segment(count);

        group.bench_with_input(BenchmarkId::new("no_match", count), &count, |b, _| {
            b.to_async(&rt).iter(|| {
                let mut segment = segment.clone();
                let ex = Exchange::new(Message::new("input"));
                async move { black_box(segment.run(ex).await) }
            })
        });

        group.bench_with_input(BenchmarkId::new("first_match", count), &count, |b, _| {
            b.to_async(&rt).iter(|| {
                let mut segment = segment.clone();
                let mut ex = Exchange::new(Message::new("input"));
                ex.input.set_header("match-0", Value::String("yes".into()));
                async move { black_box(segment.run(ex).await) }
            })
        });

        group.bench_with_input(BenchmarkId::new("last_match", count), &count, |b, _| {
            b.to_async(&rt).iter(|| {
                let mut segment = segment.clone();
                let mut ex = Exchange::new(Message::new("input"));
                let last = count - 1;
                ex.input
                    .set_header(format!("match-{last}"), Value::String("yes".into()));
                async move { black_box(segment.run(ex).await) }
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench_choice_predicates);
criterion_main!(benches);
