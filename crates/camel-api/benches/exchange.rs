use camel_api::{Exchange, Message, Value};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

fn bench_exchange_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("exchange/creation");
    group.bench_function("new_empty", |b| {
        b.iter(|| Exchange::new(Message::default()))
    });
    group.bench_function("new_with_text_body", |b| {
        b.iter(|| Exchange::new(Message::new("hello world")))
    });
    group.bench_function("default", |b| b.iter(Exchange::default));
    group.finish();
}

fn bench_exchange_clone(c: &mut Criterion) {
    let mut group = c.benchmark_group("exchange/clone");
    let ex = Exchange::new(Message::new("payload"));
    group.bench_function("clone_simple", |b| b.iter(|| ex.clone()));

    let mut ex_headers = Exchange::new(Message::new("payload"));
    for i in 0..100 {
        ex_headers
            .input
            .set_header(format!("key-{i}"), Value::String(format!("val-{i}")));
    }
    group.bench_function("clone_100_headers", |b| b.iter(|| ex_headers.clone()));
    group.finish();
}

fn bench_exchange_headers(c: &mut Criterion) {
    let mut group = c.benchmark_group("exchange/headers");

    for size in [10, 100, 1000] {
        let mut ex = Exchange::new(Message::default());
        for i in 0..size {
            ex.input
                .set_header(format!("key-{i}"), Value::String(format!("val-{i}")));
        }
        group.bench_with_input(BenchmarkId::new("get_header", size), &size, |b, _| {
            b.iter(|| {
                let _ = ex.input.header("key-0");
                let _ = ex.input.header(&format!("key-{}", size - 1));
            })
        });
    }

    for size in [10, 100, 1000] {
        group.bench_with_input(BenchmarkId::new("set_header", size), &size, |b, _| {
            b.iter(|| {
                let mut ex = Exchange::new(Message::default());
                for i in 0..size {
                    ex.input
                        .set_header(format!("k-{i}"), Value::String(format!("v-{i}")));
                }
            })
        });
    }
    group.finish();
}

fn bench_exchange_properties(c: &mut Criterion) {
    let mut group = c.benchmark_group("exchange/properties");
    let mut ex = Exchange::new(Message::default());
    for i in 0..100 {
        ex.set_property(format!("prop-{i}"), Value::String(format!("val-{i}")));
    }
    group.bench_function("get_property", |b| {
        b.iter(|| {
            let _ = ex.property("prop-0");
            let _ = ex.property("prop-99");
        })
    });
    group.bench_function("set_property", |b| {
        b.iter(|| {
            let mut ex = Exchange::default();
            for i in 0..100 {
                ex.set_property(format!("p-{i}"), Value::String(format!("v-{i}")));
            }
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_exchange_creation,
    bench_exchange_clone,
    bench_exchange_headers,
    bench_exchange_properties,
);
criterion_main!(benches);
