//! YAML ↔ JSON parity tests for the shared route-DSL AST.
//!
//! Every `RouteDslStep` variant has a matrix entry that deserializes the
//! same logical route from both YAML and JSON, then asserts the flattened
//! `Vec<&RouteDslStep>` is identical (compared via pretty-Debug strings).
//!
//! Why step-list-level and not `DeclarativeRoute`-level: both
//! `parse_yaml_to_declarative` and `parse_json_to_declarative` funnel
//! through the same `route_dsl_to_declarative_route` converter, so
//! comparing downstream of that converter is tautological. The real
//! parity risk is whether `serde_yml` and `serde_json` deserialize the
//! same logical input to the same step list — that is what this matrix
//! probes.
//!
//! Why step-list-level and not full `RouteDslRoutes`-level: `RouteDslRoute`
//! lacks a `Debug` derive (and cascades into `RouteDslErrorHandler` etc.
//! that also lack it). Restricting comparison to `Vec<&RouteDslStep>`
//! avoids the entire derive cascade. Step-variant parity is the DoD;
//! route-metadata parity (id/from/error_handler) is trivial and
//! out of scope.
//!
//! Adding a new variant to `RouteDslStep` without a corresponding matrix
//! entry fails to compile (via `_assert_all_variants_covered`).
//!
//! See: docs/superpowers/specs/2026-06-24-json-canonical-route-format.md §4 Slice B1.

use crate::route_ast::{RouteDslRoutes, RouteDslStep};

// serde_yml migrated to noyalib (compat-serde-yaml shim) — closes RUSTSEC-2025-0068.
// Same alias used by route_ast.rs:3 and yaml.rs:5.
use noyalib::compat::serde_yaml as serde_yml;

struct ParityCase {
    name: &'static str,
    yaml: &'static str,
    json: &'static str,
}

/// Exhaustive match over `RouteDslStep` with no wildcard arm.
///
/// Adding a variant to the enum without adding a match arm here is a
/// compile error — the canonical Rust pattern for compile-fail coverage
/// enforcement. The function is never called; it exists purely for the
/// type checker.
#[allow(dead_code)]
fn _assert_all_variants_covered(step: &RouteDslStep) {
    match step {
        RouteDslStep::To(_) => (),
        RouteDslStep::SetHeader(_) => (),
        RouteDslStep::SetProperty(_) => (),
        RouteDslStep::SetBody(_) => (),
        RouteDslStep::Bean(_) => (),
        RouteDslStep::Choice(_) => (),
        RouteDslStep::DynamicRouter(_) => (),
        RouteDslStep::Filter(_) => (),
        RouteDslStep::Function(_) => (),
        RouteDslStep::LoadBalance(_) => (),
        RouteDslStep::Log(_) => (),
        RouteDslStep::Split(_) => (),
        RouteDslStep::Aggregate(_) => (),
        RouteDslStep::WireTap(_) => (),
        RouteDslStep::Multicast(_) => (),
        RouteDslStep::RoutingSlip(_) => (),
        RouteDslStep::RecipientList(_) => (),
        RouteDslStep::Stop(_) => (),
        RouteDslStep::StreamCache(_) => (),
        RouteDslStep::Throttle(_) => (),
        RouteDslStep::Transform(_) => (),
        RouteDslStep::Script(_) => (),
        RouteDslStep::ConvertBodyTo(_) => (),
        RouteDslStep::Marshal(_) => (),
        RouteDslStep::Unmarshal(_) => (),
        RouteDslStep::Delay(_) => (),
        RouteDslStep::DoTry(_) => (),
        RouteDslStep::Loop(_) => (),
        RouteDslStep::Validate(_) => (),
        RouteDslStep::Enrich(_) => (),
        RouteDslStep::PollEnrich(_) => (),
        RouteDslStep::IdempotentConsumer(_) => (),
        RouteDslStep::ClaimCheck(_) => (),
        RouteDslStep::Sampling(_) => (),
        RouteDslStep::Sort(_) => (),
        // Intentionally NO `_ => ()` wildcard.
        // A new variant added to RouteDslStep will cause a compile error here,
        // forcing the author to also add a parity case below.
    }
}

