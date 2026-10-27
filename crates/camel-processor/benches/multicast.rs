use std::sync::Arc;

use camel_api::{Body, Exchange, Message, OutcomePipeline, OutcomeSegment, PipelineOutcome, Value};
use camel_processor::MulticastSegment;
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

/// CollectAll aggregator: branch bodies become a JSON array.
fn collect_all_aggregator() -> Arc<dyn Fn(Vec<Exchange>) -> Exchange + Send + Sync> {
    Arc::new(|outputs: Vec<Exchange>| {
        let bodies: Vec<Value> = outputs
            .iter()
            .map(|ex| match &ex.input.body {
                Body::Text(s) => Value::String(s.clone()),
                Body::Json(v) => v.clone(),
                Body::Xml(s) => Value::String(s.clone()),
                Body::Bytes(b) => Value::String(String::from_utf8_lossy(b).into_owned()),
                Body::Stream(s) => serde_json::json!({
                    "_stream": {
                        "origin": s.metadata.origin,
                        "placeholder": true,
                        "hint": "Materialize exchange body with .into_bytes() before multicast aggregation"
                    }
                }),
                _ => Value::Null,
            })
            .collect();
        let mut out = Exchange::default();
        out.input.body = Body::Json(Value::Array(bodies));
        out
    })
}

fn bench_multicast_endpoints(c: &mut Criterion) {
    let mut group = c.benchmark_group("multicast/endpoints");
    let rt = tokio::runtime::Runtime::new().unwrap();

    for count in [1, 5, 10, 20] {
        let seg = MulticastSegment {
            branches: (0..count).map(|_| passthrough_body()).collect(),
            parallel: false,
            parallel_limit: None,
            stop_on_exception: true,
            timeout: None,
            aggregator: collect_all_aggregator(),
        };

        group.bench_with_input(BenchmarkId::new("fanout", count), &count, |b, _| {
            b.to_async(&rt).iter(|| {
                let mut seg = seg.clone();
                let ex = Exchange::new(Message::new("payload"));
                async move {
                    match OutcomePipeline::run(&mut seg, ex).await {
                        PipelineOutcome::Completed(ex) => ex,
                        other => panic!("multicast segment did not complete: {other:?}"),
                    }
                }
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench_multicast_endpoints);
criterion_main!(benches);
