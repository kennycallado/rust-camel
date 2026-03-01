use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_core::context::CamelContext;
use camel_file::FileComponent;
use camel_log::LogComponent;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt::init();

    let input_dir = std::env::temp_dir().join("rust-camel-input");
    std::fs::create_dir_all(&input_dir).ok();

    let input_path = input_dir.to_str().unwrap();

    let mut ctx = CamelContext::new();
    ctx.register_component(FileComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from(&format!(
        "file:{}?noop=true&initialDelay=0&delay=1000",
        input_path
    ))
    .set_header("processed-by", Value::String("file-polling-example".into()))
    .to("log:file-events?showHeaders=true&showBody=true&showExchangeId=true")
    .build()?;

    ctx.add_route_definition(route)?;
    ctx.start().await?;

    println!("File polling example running.");
    println!("Input directory: {}", input_path);
    println!();
    println!("Drop files into the directory and watch them being consumed.");
    println!("Files are NOT deleted (noop=true).");
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
