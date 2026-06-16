//! Integration tests for the SurrealDB component.
//!
//! Uses testcontainers to spin up a SurrealDB instance.
//!
//! **Requires Docker to be running.** Tests will fail if Docker is unavailable.
//!
//! **Requires `integration-tests` feature to compile and run.**

#![cfg(feature = "integration-tests")]

mod support;

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use camel_api::body::Body;
use camel_api::datasource::{DatasourceCatalog, DatasourceConfig, GetPoolFuture, PoolFactory};
use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::{Component, PollingConsumer};
use camel_component_surrealdb::SurrealDbComponent;
use camel_component_surrealdb::config::SurrealDbEndpointConfig;
use camel_component_surrealdb::polling::SurrealDbPollingConsumer;
use camel_component_surrealdb::pool_factory::SurrealDbPoolFactory;
use camel_core::datasource::RuntimeDatasourceCatalog;
use camel_test::CamelTestContext;
use support::wait::wait_until;
use testcontainers::ContainerAsync;
use testcontainers::Image;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use toml::Value as TomlValue;

// ===========================================================================
// SurrealDB testcontainer image
// ===========================================================================

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

    fn expose_ports(&self) -> &[ContainerPort] {
        static PORTS: [ContainerPort; 1] = [ContainerPort::Tcp(8000)];
        &PORTS
    }
}

// ===========================================================================
// Setup helpers
// ===========================================================================

fn make_datasource_config(endpoint: &str) -> DatasourceConfig {
    let mut extra = HashMap::new();
    extra.insert("namespace".into(), TomlValue::String("test".into()));
    extra.insert("database".into(), TomlValue::String("test".into()));
    extra.insert("username".into(), TomlValue::String("root".into()));
    extra.insert("password".into(), TomlValue::String("root".into()));
    DatasourceConfig {
        db_url: endpoint.to_string(),
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
    }
}

fn make_catalog(endpoint: &str) -> RuntimeDatasourceCatalog {
    let mut configs = HashMap::new();
    configs.insert("test".to_string(), make_datasource_config(endpoint));
    let catalog = RuntimeDatasourceCatalog::new(configs);
    catalog
        .register_factory("surrealdb", Arc::new(SurrealDbPoolFactory))
        .expect("register factory");
    catalog
}

/// Create a direct SurrealDB client for DB-state verification.
async fn direct_client(endpoint: &str) -> surrealdb::Surreal<surrealdb::engine::any::Any> {
    let config = make_datasource_config(endpoint);
    let pool = SurrealDbPoolFactory
        .create(&config)
        .await
        .expect("create direct pool");
    let handle =
        camel_api::datasource::DatasourceHandle::new("direct".into(), "surrealdb".into(), pool);
    let client: Arc<surrealdb::Surreal<surrealdb::engine::any::Any>> =
        handle.downcast().expect("downcast");
    (*client).clone()
}

async fn start_surrealdb() -> (ContainerAsync<SurrealDbImage>, String) {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,camel=info".into()),
        )
        .with_test_writer()
        .try_init();
    let container = SurrealDbImage
        .start()
        .await
        .expect("failed to start surrealdb container");
    let port = container
        .get_host_port_ipv4(8000)
        .await
        .expect("failed to get host port");
    let endpoint = format!("ws://127.0.0.1:{port}");
    eprintln!("SurrealDB endpoint: {endpoint}");
    (container, endpoint)
}

/// Build a CamelTestContext with SurrealDB component wired through the catalog.
async fn setup_harness(endpoint: &str) -> CamelTestContext {
    let catalog = Arc::new(make_catalog(endpoint));
    let component = SurrealDbComponent::with_catalog(catalog);

    CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(component)
        .build()
        .await
}

