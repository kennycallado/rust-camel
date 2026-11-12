//! `on_exceptions` clause `code` matching for `CamelError::Business`
//! (business-error-model task 1.4).
//!
//! A `code` clause is one conjunctive matcher beside `kind` and
//! `message_contains`: it fires only when the error is a
//! `CamelError::Business` carrying the same code. Non-business errors and
//! business errors with another code never satisfy it.

use std::sync::Arc;

use camel_api::{Body, BoxProcessor, BoxProcessorExt, CamelError, Exchange, OpaqueProcessor};
use camel_component_api::NoOpComponentContext;
use camel_component_direct::DirectComponent;
use camel_component_mock::MockComponent;
use camel_core::CamelContext;
use camel_core::route::{BuilderStep, RouteDefinition};
use camel_dsl::parse_yaml;
use tower::ServiceExt;

/// Builds a `direct:start` route with the given `on_exceptions` clauses
/// (already indented at the clause slot) and the given `steps` body
/// (already indented at the step-item slot).
fn yaml_route(clauses: &str, steps: &str) -> String {
    format!(
        "routes:\n  - id: on-exception-code\n    from: direct:start\n    error_handler:\n      on_exceptions:\n{clauses}    steps:\n{steps}"
    )
}

/// Auxiliary route whose single processor always fails with `err`.
fn failing_route(from: &str, err: CamelError) -> RouteDefinition {
    RouteDefinition::new(
        from,
        vec![BuilderStep::Processor(OpaqueProcessor(
            BoxProcessor::from_fn(move |_ex: Exchange| {
                let err = err.clone();
                Box::pin(async move { Err(err) })
            }),
        ))],
    )
    .with_route_id(format!("fail-{}", from.replace(':', "-")))
}

async fn run(
    definitions: Vec<RouteDefinition>,
    exchange: Exchange,
) -> Result<Exchange, CamelError> {
    let mut ctx = CamelContext::builder()
        .build()
        .await
        .expect("test context builds");
    ctx.register_component(DirectComponent::new());
    ctx.register_component(MockComponent::new());
    for definition in definitions {
        ctx.add_route_definition(definition)
            .await
            .expect("route definition adds");
    }
    ctx.start().await.expect("context starts");

    let producer_ctx = ctx.producer_context();
    let component = ctx
        .registry()
        .get("direct")
        .expect("direct component is registered");
    let endpoint = component
        .create_endpoint("direct:start", &ctx)
        .expect("direct endpoint resolves");
    let producer = endpoint
        .create_producer(Arc::new(NoOpComponentContext), &producer_ctx)
        .expect("direct producer builds");
    producer.oneshot(exchange).await
}

fn assert_absorbed(exchange: &Exchange) {
    assert!(
        exchange.error.is_none(),
        "a matching handled clause must absorb the error, got {:?}",
        exchange.error
    );
}

fn assert_unmatched_business_code(err: &CamelError, expected: &str) {
    match err {
        CamelError::Business { code, .. } => {
            assert_eq!(
                code, expected,
                "exchange must carry the raised business code"
            );
        }
        other => panic!("expected an unmatched Business error with code {expected}, got {other:?}"),
    }
}

const CODE_CLAUSE: &str = "        - code: ORDER_NOT_FOUND\n          handled: true\n";

#[tokio::test]
async fn code_matches_business_error() {
    let yaml = yaml_route(CODE_CLAUSE, "      - throw: { code: ORDER_NOT_FOUND }\n");
    let definitions = parse_yaml(&yaml).expect("code clause must load");

    let exchange = run(definitions, Exchange::default())
        .await
        .expect("matching handled clause completes the pipeline");
    assert_absorbed(&exchange);
}

#[tokio::test]
async fn code_does_not_match_other_code_or_system_error() {
    // Same clause, a business error with a different code.
    let other_code = yaml_route(CODE_CLAUSE, "      - throw: { code: OTHER_CODE }\n");
    let definitions = parse_yaml(&other_code).expect("code clause must load");
    let err = run(definitions, Exchange::default())
        .await
        .expect_err("non-matching code takes the unmatched-error path");
    assert_unmatched_business_code(&err, "OTHER_CODE");

    // Same clause, a non-business (Io) error raised by a producer step.
    let io_route = yaml_route(CODE_CLAUSE, "      - to: direct:io-fail\n");
    let mut definitions = parse_yaml(&io_route).expect("code clause must load");
    definitions.push(failing_route(
        "direct:io-fail",
        CamelError::Io("disk".into()),
    ));
    let err = run(definitions, Exchange::default())
        .await
        .expect_err("code clause must not match a system error");
    match err {
        CamelError::Io(message) => assert_eq!(message, "disk"),
        other => panic!("expected the unmatched Io error, got {other:?}"),
    }
}

