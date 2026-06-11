use camel_api::body::Body;
use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_file::FileComponent;
use camel_component_log::LogComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let base_dir = std::env::temp_dir().join("rust-camel-p1-features");
    let input_dir = base_dir.join("input");
    let output_dir = base_dir.join("output");

    // Clean up from previous runs
    let _ = std::fs::remove_dir_all(&base_dir);
    std::fs::create_dir_all(&input_dir)?;
    std::fs::create_dir_all(&output_dir)?;

    // ---- Setup: create files with different extensions ----
    std::fs::write(input_dir.join("data.txt"), "hello world")?;
    std::fs::write(input_dir.join("data.txt.done"), "")?;

    std::fs::write(input_dir.join("report.csv"), "id,name\n1,Alice")?;
    std::fs::write(input_dir.join("report.csv.done"), "")?;

    std::fs::write(input_dir.join("config.json"), r#"{"key": "value"}"#)?;
    // NOTE: no .done marker for config.json — it will NOT be picked up by doneFileName

    std::fs::write(input_dir.join("notes.txt"), "some notes")?;
    // NOTE: no .done marker for notes.txt — it will NOT be picked up

    let sub_dir = input_dir.join("sub");
    std::fs::create_dir_all(&sub_dir)?;
    std::fs::write(sub_dir.join("deep.csv"), "x,y\n1,2")?;
    // No .done marker — skipped

    let input_path = input_dir.to_str().unwrap(); // allow-unwrap
    let output_path = output_dir.to_str().unwrap(); // allow-unwrap

    println!("=== P1 Features Demo ===\n");

    // ---- Route 1: doneFileName + includeExt + sortBy ----
    // Only pick up .txt and .csv files that have a .done marker.
    // Process oldest files first (sortBy=file:modified).
    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap
    ctx.register_component(FileComponent::new());
    ctx.register_component(LogComponent::new());

    let route1 = RouteBuilder::from(&format!(
        "file:{}?doneFileName=${{file:name}}.done&includeExt=txt,csv&\
         sortBy=file:modified&initialDelay=0&delay=500",
        input_path
    ))
    .route_id("p1-consumer")
    .process(|mut exchange: camel_api::Exchange| {
        Box::pin(async move {
            let filename = exchange
                .input
                .header("CamelFileNameOnly")
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            println!("  [consumer] processing: {}", filename);
            exchange.input.body = Body::Text(filename);
            Ok(exchange)
        })
    })
    .set_header_fn("CamelFileName", |ex| {
        let name = ex
            .input
            .header("CamelFileNameOnly")
            .and_then(|v| v.as_str())
            .unwrap_or("output");
        Value::String(name.to_string())
    })
    .to(format!(
        "file:{}?fileExist=TryRename&tempPrefix=.tmp",
        output_path
    ))
    .to("log:p1-done?showHeaders=false&showBody=true&showCorrelationId=false")
    .build()?;

    ctx.add_route_definition(route1).await?;
    ctx.start().await?;

    // Give polling a moment to pick up and process files
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    println!("\n--- Files in output/ (TryRename producer) ---");
    let mut entries = tokio::fs::read_dir(&output_dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.is_file() && !path.file_name().unwrap().to_str().unwrap().starts_with('.') { // allow-unwrap
            let content = tokio::fs::read_to_string(&path).await.unwrap_or_default();
            println!(
                "  {} (content: {})",
                path.file_name().unwrap().to_str().unwrap(), // allow-unwrap
                content
            );
        }
    }

    // ---- Route 2: maxMessagesPerPoll + sortBy=reverse:file:length ----
    println!("\n--- maxMessagesPerPoll + reverse sort demo ---");
    let batch_dir = base_dir.join("batch");
    std::fs::create_dir_all(&batch_dir)?;

    // Create files of varying sizes
    for i in 1..=5 {
        let content = "x".repeat(i * 20);
        std::fs::write(batch_dir.join(format!("file{}.txt", i)), content)?;
    }

    let mut ctx2 = CamelContext::builder().build().await.unwrap(); // allow-unwrap
    ctx2.register_component(FileComponent::new());

    let batch_path = batch_dir.to_str().unwrap(); // allow-unwrap

    let route2 = RouteBuilder::from(&format!(
        "file:{}?sortBy=reverse:file:length&maxMessagesPerPoll=3&initialDelay=0&delay=1000&delete=true",
        batch_path
    ))
    .route_id("p1-batch")
    .process(|exchange: camel_api::Exchange| {
        Box::pin(async move {
            let filename = exchange
                .input
                .header("CamelFileNameOnly")
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            let len = exchange
                .input
                .header("CamelFileLength")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            println!("  [batch] {} ({} bytes)", filename, len);
            Ok(exchange)
        })
    })
    .build()?;

    ctx2.add_route_definition(route2).await?;
    ctx2.start().await?;

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    println!("\n=== P1 Features Demo Complete ===");
    println!("Key features demonstrated:");
    println!("  - doneFileName: only files with .done marker are processed");
    println!("  - includeExt: only .txt and .csv extensions accepted");
    println!("  - sortBy=file:modified: oldest files processed first");
    println!("  - sortBy=reverse:file:length: largest files processed first");
    println!("  - maxMessagesPerPoll: limited to 3 files per poll");
    println!("  - fileExist=TryRename: atomic temp-file-then-rename writes");

    ctx.stop().await?;
    ctx2.stop().await?;

    Ok(())
}
