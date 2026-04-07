use camel_language_api::{Exchange, Language, Message, Value};
use camel_language_simple::SimpleLanguage;
use criterion::{Criterion, black_box, criterion_group, criterion_main};

fn bench_expression_eval(c: &mut Criterion) {
    let mut group = c.benchmark_group("language_simple/expression_eval");

    let lang = SimpleLanguage;

    let mut ex = Exchange::new(Message::new("hello world"));
    ex.input.set_header("foo", Value::String("bar".to_string()));
    ex.input.set_header("count", Value::Number(42.into()));

    let expressions = [
        ("header_value", "${header.foo}"),
        ("header_eq", "${header.foo} == 'bar'"),
        ("header_gt", "${header.count} > 10"),
        ("body_contains", "${body} contains 'world'"),
        (
            "complex",
            "route-${header.foo}-${header.count}-payload:${body}",
        ),
    ];

    for (name, script) in expressions {
        let expr = lang
            .create_expression(script)
            .expect("expression should parse");
        group.bench_function(name, |b| {
            b.iter(|| {
                let value = expr
                    .evaluate(black_box(&ex))
                    .expect("expression evaluation should succeed");
                black_box(value);
            })
        });
    }

    group.finish();
}

fn bench_predicate_eval(c: &mut Criterion) {
    let mut group = c.benchmark_group("language_simple/predicate_eval");

    let lang = SimpleLanguage;

    let mut ex = Exchange::new(Message::new("hello world"));
    ex.input.set_header("foo", Value::String("bar".to_string()));
    ex.input.set_header("count", Value::Number(42.into()));

    let predicates = [
        ("header_value", "${header.foo}"),
        ("header_eq", "${header.foo} == 'bar'"),
        ("header_gt", "${header.count} > 10"),
        ("body_contains", "${body} contains 'world'"),
        (
            "complex",
            "route-${header.foo}-${header.count}-payload:${body}",
        ),
    ];

    for (name, script) in predicates {
        let predicate = lang
            .create_predicate(script)
            .expect("predicate should parse");
        group.bench_function(name, |b| {
            b.iter(|| {
                let matched = predicate
                    .matches(black_box(&ex))
                    .expect("predicate evaluation should succeed");
                black_box(matched);
            })
        });
    }

    group.finish();
}

criterion_group!(benches, bench_expression_eval, bench_predicate_eval);
criterion_main!(benches);
