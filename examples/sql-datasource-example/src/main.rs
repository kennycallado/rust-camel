//! SQL Datasource Example
//!
//! Demonstrates using named datasources from Camel.toml instead of inline db_url:
//! 1. Producer: Timer triggers INSERT operations via SQL endpoint with datasource=appdb
//! 2. Consumer: Polls for unprocessed rows using datasource=appdb
//!
//! The example uses an in-memory SQLite database configured in Camel.toml.

use camel_api::datasource::{DatasourceCatalog, DatasourceConfig};
use camel_api::{Body, CamelError};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::bundle::ComponentBundle;
use camel_component_log::LogComponent;
use camel_component_sql::SqlBundle;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_core::datasource::RuntimeDatasourceCatalog;
use camel_processor::LogLevel;
use serde_json::json;
use sqlx::sqlite::SqlitePoolOptions;
use std::collections::HashMap;
use std::sync::Arc;

/// Database URL matching the Camel.toml datasource config
const DB_URL: &str = "sqlite:file:memdb?mode=memory&cache=shared";

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    // Install sqlx database drivers for the 'any' pool to work
    sqlx::any::install_default_drivers();

    tracing_subscriber::fmt().with_target(false).init();

    println!("=== SQL Datasource Example ===\n");

    // -------------------------------------------------------------------------
    // Step 1: Set up SQLite database
    // -------------------------------------------------------------------------
    println!("Setting up in-memory SQLite database...");

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(DB_URL)
        .await
        .expect("Failed to connect to SQLite"); // allow-unwrap

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            email TEXT NOT NULL,
            processed INTEGER DEFAULT 0
        )",
    )
    .execute(&pool)
    .await
    .expect("Failed to create table"); // allow-unwrap

    sqlx::query(
        "INSERT INTO users (name, email, processed) VALUES ('Alice', 'alice@example.com', 0)",
    )
    .execute(&pool)
    .await
    .expect("Failed to insert seed data 1"); // allow-unwrap

    sqlx::query("INSERT INTO users (name, email, processed) VALUES ('Bob', 'bob@example.com', 0)")
        .execute(&pool)
        .await
        .expect("Failed to insert seed data 2"); // allow-unwrap

    println!("Created 'users' table with 2 seed rows.\n");

    // -------------------------------------------------------------------------
    // Step 2: Create datasource catalog and wire to SQL component
    // -------------------------------------------------------------------------
    println!("Creating datasource catalog with 'appdb' datasource...");

    let mut datasources = HashMap::new();
    datasources.insert(
        "appdb".into(),
        DatasourceConfig {
            db_url: DB_URL.into(),
            provider: None,
            max_connections: Some(5),
            min_connections: None,
            idle_timeout_secs: None,
            max_lifetime_secs: None,
            ssl_mode: None,
            ssl_root_cert: None,
            ssl_cert: None,
            ssl_key: None,
        },
    );
    let catalog: Arc<dyn DatasourceCatalog> = Arc::new(RuntimeDatasourceCatalog::new(datasources));

    // -------------------------------------------------------------------------
    // Step 3: Create CamelContext and register components
    // -------------------------------------------------------------------------
    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let sql_bundle = SqlBundle::from_toml(toml::Value::Table(toml::map::Map::new())).unwrap(); // allow-unwrap
    let sql_bundle = sql_bundle.with_catalog(Arc::clone(&catalog));
    SqlBundle::register_all(sql_bundle, &mut ctx);

    // -------------------------------------------------------------------------
    // Step 4: Producer route - Timer triggers INSERT using named datasource
    // -------------------------------------------------------------------------
    println!("Creating producer route (timer -> sql INSERT with datasource=appdb)...");

    let producer_uri =
        "sql:INSERT INTO users (name, email, processed) VALUES (#, #, 0)?datasource=appdb";

    let producer_route = RouteBuilder::from("timer:sql-producer?period=3000&repeatCount=3")
        .route_id("sql-producer-route")
        // Set body to JSON array of parameters for the INSERT placeholders (#, #)
        .set_body_fn(|ex| {
            let count = ex
                .input
                .header("CamelTimerCounter")
                .and_then(|v: &camel_api::Value| v.as_i64())
                .unwrap_or(0);
            let name = format!("User{}", count + 3);
            let email = format!("{}@example.com", name.to_lowercase());
            Body::Json(json!([name, email]))
        })
        .to(producer_uri)
        .log("Inserted user from timer", LogLevel::Info)
        .build()?;

    ctx.add_route_definition(producer_route).await?;

    // -------------------------------------------------------------------------
    // Step 5: Consumer route - Poll using named datasource, mark as processed
    // -------------------------------------------------------------------------
    println!("Creating consumer route (SQL poll with datasource=appdb)...\n");

    let consumer_uri = "sql:SELECT * FROM users WHERE processed = 0?datasource=appdb&delay=2000&initialDelay=1000&onConsume=UPDATE users SET processed = 1 WHERE id = :#id";

    let consumer_route = RouteBuilder::from(consumer_uri)
        .route_id("sql-consumer-route")
        .log("Consumed user row", LogLevel::Info)
        .build()?;

    ctx.add_route_definition(consumer_route).await?;

    // -------------------------------------------------------------------------
    // Step 6: Start the context and run for a few seconds
    // -------------------------------------------------------------------------
    println!("Starting CamelContext...\n");
    ctx.start().await?;

    println!("SQL Datasource Example running:");
    println!("  - Producer: Timer inserts a new user every 3 seconds (3 times)");
    println!("  - Consumer: Polls for unprocessed users every 2 seconds");
    println!();
    println!("Watch the logs to see SQL operations. Press Ctrl+C to stop early.\n");

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            println!("\nReceived Ctrl+C...");
        }
        _ = tokio::time::sleep(std::time::Duration::from_secs(12)) => {
            println!("\nDemo timeout reached...");
        }
    }

    // -------------------------------------------------------------------------
    // Step 7: Show final state
    // -------------------------------------------------------------------------
    println!("\nFinal database state:");
    let rows: Vec<(i64, String, String, i64)> =
        sqlx::query_as("SELECT id, name, email, processed FROM users ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("Failed to fetch final state"); // allow-unwrap

    for (id, name, email, processed) in rows {
        println!(
            "  id={}, name='{}', email='{}', processed={}",
            id, name, email, processed
        );
    }

    // -------------------------------------------------------------------------
    // Step 8: Clean shutdown
    // -------------------------------------------------------------------------
    println!("\nShutting down...");
    ctx.stop().await?;
    println!("SQL Datasource Example stopped cleanly.");

    Ok(())
}
