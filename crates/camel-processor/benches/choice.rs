use std::hint::black_box;
use std::sync::Arc;

use camel_api::{
    Body, Exchange, FilterPredicate, Message, OutcomePipeline, OutcomeSegment, PipelineOutcome,
    PredicateSource, Value,
};
use camel_language_api::{EvalMeta, Language, LanguagePredicateEval};
use camel_language_rhai::RhaiLanguage;
use camel_language_simple::SimpleLanguage;
use camel_processor::{ChoiceGroupSegment, ChoiceSegment, WhenClauseSegment};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

#[derive(Clone)]
struct AppendBody {
    suffix: String,
}

impl OutcomePipeline for AppendBody {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }
    fn run<'a>(
        &'a mut self,
        mut exchange: Exchange,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PipelineOutcome> + Send + 'a>> {
        if let Some(text) = exchange.input.body.as_text() {
            let mut new_body = text.to_string();
            new_body.push_str(&self.suffix);
            exchange.input.body = Body::Text(new_body);
        }
        Box::pin(async move { PipelineOutcome::Completed(exchange) })
    }
}

fn append_body(suffix: &str) -> OutcomeSegment {
    OutcomeSegment::new(Box::new(AppendBody {
        suffix: suffix.to_string(),
    }))
}

fn choice_segment(count: usize) -> ChoiceSegment {
    let clauses = (0..count)
        .map(|i| WhenClauseSegment {
            predicate: PredicateSource::Sync(FilterPredicate::new(move |ex: &Exchange| {
                ex.input.header(&format!("match-{i}")).is_some()
            })),
            body: append_body(&format!("-matched-{i}")),
        })
        .collect();

    ChoiceSegment {
        clauses,
        otherwise: Some(append_body("-else")),
    }
}

fn bench_choice_predicates(c: &mut Criterion) {
    let mut group = c.benchmark_group("choice/predicates");
    let rt = tokio::runtime::Runtime::new().unwrap();

    for count in [3, 10, 50] {
        let segment = choice_segment(count);

        group.bench_with_input(BenchmarkId::new("no_match", count), &count, |b, _| {
            b.to_async(&rt).iter(|| {
                let mut segment = segment.clone();
                let ex = Exchange::new(Message::new("input"));
                async move { black_box(segment.run(ex).await) }
            })
        });

        group.bench_with_input(BenchmarkId::new("first_match", count), &count, |b, _| {
            b.to_async(&rt).iter(|| {
                let mut segment = segment.clone();
                let mut ex = Exchange::new(Message::new("input"));
                ex.input.set_header("match-0", Value::String("yes".into()));
                async move { black_box(segment.run(ex).await) }
            })
        });

        group.bench_with_input(BenchmarkId::new("last_match", count), &count, |b, _| {
            b.to_async(&rt).iter(|| {
                let mut segment = segment.clone();
                let mut ex = Exchange::new(Message::new("input"));
                let last = count - 1;
                ex.input
                    .set_header(format!("match-{last}"), Value::String("yes".into()));
                async move { black_box(segment.run(ex).await) }
            })
        });
    }
    group.finish();
}

fn eval_meta(language: &str, index: usize) -> EvalMeta {
    EvalMeta {
        language: language.to_string(),
        route_id: "choice-bench".to_string(),
        step_id: format!("choice#{index}"),
        verb: "choice".to_string(),
        target: None,
    }
}

/// Build the same `PredicateSource::Async` shape that
/// `camel_core::lifecycle::adapters::step_resolution::compile_filter_predicate`
/// builds for declarative `when` clauses.
fn language_clause(
    language: &dyn Language,
    language_name: &str,
    script: &str,
    index: usize,
) -> WhenClauseSegment {
    let compiled = language.create_predicate(script).unwrap();
    WhenClauseSegment {
        predicate: PredicateSource::Async(
            LanguagePredicateEval::new(Arc::from(compiled), eval_meta(language_name, index))
                .into_bool_fn(),
        ),
        body: append_body(&format!("-matched-{index}")),
    }
}

fn rhai_choice(count: usize) -> ChoiceSegment {
    let rhai = RhaiLanguage::new();
    let clauses = (0..count)
        .map(|i| {
            let script = format!("header(\"match-{i}\") == \"yes\"");
            language_clause(&rhai, "rhai", &script, i)
        })
        .collect();
    // No `otherwise` arm: a no-match run ends as a plain `Completed(exchange)`,
    // so the measurement isolates predicate dispatch cost.
    ChoiceSegment {
        clauses,
        otherwise: None,
    }
}

