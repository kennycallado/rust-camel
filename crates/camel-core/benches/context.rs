use camel_core::context::CamelContext;
use criterion::{Criterion, criterion_group, criterion_main};

fn bench_context_creation(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("context/creation");
    group.bench_function("new_empty", |b| {
        b.to_async(&rt).iter(|| async {
            CamelContext::builder()
                .build()
                .await
                .expect("build context")
        })
    });
    group.finish();
}

criterion_group!(benches, bench_context_creation);
criterion_main!(benches);
