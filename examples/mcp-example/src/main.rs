//! MCP component example for rust-camel.
//!
//! One process plays both MCP roles over a real loopback HTTP hop:
//!   1. Server (Consumer) role — exposes the `greet` tool on the shared
//!      Streamable-HTTP listener for server `demo`.
//!   2. Client (Producer) role — a timer route calls that same tool through
//!      `mcp:call`, as it would call any remote MCP server.
//!
//! Run:
//!   cargo run -p mcp-example
//!
//! Press Ctrl+C to stop.

use camel_api::CamelError;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::ComponentBundle;
use camel_component_log::LogComponent;
use camel_component_mcp::McpBundle;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};

/// Register the MCP component via its config bundle. The server `demo` and the
/// remote `demo` share one TOML config; the remote URL points at the server's
/// own listener, so the producer calls the consumer through a real HTTP hop.
fn register_mcp_bundle(ctx: &mut CamelContext) {
    let toml_str = r#"
[servers.demo]
bind = "127.0.0.1:9100"
security_policy = { require = "auth" }

[remotes.demo]
url = "http://127.0.0.1:9100/mcp"
transport = "streamable-http"
"#;
    let value: toml::Value = toml::from_str(toml_str).expect("parse toml"); // allow-unwrap
    let bundle = McpBundle::from_toml(value).expect("bundle"); // allow-unwrap
    bundle.register_all(ctx);
}

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    println!("=== MCP Example — shared listener + loopback client ===\n");

    let mut ctx = CamelContext::builder().build().await?;
    register_mcp_bundle(&mut ctx);
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // ------------------------------------------------------------------
    // Route 1: Server (Consumer) role — expose the `greet` tool.
    //
    // The from-URI carries the tool's input JSON Schema on the `schema`
    // query param (percent-encoded). The schema lives in operator
    // config, never on the wire. The step below is the tool behavior:
    // every `tools/call` reply is this body.
    // ------------------------------------------------------------------
    println!("=== Route 1: mcp:demo/tool/greet (server role) ===\n");

    let schema = serde_json::json!({
        "type": "object",
        "properties": { "name": { "type": "string" } },
        "required": ["name"]
    })
    .to_string();
    let schema_param = utf8_percent_encode(&schema, NON_ALPHANUMERIC).to_string();

    let route1 = RouteBuilder::from(format!("mcp:demo/tool/greet?schema={schema_param}").as_str())
        .route_id("mcp-greet-tool")
        .set_body("Hello from the camel MCP server!")
        .build()?;
    ctx.add_route_definition(route1).await?;

    // ------------------------------------------------------------------
    // Route 2: Client (Producer) role — call the tool.
    //
    // Added hot, after the listener is up. The Exchange body is the tool
    // arguments. The reply content lands in the body; `CamelMcpResult`
    // carries `{"is_error": false, ...}`. The producer never acts on the
    // flag — the route author decides.
    // ------------------------------------------------------------------
    println!("=== Route 2: timer -> mcp:call?server=demo&tool=greet ===\n");
    let route2 = RouteBuilder::from("timer:mcp-1?period=1000&repeatCount=1")
        .route_id("mcp-greet-call")
        .set_body(serde_json::json!({ "name": "camel" }))
        .to("mcp:call?server=demo&tool=greet")
        .to("log:info?showBody=true&showHeaders=true")
        .build()?;
    ctx.add_route_definition(route2).await?;

    println!("\nStarting MCP example... Press Ctrl+C to stop.\n");
    println!("MCP host clients can also reach the server directly:");
    println!("  URL: http://127.0.0.1:9100/mcp (Streamable HTTP, protocol 2026-07-28)\n");

    ctx.start().await?;

    tokio::signal::ctrl_c().await.ok();
    println!("\nShutting down...");
    ctx.stop().await?;
    println!("MCP example stopped.");
    Ok(())
}
