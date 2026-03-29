use camel_core::context::CamelContext;
use criterion::{Criterion, criterion_group, criterion_main};

fn bench_context_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("context/creation");
    group.bench_function("new_empty", |b| b.iter(CamelContext::new));
    group.finish();
}

criterion_group!(benches, bench_context_creation);
criterion_main!(benches);
