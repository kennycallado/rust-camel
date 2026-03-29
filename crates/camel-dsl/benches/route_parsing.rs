use camel_dsl::parse_yaml;
use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};

fn generate_routes_yaml(count: usize) -> String {
    let mut yaml = String::from("routes:\n");
    for i in 0..count {
        yaml.push_str(&format!(
            "  - id: \"route-{i}\"\n    from: \"direct:start-{i}\"\n    steps:\n      - log: \"Processing route {i}\"\n      - to: \"mock:result-{i}\"\n"
        ));
    }
    yaml
}

fn bench_route_parsing(c: &mut Criterion) {
    let mut group = c.benchmark_group("dsl/route_parsing");

    for route_count in [1usize, 5, 20] {
        let yaml = generate_routes_yaml(route_count);
        group.bench_with_input(
            BenchmarkId::new("parse_yaml", route_count),
            &yaml,
            |b, input| {
                b.iter(|| {
                    let parsed = parse_yaml(black_box(input)).expect("route YAML should parse");
                    black_box(parsed);
                })
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_route_parsing);
criterion_main!(benches);
