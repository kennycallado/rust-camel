//! # Environment Variable Interpolation Example
//!
//! Demonstrates `${env:VAR_NAME}` substitution in YAML route files.
//!
//! The DSL loader replaces `${env:VAR_NAME}` tokens before parsing YAML,
//! so they can appear anywhere: endpoint URIs, log messages, header values.
//!
//! ## Running
//!
//! ```sh
//! # With custom values
//! GREET_TARGET=log:my-app \
//! POLL_PERIOD_MS=2000 \
//! LOG_PREFIX="[demo]" \
//! cargo run -p env-interpolation
//!
//! # With defaults (unset vars stay as literal strings — you'll see them in output)
//! cargo run -p env-interpolation
//! ```

use camel_api::CamelError;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_config::{CamelConfig, discover_routes};
use camel_core::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    // Show which env vars are set so the user can see interpolation in action
    println!("=== env-interpolation example ===");
    println!(
        "  GREET_TARGET  = {}",
        std::env::var("GREET_TARGET").unwrap_or_else(|_| "<not set>".into())
    );
    println!(
        "  POLL_PERIOD_MS = {}",
        std::env::var("POLL_PERIOD_MS").unwrap_or_else(|_| "<not set>".into())
    );
    println!(
        "  LOG_PREFIX     = {}",
        std::env::var("LOG_PREFIX").unwrap_or_else(|_| "<not set>".into())
    );
    println!();
    println!("Loaded routes will substitute these values before YAML parse.");
    println!("Press Ctrl+C to stop.\n");

    let config =
        CamelConfig::from_file("Camel.toml").map_err(|e| CamelError::Config(e.to_string()))?;

    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let routes = discover_routes(&config.routes).map_err(|e| CamelError::Config(e.to_string()))?;
    for route in routes {
        ctx.add_route_definition(route).await?;
    }

    ctx.start().await?;

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    ctx.stop().await?;
    Ok(())
}
