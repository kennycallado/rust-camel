//! Integration tests for the `convert_body_to` YAML DSL pipeline.
//!
//! These tests cover:
//! 1. YAML parsing of `convert_body_to` directive
//! 2. Unknown target error handling
//! 3. All valid target types (text, json, bytes, empty)
//! 4. Full pipeline from YAML → compile → BuilderStep::Processor
//!
//! Note: Builder API tests for ConvertBodyTo processor are in
//! `camel-processor/src/convert_body.rs`.

use camel_core::route::BuilderStep;
use camel_dsl::{
    BodyTypeDef, DeclarativeRoute, DeclarativeStep, compile_declarative_route, parse_yaml,
};

// =============================================================================
// YAML Parsing Tests
// =============================================================================

#[test]
fn yaml_convert_body_to_json_produces_correct_step() {
    let yaml = r#"
routes:
  - id: "convert-json-route"
    from: "direct:start"
    steps:
      - convert_body_to: json
"#;
    let defs = parse_yaml(yaml).expect("YAML parse should succeed");
    assert_eq!(defs.len(), 1, "expected exactly one route");

    let steps = defs[0].steps();
    assert_eq!(steps.len(), 1, "expected exactly one step");

    // Verify the step compiled to a Processor wrapping ConvertBodyTo
    match &steps[0] {
        BuilderStep::Processor(_) => {
            // This is the expected path - ConvertBodyTo is wrapped in a BoxProcessor
        }
        other => {
            panic!("expected BuilderStep::Processor, got {:?}", other);
        }
    }
}

#[test]
fn yaml_convert_body_to_xml_produces_correct_step() {
    let yaml = r#"
routes:
  - id: "convert-xml-route"
    from: "direct:start"
    steps:
      - convert_body_to: xml
"#;
    let defs = parse_yaml(yaml).expect("YAML parse should succeed for xml target");
    assert_eq!(defs.len(), 1, "expected exactly one route");

    let steps = defs[0].steps();
    assert_eq!(steps.len(), 1, "expected exactly one step");

    // Verify the step compiled to a Processor wrapping ConvertBodyTo
    match &steps[0] {
        BuilderStep::Processor(_) => {
            // This is the expected path - ConvertBodyTo is wrapped in a BoxProcessor
        }
        other => {
            panic!("expected BuilderStep::Processor, got {:?}", other);
        }
    }
}

#[test]
fn yaml_convert_body_to_unknown_target_returns_error() {
    let yaml = r#"
routes:
  - id: "convert-unknown-route"
    from: "direct:start"
    steps:
      - convert_body_to: unknown_format
"#;
    let result = parse_yaml(yaml);
    assert!(result.is_err(), "expected error for unknown target");

    let err_msg = match result {
        Err(e) => e.to_string(),
        Ok(_) => panic!("expected error"),
    };
    assert!(
        err_msg.contains("unknown convert_body_to target"),
        "error message should mention 'unknown convert_body_to target', got: {}",
        err_msg
    );
    assert!(
        err_msg.contains("unknown_format"),
        "error message should mention the invalid target 'unknown_format', got: {}",
        err_msg
    );
}

#[test]
fn yaml_convert_body_to_all_valid_targets_parse() {
    // Test all five valid targets: text, json, bytes, xml, empty
    for target in ["text", "json", "bytes", "xml", "empty"] {
        let yaml = format!(
            r#"
routes:
  - id: "convert-{}-route"
    from: "direct:start"
    steps:
      - convert_body_to: {}
"#,
            target, target
        );
        let defs = parse_yaml(&yaml).unwrap_or_else(|e| {
            panic!(
                "YAML parse should succeed for target '{}', got error: {}",
                target, e
            )
        });
        assert_eq!(
            defs.len(),
            1,
            "expected exactly one route for target '{}'",
            target
        );
        assert_eq!(
            defs[0].steps().len(),
            1,
            "expected exactly one step for target '{}'",
            target
        );
    }
}

#[test]
fn yaml_convert_body_to_is_case_insensitive() {
    // Verify that uppercase and mixed case also work
    for target in ["JSON", "Json", "json"] {
        let yaml = format!(
            r#"
routes:
  - id: "convert-case-route"
    from: "direct:start"
    steps:
      - convert_body_to: {}
"#,
            target
        );
        let defs = parse_yaml(&yaml).unwrap_or_else(|e| {
            panic!(
                "YAML parse should succeed for case-insensitive target '{}', got error: {}",
                target, e
            )
        });
        assert_eq!(defs.len(), 1);
    }
}

