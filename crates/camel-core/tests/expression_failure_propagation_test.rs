//! Integration proof for the language-value-boundary Phase 1 change
//! (`openspec/changes/language-value-boundary`, task 1.9).
//!
//! A failing language evaluation must fail the step as
//! `CamelError::ExpressionFailed` carrying the full E3 diagnostic set —
//! language, route id, `{verb}#{index}` step id, verb, position and class —
//! instead of silently dropping the exchange or defaulting the value.
//!
//! The suite is data-driven: [`failing_steps`] is the per-verb table of
//! routes that fail at evaluation, and the `verb_*` macros generate one
//! independently-named `#[test]` per row (fails-step, do_try-visible,
//! on_exception-visible). Named contract tests for cross-language rows,
//! catch-when chaining, redaction, strict bool and read-only route-add
//! rejection follow at the bottom.
//!
//! FEATURE PREREQUISITE: rhai/jsonpath are optional camel-core features
//! (`lang-rhai`, `lang-jsonpath`), not in the default graph. The suite is
//! compiled only when both are enabled so default-feature builds skip
//! cleanly. Run with:
//! `cargo test -p camel-core --test expression_failure_propagation_test \
//!      --features lang-rhai,lang-jsonpath,lang-js,lang-xpath`

#![cfg(all(feature = "lang-rhai", feature = "lang-jsonpath"))]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use camel_api::{
    AggregationStrategy, BoxProcessor, BoxProcessorExt, CamelError, ErrorHandlerConfig, Exchange,
    ExpressionErrorClass, Message, OpaqueProcessor, Value,
};
use camel_component_api::RuntimeObservability;
use camel_core::{
    CamelContext, RouteDefinition,
    route::{
        BuilderStep, DeclarativeWhenStep, DoTryCatchClauseBuilder, DoTryFinallyBuilder,
        LanguageExpressionDef, ValueSourceDef,
    },
};
use camel_processor::{LogLevel, PeekStaleMissPolicy};
use tower::ServiceExt;

/// The canonical arithmetic failure: a non-numeric string coerced with
/// `parse_float()` throws at eval time (class `arithmetic`, position present).
const PARSE_FLOAT: &str = r#""no-es-un-numero".parse_float()"#;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn rt() -> Arc<dyn RuntimeObservability> {
    Arc::new(camel_component_api::NoOpComponentContext)
}

fn sanitize(verb: &str) -> String {
    verb.replace(['/', ' ', '_'], "-")
}

fn expr(language: &str, source: &str) -> LanguageExpressionDef {
    LanguageExpressionDef {
        language: language.to_string(),
        source: source.to_string(),
    }
}

fn rhai(source: &str) -> LanguageExpressionDef {
    expr("rhai", source)
}

fn literal(value: Value) -> ValueSourceDef {
    ValueSourceDef::Literal(value)
}

fn set_header_literal(key: &str, value: &str) -> BuilderStep {
    BuilderStep::DeclarativeSetHeader {
        key: key.to_string(),
        value: literal(Value::String(value.to_string())),
    }
}

fn failing_processor() -> BuilderStep {
    BuilderStep::Processor(OpaqueProcessor(BoxProcessor::from_fn(|_ex: Exchange| {
        Box::pin(async { Err(CamelError::ProcessorError("boom-A".into())) })
    })))
}

/// A fresh context with `direct` and `mock` registered (and the built-in
/// language registry / memory repositories).
async fn new_context() -> (CamelContext, camel_component_mock::MockComponent) {
    let mut ctx = CamelContext::builder()
        .build()
        .await
        .expect("context builds");
    ctx.register_component(camel_component_direct::DirectComponent::new());
    let mock = camel_component_mock::MockComponent::new();
    ctx.register_component(mock.clone());
    (ctx, mock)
}

fn direct_not_registered(err: &CamelError) -> bool {
    matches!(
        err,
        CamelError::EndpointCreationFailed(msg)
            if msg.starts_with("direct endpoint '") && msg.ends_with("' not registered")
    )
}

/// Send one exchange to a `direct:` endpoint, retrying while the route
/// consumer's spawned registration task has not yet registered the endpoint
/// (bounded by a deadline; no test sleep). Returns the raw oneshot result so
/// failing routes are observable, not panicking.
async fn send_direct(
    ctx: &CamelContext,
    uri: &str,
    exchange: Exchange,
) -> Result<Exchange, CamelError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let component = {
            let registry = ctx.registry();
            registry.get("direct").expect("direct component registered")
        };
        let producer_ctx = ctx.producer_context();
        let endpoint = component
            .create_endpoint(uri, ctx)
            .expect("direct endpoint creation");
        let producer = endpoint
            .create_producer(rt(), &producer_ctx)
            .expect("direct producer creation");

        match producer.oneshot(exchange.clone()).await {
            Err(err) if direct_not_registered(&err) && tokio::time::Instant::now() < deadline => {
                tokio::task::yield_now().await;
            }
            other => return other,
        }
    }
}

