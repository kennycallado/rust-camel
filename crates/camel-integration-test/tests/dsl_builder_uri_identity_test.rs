//! Cross-surface byte-identity (rc-l4wf): identical base URI + parameters fed
//! through the camel-dsl YAML lowering and the camel-builder `.parameters()`
//! surface must produce byte-identical canonical endpoint URIs.
//!
//! Both surfaces route through
//! `camel_api::EndpointUri::try_from_uri_and_params` and
//! `to_canonical_string` today, so the assertions compare the two surfaces
//! directly against each other (no golden literals).
//!
//! If a future inlined merge in either surface diverges — parameter ordering,
//! escaping of reserved characters, raw-query handling, or the empty-map
//! passthrough — the comparison fails.

use std::collections::BTreeMap;

use camel_api::runtime::{CanonicalRouteSpec, CanonicalStepSpec};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_dsl::parse_yaml_to_canonical;

fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Render a `parameters:` YAML block at the given indentation, or `""` when the
/// map is empty, so the document omits the key entirely (mirroring the
/// empty-map passthrough each surface applies).
fn yaml_params_block(indent: usize, pairs: &[(&str, &str)]) -> String {
    if pairs.is_empty() {
        return String::new();
    }
    let pad = " ".repeat(indent);
    let mut out = format!("{pad}parameters:\n");
    // Sorted insertion mirrors the BTreeMap the lowering deserializes into;
    // YAML mapping order does not affect the merge but keeps the generated
    // document deterministic.
    let sorted: BTreeMap<&str, &str> = pairs.iter().copied().collect();
    for (key, value) in sorted {
        // Values are quoted so every scalar deserializes as a string
        // (`deserialize_string_parameters` rejects non-string scalars).
        out.push_str(&format!("{pad}  {key}: \"{value}\"\n"));
    }
    out
}

/// Drive the camel-dsl surface: the same base URI + parameters authored as a
/// YAML route document, lowered through the shared YAML lowering
/// (`merge_endpoint_uri` → `EndpointUri`) into a canonical route spec.
fn dsl_canonical(
    from: &str,
    from_params: &[(&str, &str)],
    to: &str,
    to_params: &[(&str, &str)],
) -> CanonicalRouteSpec {
    let yaml = format!(
        "routes:\n  - id: identity\n    from: {from}\n{}    steps:\n      - to: {to}\n{}",
        yaml_params_block(4, from_params),
        yaml_params_block(8, to_params),
    );
    parse_yaml_to_canonical(&yaml, false)
        .expect("DSL YAML lowering should succeed")
        .into_iter()
        .next()
        .expect("one route document yields one spec")
        .0
}

/// Drive the camel-builder surface: the same base URI + parameters through
/// `RouteBuilder`'s pending-slot `.parameters()` merge
/// (`apply_parameter_assignments` → `EndpointUri`) into the same canonical
/// route spec type.
fn builder_canonical(
    from: &str,
    from_params: &[(&str, &str)],
    to: &str,
    to_params: &[(&str, &str)],
) -> CanonicalRouteSpec {
    let mut builder = RouteBuilder::from(from).route_id("identity");
    // `.parameters()` attaches to the most recently added endpoint slot, so
    // the `from` map goes before the step and the `to` map after it.
    if !from_params.is_empty() {
        builder = builder.parameters(params(from_params));
    }
    builder = builder.to(to);
    if !to_params.is_empty() {
        builder = builder.parameters(params(to_params));
    }
    builder
        .build_canonical()
        .expect("builder canonical build should succeed")
}

/// Extract the URI of a spec whose only step is a `To`.
fn only_to_uri(spec: &CanonicalRouteSpec) -> &str {
    match spec.steps.as_slice() {
        [CanonicalStepSpec::To { uri }] => uri,
        other => panic!("expected exactly one To step, got {other:?}"),
    }
}

/// Feed identical URI + params through both surfaces and compare the canonical
/// endpoint URIs byte-for-byte.
fn assert_surface_identity(
    label: &str,
    from: &str,
    from_params: &[(&str, &str)],
    to: &str,
    to_params: &[(&str, &str)],
) {
    let dsl = dsl_canonical(from, from_params, to, to_params);
    let builder = builder_canonical(from, from_params, to, to_params);

    assert_eq!(
        dsl.route_id, builder.route_id,
        "route_id diverged ({label})"
    );
    assert_eq!(
        dsl.from, builder.from,
        "from-endpoint canonical URI diverged ({label})"
    );
    assert_eq!(
        only_to_uri(&dsl),
        only_to_uri(&builder),
        "to-endpoint canonical URI diverged ({label})"
    );
}

#[test]
fn no_params_passthrough_is_byte_identical() {
    assert_surface_identity("no-params", "timer:tick", &[], "log:out", &[]);
}

#[test]
fn string_param_is_byte_identical() {
    assert_surface_identity(
        "string",
        "direct:start",
        &[],
        "seda:orders",
        &[("name", "alice")],
    );
}

#[test]
fn int_param_is_byte_identical() {
    assert_surface_identity(
        "int",
        "direct:start",
        &[],
        "kafka:orders",
        &[("maxPollRecords", "500")],
    );
}

#[test]
fn bool_param_is_byte_identical() {
    // The bool kind on both slots: the `from` map attaches before the step is
    // added, the `to` map after it.
    assert_surface_identity(
        "bool",
        "timer:tick",
        &[("synchronous", "true")],
        "log:out",
        &[("showBody", "true")],
    );
}

#[test]
fn list_param_is_byte_identical() {
    // EndpointUri params are `BTreeMap<String, String>`; the list convention is
    // a comma-joined value string, identical on both surfaces.
    assert_surface_identity(
        "list",
        "direct:start",
        &[],
        "kafka:orders",
        &[("brokers", "a:9092,b:9092,c:9092")],
    );
}

#[test]
fn enum_like_param_is_byte_identical() {
    assert_surface_identity(
        "enum",
        "direct:start",
        &[],
        "log:audit",
        &[("level", "DEBUG")],
    );
}

#[test]
fn reserved_char_value_is_byte_identical() {
    // Values may contain reserved query characters; both surfaces must render
    // (or escape) them identically.
    assert_surface_identity(
        "reserved",
        "direct:start",
        &[],
        "log:out",
        &[("note", "hello world"), ("q", "a&b=c")],
    );
}

#[test]
fn mixed_param_kinds_are_byte_identical() {
    let from_params = [("period", "1000"), ("synchronous", "true")];
    let to_params = [
        ("items", "a,b,c"),
        ("level", "DEBUG"),
        ("name", "alice"),
        ("showBody", "true"),
    ];

    let dsl = dsl_canonical("timer:tick", &from_params, "log:out", &to_params);
    let builder = builder_canonical("timer:tick", &from_params, "log:out", &to_params);

    // Both surfaces genuinely driven: print the intermediate canonical strings
    // so any divergence is visible in the failure output.
    println!("dsl     from: {}", dsl.from);
    println!("dsl     to:   {}", only_to_uri(&dsl));
    println!("builder from: {}", builder.from);
    println!("builder to:   {}", only_to_uri(&builder));

    assert_eq!(dsl.route_id, builder.route_id);
    assert_eq!(
        dsl.from, builder.from,
        "from-endpoint canonical URI diverged"
    );
    assert_eq!(
        only_to_uri(&dsl),
        only_to_uri(&builder),
        "to-endpoint canonical URI diverged"
    );
}
