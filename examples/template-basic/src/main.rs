//! # Template Basic Example
//!
//! Minimal example demonstrating external template rendering with the
//! `template` component using a file-based MiniJinja template.
//!
//! ## Route
//!
//! ```text
//! timer:tick → set_body("World") → set_header("title") → template:file:///... → log:info
//! ```
//!
//! The template (`templates/page.html.tmpl`) renders the exchange body as
//! `{{ body }}` and the `title` header as `{{ title }}`.

use camel_api::CamelError;
use camel_api::Value;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_language_api::MinijinjaLimitsConfig;
use camel_processor::LogLevel;
use camel_template::ExternalTemplateLimitsConfig;
use camel_template::TemplateComponent;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap

    // Register required components
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // Register the template component with default limits
    ctx.register_component(TemplateComponent::new(
        ExternalTemplateLimitsConfig::default(),
        MinijinjaLimitsConfig::default(),
    ));

    // Build the URI to the template file using the compile-time manifest dir
    let template_uri = format!(
        "template:file://{}/templates/page.html.tmpl",
        env!("CARGO_MANIFEST_DIR")
    );

    // Route: timer → set body/headers → render template → log result
    let route = RouteBuilder::from("timer:tick?period=2000")
        .route_id("template-demo")
        .set_body("World")
        .set_header("title", Value::String("Template Demo".into()))
        .to(&template_uri)
        .log("Rendered template", LogLevel::Info)
        .build()?;

    ctx.add_route_definition(route).await?;

    ctx.start().await?;

    println!("╔═════════════════════════════════════════════════════╗");
    println!("║     Template Basic Example — Running               ║");
    println!("╠═════════════════════════════════════════════════════╣");
    println!("║  Routes: template-demo                              ║");
    println!("║  Template: page.html.tmpl                           ║");
    println!("║  Press Ctrl+C to stop                               ║");
    println!("╚═════════════════════════════════════════════════════╝");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    ctx.stop().await?;
    println!("Context stopped. Goodbye!");
    Ok(())
}