/// Register `steps` as route `route_id`, run one exchange through it, stop the
/// context and return the outcome plus the mock component (whose endpoint
/// storage outlives the context).
async fn run_route(
    route_id: &str,
    steps: Vec<BuilderStep>,
    handler: Option<ErrorHandlerConfig>,
) -> (
    Result<Exchange, CamelError>,
    camel_component_mock::MockComponent,
) {
    let (mut ctx, mock) = new_context().await;
    let mut def = RouteDefinition::new(format!("direct:{route_id}"), steps).with_route_id(route_id);
    if let Some(handler) = handler {
        def = def.with_error_handler(handler);
    }
    ctx.add_route_definition(def)
        .await
        .expect("route must compile");
    ctx.start().await.expect("context start");
    let result = send_direct(
        &ctx,
        &format!("direct:{route_id}"),
        Exchange::new(Message::new("payload")),
    )
    .await;
    let _ = ctx.stop().await;
    (result, mock)
}

// ---------------------------------------------------------------------------
// E3 diagnostic assertion
// ---------------------------------------------------------------------------

/// Which `position` claim a row makes.
#[derive(Clone, Copy)]
enum Pos {
    Some,
    None,
    Any,
}

fn assert_expression_failed(
    err: &CamelError,
    route_id: &str,
    language: &str,
    verb: &str,
    class: ExpressionErrorClass,
    pos: Pos,
) {
    match err {
        CamelError::ExpressionFailed {
            language: l,
            route_id: r,
            step_id,
            verb: v,
            class: c,
            position,
            ..
        } => {
            assert_eq!(l, language, "language mismatch: {err}");
            assert_eq!(r, route_id, "route id mismatch: {err}");
            assert_eq!(v, verb, "verb mismatch: {err}");
            assert_eq!(*c, class, "class mismatch: {err}");
            let prefix = format!("{verb}#");
            assert!(
                step_id.starts_with(&prefix),
                "step id {step_id:?} must start with {prefix:?}"
            );
            let index = &step_id[prefix.len()..];
            assert!(
                index.parse::<usize>().is_ok(),
                "step id {step_id:?} must carry a numeric index after the verb"
            );
            match pos {
                Pos::Some => assert!(position.is_some(), "position must be present: {err}"),
                Pos::None => assert!(position.is_none(), "position must be absent: {err}"),
                Pos::Any => {}
            }
        }
        other => panic!("expected ExpressionFailed, got {other:?}"),
    }
}

