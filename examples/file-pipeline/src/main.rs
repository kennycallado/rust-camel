use camel_api::body::Body;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_file::FileComponent;
use camel_component_log::LogComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let base_dir = std::env::temp_dir().join("rust-camel-pipeline");
    let input_dir = base_dir.join("input");
    let output_dir = base_dir.join("output");

    std::fs::create_dir_all(&input_dir).ok();
    std::fs::create_dir_all(&output_dir).ok();

    let input_path = input_dir.to_str().unwrap();
    let output_path = output_dir.to_str().unwrap();

    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(FileComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from(&format!(
        "file:{}?delete=true&initialDelay=0&delay=500&readTimeout=5000",
        input_path
    ))
    .route_id("file-pipeline-demo")
    .process(|mut exchange: camel_api::Exchange| {
        Box::pin(async move {
            if let Body::Text(text) = &exchange.input.body {
                let original_len = text.len();
                exchange.input.body = Body::Text(text.to_uppercase());
                exchange
                    .input
                    .set_header("original-length", Value::Number(original_len.into()));
            }
            Ok(exchange)
        })
    })
    .to(format!(
        "file:{}?fileExist=Override&writeTimeout=5000",
        output_path
    ))
    .to("log:pipeline?showHeaders=true&showBody=true&showCorrelationId=true")
    .error_handler(ErrorHandlerConfig::dead_letter_channel(
        "log:dead-letter?showBody=true&showHeaders=true&showCorrelationId=true",
    ))
    .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("File pipeline example running.");
    println!("  Input:  {}", input_path);
    println!("  Output: {}", output_path);
    println!();
    println!("Drop .txt files into input/ - they will be:");
    println!("  1. Read from input/");
    println!("  2. Transformed to UPPERCASE");
    println!("  3. Written to output/");
    println!("  4. Deleted from input/ (delete=true)");
    println!();
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down file pipeline example...");
    ctx.stop().await?;
    println!("File pipeline example stopped cleanly.");

    Ok(())
}
