//! Tests for RouteBuilder `.parameters()` — pending-slot semantics, per-endpoint
//! persistence, misuse surfacing at `build()`, and DSL-consistent canonical merge.

use std::collections::BTreeMap;

use camel_api::CamelError;
use camel_api::runtime::CanonicalStepSpec;
use camel_builder::RouteBuilder;
use camel_builder::StepAccumulator;
use camel_core::route::BuilderStep;
use camel_processor::LogLevel;

fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Extract the URI from an endpoint-bearing step (To/WireTap/Enrich/PollEnrich).
fn endpoint_uri(step: &BuilderStep) -> &str {
    match step {
        BuilderStep::To(uri)
        | BuilderStep::WireTap { uri }
        | BuilderStep::Enrich { uri, .. }
        | BuilderStep::PollEnrich { uri, .. } => uri.as_str(),
        other => panic!("expected an endpoint step, got {other:?}"),
    }
}

/// Assert `build()` fails and return the error (RouteDefinition has no Debug,
/// so `Result::unwrap_err` is unavailable here).
fn build_err(builder: RouteBuilder) -> CamelError {
    match builder.build() {
        Ok(_) => panic!("expected build() to fail"),
        Err(e) => e,
    }
}

#[test]
fn builder_parameters_on_to() {
    let route = RouteBuilder::from("timer:tick")
        .route_id("r-to")
        .to("log:out")
        .parameters(params(&[("showBody", "true")]))
        .build()
        .unwrap();

    assert_eq!(route.steps().len(), 1);
    assert_eq!(endpoint_uri(&route.steps()[0]), "log:out?showBody=true");
}

#[test]
fn builder_parameters_on_from() {
    let route = RouteBuilder::from("timer:tick")
        .route_id("r-from")
        .parameters(params(&[("period", "1000")]))
        .to("log:out")
        .build()
        .unwrap();

    assert_eq!(route.from_uri(), "timer:tick?period=1000");
}

#[test]
fn builder_multiple_endpoints_each_keep_parameters() {
    let route = RouteBuilder::from("timer:tick")
        .route_id("r-multi")
        .parameters(params(&[("period", "1000")]))
        .to("log:a")
        .parameters(params(&[("showBody", "true")]))
        .to("log:b")
        .parameters(params(&[("showHeaders", "true")]))
        .build()
        .unwrap();

    assert_eq!(route.from_uri(), "timer:tick?period=1000");
    assert_eq!(route.steps().len(), 2);
    assert_eq!(endpoint_uri(&route.steps()[0]), "log:a?showBody=true");
    assert_eq!(endpoint_uri(&route.steps()[1]), "log:b?showHeaders=true");
}

#[test]
fn builder_parameters_on_wire_tap_enrich_poll_enrich() {
    let route = RouteBuilder::from("direct:a")
        .route_id("r-wt")
        .wire_tap("log:audit")
        .parameters(params(&[("showBody", "true")]))
        .build()
        .unwrap();
    assert_eq!(endpoint_uri(&route.steps()[0]), "log:audit?showBody=true");

    let route = RouteBuilder::from("direct:b")
        .route_id("r-en")
        .enrich("db:query")
        .parameters(params(&[("dataSource", "customers")]))
        .build()
        .unwrap();
    assert_eq!(
        endpoint_uri(&route.steps()[0]),
        "db:query?dataSource=customers"
    );

    let route = RouteBuilder::from("direct:c")
        .route_id("r-pe")
        .poll_enrich("file:inbox", 1000)
        .parameters(params(&[("delay", "500")]))
        .build()
        .unwrap();
    assert_eq!(endpoint_uri(&route.steps()[0]), "file:inbox?delay=500");
}

#[test]
fn builder_parameters_no_pending_endpoint_errors_at_build() {
    let err = build_err(
        RouteBuilder::from("direct:x")
            .route_id("r-nopend")
            .to("log:x")
            .log("hi", LogLevel::Info)
            .parameters(params(&[("a", "1")])),
    );

    let msg = err.to_string();
    assert!(
        matches!(&err, CamelError::RouteError(_)),
        "expected CamelError::RouteError, got: {err:?}"
    );
    assert!(
        msg.contains("parameters") && msg.contains("endpoint"),
        "expected a misuse error naming the lack of a pending endpoint, got: {msg}"
    );
}

#[test]
fn builder_consecutive_parameters_errors_at_build() {
    let err = build_err(
        RouteBuilder::from("direct:y")
            .route_id("r-twice")
            .to("log:x")
            .parameters(params(&[("a", "1")]))
            .parameters(params(&[("b", "2")])),
    );

    let msg = err.to_string();
    assert!(
        matches!(&err, CamelError::RouteError(_)),
        "expected CamelError::RouteError, got: {err:?}"
    );
    assert!(
        msg.contains("parameters"),
        "expected a misuse error naming the duplicate .parameters() call, got: {msg}"
    );
}

#[test]
fn builder_duplicate_key_errors_at_build() {
    let err = build_err(
        RouteBuilder::from("direct:z")
            .route_id("r-dup")
            .to("kafka:orders?brokers=a")
            .parameters(params(&[("brokers", "b")])),
    );

    // URI merge failures surface as the typed EndpointUri variant, not RouteError.
    assert!(
        matches!(&err, CamelError::EndpointUri(_)),
        "expected CamelError::EndpointUri, got: {err:?}"
    );
    assert!(
        err.to_string().contains("brokers"),
        "unexpected error: {err}"
    );
}

#[test]
fn builder_parameters_apply_in_build_canonical() {
    let spec = RouteBuilder::from("timer:tick")
        .route_id("r-can")
        .parameters(params(&[("showBody", "true")]))
        .to("log:out")
        .parameters(params(&[("showBody", "true")]))
        .build_canonical()
        .unwrap();

    assert_eq!(spec.from, "timer:tick?showBody=true");
    assert_eq!(spec.steps.len(), 1);
    assert!(matches!(
        spec.steps[0],
        CanonicalStepSpec::To { ref uri } if uri == "log:out?showBody=true"
    ));
}

#[test]
fn builder_empty_parameters_preserve_uri_bytes() {
    let plain = RouteBuilder::from("timer:tick")
        .route_id("r-plain")
        .to("log:out")
        .build()
        .unwrap();
    let with_empty = RouteBuilder::from("timer:tick")
        .route_id("r-empty")
        .parameters(BTreeMap::new())
        .to("log:out")
        .parameters(BTreeMap::new())
        .build()
        .unwrap();

    assert_eq!(plain.from_uri(), with_empty.from_uri());
    assert_eq!(plain.steps().len(), with_empty.steps().len());
    assert_eq!(
        endpoint_uri(&plain.steps()[0]),
        endpoint_uri(&with_empty.steps()[0])
    );
    assert_eq!(with_empty.from_uri(), "timer:tick");
    assert_eq!(endpoint_uri(&with_empty.steps()[0]), "log:out");
}