// =============================================================================
// Declarative Model Tests (YAML → DeclarativeStep)
// =============================================================================

#[test]
fn declarative_step_convert_body_to_json_compiles_correctly() {
    let route = DeclarativeRoute {
        from: "direct:start".into(),
        route_id: "test-convert".into(),
        auto_startup: true,
        startup_order: 1,
        concurrency: None,
        error_handler: None,
        circuit_breaker: None,
        unit_of_work: None,
        steps: vec![DeclarativeStep::ConvertBodyTo(BodyTypeDef::Json)],
    };

    let def = compile_declarative_route(route).expect("compile should succeed");
    assert_eq!(def.steps().len(), 1);

    // Verify it compiles to a Processor step
    assert!(
        matches!(&def.steps()[0], BuilderStep::Processor(_)),
        "expected BuilderStep::Processor"
    );
}

#[test]
fn declarative_step_convert_body_to_all_types_compile() {
    // Verify all BodyTypeDef variants compile correctly
    for (body_type_def, name) in [
        (BodyTypeDef::Text, "text"),
        (BodyTypeDef::Json, "json"),
        (BodyTypeDef::Bytes, "bytes"),
        (BodyTypeDef::Xml, "xml"),
        (BodyTypeDef::Empty, "empty"),
    ] {
        let route = DeclarativeRoute {
            from: "direct:start".into(),
            route_id: format!("test-convert-{}", name),
            auto_startup: true,
            startup_order: 1,
            concurrency: None,
            error_handler: None,
            circuit_breaker: None,
            unit_of_work: None,
            steps: vec![DeclarativeStep::ConvertBodyTo(body_type_def)],
        };

        let def = compile_declarative_route(route)
            .unwrap_or_else(|e| panic!("compile should succeed for {}, got error: {}", name, e));
        assert_eq!(def.steps().len(), 1);

        // Verify it compiles to a Processor step
        assert!(
            matches!(&def.steps()[0], BuilderStep::Processor(_)),
            "expected BuilderStep::Processor for {}",
            name
        );
    }
}

// =============================================================================
// Full Pipeline Tests (YAML → compile)
// =============================================================================

#[test]
fn full_pipeline_yaml_to_compiled_processor() {
    // Test that a YAML route with convert_body_to compiles through the full pipeline
    let yaml = r#"
routes:
  - id: "full-pipeline-route"
    from: "timer:tick?period=1000"
    steps:
      - log: "Processing message"
      - convert_body_to: json
      - to: "mock:result"
"#;
    let defs = parse_yaml(yaml).expect("YAML parse should succeed");
    assert_eq!(defs.len(), 1, "expected exactly one route");

    let steps = defs[0].steps();
    assert_eq!(steps.len(), 3, "expected exactly three steps");

    // Step 0: DeclarativeLog
    assert!(
        matches!(&steps[0], BuilderStep::DeclarativeLog { .. }),
        "first step should be DeclarativeLog"
    );

    // Step 1: Processor (ConvertBodyTo)
    assert!(
        matches!(&steps[1], BuilderStep::Processor(_)),
        "second step should be Processor (ConvertBodyTo)"
    );

    // Step 2: To
    assert!(
        matches!(&steps[2], BuilderStep::To(uri) if uri == "mock:result"),
        "third step should be To('mock:result')"
    );
}

#[test]
fn multiple_convert_body_to_steps_in_route() {
    // Test a route with multiple convert_body_to steps
    let yaml = r#"
routes:
  - id: "multi-convert-route"
    from: "direct:start"
    steps:
      - convert_body_to: json
      - convert_body_to: text
      - convert_body_to: bytes
"#;
    let defs = parse_yaml(yaml).expect("YAML parse should succeed");
    assert_eq!(defs.len(), 1);

    let steps = defs[0].steps();
    assert_eq!(steps.len(), 3);

    // All steps should be Processor
    for (i, step) in steps.iter().enumerate() {
        assert!(
            matches!(step, BuilderStep::Processor(_)),
            "step {} should be Processor",
            i
        );
    }
}
