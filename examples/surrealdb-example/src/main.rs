//! SurrealDB multi-model example for rust-camel.
//!
//! Demonstrates the three pillars of SurrealDB through Camel routes:
//!   1. Document CRUD  — timer -> create user -> select users -> log
//!   2. Graph (RELATE)  — timer -> relate user->knows->user -> log
//!   3. Vector search   — timer -> store embedding -> KNN search -> log
//!
//! A SurrealDB instance is started automatically via testcontainers (requires Docker).
//!
//!   cargo run -p surrealdb-example
//!
//! Press Ctrl+C to stop.

use std::borrow::Cow;
use std::collections::HashMap;

use camel_api::CamelError;
use camel_api::datasource::{DatasourceCatalog, DatasourceConfig};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::bundle::ComponentBundle;
use camel_component_log::LogComponent;
use camel_component_surrealdb::SurrealDbBundle;
use camel_component_surrealdb::pool_factory::SurrealDbPoolFactory;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_core::datasource::RuntimeDatasourceCatalog;
use testcontainers::core::WaitFor;
use testcontainers::{Image, runners::AsyncRunner};
use toml::Value as TomlValue;

const NS: &str = "demo";
const DB: &str = "demo";

#[derive(Debug)]
struct SurrealDbImage;

impl Image for SurrealDbImage {
    fn name(&self) -> &str {
        "surrealdb/surrealdb"
    }
    fn tag(&self) -> &str {
        "v3.1.4"
    }
    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stdout("Started web server")]
    }
    fn cmd(&self) -> impl IntoIterator<Item = impl Into<Cow<'_, str>>> {
        vec![
            "start".to_string(),
            "--user".to_string(),
            "root".to_string(),
            "--pass".to_string(),
            "root".to_string(),
            "--bind".to_string(),
            "0.0.0.0:8000".to_string(),
        ]
    }
}

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    println!("Starting SurrealDB via testcontainers (requires Docker)...");
    let container = SurrealDbImage
        .start()
        .await
        .expect("Failed to start SurrealDB container"); // allow-unwrap
    let port = container
        .get_host_port_ipv4(8000_u16)
        .await
        .expect("Failed to get SurrealDB port"); // allow-unwrap
    let endpoint = format!("ws://127.0.0.1:{port}");
    println!("SurrealDB available at {endpoint}\n");

    let mut extra = HashMap::new();
    extra.insert("namespace".into(), TomlValue::String(NS.into()));
    extra.insert("database".into(), TomlValue::String(DB.into()));
    extra.insert("username".into(), TomlValue::String("root".into()));
    extra.insert("password".into(), TomlValue::String("root".into()));

    let ds_config = DatasourceConfig {
        db_url: endpoint,
        provider: Some("surrealdb".into()),
        max_connections: None,
        min_connections: None,
        idle_timeout_secs: None,
        max_lifetime_secs: None,
        ssl_mode: None,
        ssl_root_cert: None,
        ssl_cert: None,
        ssl_key: None,
        extra,
    };

    let mut datasources = HashMap::new();
    datasources.insert("demo".to_string(), ds_config);
    let catalog = RuntimeDatasourceCatalog::new(datasources);
    catalog
        .register_factory("surrealdb", std::sync::Arc::new(SurrealDbPoolFactory))
        .expect("register factory");

    let bundle = SurrealDbBundle::default().with_catalog(std::sync::Arc::new(catalog));

    let mut ctx = CamelContext::builder().build().await?;
    SurrealDbBundle::register_all(bundle, &mut ctx);
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    println!("=== Route 1: Document CRUD ===");
    println!("  timer -> surrealdb:create users -> log\n");

    let create_route = RouteBuilder::from("timer:tick?period=2000&repeatCount=1")
        .route_id("create-user")
        .set_body(serde_json::json!({"name": "Alice", "age": 30}))
        .to("surrealdb:create?datasource=demo&table=users")
        .to("log:info?showBody=true")
        .build()?;

    let select_route = RouteBuilder::from("timer:tick?period=2000&repeatCount=1&delay=500")
        .route_id("select-users")
        .to("surrealdb:select?datasource=demo&table=users")
        .to("log:info?showBody=true")
        .build()?;

    ctx.add_route_definition(create_route).await?;
    ctx.add_route_definition(select_route).await?;

    println!("=== Route 2: Graph RELATE ===");
    println!("  timer -> surrealdb:relate user->knows->user -> log\n");

    let relate_route = RouteBuilder::from("timer:tick?period=2000&repeatCount=1&delay=1500")
        .route_id("relate-users")
        .set_body(serde_json::json!({"since": "2024-01-15"}))
        .to("surrealdb:relate?datasource=demo&table=users&edge=knows&from=users:alice&to=users:bob")
        .to("log:info?showBody=true")
        .build()?;

    ctx.add_route_definition(relate_route).await?;

    println!("=== Route 3: Vector Search ===");
    println!("  timer -> surrealdb:vector store -> surrealdb:search KNN -> log\n");

    let vector_route = RouteBuilder::from("timer:tick?period=2000&repeatCount=1&delay=2000")
        .route_id("store-embedding")
        .set_body(serde_json::json!({"text": "hello world", "embedding": [0.1, 0.2, 0.3, 0.4]}))
        .to("surrealdb:vector?datasource=demo&table=docs&vector_field=embedding")
        .to("log:info?showBody=true")
        .build()?;

    let search_route = RouteBuilder::from("timer:tick?period=2000&repeatCount=1&delay=2500")
        .route_id("search-similar")
        .set_body(serde_json::json!([0.15, 0.25, 0.35, 0.45]))
        .to("surrealdb:search?datasource=demo&table=docs&top_k=3&metric=cosine&vector_field=embedding")
        .to("log:info?showBody=true")
        .build()?;

    ctx.add_route_definition(vector_route).await?;
    ctx.add_route_definition(search_route).await?;

    println!("\nStarting SurrealDB example... Press Ctrl+C to stop.\n");

    ctx.start().await?;

    tokio::signal::ctrl_c().await.ok();
    println!("\nShutting down...");
    ctx.stop().await?;
    println!("SurrealDB example stopped.");
    Ok(())
}