async fn wait_for_mock_exchanges(
    mock: &camel_component_mock::MockComponent,
    endpoint_name: &str,
    min_count: usize,
) {
    let endpoint = mock
        .get_endpoint(endpoint_name)
        .unwrap_or_else(|| panic!("missing mock endpoint: {endpoint_name}"));

    wait_until(
        "surrealdb mock exchange arrival",
        std::time::Duration::from_secs(10),
        std::time::Duration::from_millis(100),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(endpoint.get_received_exchanges().await.len() >= min_count) }
        },
    )
    .await
    .unwrap();
}

// ===========================================================================
// CRUD Producer tests
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn producer_create() {
    let (_container, endpoint) = start_surrealdb().await;
    let db = direct_client(&endpoint).await;
    let h = setup_harness(&endpoint).await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    // Exercise surrealdb:create producer with a JSON body.
    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(serde_json::json!({"name": "Alice", "age": 30}))
        .to("surrealdb:create?datasource=test&table=users")
        .to("mock:result")
        .route_id("surrealdb-create-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    wait_for_mock_exchanges(h.mock(), "result", 1).await;
    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            let err = &errors[0].error;
            panic!("Route had errors: {:?}", err);
        }
    }

    // Verify the record was persisted by the producer (not via raw query in the route).
    let mut response = db
        .query("SELECT * FROM users")
        .await
        .expect("select failed");
    let users: Vec<serde_json::Value> = response.take(0).expect("take failed");
    assert!(!users.is_empty(), "should have at least one user");
    assert!(
        users[0].get("name").and_then(|v| v.as_str()) == Some("Alice"),
        "name should match"
    );
    assert_eq!(users[0]["age"], 30, "age should match");
}