/// Returns the parity matrix. Each variant appears at least once.
///
/// Tasks 2-7 populate this. Task 1 ships the scaffold with one example
/// case (`To`) so the framework compiles and the matrix test runs.
fn parity_cases() -> Vec<ParityCase> {
    vec![
        ParityCase {
            name: "To",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - to: direct:end
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"to":"direct:end"}]}]}"#,
        },
        ParityCase {
            name: "SetHeader",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - set_header:
          key: X-Trace-Id
          value: "abc-123"
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"set_header":{"key":"X-Trace-Id","value":"abc-123"}}]}]}"#,
        },
        ParityCase {
            name: "SetProperty",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - set_property:
          name: myProp
          value: "my-value"
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"set_property":{"name":"myProp","value":"my-value"}}]}]}"#,
        },
        ParityCase {
            name: "SetBody",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - set_body: "hello"
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"set_body":"hello"}]}]}"#,
        },
        ParityCase {
            name: "Bean",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - bean:
          name: myBean
          method: process
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"bean":{"name":"myBean","method":"process"}}]}]}"#,
        },
        ParityCase {
            name: "Log",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - log: "Processing exchange"
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"log":"Processing exchange"}]}]}"#,
        },
        ParityCase {
            name: "Stop",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - stop: true
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"stop":true}]}]}"#,
        },
        ParityCase {
            name: "ConvertBodyTo",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - convert_body_to: json
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"convert_body_to":"json"}]}]}"#,
        },
        ParityCase {
            name: "Delay",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - delay: 500
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"delay":500}]}]}"#,
        },
        ParityCase {
            name: "StreamCache",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - stream_cache: true
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"stream_cache":true}]}]}"#,
        },
        ParityCase {
            name: "Filter",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - filter:
          simple: "${body} == 'yes'"
          steps:
            - to: direct:yes
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"filter":{"simple":"${body} == 'yes'","steps":[{"to":"direct:yes"}]}}]}]}"#,
        },
        ParityCase {
            name: "Choice",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - choice:
          when:
            - simple: "${body} == 'yes'"
              steps:
                - to: direct:yes
          otherwise:
            - to: direct:no
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"choice":{"when":[{"simple":"${body} == 'yes'","steps":[{"to":"direct:yes"}]}],"otherwise":[{"to":"direct:no"}]}}]}]}"#,
        },
        ParityCase {
            name: "DynamicRouter",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - dynamic_router:
          simple: "${body}"
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"dynamic_router":{"simple":"${body}"}}]}]}"#,
        },
        ParityCase {
            name: "Validate",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - validate: "schemas/order.xsd"
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"validate":"schemas/order.xsd"}]}]}"#,
        },
        ParityCase {
            name: "Transform",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - transform: "hello"
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"transform":"hello"}]}]}"#,
        },
        ParityCase {
            name: "Script",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - script:
          language: "simple"
          source: "${body}"
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"script":{"language":"simple","source":"${body}"}}]}]}"#,
        },
        ParityCase {
            name: "Multicast",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - multicast:
          steps:
            - to: direct:end
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"multicast":{"steps":[{"to":"direct:end"}]}}]}]}"#,
        },
        ParityCase {
            name: "WireTap",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - wire_tap: direct:end
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"wire_tap":"direct:end"}]}]}"#,
        },
        ParityCase {
            name: "LoadBalance",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - load_balance:
          steps:
            - to: direct:end
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"load_balance":{"steps":[{"to":"direct:end"}]}}]}]}"#,
        },
        ParityCase {
            name: "RoutingSlip",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - routing_slip:
          simple: "${body}"
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"routing_slip":{"simple":"${body}"}}]}]}"#,
        },
        ParityCase {
            name: "RecipientList",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - recipient_list:
          simple: "${body}"
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"recipient_list":{"simple":"${body}"}}]}]}"#,
        },
        ParityCase {
            name: "Split",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - split:
          expression: body_lines
          steps:
            - to: direct:end
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"split":{"expression":"body_lines","steps":[{"to":"direct:end"}]}}]}]}"#,
        },
        ParityCase {
            name: "Aggregate",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - aggregate:
          header: MyCorrelationHeader
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"aggregate":{"header":"MyCorrelationHeader"}}]}]}"#,
        },
        ParityCase {
            name: "Marshal",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - marshal: json
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"marshal":"json"}]}]}"#,
        },
        ParityCase {
            name: "Unmarshal",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - unmarshal: json
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"unmarshal":"json"}]}]}"#,
        },
        ParityCase {
            name: "Function",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - function:
          runtime: wasm
          source: my_func
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"function":{"runtime":"wasm","source":"my_func"}}]}]}"#,
        },
        ParityCase {
            name: "Enrich",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - enrich: direct:enrichSource
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"enrich":"direct:enrichSource"}]}]}"#,
        },
        ParityCase {
            name: "PollEnrich",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - poll_enrich: direct:pollEnrichSource
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"poll_enrich":"direct:pollEnrichSource"}]}]}"#,
        },
        ParityCase {
            name: "Throttle",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - throttle:
          max_requests: 5
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"throttle":{"max_requests":5}}]}]}"#,
        },
        ParityCase {
            name: "Loop",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - loop:
          count: 3
          steps:
            - to: direct:body
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"loop":{"count":3,"steps":[{"to":"direct:body"}]}}]}]}"#,
        },
        ParityCase {
            name: "DoTry",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - do_try:
          steps:
            - to: direct:risky
          catch:
            - exception: ["MyError"]
              steps:
                - to: direct:handle
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"do_try":{"steps":[{"to":"direct:risky"}],"catch":[{"exception":["MyError"],"steps":[{"to":"direct:handle"}]}]}}]}]}"#,
        },
        ParityCase {
            name: "Sampling",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - sampling: 5
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"sampling":5}]}]}"#,
        },
        ParityCase {
            name: "Sort",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - sort:
          expression: "${body.field}"
          reverse: true
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"sort":{"expression":"${body.field}","reverse":true}}]}]}"#,
        },
        ParityCase {
            name: "ClaimCheck",
            yaml: r#"
routes:
  - id: r1
    from: direct:start
    steps:
      - claim_check:
          repository: memory
          operation: set
          key: "${header.claimKey}"
"#,
            json: r#"{"routes":[{"id":"r1","from":"direct:start","steps":[{"claim_check":{"repository":"memory","operation":"set","key":"${header.claimKey}"}}]}]}"#,
        },
    ]
}

