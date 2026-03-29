use camel_api::{Body, FromBody, Value};
use criterion::{Criterion, criterion_group, criterion_main};
use serde_json::json;

fn bench_from_body(c: &mut Criterion) {
    let mut group = c.benchmark_group("from_body");

    let text = Body::Text("hello world".into());
    group.bench_function("string_from_text", |b| {
        b.iter(|| String::from_body(&text).unwrap())
    });

    let json_string = Body::Json(Value::String("hello".into()));
    group.bench_function("string_from_json_string", |b| {
        b.iter(|| String::from_body(&json_string).unwrap())
    });

    let json_value = Body::Json(json!({ "id": 1, "name": "test" }));
    group.bench_function("value_from_json", |b| {
        b.iter(|| Value::from_body(&json_value).unwrap())
    });

    let text_bytes = Body::Text("hello".into());
    group.bench_function("vec_u8_from_text", |b| {
        b.iter(|| Vec::<u8>::from_body(&text_bytes).unwrap())
    });

    group.finish();
}

criterion_group!(benches, bench_from_body);
criterion_main!(benches);
