use std::hint::black_box;

use camel_api::{
    Exchange, FilterPredicate, Message, OutcomePipeline, OutcomeSegment, PipelineOutcome,
    PredicateSource, Value,
};
use camel_processor::FilterSegment;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

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

fn filter_segment() -> FilterSegment {
    FilterSegment {
        predicate: PredicateSource::Sync(FilterPredicate::new(|ex: &Exchange| {
            ex.input
                .body
                .as_text()
                .map(|text| text.contains("target"))
                .unwrap_or(false)
        })),
        body: OutcomeSegment::new(Box::new(Passthrough)),
    }
}

fn filter_segment_header() -> FilterSegment {
    FilterSegment {
        predicate: PredicateSource::Sync(FilterPredicate::new(|ex: &Exchange| {
            ex.input.header("pass").is_some()
        })),
        body: OutcomeSegment::new(Box::new(Passthrough)),
    }
}

fn bench_filter_hit_rates(c: &mut Criterion) {
    let mut group = c.benchmark_group("filter/hit_rate");
    let rt = tokio::runtime::Runtime::new().unwrap();

    for hit_rate in [0, 50, 100] {
        let segment = filter_segment();
        let mut idx = 0usize;

        group.bench_with_input(BenchmarkId::new("hit_rate", hit_rate), &hit_rate, |b, _| {
            b.to_async(&rt).iter(|| {
                let mut segment = segment.clone();
                let current = idx;
                idx = idx.wrapping_add(1);
                let body = match hit_rate {
                    0 => format!("miss-{current}"),
                    50 => {
                        if black_box(current.is_multiple_of(2)) {
                            format!("target-match-{current}")
                        } else {
                            format!("miss-{current}")
                        }
                    }
                    100 => format!("target-match-{current}"),
                    _ => unreachable!(),
                };
                let ex = Exchange::new(Message::new(black_box(body)));
                async move { black_box(segment.run(ex).await) }
            })
        });
    }
    group.finish();
}

fn bench_filter_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("filter/throughput");
    group.throughput(Throughput::Elements(100));
    let rt = tokio::runtime::Runtime::new().unwrap();

    let segment = filter_segment_header();

    group.bench_function("100_messages", |b| {
        let payloads: Vec<String> = (0..100).map(|i| format!("msg-{i}")).collect();
        b.to_async(&rt).iter(|| {
            let segment = segment.clone();
            let payloads = payloads.clone();
            async move {
                for payload in payloads {
                    let mut segment = segment.clone();
                    let mut ex = Exchange::new(Message::new(payload));
                    ex.input.set_header("pass", Value::String("yes".into()));
                    black_box(segment.run(ex).await);
                }
            }
        })
    });
    group.finish();
}

criterion_group!(benches, bench_filter_hit_rates, bench_filter_throughput);
criterion_main!(benches);
