//! SQL Component Example
//!
//! Demonstrates both producer and consumer patterns using SQLite:
//! 1. Producer: Timer triggers INSERT operations via SQL endpoint
//! 2. Consumer: Polls for unprocessed rows, logs them, marks as processed via onConsume
//!
//! The example uses an in-memory SQLite database.

use camel_api::{Body, CamelError};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_sql::SqlComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_processor::LogLevel;
use serde_json::json;
use sqlx::sqlite::SqlitePoolOptions;

/// Database URL for in-memory SQLite (shared cache for cross-connection access)
const DB_URL: &str = "sqlite:file:memdb?mode=memory&cache=shared";

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    // Install sqlx database drivers for the 'any' pool to work
    sqlx::any::install_default_drivers();

    tracing_subscriber::fmt().with_target(false).init();

    println!("=== SQL Component Example ===\n");

    // -------------------------------------------------------------------------
    // Step 1: Set up SQLite database with users table
    // -------------------------------------------------------------------------
    println!("Setting up in-memory SQLite database...");

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(DB_URL)
        .await
        .expect("Failed to connect to SQLite");

    sqlx::query(
        r#"
        CREATE TABLE users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            email TEXT NOT NULL,
            processed INTEGER DEFAULT 0
        )
        "#,
    )
    .execute(&pool)
    .await
    .expect("Failed to create table");

    // Insert seed data
    sqlx::query(
        "INSERT INTO users (name, email, processed) VALUES ('Alice', 'alice@example.com', 0)",
    )
    .execute(&pool)
    .await
    .expect("Failed to insert seed data 1");

    sqlx::query("INSERT INTO users (name, email, processed) VALUES ('Bob', 'bob@example.com', 0)")
        .execute(&pool)
        .await
        .expect("Failed to insert seed data 2");

    println!("Created 'users' table with 2 seed rows.\n");

    // -------------------------------------------------------------------------
    // Step 2: Create CamelContext and register components
    // -------------------------------------------------------------------------
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(SqlComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(TimerComponent::new());

    // -------------------------------------------------------------------------
    // Step 3: Producer route - Timer triggers INSERT every 3 seconds
    // -------------------------------------------------------------------------
    println!("Creating producer route (timer -> sql INSERT)...");

    let producer_uri = format!(
        "sql:INSERT INTO users (name, email, processed) VALUES (#, #, 0)?db_url={}",
        DB_URL
    );

    let producer_route = RouteBuilder::from("timer:sql-producer?period=3000&repeatCount=3")
        .route_id("sql-producer-route")
        // Set body to JSON array of parameters for the INSERT placeholders (#, #)
        .set_body_fn(|ex| {
            // Generate a name based on timer count (if available) or use default
            let count = ex
                .input
                .header("CamelTimerCounter")
                .and_then(|v: &camel_api::Value| v.as_i64())
                .unwrap_or(0);
            let name = format!("User{}", count + 3);
            let email = format!("{}@example.com", name.to_lowercase());
            Body::Json(json!([name, email]))
        })
        .to(producer_uri.as_str())
        .log("Inserted user from timer", LogLevel::Info)
        .build()?;

    ctx.add_route_definition(producer_route).await?;

    // -------------------------------------------------------------------------
    // Step 4: Consumer route - Poll for unprocessed users, mark as processed
    // -------------------------------------------------------------------------
    println!("Creating consumer route (sql SELECT -> log -> onConsume UPDATE)...\n");

    let consumer_uri = format!(
        "sql:SELECT * FROM users WHERE processed = 0?db_url={}&delay=2000&initialDelay=1000&onConsume=UPDATE users SET processed = 1 WHERE id = :#id",
        DB_URL
    );

    let consumer_route = RouteBuilder::from(consumer_uri.as_str())
        .route_id("sql-consumer-route")
        .log("Consumed user row", LogLevel::Info)
        .build()?;

    ctx.add_route_definition(consumer_route).await?;

    // -------------------------------------------------------------------------
    // Step 5: Start the context and run for a few seconds
    // -------------------------------------------------------------------------
    println!("Starting CamelContext...\n");
    ctx.start().await?;

    println!("SQL Example running:");
    println!("  - Producer: Timer inserts a new user every 3 seconds (3 times)");
    println!("  - Consumer: Polls for unprocessed users every 2 seconds, marks them processed");
    println!();
    println!("Watch the logs to see SQL operations. Press Ctrl+C to stop early.\n");

    // Wait for Ctrl+C or timeout after ~12 seconds
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            println!("\nReceived Ctrl+C...");
        }
        _ = tokio::time::sleep(std::time::Duration::from_secs(12)) => {
            println!("\nDemo timeout reached...");
        }
    }

    // -------------------------------------------------------------------------
    // Step 6: Show final state
    // -------------------------------------------------------------------------
    println!("\nFinal database state:");
    let rows: Vec<(i64, String, String, i64)> =
        sqlx::query_as("SELECT id, name, email, processed FROM users ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("Failed to fetch final state");

    for (id, name, email, processed) in rows {
        println!(
            "  id={}, name='{}', email='{}', processed={}",
            id, name, email, processed
        );
    }

    // -------------------------------------------------------------------------
    // Step 7: Clean shutdown
    // -------------------------------------------------------------------------
    println!("\nShutting down...");
    ctx.stop().await?;
    println!("SQL Example stopped cleanly.");

    Ok(())
}