fn group_eval_meta(index: usize) -> EvalMeta {
    EvalMeta {
        language: "rhai".to_string(),
        route_id: "choice-bench".to_string(),
        step_id: "choice/when#0".to_string(),
        verb: format!("choice/when[{index}]"),
        target: None,
    }
}

/// Build the same `ChoiceGroupSegment` shape that `compile_choice_group`
/// produces for declarative `when` clauses sharing one language.
fn rhai_group_choice(count: usize) -> ChoiceGroupSegment {
    let rhai = RhaiLanguage::new();
    let scripts: Vec<String> = (0..count)
        .map(|i| format!("header(\"match-{i}\") == \"yes\""))
        .collect();
    let script_refs: Vec<&str> = scripts.iter().map(String::as_str).collect();
    let group = rhai
        .create_predicate_group(&script_refs)
        .expect("rhai predicate group compiles")
        .expect("rhai supports predicate groups");
    let metas = (0..count).map(group_eval_meta).collect();
    let bodies = (0..count)
        .map(|i| append_body(&format!("-matched-{i}")))
        .collect();
    ChoiceGroupSegment::new(Arc::from(group), metas, bodies, None)
        .expect("grouped choice segment is valid")
}

fn mixed_choice(count: usize) -> ChoiceSegment {
    let rhai = RhaiLanguage::new();
    let simple = SimpleLanguage::new();
    let clauses = (0..count)
        .map(|i| {
            // Hyphenated keys are safe for Rhai, but the simple language reads
            // `match-1` in an expression as `match - 1` (subtraction), so the
            // mixed shape uses underscore keys. No header is set for this
            // no-match shape, so the key spelling does not change the outcome.
            if i % 2 == 0 {
                let script = format!("header(\"match_{i}\") == \"yes\"");
                language_clause(&rhai, "rhai", &script, i)
            } else {
                let script = format!("${{header.match_{i}}} == 'yes'");
                language_clause(&simple, "simple", &script, i)
            }
        })
        .collect();
    ChoiceSegment {
        clauses,
        otherwise: None,
    }
}

fn lang_exchange(matched: Option<usize>) -> Exchange {
    let mut ex = Exchange::new(Message::new("input"));
    if let Some(k) = matched {
        ex.input
            .set_header(format!("match-{k}"), Value::String("yes".into()));
    }
    ex
}

fn bench_choice_language_predicates(c: &mut Criterion) {
    let mut group = c.benchmark_group("choice_lang");
    let rt = tokio::runtime::Runtime::new().unwrap();

    let shapes: [(&str, ChoiceSegment, Option<usize>); 6] = [
        ("rhai_1_match", rhai_choice(1), Some(0)),
        ("rhai_10_match_first", rhai_choice(10), Some(0)),
        ("rhai_10_match_middle", rhai_choice(10), Some(5)),
        ("rhai_10_match_last", rhai_choice(10), Some(9)),
        ("rhai_10_no_match", rhai_choice(10), None),
        ("mixed_10_no_match", mixed_choice(10), None),
    ];

    for (name, segment, matched) in shapes {
        group.bench_function(name, |b| {
            b.to_async(&rt).iter(|| {
                let mut segment = segment.clone();
                let ex = lang_exchange(matched);
                async move { black_box(segment.run(ex).await) }
            })
        });
    }

    let grouped_shapes: [(&str, ChoiceGroupSegment, Option<usize>); 5] = [
        ("rhai_group_1_match", rhai_group_choice(1), Some(0)),
        ("rhai_group_10_match_first", rhai_group_choice(10), Some(0)),
        ("rhai_group_10_match_middle", rhai_group_choice(10), Some(5)),
        ("rhai_group_10_match_last", rhai_group_choice(10), Some(9)),
        ("rhai_group_10_no_match", rhai_group_choice(10), None),
    ];

    for (name, segment, matched) in grouped_shapes {
        group.bench_function(name, |b| {
            b.to_async(&rt).iter(|| {
                let mut segment = segment.clone();
                let ex = lang_exchange(matched);
                async move { black_box(segment.run(ex).await) }
            })
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_choice_predicates,
    bench_choice_language_predicates
);
criterion_main!(benches);
