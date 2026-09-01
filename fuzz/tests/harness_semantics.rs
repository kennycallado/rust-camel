//! Harness semantics tests for the `dsl_yaml` fuzz target.
//!
//! Exercises the observable contract of [`camel_fuzz::dsl_yaml_harness`]
//! without libFuzzer: a valid document parses, malformed input and hostile
//! YAML shapes (alias bombs, deep nesting) are rejected with `Err` instead
//! of panicking, and invalid UTF-8 is skipped before parsing.

use camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD;
use camel_dsl::SecurityCompileContext;
use camel_fuzz::dsl_yaml_harness;

/// Minimal valid route document, reused from the
/// `sampling_yaml_short_form_compiles` fixture in
/// `crates/camel-dsl/tests/sampling_dsl_tests.rs`.
const MINIMAL_ROUTE_YAML: &str = r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - sampling: 7
"#;

#[test]
fn valid_minimal_route_parses() {
    let routes = camel_dsl::yaml::parse_yaml(MINIMAL_ROUTE_YAML)
        .expect("minimal sampling route should parse");
    assert_eq!(routes.len(), 1, "fixture defines exactly one route");

    dsl_yaml_harness(MINIMAL_ROUTE_YAML.as_bytes());
}

#[test]
fn malformed_input_no_panic() {
    let truncated = "routes:\n  - id: r1\n    from:\n";
    let wrong_field_type = "just a plain string, not a mapping";
    let garbage_prefix = "\u{1}\u{2} garbage {{{ >>> not yaml at all";

    // The harness discards the internal `Err`; the contract under test
    // is that rejection never panics.

    // Truncated YAML: `from:` with no value.
    dsl_yaml_harness(truncated.as_bytes());

    // Wrong field type: string where a mapping is required.
    dsl_yaml_harness(wrong_field_type.as_bytes());

    // Random garbage prefix.
    dsl_yaml_harness(garbage_prefix.as_bytes());
}

#[test]
fn invalid_utf8_skipped() {
    dsl_yaml_harness(&[0xff, 0xfe, 0x00, 0x80]);
}

#[test]
fn alias_bomb_no_panic() {
    // 200 anchored sequence nodes followed by 2,000 alias references spread
    // over them (10 per anchor). The geometry is load-bearing: with 200
    // anchors the `alias_anchor_ratio = 10.0` heuristic does not trip
    // (2,000 <= 10 x 200), so the run deterministically hits
    // `max_alias_expansions = 1024` instead.
    //
    // The anchors and aliases ride in schema-valid positions
    // (`security_policy.roles`, a `Vec<String>`): deserialization drives
    // parsing lazily, so an unknown top-level key would abort with a schema
    // error long before the 1025th alias event is consumed. Anchors are
    // declared first (YAML forbids forward references); the budget error
    // then surfaces at the 1025th alias regardless of the valid schema.
    let mut yaml = String::from("routes:\n");
    for i in 0..200 {
        yaml.push_str(&format!(
            "  - id: anchor{i}\n    from: direct:start\n    security_policy:\n      roles: &a{i} [user]\n"
        ));
    }
    for i in 0..2000 {
        yaml.push_str(&format!(
            "  - id: ref{i}\n    from: direct:start\n    security_policy:\n      roles: *a{}\n",
            i % 200
        ));
    }

    dsl_yaml_harness(yaml.as_bytes());

    // expect_err is unavailable here: Vec<RouteDefinition> is not Debug.
    let Err(err) = camel_dsl::yaml::parse_yaml_with_threshold_and_security(
        &yaml,
        DEFAULT_STREAM_CACHE_THRESHOLD,
        SecurityCompileContext::default(),
    ) else {
        panic!("alias bomb document must be rejected");
    };
    let msg = err.to_string();
    assert!(
        msg.contains("alias expansion limit exceeded"),
        "expected the alias expansion budget to trip, got: {msg}"
    );
}

#[test]
fn deep_nesting_no_panic() {
    let deep = format!("{}null", "- ".repeat(1000));

    dsl_yaml_harness(deep.as_bytes());

    assert!(
        camel_dsl::yaml::parse_yaml_with_threshold_and_security(
            &deep,
            DEFAULT_STREAM_CACHE_THRESHOLD,
            SecurityCompileContext::default(),
        )
        .is_err(),
        "deeply nested block sequence must be rejected"
    );
}
