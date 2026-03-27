//! SQL Streaming Example
//!
//! Demonstrates the StreamList output type for memory-efficient processing
//! of large result sets using NDJSON streaming.
//!
//! Comparison:
//! - SelectList: Loads ALL rows into memory as JSON array
//! - StreamList: Streams rows on-demand as NDJSON (one JSON object per line)

use camel_api::{Body, CamelError, Exchange, StreamBody};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_sql::SqlComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use futures::StreamExt;
use sqlx::sqlite::SqlitePoolOptions;

const DB_URL: &str = "sqlite:file:memdb2?mode=memory&cache=shared";
const ROW_COUNT: usize = 500;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    sqlx::any::install_default_drivers();
    tracing_subscriber::fmt().with_target(false).init();

    println!("=== SQL Streaming Example ===\n");
    println!("This demo shows the difference between SelectList and StreamList\n");

    // -------------------------------------------------------------------------
    // Step 1: Set up database with many rows
    // -------------------------------------------------------------------------
    println!("Setting up SQLite with {} rows...", ROW_COUNT);

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(DB_URL)
        .await
        .expect("Failed to connect to SQLite");

    sqlx::query(
        r#"
        CREATE TABLE events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            value INTEGER NOT NULL,
            created_at TEXT DEFAULT CURRENT_TIMESTAMP
        )
        "#,
    )
    .execute(&pool)
    .await
    .expect("Failed to create table");

    // Insert rows in batches
    for batch in 0..10 {
        let batch_size = ROW_COUNT / 10;
        let start_id = batch * batch_size;
        for i in 0..batch_size {
            let name = format!("event_{}", start_id + i);
            let value = (start_id + i) as i64;
            sqlx::query("INSERT INTO events (name, value) VALUES (?, ?)")
                .bind(&name)
                .bind(value)
                .execute(&pool)
                .await
                .expect("Failed to insert");
        }
        print!(".");
        std::io::Write::flush(&mut std::io::stdout()).ok();
    }
    println!(" Done!\n");

    // -------------------------------------------------------------------------
    // Step 2: Create CamelContext
    // -------------------------------------------------------------------------
    let mut ctx = CamelContext::new();
    ctx.register_component(SqlComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(TimerComponent::new());

    // -------------------------------------------------------------------------
    // Demo 1: SelectList route - loads all into memory
    // -------------------------------------------------------------------------
    println!("┌─────────────────────────────────────────────────────────────┐");
    println!("│ Demo 1: SelectList - Loads ALL rows into memory            │");
    println!("└─────────────────────────────────────────────────────────────┘\n");

    let select_list_uri = format!(
        "sql:SELECT * FROM events ORDER BY id LIMIT 20?db_url={}&outputType=SelectList",
        DB_URL
    );

    let select_route = RouteBuilder::from("timer:select-demo?repeatCount=1")
        .route_id("select-list-route")
        .to(select_list_uri.as_str())
        .process(|ex: Exchange| async move {
            if let Body::Json(json) = &ex.input.body
                && let Some(arr) = json.as_array()
            {
                println!("  Result: JSON array with {} rows", arr.len());
                println!("  First 3 rows:");
                for (i, row) in arr.iter().take(3).enumerate() {
                    println!("    [{}] {}", i, serde_json::to_string(row).unwrap());
                }
                println!("  ... (all {} rows loaded in memory)\n", arr.len());
            }
            Ok(ex)
        })
        .build()?;
    ctx.add_route_definition(select_route).await?;

    // -------------------------------------------------------------------------
    // Demo 2: StreamList route - lazy streaming
    // -------------------------------------------------------------------------
    println!("┌─────────────────────────────────────────────────────────────┐");
    println!("│ Demo 2: StreamList - Streams rows on-demand (NDJSON)       │");
    println!("└─────────────────────────────────────────────────────────────┘\n");

    let stream_list_uri = format!(
        "sql:SELECT * FROM events ORDER BY id LIMIT 20?db_url={}&outputType=StreamList",
        DB_URL
    );

    let stream_route = RouteBuilder::from("timer:stream-demo?repeatCount=1&delay=1000")
        .route_id("stream-list-route")
        .to(stream_list_uri.as_str())
        .process(|ex: Exchange| {
            let body = ex.input.body.clone();
            async move {
                if let Body::Stream(StreamBody { stream, metadata }) = body {
                    println!("  Result: Lazy stream");
                    println!("  Content-Type: {:?}", metadata.content_type);
                    println!("  Streaming rows on-demand:\n");

                    let mut stream_guard = stream.lock().await;
                    if let Some(mut s) = stream_guard.take() {
                        for i in 0..5 {
                            if let Some(chunk_result) = s.next().await {
                                match chunk_result {
                                    Ok(bytes) => {
                                        let text = String::from_utf8_lossy(&bytes);
                                        let trimmed = text.trim_end_matches('\n');
                                        println!("    [{}] {}", i, trimmed);
                                    }
                                    Err(e) => {
                                        println!("    [{}] Error: {}", i, e);
                                        break;
                                    }
                                }
                            }
                        }

                        let mut remaining = 0u64;
                        while let Some(chunk_result) = s.next().await {
                            if chunk_result.is_ok() {
                                remaining += 1;
                            }
                        }
                        println!("\n  ... ({} more rows streamed)\n", remaining);
                    }
                }
                Ok(ex)
            }
        })
        .build()?;
    ctx.add_route_definition(stream_route).await?;

    // -------------------------------------------------------------------------
    // Start and run
    // -------------------------------------------------------------------------
    println!("Starting routes...\n");
    ctx.start().await?;

    println!("Waiting for routes to complete...\n");
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // -------------------------------------------------------------------------
    // Summary
    // -------------------------------------------------------------------------
    println!("┌─────────────────────────────────────────────────────────────┐");
    println!("│ Summary: When to use each output type                       │");
    println!("└─────────────────────────────────────────────────────────────┘\n");
    println!("  SelectList:");
    println!("    - Use when: Result set is small, need random access");
    println!("    - All rows loaded into memory as JSON array");
    println!("    - ROW_COUNT header available immediately\n");
    println!("  StreamList:");
    println!("    - Use when: Large result sets, memory efficiency matters");
    println!("    - Rows streamed on-demand as NDJSON (application/x-ndjson)");
    println!("    - No ROW_COUNT (unknown until stream exhausted)\n");
    println!("  SelectOne:");
    println!("    - Use when: Query returns single row");
    println!("    - Returns first row only, rest discarded\n");

    // -------------------------------------------------------------------------
    // Cleanup
    // -------------------------------------------------------------------------
    ctx.stop().await?;
    println!("Example completed.");

    Ok(())
}
