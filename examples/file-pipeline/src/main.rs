use camel_api::{body::Body, CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_core::context::CamelContext;
use camel_file::FileComponent;
use camel_log::LogComponent;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt::init();

    let base_dir = std::env::temp_dir().join("rust-camel-pipeline");
    let input_dir = base_dir.join("input");
    let output_dir = base_dir.join("output");

    std::fs::create_dir_all(&input_dir).ok();
    std::fs::create_dir_all(&output_dir).ok();

    let input_path = input_dir.to_str().unwrap();
    let output_path = output_dir.to_str().unwrap();

    let mut ctx = CamelContext::new();
    ctx.register_component(FileComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from(&format!(
        "file:{}?delete=true&initialDelay=0&delay=500",
        input_path
    ))
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
    .to(&format!("file:{}?fileExist=Override", output_path))
    .to("log:pipeline?showHeaders=true&showBody=true")
    .build()?;

    ctx.add_route_definition(route)?;
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

    println!("\nShutting down...");
    ctx.stop().await?;
    println!("Done.");

    Ok(())
}
