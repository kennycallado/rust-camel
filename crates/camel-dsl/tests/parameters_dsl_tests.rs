//! Tests for the `parameters` surface on endpoint-bearing DSL structs.
//!
//! Task 2.1 (AST) — asserts the authoring AST (`route_ast`) accepts a
//! `parameters: BTreeMap<String, String>` on `to`, `from`, `wire_tap`,
//! `enrich`, and `poll_enrich` surfaces, deserializing the raw AST
//! (`RouteDslRoutes`), not the lowered model.
//!
//! Task 2.2 (lowering) — asserts the shared YAML/JSON lowering in `yaml.rs`
//! merges the raw pair via `EndpointUri` into the canonical `uri` on the
//! lowered `DeclarativeRoute`/`DeclarativeStep` model.

use camel_dsl::route_ast::{EnrichBody, RouteDslRoutes, RouteDslStep};
use camel_dsl::{DeclarativeStep, parse_json_to_declarative, parse_yaml_to_declarative};
use noyalib::compat::serde_yaml as serde_yml;

fn parse_yaml_ast(yaml: &str) -> RouteDslRoutes {
    serde_yml::from_str(yaml).expect("YAML should deserialize into the DSL AST")
}

#[test]
fn ast_accepts_parameters_on_to() {
    let dsl = parse_yaml_ast(
        r#"
routes:
  - id: test
    from: timer:tick
    steps:
      - to: kafka:orders
        parameters:
          brokers: my-host:9092
"#,
    );
    let step = &dsl.routes[0].steps[0];
    match step {
        RouteDslStep::To(to) => {
            assert_eq!(to.to, "kafka:orders");
            assert_eq!(to.parameters["brokers"], "my-host:9092");
        }
        other => panic!("expected To step, got {other:?}"),
    }
}

#[test]
fn ast_accepts_parameters_on_from() {
    let dsl = parse_yaml_ast(
        r#"
routes:
  - id: test
    from: timer:tick
    parameters:
      period: "2500"
"#,
    );
    let route = &dsl.routes[0];
    assert_eq!(route.from, "timer:tick");
    assert_eq!(route.parameters["period"], "2500");
}

#[test]
fn ast_accepts_parameters_on_wire_tap_and_enrich_full() {
    let dsl = parse_yaml_ast(
        r#"
routes:
  - id: test
    from: timer:tick
    steps:
      - wire_tap: log:audit
        parameters:
          showBody: "true"
      - enrich:
          uri: db:query
          parameters:
            dataSource: customers
"#,
    );
    let steps = &dsl.routes[0].steps;
    match &steps[0] {
        RouteDslStep::WireTap(wt) => {
            assert_eq!(wt.wire_tap, "log:audit");
            assert_eq!(wt.parameters["showBody"], "true");
        }
        other => panic!("expected WireTap step, got {other:?}"),
    }
    match &steps[1] {
        RouteDslStep::Enrich(enrich) => match &enrich.enrich {
            EnrichBody::Full(config) => {
                assert_eq!(config.uri, "db:query");
                assert_eq!(config.parameters["dataSource"], "customers");
            }
            other => panic!("expected full-form enrich, got {other:?}"),
        },
        other => panic!("expected Enrich step, got {other:?}"),
    }
}

#[test]
fn ast_accepts_parameters_on_enrich_shorthand_and_poll_enrich() {
    let dsl = parse_yaml_ast(
        r#"
routes:
  - id: test
    from: timer:tick
    steps:
      - enrich: db:query
        parameters:
          dataSource: customers
      - poll_enrich: file:inbox
        parameters:
          delay: "500"
      - poll_enrich:
          uri: file:inbox
          parameters:
            delay: "500"
"#,
    );
    let steps = &dsl.routes[0].steps;

    match &steps[0] {
        RouteDslStep::Enrich(enrich) => {
            match &enrich.enrich {
                EnrichBody::Uri(uri) => assert_eq!(uri, "db:query"),
                other => panic!("expected shorthand enrich, got {other:?}"),
            }
            assert_eq!(enrich.parameters["dataSource"], "customers");
        }
        other => panic!("expected Enrich step, got {other:?}"),
    }

    match &steps[1] {
        RouteDslStep::PollEnrich(poll) => {
            match &poll.poll_enrich {
                EnrichBody::Uri(uri) => assert_eq!(uri, "file:inbox"),
                other => panic!("expected shorthand poll_enrich, got {other:?}"),
            }
            assert_eq!(poll.parameters["delay"], "500");
        }
        other => panic!("expected PollEnrich step, got {other:?}"),
    }

    match &steps[2] {
        RouteDslStep::PollEnrich(poll) => match &poll.poll_enrich {
            EnrichBody::Full(config) => {
                assert_eq!(config.uri, "file:inbox");
                assert_eq!(config.parameters["delay"], "500");
            }
            other => panic!("expected full-form poll_enrich, got {other:?}"),
        },
        other => panic!("expected PollEnrich step, got {other:?}"),
    }
}

