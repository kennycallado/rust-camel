//! # CSV Marshal/Unmarshal Example
//!
//! Demonstrates:
//! 1. CSV text → unmarshal → Body::Json (Map mode default)
//! 2. Body::Json → marshal → CSV text
//! 3. capture_header_record effect on Exchange headers

use camel_api::CamelError;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_processor::LogLevel;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap

    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // =========================================================================
    // Route 1: CSV unmarshal (Text -> Json Map)
    // =========================================================================
    // Demonstrates unmarshalling CSV text into structured Json bodies.
    // The first row is treated as headers; subsequent rows become JSON objects.
    let route_csv_unmarshal = RouteBuilder::from("timer:csv-unmarshal?period=2000&repeatCount=3")
        .route_id("csv-unmarshal")
        .set_body("name,age\nAlice,30\nBob,25")
        .log("Route 1: starting CSV unmarshal", LogLevel::Info)
        .unmarshal("csv")?
        .log("Route 1: unmarshalled CSV -> Json Map", LogLevel::Info)
        .to("log:info?showBody=true")
        .error_handler(ErrorHandlerConfig::log_only())
        .build()?;

    ctx.add_route_definition(route_csv_unmarshal).await?;

    // =========================================================================
    // Route 2: CSV marshal (Json -> Text CSV)
    // =========================================================================
    // Demonstrates marshalling a JSON array into CSV text. The JSON objects
    // are converted to CSV rows with headers derived from object keys.
    let route_csv_marshal = RouteBuilder::from("timer:csv-marshal?period=2000&repeatCount=3")
        .route_id("csv-marshal")
        .set_body(r#"[{"name":"Carol","age":28},{"name":"Dave","age":35}]"#)
        .unmarshal("json")?
        .log("Route 2: starting CSV marshal from Json", LogLevel::Info)
        .marshal("csv")?
        .log("Route 2: marshalled Json -> CSV Text", LogLevel::Info)
        .to("log:info?showBody=true")
        .error_handler(ErrorHandlerConfig::log_only())
        .build()?;

    ctx.add_route_definition(route_csv_marshal).await?;

    // Demonstrate capture_header_record (D5 feature) outside the DSL:
    // The registry default has capture_header_record=false, so we show
    // the hook API directly with a custom CsvDataFormat instance.
    demonstrate_capture_header_record();

    // Start the Camel context
    ctx.start().await?;

    println!("+==============================================================+");
    println!("|        CSV Marshal/Unmarshal Example - Running              |");
    println!("+==============================================================+");
    println!("|  Route 1: Text CSV -> Json Map                               |");
    println!("|  Route 2: Json Map -> Text CSV                               |");
    println!("+==============================================================+");
    println!("|  Press Ctrl+C to stop                                        |");
    println!("+==============================================================+");

    // Wait for Ctrl+C signal
    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;

    println!("Context stopped. Goodbye!");
    Ok(())
}

fn demonstrate_capture_header_record() {
    use camel_api::Exchange;
    use camel_api::body::Body;
    use camel_api::data_format::DataFormat;
    use camel_processor::{CAMEL_CSV_HEADER_RECORD, CsvConfig, CsvDataFormat};

    println!("--- capture_header_record demonstration ---");

    let cfg = CsvConfig::default().capture_header_record(true);
    let df = CsvDataFormat::new(cfg);

    let body = Body::Text("name,age\nAlice,30\nBob,25".into());
    let mut ex = Exchange::default();
    let _ = df.unmarshal_in_exchange(&mut ex, body).unwrap(); // allow-unwrap

    let captured = ex.input.headers.get(CAMEL_CSV_HEADER_RECORD);
    println!("  Input CSV:  name,age\\nAlice,30\\nBob,25");
    println!("  Captured header: {:?}", captured);
    println!("  (populated in exchange.input.headers[CAMEL_CSV_HEADER_RECORD])");
    println!("--- end demonstration ---\n");
}
