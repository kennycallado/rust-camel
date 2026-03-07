//! # Body Type Conversion Example
//!
//! This example demonstrates the `convert_body_to` feature in rust-camel,
//! which allows converting message bodies between different types:
//!
//! - `Text` ↔ `Json` ↔ `Bytes`
//!
//! The conversion chain flows through multiple routes, each demonstrating
//! a specific conversion type and how errors are handled gracefully.

use camel_api::body::Body;
use camel_api::body_converter::BodyType;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::CamelError;
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
    // Route 1: Text → Json
    // =========================================================================
    // Demonstrates converting a valid JSON string (as Text) into a parsed Json body.
    // This is useful when you receive JSON as a string and need to work with it
    // as structured data.
    let route_text_to_json = RouteBuilder::from("timer:text-to-json?period=2000&repeatCount=3")
        .route_id("text-to-json")
        .set_body(r#"{"message": "hello from text", "count": 42}"#)
        .log("Route 1: Starting with Text body containing JSON string", LogLevel::Info)
        .convert_body_to(BodyType::Json)
        .log("Route 1: Converted Text → Json successfully!", LogLevel::Info)
        .to("log:info?showBody=true")
        .error_handler(ErrorHandlerConfig::log_only())
        .build()?;

    ctx.add_route_definition(route_text_to_json)?;

    // =========================================================================
    // Route 2: Json → Bytes
    // =========================================================================
    // Demonstrates serializing a Json body back to raw bytes.
    // Useful when you need to send JSON data over a binary protocol
    // or write to a file/stream.
    let route_json_to_bytes = RouteBuilder::from("timer:json-to-bytes?period=2000&repeatCount=3")
        .route_id("json-to-bytes")
        .process(|mut exchange| async move {
            // Create a Json body programmatically
            exchange.input.body = Body::Json(serde_json::json!({
                "event": "converted",
                "source": "route-2"
            }));
            Ok(exchange)
        })
        .log("Route 2: Starting with Json body", LogLevel::Info)
        .convert_body_to(BodyType::Bytes)
        .log("Route 2: Converted Json → Bytes successfully!", LogLevel::Info)
        .to("log:info?showBody=true")
        .error_handler(ErrorHandlerConfig::log_only())
        .build()?;

    ctx.add_route_definition(route_json_to_bytes)?;

    // =========================================================================
    // Route 3: Bytes → Text
    // =========================================================================
    // Demonstrates decoding UTF-8 bytes back into a Text string.
    // Useful when receiving raw byte data that should be interpreted as text.
    let route_bytes_to_text = RouteBuilder::from("timer:bytes-to-text?period=2000&repeatCount=3")
        .route_id("bytes-to-text")
        .process(|mut exchange| async move {
            // Create a Bytes body with UTF-8 content
            use bytes::Bytes;
            exchange.input.body = Body::Bytes(Bytes::from("Hello from bytes!"));
            Ok(exchange)
        })
        .log("Route 3: Starting with Bytes body (UTF-8)", LogLevel::Info)
        .convert_body_to(BodyType::Text)
        .log("Route 3: Converted Bytes → Text successfully!", LogLevel::Info)
        .to("log:info?showBody=true")
        .error_handler(ErrorHandlerConfig::log_only())
        .build()?;

    ctx.add_route_definition(route_bytes_to_text)?;

    // =========================================================================
    // Route 4: Error Path - Empty → Text (fails gracefully)
    // =========================================================================
    // Demonstrates that conversion errors are handled gracefully.
    // Converting an Empty body to Text is not possible, so this route
    // will trigger the error handler, which logs the error and continues.
    let route_error_path = RouteBuilder::from("timer:error-path?period=2000&repeatCount=2")
        .route_id("error-path-demo")
        .process(|mut exchange| async move {
            // Explicitly set an Empty body to trigger conversion error
            exchange.input.body = Body::Empty;
            Ok(exchange)
        })
        .log("Route 4: Starting with Empty body (will fail conversion)", LogLevel::Info)
        .convert_body_to(BodyType::Text) // This will fail!
        .log("Route 4: This log should NOT appear (conversion failed)", LogLevel::Info)
        .to("log:info?showBody=true")
        .error_handler(
            ErrorHandlerConfig::log_only()
                .on_exception(|e| matches!(e, CamelError::TypeConversionFailed(_)))
                .build(),
        )
        .build()?;

    ctx.add_route_definition(route_error_path)?;

    // Start the Camel context
    ctx.start().await?;

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║        Body Type Conversion Example - Running                ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("║  Route 1: Text → Json   (timer:text-to-json)                 ║");
    println!("║  Route 2: Json → Bytes  (timer:json-to-bytes)                ║");
    println!("║  Route 3: Bytes → Text  (timer:bytes-to-text)                ║");
    println!("║  Route 4: Empty → Text  (error demonstration)                ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("║  Press Ctrl+C to stop                                        ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    // Wait for Ctrl+C signal
    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;

    println!("Context stopped. Goodbye!");
    Ok(())
}
