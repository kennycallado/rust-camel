//! # ZIP Splitter Example
//!
//! Demonstrates ZIP data format and streaming splitter in rust-camel:
//!
//! 1. **ZipDataFormat** — marshal text into a single-entry ZIP, unmarshal back
//! 2. **ZipSplitter** — split a multi-entry ZIP into one exchange per entry
//! 3. **StreamingSplitterService** — process entries sequentially with aggregation
//!
//! Flow:
//!   timer -> marshal to ZIP -> unmarshal (single-entry roundtrip)
//!   timer -> create multi-entry ZIP -> split entries -> log per-entry
//!   -> log aggregated result

use bytes::Bytes;
use camel_api::body::Body;
use camel_api::data_format::DataFormat;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::splitter::AggregationStrategy;
use camel_api::{BoxProcessor, BoxProcessorExt, CamelError, Exchange, Message};
use camel_builder::StepAccumulator;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_processor::data_format::ZipDataFormat;
use camel_processor::log::LogLevel;
use camel_processor::streaming_splitter::StreamingSplitterService;
use camel_processor::zip_splitter::{
    CAMEL_ZIP_ENTRY_INDEX, CAMEL_ZIP_ENTRY_NAME, CAMEL_ZIP_ENTRY_PATH, CAMEL_ZIP_ENTRY_SIZE,
    ZipSplitConfig, zip_splitter,
};
use std::io::Write;

fn create_multi_entry_zip() -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("hello.txt", options).unwrap(); // allow-unwrap
        writer.write_all(b"Hello from ZIP!").unwrap(); // allow-unwrap
        writer.start_file("data/report.csv", options).unwrap(); // allow-unwrap
        writer
            .write_all(b"name,amount\nalice,100\nbob,200")
            .unwrap(); // allow-unwrap
        writer.start_file("config.json", options).unwrap(); // allow-unwrap
        writer
            .write_all(br#"{"version":"1.0","debug":true}"#)
            .unwrap(); // allow-unwrap
        writer.finish().unwrap(); // allow-unwrap
    }
    buf
}

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::builder().build().await?;

    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let df = ZipDataFormat::default();

    // =========================================================================
    // Route 1: Single-entry ZIP roundtrip (marshal -> unmarshal)
    // =========================================================================
    let compressed = df.marshal(Body::Text("compress me!".to_string()))?;
    let compressed_size = match &compressed {
        Body::Bytes(b) => b.len(),
        _ => 0,
    };
    let decompressed = df.unmarshal(compressed)?;
    match &decompressed {
        Body::Bytes(b) => println!(
            "Route 1: Roundtrip OK — marshal Text -> ZIP ({} bytes) -> unmarshal -> Bytes ({} bytes)",
            compressed_size,
            b.len()
        ),
        other => println!("Route 1: Unexpected body type: {other:?}"),
    }

    // =========================================================================
    // Route 2: Multi-entry ZIP split with StreamingSplitterService
    // =========================================================================
    let zip_data = create_multi_entry_zip();
    println!(
        "\nRoute 2: Created multi-entry ZIP ({} bytes)",
        zip_data.len()
    );
    println!("  Entries: hello.txt, data/report.csv, config.json\n");

    let config = ZipSplitConfig {
        max_entries: 100,
        max_total_decompressed_size: 10 * 1024 * 1024,
        max_per_entry_size: 5 * 1024 * 1024,
        ..Default::default()
    };
    let expression = zip_splitter(config);

    let log_pipeline = BoxProcessor::from_fn(|ex: Exchange| {
        Box::pin(async move {
            let name = ex
                .input
                .headers
                .get(CAMEL_ZIP_ENTRY_NAME)
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let path = ex
                .input
                .headers
                .get(CAMEL_ZIP_ENTRY_PATH)
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let size = ex
                .input
                .headers
                .get(CAMEL_ZIP_ENTRY_SIZE)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let idx = ex
                .input
                .headers
                .get(CAMEL_ZIP_ENTRY_INDEX)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let body_preview = match &ex.input.body {
                Body::Bytes(b) => {
                    let s = String::from_utf8_lossy(b);
                    if s.len() > 50 {
                        format!("{}...", &s[..50])
                    } else {
                        s.to_string()
                    }
                }
                _ => "(empty)".to_string(),
            };
            println!(
                "  Entry #{idx}: path=\"{path}\" name=\"{name}\" size={size} body=\"{body_preview}\""
            );
            Ok(ex)
        })
    });

    let svc = StreamingSplitterService::new(
        expression,
        log_pipeline,
        AggregationStrategy::LastWins,
        true,
    );

    let input_exchange = Exchange::new(Message::new(Body::Bytes(Bytes::from(zip_data))));
    let result = tower::ServiceExt::oneshot(svc, input_exchange).await;

    match result {
        Ok(_) => println!("\n  ZIP split completed successfully."),
        Err(e) => println!("\n  ZIP split failed: {e}"),
    }

    // =========================================================================
    // Route 3: ZipDataFormat in a Camel route (timer-driven)
    // =========================================================================
    let route = camel_builder::RouteBuilder::from("timer:zip-marshal?period=3000&repeatCount=2")
        .route_id("zip-marshal-route")
        .set_body("payload for ZIP compression")
        .log("Route 3: Original body", LogLevel::Info)
        .marshal("zip")?
        .log("Route 3: After marshal to ZIP", LogLevel::Info)
        .unmarshal("zip")?
        .log(
            "Route 3: After unmarshal from ZIP (round-trip!)",
            LogLevel::Info,
        )
        .to("log:zip-result?showBody=true")
        .error_handler(ErrorHandlerConfig::log_only())
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("\n+==========================================================+");
    println!("|        ZIP Splitter Example - Running                    |");
    println!("+==========================================================+");
    println!("|  Route 3: timer -> marshal zip -> unmarshal zip          |");
    println!("+==========================================================+");
    println!("|  Press Ctrl+C to stop                                    |");
    println!("+==========================================================+");

    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;

    println!("Context stopped. Goodbye!");
    Ok(())
}
