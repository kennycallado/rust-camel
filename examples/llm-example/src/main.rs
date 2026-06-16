//! LLM component example for rust-camel.
//!
//! Demonstrates three LLM chat modes with the mock provider:
//!   1. Materialized (non-streaming) — `llm:chat?stream=false`
//!   2. Streaming                  — `llm:chat?stream=true`
//!   3. Tool calling               — `llm:chat` with `CamelLlmTools` header
//!
//! Run:
//!   cargo run -p llm-example
//!
//! Press Ctrl+C to stop.

use camel_api::CamelError;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::ComponentBundle;
use camel_component_llm::LlmBundle;
use camel_component_llm::headers::*;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

/// Register the LLM component via its config bundle (mock provider, echo mode).
fn register_llm_bundle(ctx: &mut CamelContext) {
    let toml_str = r#"
max_prompt_bytes = 1048576

[providers.mock]
type = "mock"
response = "echo"
default_model = "test"
"#;
    let value: toml::Value = toml::from_str(toml_str).expect("parse toml"); // allow-unwrap
    let bundle = LlmBundle::from_toml(value).expect("bundle"); // allow-unwrap
    bundle.register_all(ctx);
}

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    println!("=== LLM Example — Mock Provider ===\n");

    let mut ctx = CamelContext::builder().build().await?;
    register_llm_bundle(&mut ctx);
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // ------------------------------------------------------------------
    // Route 1: Materialized Chat (non-streaming, echo mock)
    // ------------------------------------------------------------------
    println!("=== Route 1: Materialized Chat (stream=false) ===");
    println!("  timer -> llm:chat?stream=false -> log\n");

    let route1 = RouteBuilder::from("timer:llm-1?period=500&repeatCount=1")
        .route_id("llm-materialized")
        .set_body("Hello LLM! This is a materialized (non-streaming) chat request.")
        .to("llm:chat?provider=mock&model=test&stream=false")
        .to("log:info?showBody=true&showHeaders=true")
        .build()?;
    ctx.add_route_definition(route1).await?;

    // ------------------------------------------------------------------
    // Route 2: Streaming Chat (echo mock)
    // ------------------------------------------------------------------
    println!("=== Route 2: Streaming Chat (stream=true) ===");
    println!("  timer -> llm:chat?stream=true -> stream_cache -> log\n");

    let route2 = RouteBuilder::from("timer:llm-2?period=500&repeatCount=1&delay=100")
        .route_id("llm-streaming")
        .set_body("Hello LLM! This is a streaming chat request.")
        .to("llm:chat?provider=mock&model=test&stream=true")
        // Stream body must be materialized before logging
        .stream_cache_default()
        .to("log:info?showBody=true&showHeaders=true")
        .build()?;
    ctx.add_route_definition(route2).await?;

    // ------------------------------------------------------------------
    // Route 3: Tool Calling (with CamelLlmTools header)
    // ------------------------------------------------------------------
    println!("=== Route 3: Tool Calling (CamelLlmTools header) ===");
    println!("  timer -> set_header CamelLlmTools -> llm:chat?stream=false -> log\n");

    let route3 = RouteBuilder::from("timer:llm-3?period=500&repeatCount=1&delay=200")
        .route_id("llm-tool-calling")
        .set_body("What's the weather in London?")
        // Set tool definitions — with a real provider these would trigger
        // tool call responses. The mock provider echoes the prompt but
        // the header is forwarded for demonstration.
        .set_header(
            CAMEL_LLM_TOOLS,
            serde_json::json!([
                {
                    "name": "get_weather",
                    "description": "Get weather for a city",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "city": {
                                "type": "string",
                                "description": "City name"
                            }
                        },
                        "required": ["city"]
                    }
                }
            ]),
        )
        .to("llm:chat?provider=mock&model=test&stream=false")
        .to("log:info?showBody=true&showHeaders=true")
        .build()?;
    ctx.add_route_definition(route3).await?;

    println!("\nStarting LLM example... Press Ctrl+C to stop.\n");

    ctx.start().await?;

    tokio::signal::ctrl_c().await.ok();
    println!("\nShutting down...");
    ctx.stop().await?;
    println!("LLM example stopped.");
    Ok(())
}
