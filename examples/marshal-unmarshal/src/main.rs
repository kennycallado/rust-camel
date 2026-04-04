//! # Marshal/Unmarshal EIP Example
//!
//! This example demonstrates the `marshal` and `unmarshal` EIP features in
//! rust-camel, which transform message bodies between wire formats and
//! structured data:
//!
//! - `unmarshal("json")` — Text/Bytes with JSON content -> `Body::Json`
//! - `marshal("json")`   — `Body::Json` -> `Body::Text` (JSON string)
//! - `unmarshal("xml")`  — Text/Bytes with XML content -> `Body::Xml`
//!
//! Each route demonstrates a specific marshal/unmarshal flow and how errors
//! are handled gracefully via the error handler.

use camel_api::CamelError;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_processor::LogLevel;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    // Initialize tracing for structured logging
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::new();

    // Register required components
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // =========================================================================
    // Route 1: JSON round-trip (Text -> Json -> Text)
    // =========================================================================
    // Demonstrates a full marshal/unmarshal cycle: a JSON string is first
    // unmarshalled into a structured Json body, then marshalled back to a
    // Text body. This is the classic Text -> Json -> Text round-trip.
    let route_json_roundtrip = RouteBuilder::from("timer:json-roundtrip?period=2000&repeatCount=3")
        .route_id("json-roundtrip")
        .set_body(r#"{"message": "hello", "count": 42}"#)
        .log(
            "Route 1: Starting with Text body containing JSON string",
            LogLevel::Info,
        )
        .unmarshal("json")
        .log("Route 1: Unmarshalled Text -> Json", LogLevel::Info)
        .marshal("json")
        .log(
            "Route 1: Marshalled Json -> Text (round-trip complete!)",
            LogLevel::Info,
        )
        .to("log:info?showBody=true")
        .error_handler(ErrorHandlerConfig::log_only())
        .build()?;

    ctx.add_route_definition(route_json_roundtrip).await?;

    // =========================================================================
    // Route 2: XML unmarshal (Text -> Xml)
    // =========================================================================
    // Demonstrates unmarshalling an XML string into a structured Xml body.
    // Useful when receiving XML payloads that need to be processed as
    // structured data.
    let route_xml_unmarshal = RouteBuilder::from("timer:xml-unmarshal?period=2000&repeatCount=3")
        .route_id("xml-unmarshal")
        .set_body("<root><item>hello</item></root>")
        .log(
            "Route 2: Starting with Text body containing XML string",
            LogLevel::Info,
        )
        .unmarshal("xml")
        .log("Route 2: Unmarshalled Text -> Xml", LogLevel::Info)
        .to("log:info?showBody=true")
        .error_handler(ErrorHandlerConfig::log_only())
        .build()?;

    ctx.add_route_definition(route_xml_unmarshal).await?;

    // =========================================================================
    // Route 3: Error path - invalid JSON (fails gracefully)
    // =========================================================================
    // Demonstrates that unmarshal errors are handled gracefully. Passing
    // invalid JSON to unmarshal("json") triggers TypeConversionFailed,
    // which the error handler catches and logs.
    let route_error_path = RouteBuilder::from("timer:marshal-error?period=2000&repeatCount=2")
        .route_id("marshal-error-demo")
        .set_body("this is NOT valid json!")
        .log(
            "Route 3: Starting with invalid body (will fail unmarshal)",
            LogLevel::Info,
        )
        .unmarshal("json") // This will fail!
        .log(
            "Route 3: This log should NOT appear (unmarshal failed)",
            LogLevel::Info,
        )
        .to("log:info?showBody=true")
        .error_handler(
            ErrorHandlerConfig::log_only()
                .on_exception(|e| matches!(e, CamelError::TypeConversionFailed(_)))
                .build(),
        )
        .build()?;

    ctx.add_route_definition(route_error_path).await?;

    // Start the Camel context
    ctx.start().await?;

    println!("+==============================================================+");
    println!("|        Marshal/Unmarshal EIP Example - Running              |");
    println!("+==============================================================+");
    println!("|  Route 1: Text -> Json -> Text  (json roundtrip)             |");
    println!("|  Route 2: Text -> Xml           (xml unmarshal)              |");
    println!("|  Route 3: Invalid -> Json        (error demonstration)       |");
    println!("+==============================================================+");
    println!("|  Press Ctrl+C to stop                                        |");
    println!("+==============================================================+");

    // Wait for Ctrl+C signal
    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;

    println!("Context stopped. Goodbye!");
    Ok(())
}