#[tokio::test]
async fn code_conjunction_with_kind() {
    let clause =
        "        - kind: Business\n          code: ORDER_NOT_FOUND\n          handled: true\n";

    let business = yaml_route(clause, "      - throw: { code: ORDER_NOT_FOUND }\n");
    let definitions = parse_yaml(&business).expect("kind+code clause must load");
    let exchange = run(definitions, Exchange::default())
        .await
        .expect("both conjuncts match the business error");
    assert_absorbed(&exchange);

    // The kind conjunct alone would match; the code conjunct must still
    // reject a business error carrying a different code.
    let other_code = yaml_route(clause, "      - throw: { code: OTHER_CODE }\n");
    let definitions = parse_yaml(&other_code).expect("kind+code clause must load");
    let err = run(definitions, Exchange::default())
        .await
        .expect_err("kind+code clause must not match another business code");
    assert_unmatched_business_code(&err, "OTHER_CODE");

    // The code conjunct alone would match; the kind conjunct must still
    // reject a business error when the clause names another kind.
    let io_kind_clause =
        "        - kind: Io\n          code: ORDER_NOT_FOUND\n          handled: true\n";
    let business_under_io =
        yaml_route(io_kind_clause, "      - throw: { code: ORDER_NOT_FOUND }\n");
    let definitions = parse_yaml(&business_under_io).expect("kind+code clause must load");
    let err = run(definitions, Exchange::default())
        .await
        .expect_err("Io kind clause must not match a Business error carrying the code");
    assert_unmatched_business_code(&err, "ORDER_NOT_FOUND");

    // The code conjunct alone would not match Io, and neither does the pair.
    let io_route = yaml_route(clause, "      - to: direct:io-fail\n");
    let mut definitions = parse_yaml(&io_route).expect("kind+code clause must load");
    definitions.push(failing_route(
        "direct:io-fail",
        CamelError::Io("disk".into()),
    ));
    let err = run(definitions, Exchange::default())
        .await
        .expect_err("kind+code clause must not match Io");
    match err {
        CamelError::Io(message) => assert_eq!(message, "disk"),
        other => panic!("expected the unmatched Io error, got {other:?}"),
    }
}

#[tokio::test]
async fn kind_business_matches_only_business_runtime() {
    let clause = "        - kind: Business\n          handled: true\n";

    let business = yaml_route(clause, "      - throw: { code: STOCK_LOW }\n");
    let definitions = parse_yaml(&business).expect("kind clause must load");
    let exchange = run(definitions, Exchange::default())
        .await
        .expect("Business kind matches the business error");
    assert_absorbed(&exchange);

    let validation_route = yaml_route(clause, "      - to: direct:validation-fail\n");
    let mut definitions = parse_yaml(&validation_route).expect("kind clause must load");
    definitions.push(failing_route(
        "direct:validation-fail",
        CamelError::ValidationError("bad payload".into()),
    ));
    let err = run(definitions, Exchange::default())
        .await
        .expect_err("Business kind must not match ValidationError");
    match err {
        CamelError::ValidationError(message) => assert_eq!(message, "bad payload"),
        other => panic!("expected the unmatched ValidationError, got {other:?}"),
    }
}

#[tokio::test]
async fn first_match_wins_with_code_clauses() {
    // Both clauses match code `A`; only the first may fire, so the second
    // clause's marker step must never run (the body stays `Empty`).
    let clauses = "        - code: A\n          handled: true\n        - code: A\n          handled: true\n          steps:\n            - convert_body_to: json\n";
    let yaml = yaml_route(clauses, "      - throw: { code: A }\n");
    let definitions = parse_yaml(&yaml).expect("code clauses must load");

    let exchange = run(definitions, Exchange::default())
        .await
        .expect("first matching handled clause completes the pipeline");
    assert_absorbed(&exchange);
    assert!(
        matches!(exchange.input.body, Body::Empty),
        "the second same-code clause must not run, body is {:?}",
        exchange.input.body
    );
}
