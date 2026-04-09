use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_file::FileComponent;
use camel_component_log::LogComponent;
use camel_core::context::CamelContext;

// noop=true enables the idempotent consumer: files are never deleted or moved,
// and files already seen in the current run are automatically skipped.

fn chrono_timestamp() -> String {
    chrono::Local::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string()
}

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let input_dir = std::env::temp_dir().join("rust-camel-input");
    std::fs::create_dir_all(&input_dir).ok();

    let input_path = input_dir.to_str().unwrap();

    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(FileComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from(&format!(
        "file:{}?noop=true&initialDelay=0&delay=1000&readTimeout=5000",
        input_path
    ))
    .route_id("file-polling-demo")
    .set_header("processed-by", Value::String("file-polling-example".into()))
    .set_header("timestamp", Value::String(chrono_timestamp()))
    .to("log:file-events?showHeaders=true&showBody=true&showExchangeId=true&showCorrelationId=true")
    .error_handler(
        ErrorHandlerConfig::log_only()
            .on_exception(|_| true)
            .retry(2)
            .build(),
    )
    .build()?;

    ctx.add_route_definition(route).await?;
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

    println!("\nShutting down file polling example...");
    ctx.stop().await?;
    println!("File polling example stopped cleanly.");

    Ok(())
}
