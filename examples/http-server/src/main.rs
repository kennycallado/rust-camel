use camel_api::body::Body;
use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_core::context::CamelContext;
use camel_http::HttpComponent;

/// HTTP Server example demonstrating multiple routes on the same port.
///
/// Routes:
///   POST /echo        - Echoes request body back
///   GET  /api/status  - Returns JSON health status
///   GET  /proxy       - Reverse proxy to httpbin.org
///   POST /transform   - Transforms text to uppercase JSON
///
/// All routes share port 8080 via ServerRegistry.
#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt::init();

    let mut ctx = CamelContext::new();
    ctx.register_component(HttpComponent::new());

    // Route 1: Echo server - returns request body unchanged
    let echo_route = RouteBuilder::from("http://0.0.0.0:8080/echo")
        .process(|exchange| async move {
            // Log the request
            let method = exchange
                .input
                .header("CamelHttpMethod")
                .and_then(|v| v.as_str())
                .unwrap_or("UNKNOWN");
            tracing::info!(method, "Echo request received");
            
            // Echo the body back (no modification needed)
            Ok(exchange)
        })
        .build()?;

    // Route 2: JSON API - returns health status
    let api_route = RouteBuilder::from("http://0.0.0.0:8080/api/status")
        .process(|mut exchange| async move {
            exchange.input.body = Body::Json(serde_json::json!({
                "status": "healthy",
                "service": "rust-camel-http-server",
                "timestamp": chrono_timestamp(),
            }));
            exchange
                .input
                .set_header("Content-Type", Value::String("application/json".into()));
            Ok(exchange)
        })
        .build()?;

    // Route 3: Reverse proxy to httpbin.org
    let proxy_route = RouteBuilder::from("http://0.0.0.0:8080/proxy")
        .to("https://httpbin.org/get?source=rust-camel&throwExceptionOnFailure=false")
        .build()?;

    // Route 4: POST handler with JSON transformation
    let transform_route = RouteBuilder::from("http://0.0.0.0:8080/transform")
        .process(|mut exchange| async move {
            // Read request body and transform
            let input = exchange.input.body.as_text().unwrap_or("");
            
            exchange.input.body = Body::Json(serde_json::json!({
                "original": input,
                "transformed": input.to_uppercase(),
                "length": input.len(),
            }));
            exchange
                .input
                .set_header("Content-Type", Value::String("application/json".into()));
            exchange
                .input
                .set_header("CamelHttpResponseCode", Value::from(201u16));
            
            Ok(exchange)
        })
        .build()?;

    ctx.add_route_definition(echo_route)?;
    ctx.add_route_definition(api_route)?;
    ctx.add_route_definition(proxy_route)?;
    ctx.add_route_definition(transform_route)?;

    ctx.start().await?;

    println!("HTTP server running on http://0.0.0.0:8080");
    println!();
    println!("Available routes:");
    println!("  POST http://localhost:8080/echo");
    println!("       Returns request body unchanged");
    println!();
    println!("  GET  http://localhost:8080/api/status");
    println!("       Returns JSON health status");
    println!();
    println!("  GET  http://localhost:8080/proxy");
    println!("       Reverse proxy to httpbin.org");
    println!();
    println!("  POST http://localhost:8080/transform");
    println!("       Transforms text to uppercase JSON");
    println!();
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down...");
    ctx.stop().await?;
    println!("Done.");

    Ok(())
}

fn chrono_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
