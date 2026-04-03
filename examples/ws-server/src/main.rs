use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_ws::WsComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::new();
    ctx.register_component(WsComponent);

    // Echo: messages are sent back to the same connection.
    // The consumer sets CamelWsConnectionKey, and the producer (in server-send
    // mode because a local consumer exists) sends back to that connection key.
    let echo_route = RouteBuilder::from("ws://0.0.0.0:9000/echo")
        .to("ws://0.0.0.0:9000/echo")
        .build()?;

    // Chat: broadcast every incoming message to ALL connected clients.
    let chat_route = RouteBuilder::from("ws://0.0.0.0:9000/chat")
        .set_header("CamelWsSendToAll", Value::Bool(true))
        .to("ws://0.0.0.0:9000/chat")
        .build()?;

    ctx.add_route_definition(echo_route).await?;
    ctx.add_route_definition(chat_route).await?;
    ctx.start().await?;

    println!("WebSocket Server running on ws://0.0.0.0:9000\n");
    println!("Endpoints:");
    println!("  ws://localhost:9000/echo  - Echo (sends back to sender)");
    println!("  ws://localhost:9000/chat  - Chat (broadcasts to all)\n");
    println!("Test with:");
    println!("  websocat ws://localhost:9000/echo");
    println!("  websocat ws://localhost:9000/chat\n");
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|_| CamelError::ProcessorError("failed to listen for ctrl+c".into()))?;

    println!("\nShutting down...");
    ctx.stop().await
}