#[test]
fn non_string_parameter_value_rejected() {
    // A non-string parameter value must be rejected with an error that names
    // the offending key (spec: "deserialization fails with an error naming
    // the offending key `retries`"). The route-level surface
    // (`RouteDslRoute.parameters`) is a plain struct field, so the custom
    // deserializer's key-naming error surfaces verbatim.
    let yaml = r#"
routes:
  - id: test
    from: timer:tick
    parameters:
      retries: 3
"#;
    let result: Result<RouteDslRoutes, _> = serde_yml::from_str(yaml);
    let err = match result {
        Ok(_) => panic!("non-string parameter value should be rejected"),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("retries"),
        "error should name the offending key, got: {err}"
    );

    // JSON variant: same rejection, same key-naming requirement.
    let json = r#"{"routes":[{"id":"test","from":"timer:tick","parameters":{"retries":3}}]}"#;
    let result: Result<RouteDslRoutes, _> = serde_json::from_str(json);
    let err = match result {
        Ok(_) => panic!("JSON non-string parameter value should be rejected"),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("retries"),
        "JSON error should name the offending key, got: {err}"
    );

    // Step-level surfaces (to/wire_tap/enrich/poll_enrich) route through the
    // `#[serde(untagged)]` RouteDslStep enum. serde 1.0.229's derive discards
    // every variant-attempt error and reports only "data did not match any
    // variant of untagged enum RouteDslStep", so the key name is not
    // recoverable there without a manual error-preserving Deserialize for
    // RouteDslStep (serde's own TODO in enum_untagged.rs). The rejection
    // itself still holds.
    let yaml_step = r#"
routes:
  - id: test
    from: timer:tick
    steps:
      - to: kafka:orders
        parameters:
          retries: 3
"#;
    let result: Result<RouteDslRoutes, _> = serde_yml::from_str(yaml_step);
    assert!(
        result.is_err(),
        "non-string parameter value should be rejected on step surfaces"
    );
}