#[tokio::test(flavor = "multi_thread")]
async fn producer_select_one() {
    let (_container, endpoint) = start_surrealdb().await;
    let db = direct_client(&endpoint).await;

    // Seed a record via direct query (setup only, not part of the tested route).
    db.query("CREATE user:alice SET name = 'Alice', age = 30")
        .await
        .expect("seed failed");

    let h = setup_harness(&endpoint).await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    // Exercise surrealdb:select producer with table + id (returns one record).
    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .to("surrealdb:select?datasource=test&table=user&id=alice")
        .to("mock:result")
        .route_id("surrealdb-select-one-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    wait_for_mock_exchanges(h.mock(), "result", 1).await;
    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint = h.mock().get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;

    // Verify the producer returned the record in the output body.
    let exchanges = endpoint.get_received_exchanges().await;
    if let Some(output) = &exchanges[0].output {
        if let Body::Json(v) = &output.body {
            let arr = v.as_array().expect("select-one result should be array");
            assert_eq!(arr.len(), 1, "should return exactly one record");
            assert_eq!(arr[0]["name"], "Alice", "name from select-one");
            assert_eq!(arr[0]["age"], 30, "age from select-one");
        } else {
            panic!("expected Body::Json on output");
        }
    } else {
        panic!("expected output message");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn producer_select_all() {
    let (_container, endpoint) = start_surrealdb().await;
    let db = direct_client(&endpoint).await;

    // Seed multiple records.
    db.query("CREATE user:bob SET name = 'Bob'")
        .await
        .expect("seed bob");
    db.query("CREATE user:charlie SET name = 'Charlie'")
        .await
        .expect("seed charlie");

    let h = setup_harness(&endpoint).await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    // Exercise surrealdb:select producer WITHOUT id (returns all records).
    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .to("surrealdb:select?datasource=test&table=user")
        .to("mock:result")
        .route_id("surrealdb-select-all-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    wait_for_mock_exchanges(h.mock(), "result", 1).await;
    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint = h.mock().get_endpoint("result").unwrap();
    let exchanges = endpoint.get_received_exchanges().await;
    if let Some(output) = &exchanges[0].output {
        if let Body::Json(v) = &output.body {
            let arr = v.as_array().expect("select-all result should be array");
            assert!(
                arr.len() >= 2,
                "should return at least 2 records, got {}",
                arr.len()
            );
        } else {
            panic!("expected Body::Json on output");
        }
    } else {
        panic!("expected output message");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn producer_query() {
    let (_container, endpoint) = start_surrealdb().await;
    let db = direct_client(&endpoint).await;

    // Seed records directly.
    db.query("CREATE type::record('users', 'bob') SET name = 'Bob', age = 25")
        .await
        .expect("seed bob");
    db.query("CREATE type::record('users', 'charlie') SET name = 'Charlie', age = 35")
        .await
        .expect("seed charlie");

    let h = setup_harness(&endpoint).await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(Value::String(
            "SELECT name, age FROM users ORDER BY name".into(),
        ))
        .to("surrealdb:query?datasource=test")
        .to("mock:result")
        .route_id("surrealdb-query-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    wait_for_mock_exchanges(h.mock(), "result", 1).await;
    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint = h.mock().get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;

    // Verify output body contains query results
    let exchanges = endpoint.get_received_exchanges().await;
    if let Some(output) = &exchanges[0].output {
        if let Body::Json(v) = &output.body {
            let arr = v.as_array().expect("query result should be array");
            assert_eq!(arr.len(), 2, "should have 2 rows");
            assert_eq!(arr[0]["name"], "Bob");
            assert_eq!(arr[1]["name"], "Charlie");
        } else {
            panic!("expected Body::Json on output");
        }
    } else {
        panic!("expected output message");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn producer_update() {
    let (_container, endpoint) = start_surrealdb().await;
    let db = direct_client(&endpoint).await;

    // Seed a record.
    db.query("CREATE user:dave SET name = 'Dave', age = 40")
        .await
        .expect("seed dave");

    let h = setup_harness(&endpoint).await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    // Exercise surrealdb:update producer with a JSON MERGE body.
    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(serde_json::json!({"age": 99}))
        .to("surrealdb:update?datasource=test&table=user&id=dave")
        .to("mock:result")
        .route_id("surrealdb-update-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    wait_for_mock_exchanges(h.mock(), "result", 1).await;
    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    // Verify DB state reflects the MERGE.
    let mut response = db
        .query("SELECT name, age FROM user:dave")
        .await
        .expect("select dave");
    let result: Vec<serde_json::Value> = response.take(0).expect("take failed");
    assert!(!result.is_empty(), "dave should exist");
    assert_eq!(result[0]["age"], 99, "age should be updated to 99");
    assert_eq!(
        result[0]["name"], "Dave",
        "name should be preserved (MERGE)"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn producer_delete() {
    let (_container, endpoint) = start_surrealdb().await;
    let db = direct_client(&endpoint).await;

    // Seed a record.
    db.query("CREATE user:eve SET name = 'Eve', age = 50")
        .await
        .expect("seed eve");

    let h = setup_harness(&endpoint).await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    // Exercise surrealdb:delete producer (no body needed).
    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .to("surrealdb:delete?datasource=test&table=user&id=eve")
        .to("mock:result")
        .route_id("surrealdb-delete-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    wait_for_mock_exchanges(h.mock(), "result", 1).await;
    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    // Verify deletion: eve should not exist.
    let mut response = db
        .query("SELECT * FROM user:eve")
        .await
        .expect("select eve");
    let result: Vec<serde_json::Value> = response.take(0).expect("take failed");
    assert!(result.is_empty(), "eve should be deleted");
}

#[tokio::test(flavor = "multi_thread")]
async fn producer_upsert() {
    let (_container, endpoint) = start_surrealdb().await;
    let db = direct_client(&endpoint).await;

    let h = setup_harness(&endpoint).await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    // First upsert: creates the record (it doesn't exist yet).
    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(serde_json::json!({"name": "Frank", "age": 50}))
        .to("surrealdb:upsert?datasource=test&table=users&id=frank")
        .to("mock:result")
        .route_id("surrealdb-upsert-create")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    wait_for_mock_exchanges(h.mock(), "result", 1).await;
    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    // Verify the record was created.
    let mut response = db
        .query("SELECT name, age FROM users:frank")
        .await
        .expect("select frank");
    let result: Vec<serde_json::Value> = response.take(0).expect("take failed");
    assert!(!result.is_empty(), "frank should exist after upsert");
    assert_eq!(result[0]["name"], "Frank", "name should be Frank");
    assert_eq!(result[0]["age"], 50, "age should be 50");

    // Second upsert: replaces the record content (CONTENT vs MERGE).
    let h2 = setup_harness(&endpoint).await;
    let route2 = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(serde_json::json!({"name": "Frank", "age": 51}))
        .to("surrealdb:upsert?datasource=test&table=users&id=frank")
        .to("mock:result")
        .route_id("surrealdb-upsert-update")
        .build()
        .unwrap();

    h2.add_route(route2).await.unwrap();
    h2.start().await;
    wait_for_mock_exchanges(h2.mock(), "result", 1).await;
    h2.stop().await;

    // Verify the record was updated.
    let mut response = db
        .query("SELECT name, age FROM users:frank")
        .await
        .expect("select frank again");
    let result: Vec<serde_json::Value> = response.take(0).expect("take failed");
    assert!(!result.is_empty(), "frank should still exist");
    assert_eq!(result[0]["age"], 51, "age should be updated to 51");
}

#[tokio::test(flavor = "multi_thread")]
async fn producer_patch() {
    let (_container, endpoint) = start_surrealdb().await;
    let db = direct_client(&endpoint).await;

    // Seed a record.
    db.query("CREATE user:grace SET name = 'Grace', age = 25, city = 'NYC'")
        .await
        .expect("seed grace");

    let h = setup_harness(&endpoint).await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    // Apply RFC 6902 patch: replace age, add email, remove city.
    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(serde_json::json!([
            {"op": "replace", "path": "/age", "value": 26},
            {"op": "add", "path": "/email", "value": "grace@example.com"},
            {"op": "remove", "path": "/city"}
        ]))
        .to("surrealdb:patch?datasource=test&table=user&id=grace")
        .to("mock:result")
        .route_id("surrealdb-patch-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    wait_for_mock_exchanges(h.mock(), "result", 1).await;
    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    // Verify the patch was applied.
    let mut response = db
        .query("SELECT name, age, email, city FROM user:grace")
        .await
        .expect("select grace");
    let result: Vec<serde_json::Value> = response.take(0).expect("take failed");
    assert!(!result.is_empty(), "grace should exist");
    assert_eq!(result[0]["age"], 26, "age should be patched to 26");
    assert_eq!(
        result[0]["email"],
        serde_json::json!("grace@example.com"),
        "email should be added"
    );
    assert!(
        result[0].get("city").is_none() || result[0]["city"].is_null(),
        "city should be removed"
    );
    assert_eq!(result[0]["name"], "Grace", "name should be unchanged");
}

#[tokio::test(flavor = "multi_thread")]
async fn producer_run() {
    let (_container, endpoint) = start_surrealdb().await;
    let db = direct_client(&endpoint).await;

    // Define a custom SurrealDB function.
    db.query("DEFINE FUNCTION fn::add($a: int, $b: int) { RETURN $a + $b }")
        .await
        .expect("define function");

    let h = setup_harness(&endpoint).await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    // Run the user-defined function with positional args.
    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(serde_json::json!([17, 25]))
        .to("surrealdb:run?datasource=test&function=fn%3A%3Aadd")
        .to("mock:result")
        .route_id("surrealdb-run-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    wait_for_mock_exchanges(h.mock(), "result", 1).await;
    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    // Verify the mock received the function result (17 + 25 = 42).
    let result_ep = h.mock().get_endpoint("result").expect("result endpoint");
    let exchanges = result_ep.get_received_exchanges().await;
    assert!(!exchanges.is_empty(), "should have received the result");
    let output = exchanges[0]
        .output
        .as_ref()
        .expect("expected output message");
    let result_value = match &output.body {
        Body::Json(v) => v.clone(),
        Body::Text(s) => serde_json::from_str(s).expect("parse body as json"),
        other => panic!("unexpected body type: {other:?}"),
    };
    assert_eq!(
        result_value.as_f64(),
        Some(42.0),
        "fn::add(17, 25) should return 42, got: {result_value}"
    );
}

// ===========================================================================
// Relate test
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn producer_relate() {
    let (_container, endpoint) = start_surrealdb().await;
    let db = direct_client(&endpoint).await;

    // Seed source and target records.
    db.query("CREATE user:1 SET name = 'Alice'")
        .await
        .expect("create alice");
    db.query("CREATE topic:42 SET title = 'Rust'")
        .await
        .expect("create topic");

    let h = setup_harness(&endpoint).await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    // Exercise surrealdb:relate producer with JSON body for edge properties.
    // `from` and `to` are full RecordIds (`table:key`) — see README.
    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(serde_json::json!({"weight": 0.9}))
        .to("surrealdb:relate?datasource=test&table=user&from=user:1&edge=knows&to=topic:42")
        .to("mock:result")
        .route_id("surrealdb-relate-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    wait_for_mock_exchanges(h.mock(), "result", 1).await;
    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    // Verify via direct traversal that the edge was created with correct in/out.
    let mut response = db
        .query("SELECT ->knows->topic.title AS topics FROM user:1")
        .await
        .expect("traverse failed");
    let results: Vec<serde_json::Value> = response.take(0).expect("take failed");
    assert_eq!(results.len(), 1, "should return traversal result");
    let topics = &results[0]["topics"];
    assert!(topics.is_array(), "topics should be an array");
    assert!(
        topics.as_array().unwrap().iter().any(|v| v == "Rust"),
        "traversal should reach topic:42"
    );
}

/// Regression for the RELATE body.id + CamelSurrealDbRecordId contract
/// (Follow-ups #2 + #3):
///
/// The producer's output body `id` MUST be the EDGE record's id (table `knows`),
/// never the source node (`user:1`) or the target (`topic:42`). And, per
/// Option B, the `CamelSurrealDbRecordId` header MUST be set to that same
/// edge id (extracted from `body.id`).
///
/// Handles both SurrealDB RecordId serializations: a flat `table:key` string
/// or a `{"tb":..,"id":..}` object.
#[tokio::test(flavor = "multi_thread")]
async fn producer_relate_body_and_header_carry_edge_id() {
    let (_container, endpoint) = start_surrealdb().await;
    let db = direct_client(&endpoint).await;

    db.query("CREATE user:1 SET name = 'Alice'")
        .await
        .expect("create alice");
    db.query("CREATE topic:42 SET title = 'Rust'")
        .await
        .expect("create topic");

    let h = setup_harness(&endpoint).await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(serde_json::json!({"weight": 0.9}))
        .to("surrealdb:relate?datasource=test&table=user&from=user:1&edge=knows&to=topic:42")
        .to("mock:result")
        .route_id("surrealdb-relate-edge-id-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    wait_for_mock_exchanges(h.mock(), "result", 1).await;
    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint_ep = h.mock().get_endpoint("result").unwrap();
    endpoint_ep.assert_exchange_count(1).await;
    let exchanges = endpoint_ep.get_received_exchanges().await;
    let output = exchanges[0]
        .output
        .as_ref()
        .expect("expected output message");
    let body = match &output.body {
        Body::Json(v) => v,
        other => panic!("expected Body::Json on relate output, got {other:?}"),
    };

    // Extract the record id from body.id, accepting either the flat string
    // form (`knows:xxx`) or the object form (`{"tb":"knows","id":...}`).
    let id_as_string = |v: &serde_json::Value| -> Option<String> {
        match v {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Object(obj) => {
                let tb = obj.get("tb").and_then(|x| x.as_str())?;
                let key = match obj.get("id")? {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    _ => return None,
                };
                Some(format!("{tb}:{key}"))
            }
            _ => None,
        }
    };

    let body_id = body
        .get("id")
        .and_then(|v| id_as_string(v))
        .expect("body.id must be present as a RecordId");

    // The edge record lives in the `knows` table, never the source/target.
    assert!(
        body_id.starts_with("knows:"),
        "body.id must be an edge record id (knows:...), got: {body_id}"
    );
    assert_ne!(body_id, "user:1", "body.id must NOT be the source node id");
    assert_ne!(
        body_id, "topic:42",
        "body.id must NOT be the target node id"
    );

    // Option B: CamelSurrealDbRecordId header MUST be set to the same edge id.
    let header = output
        .headers
        .get("CamelSurrealDbRecordId")
        .and_then(|v| v.as_str())
        .expect("CamelSurrealDbRecordId header must be set for relate (Option B)");
    assert_eq!(
        header, body_id,
        "CamelSurrealDbRecordId header must equal the edge record body.id"
    );
}

// ===========================================================================
// Vector tests
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn vector_search_via_producer() {
    let (_container, endpoint) = start_surrealdb().await;
    let db = direct_client(&endpoint).await;

    // Define HNSW index for 3D float vectors with cosine distance.
    db.query(
        "DEFINE INDEX IF NOT EXISTS idx_emb \
         ON TABLE embeddings FIELDS embedding HNSW DIMENSION 3 DIST COSINE TYPE F64",
    )
    .await
    .expect("define HNSW index");

    // Seed vector data directly.
    db.query("CREATE embeddings SET embedding = [0.1, 0.2, 0.3], content = 'doc1'")
        .await
        .expect("create doc1");
    db.query("CREATE embeddings SET embedding = [0.9, 0.8, 0.7], content = 'doc2'")
        .await
        .expect("create doc2");
    db.query("CREATE embeddings SET embedding = [0.15, 0.25, 0.35], content = 'doc3'")
        .await
        .expect("create doc3");

    let h = setup_harness(&endpoint).await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    // Exercise surrealdb:search producer: body carries the query vector.
    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(serde_json::json!({"vector": [0.1, 0.2, 0.3]}))
        .to("surrealdb:search?datasource=test&table=embeddings&vector_field=embedding&top_k=2&metric=COSINE")
        .to("mock:result")
        .route_id("surrealdb-search-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    wait_for_mock_exchanges(h.mock(), "result", 1).await;
    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint = h.mock().get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;

    // The search producer returns the top-K nearest neighbours.
    let exchanges = endpoint.get_received_exchanges().await;
    if let Some(output) = &exchanges[0].output {
        if let Body::Json(v) = &output.body {
            let arr = v.as_array().expect("search result should be array");
            assert_eq!(arr.len(), 2, "KNN top_k=2 should return 2 results");
            // The nearest neighbour to [0.1, 0.2, 0.3] is doc1 (exact match).
            assert_eq!(
                arr[0]["content"], "doc1",
                "nearest neighbour should be doc1"
            );
        } else {
            panic!("expected Body::Json on output");
        }
    } else {
        panic!("expected output message");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn vector_store_via_producer() {
    let (_container, endpoint) = start_surrealdb().await;
    let db = direct_client(&endpoint).await;

    // Define HNSW index first.
    db.query(
        "DEFINE INDEX IF NOT EXISTS idx_vec \
         ON TABLE vectors FIELDS embedding HNSW DIMENSION 2 DIST COSINE TYPE F64",
    )
    .await
    .expect("define index");

    let h = setup_harness(&endpoint).await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    // Exercise surrealdb:vector producer: body carries fields including `embedding`.
    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(serde_json::json!({
            "embedding": [0.5, 0.5],
            "content": "stored-via-producer"
        }))
        .to("surrealdb:vector?datasource=test&table=vectors&vector_field=embedding")
        .to("mock:result")
        .route_id("surrealdb-vector-store-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    wait_for_mock_exchanges(h.mock(), "result", 1).await;
    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    // Verify the vector was persisted by the producer.
    let mut response = db
        .query("SELECT content, embedding FROM vectors WHERE content = 'stored-via-producer'")
        .await
        .expect("select stored vector");
    let results: Vec<serde_json::Value> = response.take(0).expect("take failed");
    assert!(
        !results.is_empty(),
        "vector should have been stored via producer"
    );
    assert_eq!(
        results[0]["embedding"],
        serde_json::json!([0.5, 0.5]),
        "embedding should match"
    );
}

// ===========================================================================
// PollingConsumer test
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn polling_consumer_select() {
    let (_container, endpoint) = start_surrealdb().await;
    let db = direct_client(&endpoint).await;

    // Seed a record using type::record (v3 syntax).
    db.query("CREATE type::record('docs', 'poll-test') SET title = 'Polling Test', value = 42")
        .await
        .expect("seed doc");

    // Build catalog with pool factory and get a pool.
    let catalog: Arc<dyn DatasourceCatalog> = Arc::new(make_catalog(&endpoint));
    let _handle = catalog.get_pool("test").await.expect("get pool");

    // Build polling consumer for select-by-ID
    let endpoint_config = SurrealDbEndpointConfig::from_uri(
        "surrealdb:select?datasource=test&table=docs&id=poll-test",
    )
    .expect("parse URI");

    let mut polling = SurrealDbPollingConsumer::new(endpoint_config, Some(Arc::clone(&catalog)));

    let result = polling
        .receive(std::time::Duration::from_secs(5))
        .await
        .expect("receive should succeed");
    assert!(result.is_some(), "polling consumer should find the record");

    let exchange = result.unwrap();
    match &exchange.input.body {
        Body::Json(json) => {
            // PollingConsumer unwraps the first result element, so body is the record object
            assert_eq!(
                json["title"], "Polling Test",
                "body should contain the record title"
            );
            assert_eq!(json["value"], 42, "body should contain the record value");
        }
        _ => panic!("expected Body::Json, got {:?}", exchange.input.body),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn polling_consumer_returns_none_when_empty() {
    let (_container, endpoint) = start_surrealdb().await;
    let db = direct_client(&endpoint).await;

    // Ensure table exists (type::record requires existing table in v3).
    db.query("CREATE docs SET _init = true")
        .await
        .expect("create init record");
    db.query("DELETE docs WHERE _init = true")
        .await
        .expect("delete init record");

    // Build catalog with pool factory.
    let catalog: Arc<dyn DatasourceCatalog> = Arc::new(make_catalog(&endpoint));
    let _handle = catalog.get_pool("test").await.expect("get pool");

    // Query non-existent record by ID
    let endpoint_config = SurrealDbEndpointConfig::from_uri(
        "surrealdb:select?datasource=test&table=docs&id=nonexistent",
    )
    .expect("parse URI");

    let mut polling = SurrealDbPollingConsumer::new(endpoint_config, Some(Arc::clone(&catalog)));

    let result = polling
        .receive(std::time::Duration::from_secs(5))
        .await
        .expect("receive should succeed");
    assert!(
        result.is_none(),
        "should return None for nonexistent record"
    );
}

// ===========================================================================
// LIVE SELECT — Consumer test
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn consumer_live_receives_notifications() {
    let (_container, endpoint) = start_surrealdb().await;
    let db = direct_client(&endpoint).await;

    // Per SDK docs (https://surrealdb.com/docs/languages/rust/methods/select-live),
    // the table must be DEFINED before starting a live query. Schemaless CREATE
    // alone does not set up the change-data-capture infrastructure that
    // `select().live()` depends on.
    db.query("DEFINE TABLE IF NOT EXISTS events")
        .await
        .expect("define table");

    // The live consumer route uses ws:// protocol (validated by component).
    let live_uri = format!("surrealdb:live?datasource=test&table=events");

    let h = CamelTestContext::builder()
        .with_mock()
        .with_component(SurrealDbComponent::with_catalog(Arc::new(make_catalog(
            &endpoint,
        ))))
        .build()
        .await;

    let live_route = RouteBuilder::from(live_uri.as_str())
        .to("mock:live-events")
        .route_id("surrealdb-live-test")
        .build()
        .unwrap();

    h.add_route(live_route).await.unwrap();
    h.start().await;

    // Wait for the LIVE SELECT to be established on the server.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Create a record to trigger notification.
    db.query("CREATE events SET name = 'test_live_event'")
        .await
        .expect("create event");

    // Wait for notification to arrive at mock.
    let endpoint = h.mock().get_endpoint("live-events").unwrap();
    wait_until(
        "live consumer receives notification",
        std::time::Duration::from_secs(15),
        std::time::Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;

    let exchanges = endpoint.get_received_exchanges().await;
    assert!(
        !exchanges.is_empty(),
        "Live consumer should have received at least one notification"
    );

    // Check notification body and headers.
    let ex = &exchanges[0];
    let action = ex
        .input
        .header("CamelSurrealDbAction")
        .and_then(|v| v.as_str());
    assert_eq!(
        action,
        Some("CREATE"),
        "notification should be a CREATE action"
    );

    match &ex.input.body {
        Body::Json(v) => {
            assert_eq!(
                v.get("name").and_then(|v| v.as_str()),
                Some("test_live_event"),
                "notification data should contain event name"
            );
        }
        other => panic!("expected Body::Json, got {:?}", other),
    }
}

// ===========================================================================
// Error handling test
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn live_rejects_http_scheme() {
    use std::sync::Mutex;

    struct HttpOnlyCatalog {
        configs: Mutex<HashMap<String, DatasourceConfig>>,
    }

    impl DatasourceCatalog for HttpOnlyCatalog {
        fn get_config(&self, name: &str) -> Option<DatasourceConfig> {
            self.configs.lock().unwrap().get(name).cloned()
        }
        fn get_pool<'a>(&'a self, _name: &'a str) -> GetPoolFuture<'a> {
            Box::pin(async move { Err(CamelError::Config("no pool for http-ds".into())) })
        }
        fn register_factory(
            &self,
            _kind: &str,
            _factory: Arc<dyn PoolFactory>,
        ) -> Result<(), CamelError> {
            Ok(())
        }
    }

    let ds_config = DatasourceConfig {
        db_url: "http://localhost:8000".into(),
        provider: None,
        max_connections: None,
        min_connections: None,
        idle_timeout_secs: None,
        max_lifetime_secs: None,
        ssl_mode: None,
        ssl_root_cert: None,
        ssl_cert: None,
        ssl_key: None,
        extra: HashMap::new(),
    };

    let mut configs = HashMap::new();
    configs.insert("http-ds".to_string(), ds_config);
    let catalog = Arc::new(HttpOnlyCatalog {
        configs: Mutex::new(configs),
    });

    let component = SurrealDbComponent::with_catalog(catalog);
    let ctx = camel_component_api::NoOpComponentContext;

    let result = component.create_endpoint("surrealdb:live?datasource=http-ds&table=events", &ctx);

    assert!(result.is_err(), "http:// must be rejected for live");
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("http:// must be rejected for live"),
    };
    let err_msg = format!("{:?}", err);
    assert!(
        err_msg.contains("LiveRequiresWebSocket") || err_msg.contains("WebSocket"),
        "expected WebSocket protocol error, got: {err_msg}"
    );
}

// ===========================================================================
// Health check test
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn datasource_health_check() {
    let (_container, endpoint) = start_surrealdb().await;
    let config = make_datasource_config(&endpoint);

    let factory = SurrealDbPoolFactory;
    let pool = factory.create(&config).await.expect("create pool");
    let handle =
        camel_api::datasource::DatasourceHandle::new("test".into(), "surrealdb".into(), pool);

    let status = factory.check(&handle).await;
    assert_eq!(
        status,
        camel_api::lifecycle::HealthStatus::Healthy,
        "datasource should be healthy"
    );
}
