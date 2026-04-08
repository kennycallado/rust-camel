//! # RecipientList EIP Example
//!
//! Demonstrates the RecipientList pattern — dynamically resolve endpoint URIs
//! from an expression evaluated at runtime.
//!
//! A timer fires periodically and sets the `destinations` header to a
//! comma-separated list of log endpoint URIs. The RecipientList processor
//! evaluates the expression, splits the result by delimiter, and sends a copy
//! of the exchange to each resolved endpoint in parallel.
//!
//! Flow:
//!   timer -> set_header("destinations", "log:a,log:b,log:c")
//!          -> recipient_list(expression = header "destinations")
//!          -> log:summary
//!
//! ## Running
//!
//! ```sh
//! cargo run -p recipientlist
//! ```

use std::sync::Arc;

use camel_api::recipient_list::{RecipientListConfig, RecipientListExpression};
use camel_api::{CamelError, Value};
use camel_builder::RouteBuilder;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let expression: RecipientListExpression = Arc::new(|ex| {
        ex.input
            .header("destinations")
            .and_then(|v| match v {
                Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default()
    });

    let route = RouteBuilder::from("timer:tick?period=2000&repeatCount=3")
        .route_id("recipientlist-demo")
        .set_header(
            "destinations",
            Value::String("log:channel-a?showBody=true,log:channel-b?showBody=true,log:channel-c?showBody=true".into()),
        )
        .recipient_list_with_config(
            RecipientListConfig::new(expression)
                .parallel(true),
        )
        .to("log:summary?showBody=true")
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("RecipientList example running.");
    println!("Each tick resolves 3 endpoints from the 'destinations' header.");
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down...");
    ctx.stop().await?;
    println!("Stopped.");
    Ok(())
}
