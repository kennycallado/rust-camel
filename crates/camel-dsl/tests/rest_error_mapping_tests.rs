//! Route-level HTTP error mapping — DSL-side pins (business-error-model
//! task 2.2).
//!
//! Covers:
//!
//! - rest-block `error_mapping` table parsing (`{codes, kinds}` mirror type);
//! - rest lowering ALWAYS setting the route-level `http_error_mapping` seam
//!   field (empty tables when the block declares none) while keeping
//!   `error_handler: None` exactly as before;
//! - the canonical/hot-reload path never dropping the mapping silently
//!   (rejected when `!allow_loss`, recorded in `CanonicalLossReport`
//!   otherwise);
//! - load-time validation (blank keys, 400-599-only values) with the typed
//!   camel-api error text;
//! - the `#[serde(skip_deserializing)]` fence: neither the `error_handler:`
//!   block nor a plain route document accepts an `http_error_mapping` key.

use camel_api::{CanonicalLossReport, HttpErrorMapping};
use camel_dsl::yaml::extract_rest_blocks;
use camel_dsl::{
    DeclarativeRoute, DeclarativeStep, compile_declarative_route,
    compile_declarative_route_to_canonical, parse_yaml_to_declarative,
};

/// Minimal loadable rest block: one GET operation whose sub-pipeline throws a
/// business error. `mapping` is the YAML fragment for the block-level
/// `error_mapping:` table (already indented for its position, `None` omits it).
fn rest_yaml(mapping: Option<&str>) -> String {
    let mut yaml =
        String::from("rest:\n  - host: 127.0.0.1\n    port: 8081\n    path: \"/orders\"\n");
    if let Some(fragment) = mapping {
        yaml.push_str("    error_mapping:\n");
        yaml.push_str(fragment);
        yaml.push('\n');
    }
    yaml.push_str(
        "    operations:\n      - method: GET\n        path: \"/{id}\"\n        steps:\n          - throw: { code: \"ORDERS_X\", kind: \"not-found\" }\n",
    );
    yaml
}

#[test]
fn rest_block_parses_error_mapping() {
    let yaml = rest_yaml(Some("      kinds: { \"not-found\": 410 }"));
    let blocks = extract_rest_blocks(&yaml).expect("rest block with error_mapping must parse");
    assert_eq!(blocks.len(), 1, "exactly one rest block");
    let table = blocks[0]
        .error_mapping
        .as_ref()
        .expect("error_mapping table must be present on the parsed rest definition");
    assert_eq!(
        table.kinds.get("not-found"),
        Some(&410),
        "explicit kind entry must survive parsing"
    );
    assert!(table.codes.is_empty(), "codes table defaults to empty");
}

#[test]
fn rest_lowering_sets_route_field_keeps_error_handler_none() {
    let yaml = rest_yaml(Some("      kinds: { \"not-found\": 410 }"));
    let routes = parse_yaml_to_declarative(&yaml).expect("rest document must load");
    assert_eq!(routes.len(), 1, "one operation lowers to one route");
    let route = &routes[0];
    let mapping = route
        .http_error_mapping
        .as_ref()
        .expect("lowered route must carry http_error_mapping");
    assert_eq!(
        mapping.kinds.get("not-found"),
        Some(&410),
        "the block table must travel to the lowered route"
    );
    assert!(
        route.error_handler.is_none(),
        "rest lowering keeps error_handler: None exactly as before"
    );

    // The mapping also rides the compiled RouteDefinition (camel-core seam).
    let definition =
        compile_declarative_route(routes[0].clone()).expect("declarative route must compile");
    assert_eq!(
        definition
            .http_error_mapping()
            .map(|m| m.kinds.get("not-found")),
        Some(Some(&410)),
        "compiled RouteDefinition carries the mapping"
    );
}

#[test]
fn rest_without_mapping_sets_empty_tables() {
    let yaml = rest_yaml(None);
    let routes = parse_yaml_to_declarative(&yaml).expect("rest document must load");
    assert_eq!(routes.len(), 1, "one operation lowers to one route");
    let mapping = routes[0]
        .http_error_mapping
        .as_ref()
        .expect("rest lowering always marks the route as a seam route");
    assert!(
        mapping.codes.is_empty() && mapping.kinds.is_empty(),
        "absent block table lowers to Some(empty tables), got: {mapping:?}"
    );
    assert!(
        routes[0].error_handler.is_none(),
        "rest lowering keeps error_handler: None exactly as before"
    );
}