/// Language/verb-only assertion for the cross-language rows where the exact
/// class is engine-specific.
fn assert_language_verb(err: &CamelError, route_id: &str, language: &str, verb: &str) {
    match err {
        CamelError::ExpressionFailed {
            language: l,
            route_id: r,
            step_id,
            verb: v,
            ..
        } => {
            assert_eq!(l, language, "language mismatch: {err}");
            assert_eq!(r, route_id, "route id mismatch: {err}");
            assert_eq!(v, verb, "verb mismatch: {err}");
            assert!(
                step_id.starts_with(&format!("{verb}#")),
                "step id {step_id:?} must carry the verb"
            );
        }
        other => panic!("expected ExpressionFailed, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Per-verb failing-route table
// ---------------------------------------------------------------------------

/// Build the minimal steps that make `key` fail at language evaluation.
///
/// One entry per verb from task 1.9 step 1. `script_readonly` and
/// `script_mutating` are distinct table keys that both surface as verb
/// `script`.
fn failing_steps(key: &str) -> Vec<BuilderStep> {
    match key {
        "set_property" => vec![BuilderStep::DeclarativeSetProperty {
            key: "p".to_string(),
            value_source: ValueSourceDef::Expression(rhai(PARSE_FLOAT)),
        }],
        "set_header" => vec![BuilderStep::DeclarativeSetHeader {
            key: "h".to_string(),
            value: ValueSourceDef::Expression(rhai(PARSE_FLOAT)),
        }],
        "set_header_if_absent" => vec![BuilderStep::DeclarativeSetHeaderIfAbsent {
            key: "h".to_string(),
            value: ValueSourceDef::Expression(rhai(PARSE_FLOAT)),
        }],
        "set_body" => vec![BuilderStep::DeclarativeSetBody {
            value: ValueSourceDef::Expression(rhai(PARSE_FLOAT)),
        }],
        // A language WITHOUT MutatingExpression degrades to the read-only
        // Expression -> SetBody fallback; the simple comparison coerces a
        // non-numeric header and fails as type-mismatch.
        "script_readonly" => vec![
            set_header_literal("secret", "abc"),
            BuilderStep::DeclarativeScript {
                expression: expr("simple", "${header.secret} > 1"),
            },
        ],
        // Rhai owns a MutatingExpression: the ScriptMutator path throws.
        "script_mutating" => vec![BuilderStep::DeclarativeScript {
            expression: rhai(r#"throw "boom""#),
        }],
        "filter" => vec![BuilderStep::DeclarativeFilter {
            predicate: rhai(PARSE_FLOAT),
            steps: vec![],
        }],
        "choice/when" => vec![BuilderStep::DeclarativeChoice {
            whens: vec![DeclarativeWhenStep {
                predicate: rhai(PARSE_FLOAT),
                steps: vec![],
            }],
            otherwise: None,
        }],
        "loop while" => vec![BuilderStep::DeclarativeLoop {
            count: None,
            while_predicate: Some(rhai(PARSE_FLOAT)),
            steps: vec![],
            max_iterations: None,
        }],
        "validate" => vec![BuilderStep::Validate {
            predicate: rhai(PARSE_FLOAT),
        }],
        "catch when" => vec![BuilderStep::DeclarativeDoTry {
            try_steps: vec![failing_processor()],
            catch: vec![DoTryCatchClauseBuilder {
                exception: None,
                when: Some(rhai(PARSE_FLOAT)),
                on_when: None,
                disposition: camel_api::ExceptionDisposition::Handled,
                steps: vec![],
            }],
            finally: None,
        }],
        "catch on_when" => vec![BuilderStep::DeclarativeDoTry {
            try_steps: vec![failing_processor()],
            catch: vec![DoTryCatchClauseBuilder {
                exception: Some(vec!["ProcessorError".to_string()]),
                when: None,
                on_when: Some(rhai(PARSE_FLOAT)),
                disposition: camel_api::ExceptionDisposition::Handled,
                steps: vec![],
            }],
            finally: None,
        }],
        "finally on_when" => vec![BuilderStep::DeclarativeDoTry {
            try_steps: vec![],
            catch: vec![],
            finally: Some(DoTryFinallyBuilder {
                on_when: Some(rhai(PARSE_FLOAT)),
                steps: vec![],
            }),
        }],
        "split" => vec![BuilderStep::DeclarativeSplit {
            expression: rhai(PARSE_FLOAT),
            aggregation: AggregationStrategy::Original,
            parallel: false,
            parallel_limit: None,
            trace_item_threshold: None,
            stop_on_exception: false,
            steps: vec![],
        }],
        "dynamic_router" => vec![BuilderStep::DeclarativeDynamicRouter {
            expression: rhai(PARSE_FLOAT),
            uri_delimiter: ",".to_string(),
            cache_size: 8,
            ignore_invalid_endpoints: true,
            max_iterations: 3,
        }],
        "routing_slip" => vec![BuilderStep::DeclarativeRoutingSlip {
            expression: rhai(PARSE_FLOAT),
            uri_delimiter: ",".to_string(),
            cache_size: 8,
            ignore_invalid_endpoints: true,
        }],
        "recipient_list" => vec![BuilderStep::DeclarativeRecipientList {
            expression: rhai(PARSE_FLOAT),
            delimiter: ",".to_string(),
            parallel: false,
            parallel_limit: None,
            stop_on_exception: false,
            aggregation: "original".to_string(),
        }],
        "sort" => vec![
            BuilderStep::DeclarativeSetBody {
                value: literal(Value::Array(vec![Value::from(1), Value::from(2)])),
            },
            BuilderStep::Sort {
                expression: rhai(PARSE_FLOAT),
                reverse: false,
            },
        ],
        "claim_check" => vec![BuilderStep::ClaimCheck {
            repository: "memory".to_string(),
            operation: "set".to_string(),
            key: rhai(PARSE_FLOAT),
            filter: None,
        }],
        "idempotent_consumer" => vec![BuilderStep::IdempotentConsumer {
            repository: "memory".to_string(),
            expression: rhai(PARSE_FLOAT),
            steps: vec![],
            eager: false,
            remove_on_failure: false,
        }],
        "log" => vec![BuilderStep::DeclarativeLog {
            level: LogLevel::Info,
            message: ValueSourceDef::Expression(rhai(PARSE_FLOAT)),
        }],
        "cache_invalidate" => vec![BuilderStep::CacheInvalidate {
            repository: None,
            key: Some(rhai(PARSE_FLOAT)),
            key_prefix: None,
        }],
        "cache_peek_stale" => vec![BuilderStep::CachePeekStale {
            repository: None,
            key: rhai(PARSE_FLOAT),
            on_miss: PeekStaleMissPolicy::Continue,
        }],
        other => panic!("unknown failing-verb table key: {other}"),
    }
}

/// The E3 (language, verb, class, position) tuple a table key expects.
fn verb_diagnostics(key: &str) -> (&'static str, &'static str, ExpressionErrorClass, Pos) {
    match key {
        "script_readonly" => (
            "simple",
            "script",
            ExpressionErrorClass::TypeMismatch,
            Pos::None,
        ),
        "script_mutating" => ("rhai", "script", ExpressionErrorClass::Runtime, Pos::Some),
        "set_property" => (
            "rhai",
            "set_property",
            ExpressionErrorClass::Arithmetic,
            Pos::Some,
        ),
        "set_header" => (
            "rhai",
            "set_header",
            ExpressionErrorClass::Arithmetic,
            Pos::Some,
        ),
        "set_header_if_absent" => (
            "rhai",
            "set_header_if_absent",
            ExpressionErrorClass::Arithmetic,
            Pos::Some,
        ),
        "set_body" => (
            "rhai",
            "set_body",
            ExpressionErrorClass::Arithmetic,
            Pos::Some,
        ),
        "filter" => (
            "rhai",
            "filter",
            ExpressionErrorClass::Arithmetic,
            Pos::Some,
        ),
        "choice/when" => (
            "rhai",
            "choice/when",
            ExpressionErrorClass::Arithmetic,
            Pos::Some,
        ),
        "loop while" => (
            "rhai",
            "loop while",
            ExpressionErrorClass::Arithmetic,
            Pos::Some,
        ),
        "validate" => (
            "rhai",
            "validate",
            ExpressionErrorClass::Arithmetic,
            Pos::Some,
        ),
        "catch when" => (
            "rhai",
            "catch when",
            ExpressionErrorClass::Arithmetic,
            Pos::Some,
        ),
        "catch on_when" => (
            "rhai",
            "catch on_when",
            ExpressionErrorClass::Arithmetic,
            Pos::Some,
        ),
        "finally on_when" => (
            "rhai",
            "finally on_when",
            ExpressionErrorClass::Arithmetic,
            Pos::Some,
        ),
        "split" => ("rhai", "split", ExpressionErrorClass::Arithmetic, Pos::Some),
        "dynamic_router" => (
            "rhai",
            "dynamic_router",
            ExpressionErrorClass::Arithmetic,
            Pos::Some,
        ),
        "routing_slip" => (
            "rhai",
            "routing_slip",
            ExpressionErrorClass::Arithmetic,
            Pos::Some,
        ),
        "recipient_list" => (
            "rhai",
            "recipient_list",
            ExpressionErrorClass::Arithmetic,
            Pos::Some,
        ),
        "sort" => ("rhai", "sort", ExpressionErrorClass::Arithmetic, Pos::Some),
        "claim_check" => (
            "rhai",
            "claim_check",
            ExpressionErrorClass::Arithmetic,
            Pos::Some,
        ),
        "idempotent_consumer" => (
            "rhai",
            "idempotent_consumer",
            ExpressionErrorClass::Arithmetic,
            Pos::Some,
        ),
        "log" => ("rhai", "log", ExpressionErrorClass::Arithmetic, Pos::Some),
        "cache_invalidate" => (
            "rhai",
            "cache_invalidate",
            ExpressionErrorClass::Arithmetic,
            Pos::Some,
        ),
        "cache_peek_stale" => (
            "rhai",
            "cache_peek_stale",
            ExpressionErrorClass::Arithmetic,
            Pos::Some,
        ),
        other => panic!("unknown failing-verb table key: {other}"),
    }
}

/// Run one table row through a plain route and assert the full E3 set.
async fn assert_fails_step(key: &str) {
    let steps = failing_steps(key);
    let route_id = format!("verb-{}", sanitize(key));
    let (result, _mock) = run_route(&route_id, steps, None).await;
    let err = result.expect_err("route must fail with ExpressionFailed");
    let (language, verb, class, pos) = verb_diagnostics(key);
    assert_expression_failed(&err, &route_id, language, verb, class, pos);
}

/// Handler visibility (a): the same failing row inside
/// `do_try/catch {exception: [ExpressionFailed]}` must be caught and the route
/// must complete.
async fn assert_do_try_visible(key: &str) {
    let route_id = format!("verb-{}-dotry", sanitize(key));
    let wrapped = vec![
        BuilderStep::DeclarativeDoTry {
            try_steps: failing_steps(key),
            catch: vec![DoTryCatchClauseBuilder {
                exception: Some(vec!["ExpressionFailed".to_string()]),
                when: None,
                on_when: None,
                disposition: camel_api::ExceptionDisposition::Handled,
                steps: vec![BuilderStep::To("mock:caught".to_string())],
            }],
            finally: None,
        },
        BuilderStep::To("mock:done".to_string()),
    ];
    let (result, mock) = run_route(&route_id, wrapped, None).await;
    assert!(
        result.is_ok(),
        "do_try catch must absorb ExpressionFailed for {key}: {result:?}"
    );
    let caught = mock.get_endpoint("caught").expect("caught endpoint exists");
    caught.await_exchanges(1, Duration::from_secs(5)).await;
    caught.assert_exchange_count(1).await;
    mock.get_endpoint("done")
        .expect("done endpoint exists")
        .assert_exchange_count(1)
        .await;
}

/// Handler visibility (b): a route-level `on_exception` matching
/// `ExpressionFailed` must observe the failure.
async fn assert_on_exception_visible(key: &str) {
    let route_id = format!("verb-{}-onexc", sanitize(key));
    let handler = ErrorHandlerConfig::log_only()
        .on_exception(|e| matches!(e, CamelError::ExpressionFailed { .. }))
        .handled(true)
        .handled_by("mock:observed")
        .build();
    let (result, mock) = run_route(&route_id, failing_steps(key), Some(handler)).await;
    assert!(
        result.is_ok(),
        "handled on_exception must absorb ExpressionFailed for {key}: {result:?}"
    );
    let observed = mock
        .get_endpoint("observed")
        .expect("observed endpoint exists");
    observed.await_exchanges(1, Duration::from_secs(5)).await;
    observed.assert_exchange_count(1).await;
    let received = observed.get_received_exchanges().await;
    let message = received[0]
        .property(camel_api::exchange::PROPERTY_EXCEPTION_MESSAGE)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    assert!(
        message.contains("expression failed"),
        "handler must observe ExpressionFailed for {key}, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// Generated per-verb tests (one #[test] per row)
// ---------------------------------------------------------------------------

macro_rules! verb_fails_test {
    ($name:ident, $key:literal) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn $name() {
            assert_fails_step($key).await;
        }
    };
}

macro_rules! verb_do_try_test {
    ($name:ident, $key:literal) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn $name() {
            assert_do_try_visible($key).await;
        }
    };
}

macro_rules! verb_on_exception_test {
    ($name:ident, $key:literal) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn $name() {
            assert_on_exception_visible($key).await;
        }
    };
}

verb_fails_test!(
    verb_set_property_expression_error_fails_step,
    "set_property"
);
verb_do_try_test!(verb_set_property_do_try_visible, "set_property");
verb_on_exception_test!(verb_set_property_on_exception_visible, "set_property");

verb_fails_test!(verb_set_header_expression_error_fails_step, "set_header");
verb_do_try_test!(verb_set_header_do_try_visible, "set_header");
verb_on_exception_test!(verb_set_header_on_exception_visible, "set_header");

verb_fails_test!(
    verb_set_header_if_absent_expression_error_fails_step,
    "set_header_if_absent"
);
verb_do_try_test!(
    verb_set_header_if_absent_do_try_visible,
    "set_header_if_absent"
);
verb_on_exception_test!(
    verb_set_header_if_absent_on_exception_visible,
    "set_header_if_absent"
);

verb_fails_test!(verb_set_body_expression_error_fails_step, "set_body");
verb_do_try_test!(verb_set_body_do_try_visible, "set_body");
verb_on_exception_test!(verb_set_body_on_exception_visible, "set_body");

verb_fails_test!(
    verb_script_readonly_expression_error_fails_step,
    "script_readonly"
);
verb_do_try_test!(verb_script_readonly_do_try_visible, "script_readonly");
verb_on_exception_test!(verb_script_readonly_on_exception_visible, "script_readonly");

verb_fails_test!(
    verb_script_mutating_expression_error_fails_step,
    "script_mutating"
);
verb_do_try_test!(verb_script_mutating_do_try_visible, "script_mutating");
verb_on_exception_test!(verb_script_mutating_on_exception_visible, "script_mutating");

verb_fails_test!(verb_filter_expression_error_fails_step, "filter");
verb_do_try_test!(verb_filter_do_try_visible, "filter");
verb_on_exception_test!(verb_filter_on_exception_visible, "filter");

verb_fails_test!(verb_choice_when_expression_error_fails_step, "choice/when");
verb_do_try_test!(verb_choice_when_do_try_visible, "choice/when");
verb_on_exception_test!(verb_choice_when_on_exception_visible, "choice/when");

verb_fails_test!(verb_loop_while_expression_error_fails_step, "loop while");
verb_do_try_test!(verb_loop_while_do_try_visible, "loop while");
verb_on_exception_test!(verb_loop_while_on_exception_visible, "loop while");

verb_fails_test!(verb_validate_expression_error_fails_step, "validate");
verb_do_try_test!(verb_validate_do_try_visible, "validate");
verb_on_exception_test!(verb_validate_on_exception_visible, "validate");

verb_fails_test!(verb_catch_when_expression_error_fails_step, "catch when");
verb_do_try_test!(verb_catch_when_do_try_visible, "catch when");
verb_on_exception_test!(verb_catch_when_on_exception_visible, "catch when");

verb_fails_test!(
    verb_catch_on_when_expression_error_fails_step,
    "catch on_when"
);
verb_do_try_test!(verb_catch_on_when_do_try_visible, "catch on_when");
verb_on_exception_test!(verb_catch_on_when_on_exception_visible, "catch on_when");

verb_fails_test!(
    verb_finally_on_when_expression_error_fails_step,
    "finally on_when"
);
verb_do_try_test!(verb_finally_on_when_do_try_visible, "finally on_when");
verb_on_exception_test!(verb_finally_on_when_on_exception_visible, "finally on_when");

verb_fails_test!(verb_split_expression_error_fails_step, "split");
verb_do_try_test!(verb_split_do_try_visible, "split");
verb_on_exception_test!(verb_split_on_exception_visible, "split");

verb_fails_test!(
    verb_dynamic_router_expression_error_fails_step,
    "dynamic_router"
);
verb_do_try_test!(verb_dynamic_router_do_try_visible, "dynamic_router");
verb_on_exception_test!(verb_dynamic_router_on_exception_visible, "dynamic_router");

verb_fails_test!(
    verb_routing_slip_expression_error_fails_step,
    "routing_slip"
);
verb_do_try_test!(verb_routing_slip_do_try_visible, "routing_slip");
verb_on_exception_test!(verb_routing_slip_on_exception_visible, "routing_slip");

verb_fails_test!(
    verb_recipient_list_expression_error_fails_step,
    "recipient_list"
);
verb_do_try_test!(verb_recipient_list_do_try_visible, "recipient_list");
verb_on_exception_test!(verb_recipient_list_on_exception_visible, "recipient_list");

verb_fails_test!(verb_sort_expression_error_fails_step, "sort");
verb_do_try_test!(verb_sort_do_try_visible, "sort");
verb_on_exception_test!(verb_sort_on_exception_visible, "sort");

verb_fails_test!(verb_claim_check_expression_error_fails_step, "claim_check");
verb_do_try_test!(verb_claim_check_do_try_visible, "claim_check");
verb_on_exception_test!(verb_claim_check_on_exception_visible, "claim_check");

verb_fails_test!(
    verb_idempotent_consumer_expression_error_fails_step,
    "idempotent_consumer"
);
verb_do_try_test!(
    verb_idempotent_consumer_do_try_visible,
    "idempotent_consumer"
);
verb_on_exception_test!(
    verb_idempotent_consumer_on_exception_visible,
    "idempotent_consumer"
);

verb_fails_test!(verb_log_expression_error_fails_step, "log");
verb_do_try_test!(verb_log_do_try_visible, "log");
verb_on_exception_test!(verb_log_on_exception_visible, "log");

verb_fails_test!(
    verb_cache_invalidate_expression_error_fails_step,
    "cache_invalidate"
);
verb_do_try_test!(verb_cache_invalidate_do_try_visible, "cache_invalidate");
verb_on_exception_test!(
    verb_cache_invalidate_on_exception_visible,
    "cache_invalidate"
);

verb_fails_test!(
    verb_cache_peek_stale_expression_error_fails_step,
    "cache_peek_stale"
);
verb_do_try_test!(verb_cache_peek_stale_do_try_visible, "cache_peek_stale");
verb_on_exception_test!(
    verb_cache_peek_stale_on_exception_visible,
    "cache_peek_stale"
);

// ---------------------------------------------------------------------------
// Cross-language rows (Q5): the swallow fix is language-agnostic
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cross_language_simple_and_jsonpath_rows() {
    // simple through set_property: a non-numeric header in a numeric
    // comparison fails with TypeMismatch and no operand leaks.
    let steps = vec![
        set_header_literal("x", "abc"),
        BuilderStep::DeclarativeSetProperty {
            key: "p".to_string(),
            value_source: ValueSourceDef::Expression(expr("simple", "${header.x} > 1")),
        },
    ];
    let (result, _mock) = run_route("xlang-simple-prop", steps, None).await;
    let err = result.expect_err("simple set_property must fail");
    assert_expression_failed(
        &err,
        "xlang-simple-prop",
        "simple",
        "set_property",
        ExpressionErrorClass::TypeMismatch,
        Pos::None,
    );

    // simple through filter: the predicate form of the same coercion.
    let steps = vec![
        set_header_literal("x", "abc"),
        BuilderStep::DeclarativeFilter {
            predicate: expr("simple", "${header.x} > 1"),
            steps: vec![],
        },
    ];
    let (result, _mock) = run_route("xlang-simple-filter", steps, None).await;
    let err = result.expect_err("simple filter must fail");
    assert_expression_failed(
        &err,
        "xlang-simple-filter",
        "simple",
        "filter",
        ExpressionErrorClass::TypeMismatch,
        Pos::None,
    );

    // jsonpath through filter: a Text body that is not JSON.
    let steps = vec![BuilderStep::DeclarativeFilter {
        predicate: expr("jsonpath", "$.x"),
        steps: vec![],
    }];
    let (result, _mock) = run_route("xlang-jsonpath-filter", steps, None).await;
    let err = result.expect_err("jsonpath filter on invalid JSON must fail");
    assert_language_verb(&err, "xlang-jsonpath-filter", "jsonpath", "filter");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cross_language_jsonpath_set_property_row() {
    // jsonpath through set_property: the same non-JSON Text body reaches the
    // evaluator as Conversion (position absent — the parser cannot point into
    // a body that is not JSON).
    let steps = vec![BuilderStep::DeclarativeSetProperty {
        key: "p".to_string(),
        value_source: ValueSourceDef::Expression(expr("jsonpath", "$.x")),
    }];
    let (result, _mock) = run_route("xlang-jsonpath-prop", steps, None).await;
    let err = result.expect_err("jsonpath set_property on invalid JSON must fail");
    assert_expression_failed(
        &err,
        "xlang-jsonpath-prop",
        "jsonpath",
        "set_property",
        ExpressionErrorClass::Conversion,
        Pos::None,
    );
}

// ---------------------------------------------------------------------------
// Catch-when chaining (P2)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catch_when_error_chains_original() {
    let steps = vec![BuilderStep::DeclarativeDoTry {
        try_steps: vec![failing_processor()],
        catch: vec![DoTryCatchClauseBuilder {
            exception: None,
            when: Some(rhai(PARSE_FLOAT)),
            on_when: None,
            disposition: camel_api::ExceptionDisposition::Handled,
            steps: vec![],
        }],
        finally: None,
    }];
    let (result, _mock) = run_route("catch-when-chain", steps, None).await;
    let err = result.expect_err("catch-when predicate error must surface");
    match err {
        CamelError::ExpressionFailed {
            ref verb,
            ref cause,
            ..
        } => {
            assert_eq!(verb, "catch when");
            let cause = cause.as_deref().expect("original error must be chained");
            assert!(
                cause.to_string().contains("boom-A"),
                "chained cause must render the original ProcessorError, got {cause}"
            );
        }
        other => panic!("expected ExpressionFailed with cause, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catch_on_when_error_chains_original() {
    let steps = vec![BuilderStep::DeclarativeDoTry {
        try_steps: vec![failing_processor()],
        catch: vec![DoTryCatchClauseBuilder {
            exception: Some(vec!["ProcessorError".to_string()]),
            when: None,
            on_when: Some(rhai(PARSE_FLOAT)),
            disposition: camel_api::ExceptionDisposition::Handled,
            steps: vec![],
        }],
        finally: None,
    }];
    let (result, _mock) = run_route("catch-on-when-chain", steps, None).await;
    let err = result.expect_err("catch on_when predicate error must surface");
    match err {
        CamelError::ExpressionFailed {
            ref verb,
            ref cause,
            ..
        } => {
            assert_eq!(verb, "catch on_when");
            let cause = cause.as_deref().expect("original error must be chained");
            assert!(
                cause.to_string().contains("boom-A"),
                "chained cause must render the original ProcessorError, got {cause}"
            );
        }
        other => panic!("expected ExpressionFailed with cause, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Redaction end-to-end (task 1.9 step 5)
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// The three redaction surfaces: returned error Display, captured logs, DLC
/// error text.
struct RedactionSurfaces {
    error_display: String,
    dlc_message: String,
    dlc_error_debug: String,
}

async fn run_redaction_case() -> RedactionSurfaces {
    // (a) Returned error Display — plain route, no handler.
    let steps = vec![BuilderStep::DeclarativeSetProperty {
        key: "s".to_string(),
        value_source: ValueSourceDef::Expression(rhai(r#""SECRET".parse_float()"#)),
    }];
    let (result, _mock) = run_route("redaction-plain", steps, None).await;
    let error_display = result.expect_err("SECRET parse must fail").to_string();

    // (c) DLC — on_exception forwards the failure to a mock dead-letter
    // endpoint with the error attached.
    let handler = ErrorHandlerConfig::log_only()
        .on_exception(|e| matches!(e, CamelError::ExpressionFailed { .. }))
        .handled(true)
        .handled_by("mock:dlc")
        .build();
    let steps = vec![BuilderStep::DeclarativeSetProperty {
        key: "s".to_string(),
        value_source: ValueSourceDef::Expression(rhai(r#""SECRET".parse_float()"#)),
    }];
    let (result, mock) = run_route("redaction-dlc", steps, Some(handler)).await;
    assert!(result.is_ok(), "handled DLC must absorb the error");
    let dlc = mock.get_endpoint("dlc").expect("dlc endpoint exists");
    dlc.await_exchanges(1, Duration::from_secs(5)).await;
    dlc.assert_exchange_count(1).await;
    let received = dlc.get_received_exchanges().await;
    let dlc_message = received[0]
        .property(camel_api::exchange::PROPERTY_EXCEPTION_MESSAGE)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let dlc_error_debug = format!("{:?}", received[0].error);

    // Emit a probe so the captured buffer is provably non-empty and
    // attributable to this scope.
    tracing::warn!("redaction-capture-probe");

    RedactionSurfaces {
        error_display,
        dlc_message,
        dlc_error_debug,
    }
}

#[test]
fn redaction_no_secret_in_error_logs_or_dlc() {
    let buffer = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing_subscriber::filter::LevelFilter::WARN)
        .without_time()
        .with_writer(buffer.clone())
        .finish();

    let surfaces = {
        use tracing_subscriber::util::SubscriberInitExt;
        let _guard = subscriber.set_default();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread runtime")
            .block_on(run_redaction_case())
    };

    // (a) Returned error Display.
    assert!(
        !surfaces.error_display.contains("SECRET"),
        "error Display leaked SECRET: {}",
        surfaces.error_display
    );
    // (b) Captured tracing records: non-empty (probe present) and redacted.
    let logs = String::from_utf8_lossy(&buffer.0.lock().expect("log buffer lock")).to_string();
    assert!(
        logs.contains("redaction-capture-probe"),
        "log capture must be wired (probe missing); captured: {logs}"
    );
    assert!(
        !logs.contains("SECRET"),
        "captured logs leaked SECRET: {logs}"
    );
    // (c) DLC payload.
    assert!(
        !surfaces.dlc_message.contains("SECRET"),
        "DLC CamelExceptionMessage leaked SECRET: {}",
        surfaces.dlc_message
    );
    assert!(
        !surfaces.dlc_error_debug.contains("SECRET"),
        "DLC error Debug leaked SECRET: {}",
        surfaces.dlc_error_debug
    );
}

// ---------------------------------------------------------------------------
// Strict bool predicate (P1)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn strict_bool_filter_errors() {
    let steps = vec![BuilderStep::DeclarativeFilter {
        predicate: rhai(r#""false""#),
        steps: vec![],
    }];
    let (result, _mock) = run_route("strict-bool-filter", steps, None).await;
    let err = result.expect_err("non-bool predicate must fail");
    assert_expression_failed(
        &err,
        "strict-bool-filter",
        "rhai",
        "filter",
        ExpressionErrorClass::TypeMismatch,
        Pos::Any,
    );
}

// ---------------------------------------------------------------------------
// Read-only mutation rejection at route add (B4)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_only_setter_rejected_at_route_add() {
    let (ctx, _mock) = new_context().await;
    let steps = vec![BuilderStep::DeclarativeSetProperty {
        key: "k".to_string(),
        value_source: ValueSourceDef::Expression(rhai(r#"set_property("k", 1)"#)),
    }];
    let def = RouteDefinition::new("direct:readonly-add", steps).with_route_id("readonly-add");
    let err = ctx
        .add_route_definition(def)
        .await
        .expect_err("read-only mutation must be rejected at route add");
    let text = err.to_string();
    assert!(
        text.contains("script:"),
        "error must point at the real script (script:), got: {text}"
    );
}

/// The read-only mutation walk resolves STATIC computed member names
/// (`camel[...]`), so a computed mutation is rejected at route add exactly
/// like its dot-notation twin.
#[cfg(feature = "lang-js")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn js_read_only_computed_mutation_rejected_at_route_add() {
    let (ctx, _mock) = new_context().await;
    let steps = vec![BuilderStep::DeclarativeFilter {
        predicate: expr("js", "camel[\"headers\"][\"set\"](\"k\", 1)"),
        steps: vec![],
    }];
    let def = RouteDefinition::new("direct:js-readonly-computed", steps)
        .with_route_id("js-readonly-computed");
    let err = ctx
        .add_route_definition(def)
        .await
        .expect_err("read-only computed mutation must be rejected at route add");
    let text = err.to_string();
    assert!(
        text.contains("script:"),
        "error must point at the real script (script:), got: {text}"
    );
}

/// A dynamic (statically undecidable) read-only JS mutation must fail the
/// step at runtime with a typed `ExpressionFailed`; the write is never
/// silently discarded with the snapshot. The body payload must not leak into
/// the error.
#[cfg(feature = "lang-js")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn js_dynamic_write_fails_route() {
    let steps = vec![BuilderStep::DeclarativeFilter {
        predicate: expr("js", "const k = \"body\"; camel[k] = 1; true"),
        steps: vec![],
    }];
    let (result, _mock) = run_route("js-dynamic-write", steps, None).await;
    let err = result.expect_err("dynamic read-only JS write must fail the route");
    assert_expression_failed(
        &err,
        "js-dynamic-write",
        "js",
        "filter",
        ExpressionErrorClass::Runtime,
        Pos::None,
    );
    let rendered = format!("{err} | {err:?}");
    assert!(
        !rendered.contains("payload"),
        "typed error leaked the body payload: {rendered}"
    );
}

// ---------------------------------------------------------------------------
// Conversion refusal names the trusted target (task 2.1)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_property_conversion_refusal_names_trusted_target() {
    // A NaN script result must fail the step with a typed conversion error
    // whose target is the TRUSTED compile-time destination ("property p"),
    // never runtime-derived key text.
    let steps = vec![BuilderStep::DeclarativeSetProperty {
        key: "p".to_string(),
        value_source: ValueSourceDef::Expression(rhai("0.0/0.0")),
    }];
    let (result, _mock) = run_route("conversion-refusal-target", steps, None).await;
    let err = result.expect_err("NaN result must fail the step");
    assert_expression_failed(
        &err,
        "conversion-refusal-target",
        "rhai",
        "set_property",
        ExpressionErrorClass::Conversion,
        Pos::None,
    );
    match &err {
        CamelError::ExpressionFailed {
            conversion: Some(detail),
            ..
        } => {
            assert_eq!(detail.source_type, "float (non-finite)");
            // The landed carrier rewrites the generic target to the trusted
            // EvalMeta.target, which compiler sites populate with the bare
            // property/header key (or "body") — never runtime data.
            assert_eq!(detail.target, "p");
        }
        other => panic!("expected conversion detail on ExpressionFailed, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inbound_scope_refusal_keeps_generic_target() {
    // An UNRELATED existing property that rhai cannot represent (u64::MAX)
    // fails when `make_scope` reads it into the script scope. That refusal is
    // about the property-entry surface, NOT this step's destination `p`, so
    // the carrier must keep the generic target instead of rewriting it.
    let steps = vec![
        BuilderStep::DeclarativeSetProperty {
            key: "huge".to_string(),
            value_source: literal(Value::from(u64::MAX)),
        },
        BuilderStep::DeclarativeSetProperty {
            key: "p".to_string(),
            value_source: ValueSourceDef::Expression(rhai("1")),
        },
    ];
    let (result, _mock) = run_route("inbound-scope-refusal", steps, None).await;
    let err = result.expect_err("unrepresentable inbound property must fail the step");
    assert_expression_failed(
        &err,
        "inbound-scope-refusal",
        "rhai",
        "set_property",
        ExpressionErrorClass::Conversion,
        Pos::None,
    );
    match &err {
        CamelError::ExpressionFailed {
            conversion: Some(detail),
            ..
        } => {
            assert_eq!(detail.source_type, "u64 > i64::MAX");
            assert_eq!(detail.target, "property entry");
        }
        other => panic!("expected conversion detail on ExpressionFailed, got {other:?}"),
    }
}
