use camel_api::{
    AggregationStrategy, Body, Exchange, Message, OutcomePipeline, OutcomeSegment, PipelineOutcome,
    SplitSource, split_body,
};
use camel_processor::SplitSegment;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

/// OutcomePipeline body that passes the exchange through unchanged.
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

fn make_lines_body(count: usize) -> String {
    (0..count)
        .map(|i| format!("line-{i}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn bench_splitter_fragments(c: &mut Criterion) {
    let mut group = c.benchmark_group("splitter/fragments");
    let rt = tokio::runtime::Runtime::new().unwrap();

    for count in [1, 10, 100, 1000] {
        let seg = SplitSegment {
            splitter: SplitSource::Sync(split_body(|body: &Body| match body {
                Body::Text(text) => text
                    .split('\n')
                    .map(|s| Body::Text(s.to_string()))
                    .collect(),
                _ => Vec::new(),
            })),
            body: passthrough_body(),
            parallel: false,
            parallel_limit: None,
            stop_on_exception: true,
            aggregation: AggregationStrategy::CollectAll,
        };
        let body = make_lines_body(count);

        group.bench_with_input(BenchmarkId::new("split", count), &count, |b, _| {
            b.to_async(&rt).iter(|| {
                let mut seg = seg.clone();
                let ex = Exchange::new(Message::new(body.clone()));
                async move {
                    match OutcomePipeline::run(&mut seg, ex).await {
                        PipelineOutcome::Completed(ex) => ex,
                        other => panic!("split segment did not complete: {other:?}"),
                    }
                }
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench_splitter_fragments);
criterion_main!(benches);