/// Minimal `DeclarativeRoute` with only the mapping varying — everything else
/// stays canonical-path-clean (no error_handler, no security_policy, no UoW).
fn canonical_route(mapping: Option<HttpErrorMapping>) -> DeclarativeRoute {
    DeclarativeRoute {
        from: "direct:canonical-seam".into(),
        route_id: "canonical-seam".into(),
        auto_startup: true,
        startup_order: 1000,
        concurrency: None,
        error_handler: None,
        circuit_breaker: None,
        security_policy: None,
        unit_of_work: None,
        http_error_mapping: mapping,
        steps: vec![DeclarativeStep::Stop],
    }
}

#[test]
fn canonical_path_never_drops_mapping_silently() {
    let mapped = HttpErrorMapping::new().with_kind("not-found", 410);

    // Strict mode: a mapping is unsupported on the canonical/hot-reload path,
    // so compilation is REJECTED — the mapping is never silently absent.
    let rejected =
        compile_declarative_route_to_canonical(canonical_route(Some(mapped.clone())), false);
    assert!(
        rejected.is_err(),
        "canonical compilation must reject http_error_mapping when !allow_loss, \
         got: {rejected:?}"
    );

    // Lossy mode: the drop is recorded in CanonicalLossReport.
    let (_, report) = compile_declarative_route_to_canonical(canonical_route(Some(mapped)), true)
        .expect("canonical compilation with allow_loss must succeed");
    let report: CanonicalLossReport = report.expect("a dropped mapping must produce a loss report");
    assert!(
        report
            .dropped_fields
            .iter()
            .any(|loss| loss.field == "http_error_mapping"),
        "the mapping must be recorded in CanonicalLossReport, got: {report:?}"
    );
}

#[test]
fn rest_mapping_rejects_out_of_domain_status() {
    for status in [200, 101, 304, 700] {
        let yaml = rest_yaml(Some(&format!("      codes: {{ \"X\": {status} }}")));
        let err = parse_yaml_to_declarative(&yaml)
            .expect_err("an out-of-domain mapping value must fail the load");
        assert!(
            err.to_string().contains("outside 400-599"),
            "load must fail with the typed out-of-domain error for {status}, got: {err}"
        );
    }
}

#[test]
fn rest_mapping_rejects_blank_key() {
    let blank_code_key = rest_yaml(Some("      codes: { \"\": 404 }"));
    let err = parse_yaml_to_declarative(&blank_code_key)
        .expect_err("a blank codes key must fail the load");
    assert!(
        err.to_string()
            .contains("blank `codes` key in HTTP error mapping"),
        "load must fail naming the blank code key's table, got: {err}"
    );

    let blank_kind_key = rest_yaml(Some("      kinds: { \"   \": 404 }"));
    let err = parse_yaml_to_declarative(&blank_kind_key)
        .expect_err("a blank kinds key must fail the load");
    assert!(
        err.to_string()
            .contains("blank `kinds` key in HTTP error mapping"),
        "load must fail naming the blank kind key's table, got: {err}"
    );
}

#[test]
fn route_error_handler_block_and_route_field_reject_mapping() {
    // The error_handler block has no http_error_mapping field — the mapping is
    // deliberately NOT an error-handler concern (owner decision Q1).
    let in_handler = r#"
routes:
  - id: "fence-handler"
    from: "direct:fence"
    error_handler:
      http_error_mapping: { kinds: { "not-found": 404 } }
    steps: []
"#;
    let err = parse_yaml_to_declarative(in_handler)
        .expect_err("error_handler.http_error_mapping must be rejected as an unknown field");
    let message = err.to_string();
    assert!(
        message.contains("unknown field"),
        "the serde fence must reject the key as unknown, got: {message}"
    );
    assert!(
        message.contains("http_error_mapping"),
        "the rejection must name the offending key, got: {message}"
    );

    // Plain-route YAML authoring is deferred to the phase-3 errors catalog
    // (owner decision Q5): the route document rejects the top-level key too.
    let on_route = r#"
routes:
  - id: "fence-route"
    from: "direct:fence"
    http_error_mapping: { kinds: { "not-found": 404 } }
    steps: []
"#;
    let err = parse_yaml_to_declarative(on_route)
        .expect_err("a route document must not accept a top-level http_error_mapping key");
    let message = err.to_string();
    assert!(
        message.contains("unknown field") && message.contains("http_error_mapping"),
        "the serde-skip fence must reject the route-level key as unknown, got: {message}"
    );
}