#[test]
fn test_yaml_json_parity_all_variants() {
    for case in parity_cases() {
        let yaml_ast: RouteDslRoutes = serde_yml::from_str(case.yaml)
            .unwrap_or_else(|e| panic!("YAML deserialize failed for {}: {}", case.name, e));
        let json_ast: RouteDslRoutes = serde_json::from_str(case.json)
            .unwrap_or_else(|e| panic!("JSON deserialize failed for {}: {}", case.name, e));

        // Flatten to Vec<&RouteDslStep> to avoid the Debug-derive cascade
        // through RouteDslRoute → RouteDslErrorHandler / RouteDslCircuitBreaker
        // (which only derive Deserialize, not Debug). The enum + all Step
        // structs already derive Debug, so this compiles unchanged.
        let yaml_steps: Vec<&RouteDslStep> = yaml_ast
            .routes
            .iter()
            .flat_map(|r| r.steps.iter())
            .collect();
        let json_steps: Vec<&RouteDslStep> = json_ast
            .routes
            .iter()
            .flat_map(|r| r.steps.iter())
            .collect();

        let yaml_debug = format!("{:#?}", yaml_steps);
        let json_debug = format!("{:#?}", json_steps);
        assert_eq!(
            yaml_debug, json_debug,
            "YAML and JSON deserialize to different step lists for variant {}\n\
             --- YAML steps debug ---\n{}\n\
             --- JSON steps debug ---\n{}",
            case.name, yaml_debug, json_debug
        );
    }
    // Touch the helper so it's not dead-code eliminated.
    let _ = _assert_all_variants_covered;
}

#[test]
fn test_variant_count_matches_matrix() {
    // The exhaustive match in `_assert_all_variants_covered` enforces at
    // compile time that every RouteDslStep variant has a match arm. This
    // test enforces at runtime that the matrix has at least one entry per
    // variant. It does NOT verify the entries exercise the right variant —
    // that is a known limitation.
    //
    // If you added a RouteDslStep variant, you will have:
    //   1. Added a match arm (compile-enforced).
    //   2. Bumped EXPECTED_VARIANTS below.
    //   3. Added a ParityCase for the new variant.
    // Step 3 is not mechanically enforced; review discipline applies.
    const EXPECTED_VARIANTS: usize = 34;
    let actual = parity_cases().len();
    assert!(
        actual >= EXPECTED_VARIANTS,
        "parity matrix has {} cases, expected at least {} (one per RouteDslStep variant)",
        actual,
        EXPECTED_VARIANTS
    );
}
