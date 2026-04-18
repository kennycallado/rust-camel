//! # Validator Component Example
//!
//! Demonstrates schema validation of message bodies using the `validator` component:
//!
//! - **Route 1 (XSD):** Validates XML orders against an XSD schema
//! - **Route 2 (JSON Schema):** Validates JSON shipments against a JSON Schema
//! - **Route 3 (YAML Schema):** Validates YAML config against a JSON Schema (YAML→JSON)
//! - **Route 4 (Error path):** Shows how invalid XML is caught by the error handler

use std::path::PathBuf;

use camel_api::CamelError;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_component_validator::ValidatorComponent;
use camel_core::context::CamelContext;
use camel_processor::LogLevel;

fn schema_path(name: &str) -> String {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    base.join("schemas").join(name).to_string_lossy().into()
}

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::builder().build().await.unwrap();

    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(ValidatorComponent::new());

    // =========================================================================
    // Route 1: XML validation (XSD)
    // =========================================================================
    // A valid XML order is validated against order.xsd. The body passes
    // through unchanged if valid; otherwise an error is returned.
    let xsd = schema_path("order.xsd");
    let route_xsd = RouteBuilder::from("timer:xsd-valid?period=3000&repeatCount=2")
        .route_id("xsd-valid")
        .set_body("<order><id>A1</id><amount>5</amount></order>")
        .log("Route 1: Validating XML order against XSD", LogLevel::Info)
        .validate(&xsd)
        .log("Route 1: XML is valid!", LogLevel::Info)
        .to("log:info?showBody=true")
        .build()?;

    ctx.add_route_definition(route_xsd).await?;

    // =========================================================================
    // Route 2: JSON Schema validation
    // =========================================================================
    // A valid JSON shipment is validated against shipment.json.
    let json_schema = schema_path("shipment.json");
    let route_json = RouteBuilder::from("timer:json-valid?period=3000&repeatCount=2")
        .route_id("json-valid")
        .set_body(r#"{"id": "S42", "status": "shipped"}"#)
        .unmarshal("json")
        .log("Route 2: Validating JSON shipment", LogLevel::Info)
        .validate(&json_schema)
        .log("Route 2: JSON is valid!", LogLevel::Info)
        .to("log:info?showBody=true")
        .build()?;

    ctx.add_route_definition(route_json).await?;

    // =========================================================================
    // Route 3: YAML validation (via JSON Schema)
    // =========================================================================
    // A valid YAML config string is validated against config.yaml.
    let yaml_schema = schema_path("config.yaml");
    let route_yaml = RouteBuilder::from("timer:yaml-valid?period=3000&repeatCount=2")
        .route_id("yaml-valid")
        .set_body("host: localhost\nport: 8080\n")
        .log("Route 3: Validating YAML config", LogLevel::Info)
        .validate(&yaml_schema)
        .log("Route 3: YAML is valid!", LogLevel::Info)
        .to("log:info?showBody=true")
        .build()?;

    ctx.add_route_definition(route_yaml).await?;

    // =========================================================================
    // Route 4: Invalid XML (error handling)
    // =========================================================================
    // An invalid XML (missing required element) triggers a validation error,
    // caught by the error handler.
    let route_error = RouteBuilder::from("timer:xsd-error?period=3000&repeatCount=2")
        .route_id("xsd-error")
        .set_body("<order/>")
        .log(
            "Route 4: Attempting to validate invalid XML",
            LogLevel::Info,
        )
        .validate(&xsd)
        .log(
            "Route 4: This should NOT appear (validation failed)",
            LogLevel::Info,
        )
        .to("log:info?showBody=true")
        .error_handler(
            ErrorHandlerConfig::log_only()
                .on_exception(|e| matches!(e, CamelError::ProcessorError(_)))
                .build(),
        )
        .build()?;

    ctx.add_route_definition(route_error).await?;

    ctx.start().await?;

    println!("+==============================================================+");
    println!("|        Validator Component Example - Running                 |");
    println!("+==============================================================+");
    println!("|  Route 1: XML order  -> XSD validation (valid)               |");
    println!("|  Route 2: JSON ship. -> JSON Schema validation (valid)       |");
    println!("|  Route 3: YAML config -> JSON Schema validation (valid)      |");
    println!("|  Route 4: XML order  -> XSD validation (invalid, error)      |");
    println!("+==============================================================+");
    println!("|  Press Ctrl+C to stop                                        |");
    println!("+==============================================================+");

    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;

    println!("Context stopped. Goodbye!");
    Ok(())
}
