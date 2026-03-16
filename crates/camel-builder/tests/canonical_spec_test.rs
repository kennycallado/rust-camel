use camel_api::CircuitBreakerConfig;
use camel_api::runtime::CanonicalStepSpec;
use camel_builder::RouteBuilder;
use camel_builder::StepAccumulator;
use camel_processor::LogLevel;

#[test]
fn builder_emits_canonical_route_spec() {
    let spec = RouteBuilder::from("direct:start")
        .route_id("r1")
        .to("mock:out")
        .log("hello", LogLevel::Info)
        .stop()
        .build_canonical()
        .unwrap();

    assert_eq!(spec.route_id, "r1");
    assert_eq!(spec.from, "direct:start");
    assert_eq!(spec.version, 1);
    assert_eq!(spec.steps.len(), 3);
    assert!(matches!(
        spec.steps[0],
        CanonicalStepSpec::To { ref uri } if uri == "mock:out"
    ));
    assert!(matches!(
        spec.steps[1],
        CanonicalStepSpec::Log { ref message } if message == "hello"
    ));
    assert!(matches!(spec.steps[2], CanonicalStepSpec::Stop));
}

#[test]
fn builder_canonical_rejects_unsupported_steps() {
    let err = RouteBuilder::from("direct:start")
        .route_id("r2")
        .set_header("k", "v")
        .build_canonical()
        .unwrap_err()
        .to_string();

    assert!(
        err.contains("canonical v1 does not support step `processor`"),
        "unexpected error: {err}"
    );
}

#[test]
fn builder_canonical_supports_wiretap_and_route_circuit_breaker() {
    let spec = RouteBuilder::from("direct:start")
        .route_id("r3")
        .wire_tap("mock:tap")
        .circuit_breaker(
            CircuitBreakerConfig::new()
                .failure_threshold(3)
                .open_duration(std::time::Duration::from_millis(250)),
        )
        .build_canonical()
        .unwrap();

    assert_eq!(spec.steps.len(), 1);
    assert!(matches!(
        spec.steps[0],
        CanonicalStepSpec::WireTap { ref uri } if uri == "mock:tap"
    ));
    let cb = spec.circuit_breaker.expect("circuit breaker expected");
    assert_eq!(cb.failure_threshold, 3);
    assert_eq!(cb.open_duration_ms, 250);
}

#[test]
fn builder_canonical_rejects_closure_filter_with_explicit_subset_reason() {
    let err = RouteBuilder::from("direct:start")
        .route_id("r4")
        .filter(|_| true)
        .to("mock:out")
        .end_filter()
        .build_canonical()
        .unwrap_err()
        .to_string();

    assert!(
        err.contains("canonical v1 does not support step `filter`"),
        "unexpected error: {err}"
    );
    assert!(
        err.contains("declarative"),
        "expected explicit declarative-subset reason, got: {err}"
    );
}
