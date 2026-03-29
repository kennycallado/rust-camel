use std::hint::black_box;

use bytes::Bytes;
use camel_api::{Body, Message};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

fn bench_message_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("message/creation");
    group.bench_function("default", |b| b.iter(Message::default));
    group.bench_function("new_text", |b| b.iter(|| Message::new(black_box("hello"))));
    group.bench_function("new_bytes_1kb", |b| {
        let data = vec![0u8; 1024];
        b.iter(|| Message::new(Bytes::from(black_box(data.clone()))))
    });
    group.bench_function("new_bytes_10kb", |b| {
        let data = vec![0u8; 10 * 1024];
        b.iter(|| Message::new(Bytes::from(black_box(data.clone()))))
    });
    group.bench_function("new_bytes_100kb", |b| {
        let data = vec![0u8; 100 * 1024];
        b.iter(|| Message::new(Bytes::from(black_box(data.clone()))))
    });
    group.finish();
}

fn bench_message_body_access(c: &mut Criterion) {
    let mut group = c.benchmark_group("message/body_access");
    group.bench_function("set_get_text", |b| {
        let mut i = 0usize;
        b.iter(|| {
            let value = if black_box(i.is_multiple_of(2)) {
                "hello world"
            } else {
                "goodbye world"
            };
            i = i.wrapping_add(1);
            let msg = Message::new(black_box(value));
            let text_len = msg.body.as_text().map(str::len).unwrap_or(0);
            black_box(text_len)
        })
    });

    let json_val = serde_json::json!({"key": "value", "nested": {"a": 1}});
    let msg_json = Message::new(json_val);
    let msg_text = Message::new("not json");
    group.bench_function("body_is_json", |b| {
        let mut i = 0usize;
        b.iter(|| {
            let msg = if black_box(i.is_multiple_of(2)) {
                &msg_json
            } else {
                &msg_text
            };
            i = i.wrapping_add(1);
            let is_json = matches!(&msg.body, Body::Json(_));
            black_box(is_json)
        })
    });
    group.finish();
}

fn bench_message_clone(c: &mut Criterion) {
    let mut group = c.benchmark_group("message/clone");
    for size in [64, 1024, 10 * 1024, 100 * 1024] {
        let data = vec![0u8; size];
        let msg = Message::new(Bytes::from(data));
        group.bench_with_input(BenchmarkId::new("clone_bytes", size), &size, |b, _| {
            b.iter(|| msg.clone())
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_message_creation,
    bench_message_body_access,
    bench_message_clone,
);
criterion_main!(benches);