#[test]
fn json_variant_accepts_parameters() {
    let json = r#"{"routes":[{"id":"test","from":"timer:tick","steps":[{"to":"kafka:orders","parameters":{"brokers":"x"}}]}]}"#;
    let dsl: RouteDslRoutes = serde_json::from_str(json).expect("JSON should deserialize");
    match &dsl.routes[0].steps[0] {
        RouteDslStep::To(to) => {
            assert_eq!(to.to, "kafka:orders");
            assert_eq!(to.parameters["brokers"], "x");
        }
        other => panic!("expected To step, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Task 2.2 — lowering tests: the shared YAML/JSON lowering merges the raw
// `uri` + `parameters` pair into a canonical merged URI on the model.
// ---------------------------------------------------------------------------

#[test]
fn from_parameters_merge_to_canonical() {
    let routes = parse_yaml_to_declarative(
        r#"
routes:
  - id: test
    from: timer:tick
    parameters:
      period: "1000"
"#,
    )
    .expect("route should lower");
    assert_eq!(routes[0].from, "timer:tick?period=1000");
}

#[test]
fn to_parameters_merge_to_canonical() {
    let routes = parse_yaml_to_declarative(
        r#"
routes:
  - id: test
    from: timer:tick
    steps:
      - to: kafka:orders
        parameters:
          brokers: my-host:9092
"#,
    )
    .expect("route should lower");
    match &routes[0].steps[0] {
        DeclarativeStep::To(to) => assert_eq!(to.uri, "kafka:orders?brokers=my-host:9092"),
        other => panic!("expected To step, got {other:?}"),
    }
}

#[test]
fn query_string_and_parameters_equivalent() {
    let query_string_form = parse_yaml_to_declarative(
        r#"
routes:
  - id: test
    from: timer:tick
    steps:
      - to: log:out?showBody=true
"#,
    )
    .expect("query-string route should lower");
    let params_form = parse_yaml_to_declarative(
        r#"
routes:
  - id: test
    from: timer:tick
    steps:
      - to: log:out
        parameters:
          showBody: "true"
"#,
    )
    .expect("parameters route should lower");

    let qs_uri = match &query_string_form[0].steps[0] {
        DeclarativeStep::To(to) => to.uri.clone(),
        other => panic!("expected To step, got {other:?}"),
    };
    let params_uri = match &params_form[0].steps[0] {
        DeclarativeStep::To(to) => to.uri.clone(),
        other => panic!("expected To step, got {other:?}"),
    };
    assert_eq!(qs_uri, "log:out?showBody=true");
    assert_eq!(qs_uri, params_uri);
}

#[test]
fn wire_tap_and_enrich_and_poll_enrich_merge() {
    let routes = parse_yaml_to_declarative(
        r#"
routes:
  - id: test
    from: timer:tick
    steps:
      - wire_tap: log:audit
        parameters:
          showBody: "true"
      - enrich: db:query
        parameters:
          dataSource: customers
      - enrich:
          uri: db:query
          parameters:
            dataSource: customers
      - poll_enrich: file:inbox
        parameters:
          delay: "500"
"#,
    )
    .expect("route should lower");
    let steps = &routes[0].steps;

    match &steps[0] {
        DeclarativeStep::WireTap(wt) => assert_eq!(wt.uri, "log:audit?showBody=true"),
        other => panic!("expected WireTap step, got {other:?}"),
    }

    // Enrich shorthand and full form must both lower to the same canonical URI.
    let shorthand_uri = match &steps[1] {
        DeclarativeStep::Enrich(e) => e.uri.clone(),
        other => panic!("expected Enrich step, got {other:?}"),
    };
    let full_uri = match &steps[2] {
        DeclarativeStep::Enrich(e) => e.uri.clone(),
        other => panic!("expected Enrich step, got {other:?}"),
    };
    assert_eq!(shorthand_uri, "db:query?dataSource=customers");
    assert_eq!(full_uri, "db:query?dataSource=customers");
    assert_eq!(shorthand_uri, full_uri);

    match &steps[3] {
        DeclarativeStep::PollEnrich(p) => assert_eq!(p.uri, "file:inbox?delay=500"),
        other => panic!("expected PollEnrich step, got {other:?}"),
    }
}

#[test]
fn duplicate_key_is_lowering_error_yaml_and_json() {
    let yaml = r#"
routes:
  - id: test
    from: timer:tick
    steps:
      - to: kafka:orders?brokers=a
        parameters:
          brokers: b
"#;
    let yaml_err =
        parse_yaml_to_declarative(yaml).expect_err("duplicate key must be a lowering error");
    assert!(
        yaml_err.to_string().contains("brokers"),
        "YAML error should name the offending key, got: {yaml_err}"
    );

    let json = r#"{"routes":[{"id":"test","from":"timer:tick","steps":[{"to":"kafka:orders?brokers=a","parameters":{"brokers":"b"}}]}]}"#;
    let json_err =
        parse_json_to_declarative(json).expect_err("duplicate key must be a lowering error");
    assert!(
        json_err.to_string().contains("brokers"),
        "JSON error should name the offending key, got: {json_err}"
    );
}

#[test]
fn empty_parameters_preserve_uri_bytes() {
    let raw = "timer:tick?period=1000&repeatCount=6";

    // Explicit empty map.
    let explicit = parse_yaml_to_declarative(&format!(
        "routes:\n  - id: test\n    from: timer:tick\n    steps:\n      - to: {raw}\n        parameters: {{}}\n"
    ))
    .expect("route should lower");
    match &explicit[0].steps[0] {
        DeclarativeStep::To(to) => assert_eq!(to.uri, raw),
        other => panic!("expected To step, got {other:?}"),
    }

    // Absent parameters map.
    let absent = parse_yaml_to_declarative(&format!(
        "routes:\n  - id: test\n    from: timer:tick\n    steps:\n      - to: {raw}\n"
    ))
    .expect("route should lower");
    match &absent[0].steps[0] {
        DeclarativeStep::To(to) => assert_eq!(to.uri, raw),
        other => panic!("expected To step, got {other:?}"),
    }
}

#[test]
fn enrich_config_and_step_parameters_overlap_is_error() {
    // (a) Both-set with an overlapping key: lowering fails closed, naming the
    // colliding key. The full-form `config.parameters` and the step-level
    // `parameters` map declare the same key.
    let overlap = parse_yaml_to_declarative(
        r#"
routes:
  - id: test
    from: timer:tick
    steps:
      - enrich:
          uri: db:query
          parameters:
            dataSource: customers
        parameters:
          dataSource: other
"#,
    )
    .expect_err("duplicate key across full-form and step-level parameters must fail closed");
    assert!(
        overlap.to_string().contains("dataSource"),
        "error should name the colliding key, got: {overlap}"
    );

    // poll_enrich: the same both-set overlap must fail closed too.
    let poll_overlap = parse_yaml_to_declarative(
        r#"
routes:
  - id: test
    from: timer:tick
    steps:
      - poll_enrich:
          uri: file:inbox
          parameters:
            delay: "500"
        parameters:
          delay: "1000"
"#,
    )
    .expect_err("poll_enrich duplicate key must fail closed");
    assert!(
        poll_overlap.to_string().contains("delay"),
        "error should name the colliding key, got: {poll_overlap}"
    );

    // (b) Both-set with disjoint keys: merged canonical URI contains the
    // parameters from both maps.
    let merged = parse_yaml_to_declarative(
        r#"
routes:
  - id: test
    from: timer:tick
    steps:
      - enrich:
          uri: db:query
          parameters:
            dataSource: customers
        parameters:
          timeout: "5000"
"#,
    )
    .expect("disjoint parameters should merge");
    match &merged[0].steps[0] {
        DeclarativeStep::Enrich(e) => {
            assert_eq!(e.uri, "db:query?dataSource=customers&timeout=5000");
        }
        other => panic!("expected Enrich step, got {other:?}"),
    }
}
