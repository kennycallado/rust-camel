use std::hint::black_box;

use camel_api::{BoxProcessor, BoxProcessorExt, Exchange, Message, Value};
use camel_processor::filter::FilterService;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tower::ServiceExt;

fn pass_through() -> BoxProcessor {
    BoxProcessor::from_fn(|ex: Exchange| Box::pin(async move { Ok(ex) }))
}

fn bench_filter_hit_rates(c: &mut Criterion) {
    let mut group = c.benchmark_group("filter/hit_rate");
    let rt = tokio::runtime::Runtime::new().unwrap();

    for hit_rate in [0, 50, 100] {
        let svc = FilterService::new(
            move |ex: &Exchange| {
                ex.input
                    .body
                    .as_text()
                    .map(|text| text.contains("target"))
                    .unwrap_or(false)
            },
            pass_through(),
        );
        let mut idx = 0usize;

        group.bench_with_input(BenchmarkId::new("hit_rate", hit_rate), &hit_rate, |b, _| {
            b.to_async(&rt).iter(|| {
                let svc = svc.clone();
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
                async move { svc.oneshot(ex).await.unwrap() }
            })
        });
    }
    group.finish();
}

fn bench_filter_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("filter/throughput");
    group.throughput(Throughput::Elements(100));
    let rt = tokio::runtime::Runtime::new().unwrap();

    let svc = FilterService::new(
        |ex: &Exchange| ex.input.header("pass").is_some(),
        pass_through(),
    );

    group.bench_function("100_messages", |b| {
        let payloads: Vec<String> = (0..100).map(|i| format!("msg-{i}")).collect();
        b.to_async(&rt).iter(|| {
            let svc = svc.clone();
            let payloads = payloads.clone();
            async move {
                for payload in payloads {
                    let mut ex = Exchange::new(Message::new(payload));
                    ex.input.set_header("pass", Value::String("yes".into()));
                    svc.clone().oneshot(ex).await.unwrap();
                }
            }
        })
    });
    group.finish();
}

criterion_group!(benches, bench_filter_hit_rates, bench_filter_throughput);
criterion_main!(benches);
