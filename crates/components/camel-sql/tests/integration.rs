//! Integration tests for SQL component.
//!
//! Uses testcontainers to spin up PostgreSQL instances for testing.
//!
//! **Requires Docker to be running.** Tests will fail if Docker is unavailable.
//!
//! Run with: `cargo test -p camel-component-sql -- --ignored`

use camel_api::Value;
use camel_api::body::Body;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_mock::MockComponent;
use camel_component_sql::SqlComponent;
use camel_component_timer::TimerComponent;
use camel_core::CamelContext;
use sqlx::AnyPool;
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

/// Install sqlx default drivers for AnyPool to work with PostgreSQL.
/// This must be called before any database connections are made.
fn install_sqlx_drivers() {
    // This is safe to call multiple times
    let _ = sqlx::any::install_default_drivers();
}

async fn setup_postgres_container() -> ContainerAsync<Postgres> {
    install_sqlx_drivers();
    Postgres::default().start().await.unwrap()
}

async fn get_connection_string(container: &ContainerAsync<Postgres>) -> String {
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let conn_str = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    eprintln!("PostgreSQL connection: {}", conn_str);
    conn_str
}

/// Create a connection pool for direct database setup operations.
async fn create_pool(conn_str: &str) -> AnyPool {
    install_sqlx_drivers();
    sqlx::any::AnyPoolOptions::new()
        .max_connections(5)
        .connect(conn_str)
        .await
        .expect("Failed to connect to database")
}

/// Setup a test table with some initial data.
async fn setup_test_table(pool: &AnyPool, table_name: &str) {
    sqlx::query(&format!("DROP TABLE IF EXISTS {}", table_name))
        .execute(pool)
        .await
        .ok();

    sqlx::query(&format!(
        "CREATE TABLE {} (id SERIAL PRIMARY KEY, name VARCHAR(255), value INTEGER)",
        table_name
    ))
    .execute(pool)
    .await
    .expect("Failed to create table");
}

// ===========================================================================
// Producer SELECT tests
// ===========================================================================

/// Test 1: Producer SELECT - Create table, insert data, query via producer, verify JSON body
#[tokio::test]
#[ignore = "Requires Docker"]
async fn test_producer_select() {
    let container = setup_postgres_container().await;
    let conn_str = get_connection_string(&container).await;
    let pool = create_pool(&conn_str).await;

    // Setup table with test data
    setup_test_table(&pool, "test_select").await;
    sqlx::query("INSERT INTO test_select (name, value) VALUES ('Alice', 100), ('Bob', 200)")
        .execute(&pool)
        .await
        .expect("Failed to insert test data");

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(SqlComponent::new());
    ctx.register_component(mock.clone());
    ctx.set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .to(format!(
            "sql:SELECT * FROM test_select ORDER BY id?db_url={}",
            conn_str
        ))
        .to("mock:result")
        .route_id("sql-select-test")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    ctx.stop().await.unwrap();

    // Check for errors
    if let Some(error_ep) = mock.get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint = mock.get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    let body = &exchanges[0].input.body;

    // Verify the body is a JSON array with 2 elements
    let json = match body {
        Body::Json(v) => v,
        _ => panic!("Body should be JSON"),
    };
    let arr = json.as_array().expect("Body should be a JSON array");
    assert_eq!(arr.len(), 2, "Should have 2 rows");

    // Verify first row
    let first = &arr[0];
    assert_eq!(first.get("name").and_then(|v| v.as_str()), Some("Alice"));
    assert_eq!(first.get("value").and_then(|v| v.as_i64()), Some(100));

    // Verify CamelSql.RowCount header
    let row_count = exchanges[0]
        .input
        .header("CamelSql.RowCount")
        .and_then(|v| v.as_u64());
    assert_eq!(row_count, Some(2));
}

// ===========================================================================
// Producer INSERT tests
// ===========================================================================

/// Test 2: Producer INSERT - Insert via producer with named params, verify row count header
#[tokio::test]
#[ignore = "Requires Docker"]
async fn test_producer_insert() {
    let container = setup_postgres_container().await;
    let conn_str = get_connection_string(&container).await;
    let pool = create_pool(&conn_str).await;

    setup_test_table(&pool, "test_insert").await;

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(SqlComponent::new());
    ctx.register_component(mock.clone());
    ctx.set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("name", Value::String("Charlie".into()))
        .set_header("value", Value::Number(300.into()))
        .to(format!(
            "sql:INSERT INTO test_insert (name, value) VALUES (:#name, :#value)?db_url={}",
            conn_str
        ))
        .to("mock:result")
        .route_id("sql-insert-test")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    ctx.stop().await.unwrap();

    // Check for errors
    if let Some(error_ep) = mock.get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint = mock.get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;

    // Verify CamelSql.UpdateCount header
    let update_count = exchanges[0]
        .input
        .header("CamelSql.UpdateCount")
        .and_then(|v| v.as_u64());
    assert_eq!(update_count, Some(1), "Should have inserted 1 row");

    // Verify the row was actually inserted
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM test_insert")
        .fetch_one(&pool)
        .await
        .expect("Failed to count rows");
    assert_eq!(count, 1, "Table should have 1 row");
}

// ===========================================================================
// Producer UPDATE tests
// ===========================================================================

/// Test 3: Producer UPDATE - Update via producer, verify update count header
#[tokio::test]
#[ignore = "Requires Docker"]
async fn test_producer_update() {
    let container = setup_postgres_container().await;
    let conn_str = get_connection_string(&container).await;
    let pool = create_pool(&conn_str).await;

    setup_test_table(&pool, "test_update").await;
    sqlx::query("INSERT INTO test_update (name, value) VALUES ('Dave', 400)")
        .execute(&pool)
        .await
        .expect("Failed to insert test data");

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(SqlComponent::new());
    ctx.register_component(mock.clone());
    ctx.set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("new_value", Value::Number(999.into()))
        .set_header("target_name", Value::String("Dave".into()))
        .to(format!(
            "sql:UPDATE test_update SET value = :#new_value WHERE name = :#target_name?db_url={}",
            conn_str
        ))
        .to("mock:result")
        .route_id("sql-update-test")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    ctx.stop().await.unwrap();

    // Check for errors
    if let Some(error_ep) = mock.get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint = mock.get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;

    // Verify CamelSql.UpdateCount header
    let update_count = exchanges[0]
        .input
        .header("CamelSql.UpdateCount")
        .and_then(|v| v.as_u64());
    assert_eq!(update_count, Some(1), "Should have updated 1 row");

    // Verify the value was actually updated
    let value: i64 = sqlx::query_scalar("SELECT value FROM test_update WHERE name = 'Dave'")
        .fetch_one(&pool)
        .await
        .expect("Failed to get value");
    assert_eq!(value, 999, "Value should be updated to 999");
}

// ===========================================================================
// Producer DELETE tests
// ===========================================================================

/// Test 4: Producer DELETE - Delete via producer, verify update count
#[tokio::test]
#[ignore = "Requires Docker"]
async fn test_producer_delete() {
    let container = setup_postgres_container().await;
    let conn_str = get_connection_string(&container).await;
    let pool = create_pool(&conn_str).await;

    setup_test_table(&pool, "test_delete").await;
    sqlx::query("INSERT INTO test_delete (name, value) VALUES ('ToDelete', 500), ('ToKeep', 600)")
        .execute(&pool)
        .await
        .expect("Failed to insert test data");

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(SqlComponent::new());
    ctx.register_component(mock.clone());
    ctx.set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("target_name", Value::String("ToDelete".into()))
        .to(format!(
            "sql:DELETE FROM test_delete WHERE name = :#target_name?db_url={}",
            conn_str
        ))
        .to("mock:result")
        .route_id("sql-delete-test")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    ctx.stop().await.unwrap();

    // Check for errors
    if let Some(error_ep) = mock.get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint = mock.get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;

    // Verify CamelSql.UpdateCount header
    let update_count = exchanges[0]
        .input
        .header("CamelSql.UpdateCount")
        .and_then(|v| v.as_u64());
    assert_eq!(update_count, Some(1), "Should have deleted 1 row");

    // Verify the row was actually deleted
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM test_delete")
        .fetch_one(&pool)
        .await
        .expect("Failed to count rows");
    assert_eq!(count, 1, "Table should have 1 row remaining");
}

// ===========================================================================
// Producer SelectOne tests
// ===========================================================================

/// Test 5: Producer SelectOne - Query single row, verify single JSON object (not array)
#[tokio::test]
#[ignore = "Requires Docker"]
async fn test_producer_select_one() {
    let container = setup_postgres_container().await;
    let conn_str = get_connection_string(&container).await;
    let pool = create_pool(&conn_str).await;

    setup_test_table(&pool, "test_select_one").await;
    sqlx::query("INSERT INTO test_select_one (name, value) VALUES ('SingleRow', 700)")
        .execute(&pool)
        .await
        .expect("Failed to insert test data");

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(SqlComponent::new());
    ctx.register_component(mock.clone());
    ctx.set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .to(format!(
            "sql:SELECT * FROM test_select_one WHERE name = 'SingleRow'?db_url={}&outputType=SelectOne",
            conn_str
        ))
        .to("mock:result")
        .route_id("sql-select-one-test")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    ctx.stop().await.unwrap();

    // Check for errors
    if let Some(error_ep) = mock.get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint = mock.get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    let body = &exchanges[0].input.body;

    // Verify the body is a JSON object (not array)
    let json = match body {
        Body::Json(v) => v,
        _ => panic!("Body should be JSON"),
    };
    assert!(json.is_object(), "Body should be a JSON object, not array");
    assert_eq!(
        json.get("name").and_then(|v| v.as_str()),
        Some("SingleRow")
    );
    assert_eq!(json.get("value").and_then(|v| v.as_i64()), Some(700));
}

// ===========================================================================
// Producer batch tests
// ===========================================================================

/// Test 6: Producer batch - Batch insert multiple rows
#[tokio::test]
#[ignore = "Requires Docker"]
async fn test_producer_batch() {
    let container = setup_postgres_container().await;
    let conn_str = get_connection_string(&container).await;
    let pool = create_pool(&conn_str).await;

    setup_test_table(&pool, "test_batch").await;

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(SqlComponent::new());
    ctx.register_component(mock.clone());
    ctx.set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

    // Batch body: array of arrays with params
    let batch_body = serde_json::json!([
        ["Alice", 100],
        ["Bob", 200],
        ["Charlie", 300]
    ]);

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(Value::from(batch_body))
        .to(format!(
            "sql:INSERT INTO test_batch (name, value) VALUES (#, #)?db_url={}&batch=true",
            conn_str
        ))
        .to("mock:result")
        .route_id("sql-batch-test")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    ctx.stop().await.unwrap();

    // Check for errors
    if let Some(error_ep) = mock.get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint = mock.get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;

    // Verify CamelSql.UpdateCount header (3 rows inserted)
    let update_count = exchanges[0]
        .input
        .header("CamelSql.UpdateCount")
        .and_then(|v| v.as_u64());
    assert_eq!(update_count, Some(3), "Should have inserted 3 rows");

    // Verify the rows were actually inserted
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM test_batch")
        .fetch_one(&pool)
        .await
        .expect("Failed to count rows");
    assert_eq!(count, 3, "Table should have 3 rows");
}

// ===========================================================================
// Producer noop tests
// ===========================================================================

/// Test 7: Producer noop - Execute query but verify original body preserved
#[tokio::test]
#[ignore = "Requires Docker"]
async fn test_producer_noop() {
    let container = setup_postgres_container().await;
    let conn_str = get_connection_string(&container).await;
    let pool = create_pool(&conn_str).await;

    setup_test_table(&pool, "test_noop").await;
    sqlx::query("INSERT INTO test_noop (name, value) VALUES ('NoOpTest', 800)")
        .execute(&pool)
        .await
        .expect("Failed to insert test data");

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(SqlComponent::new());
    ctx.register_component(mock.clone());
    ctx.set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(Body::Text("OriginalBody".into()))
        .to(format!(
            "sql:UPDATE test_noop SET value = 999 WHERE name = 'NoOpTest'?db_url={}&noop=true",
            conn_str
        ))
        .to("mock:result")
        .route_id("sql-noop-test")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    ctx.stop().await.unwrap();

    // Check for errors
    if let Some(error_ep) = mock.get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint = mock.get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;

    // Verify original body is preserved
    let body = exchanges[0].input.body.as_text();
    assert_eq!(body, Some("OriginalBody"), "Body should be preserved in noop mode");

    // Verify CamelSql.UpdateCount header is still set
    let update_count = exchanges[0]
        .input
        .header("CamelSql.UpdateCount")
        .and_then(|v| v.as_u64());
    assert_eq!(update_count, Some(1), "Update count should still be set");

    // Verify the update was actually executed
    let value: i64 = sqlx::query_scalar("SELECT value FROM test_noop WHERE name = 'NoOpTest'")
        .fetch_one(&pool)
        .await
        .expect("Failed to get value");
    assert_eq!(value, 999, "Value should be updated even in noop mode");
}

// ===========================================================================
// Consumer polling tests
// ===========================================================================

/// Test 8: Consumer polling - Start consumer, insert row, verify exchange received
#[tokio::test]
#[ignore = "Requires Docker"]
async fn test_consumer_polling() {
    let container = setup_postgres_container().await;
    let conn_str = get_connection_string(&container).await;
    let pool = create_pool(&conn_str).await;

    // Setup table with "unprocessed" rows
    setup_test_table(&pool, "test_consumer").await;
    sqlx::query("INSERT INTO test_consumer (name, value) VALUES ('ConsumerRow', 900)")
        .execute(&pool)
        .await
        .expect("Failed to insert test data");

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(SqlComponent::new());
    ctx.register_component(mock.clone());
    ctx.set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

    // Consumer polls the table and sends to mock:consumed
    let sql_uri = format!(
        "sql:SELECT * FROM test_consumer WHERE name = 'ConsumerRow'?db_url={}&delay=100&initialDelay=50",
        conn_str
    );
    let consumer_route = RouteBuilder::from(sql_uri.as_str())
        .to("mock:consumed")
        .route_id("sql-consumer-test")
        .build()
        .unwrap();

    ctx.add_route_definition(consumer_route).unwrap();
    ctx.start().await.unwrap();

    // Wait for consumer to poll and process
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    ctx.stop().await.unwrap();

    // Check for errors
    if let Some(error_ep) = mock.get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint = mock.get_endpoint("consumed").unwrap();
    let exchanges = endpoint.get_received_exchanges().await;

    // Consumer should have processed the row (potentially multiple times since it's a polling query)
    assert!(!exchanges.is_empty(), "Consumer should have received at least one exchange");

    // Verify the body contains the row data
    let first_exchange = &exchanges[0];
    let body = match &first_exchange.input.body {
        Body::Json(v) => v,
        _ => panic!("Body should be JSON"),
    };
    assert_eq!(
        body.get("name").and_then(|v| v.as_str()),
        Some("ConsumerRow")
    );
    assert_eq!(body.get("value").and_then(|v| v.as_i64()), Some(900));
}

// ===========================================================================
// Consumer onConsume tests
// ===========================================================================

/// Test 9: Consumer onConsume - Verify post-processing query executes after successful processing
#[tokio::test]
#[ignore = "Requires Docker"]
async fn test_consumer_on_consume() {
    let container = setup_postgres_container().await;
    let conn_str = get_connection_string(&container).await;
    let pool = create_pool(&conn_str).await;

    // Setup tables
    setup_test_table(&pool, "test_on_consume").await;
    sqlx::query("CREATE TABLE IF NOT EXISTS processed_rows (id INTEGER, name VARCHAR(255))")
        .execute(&pool)
        .await
        .expect("Failed to create processed_rows table");

    sqlx::query("INSERT INTO test_on_consume (id, name, value) VALUES (1, 'ToProcess', 1000)")
        .execute(&pool)
        .await
        .expect("Failed to insert test data");

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(SqlComponent::new());
    ctx.register_component(mock.clone());
    ctx.set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

    // Consumer with onConsume to insert into processed_rows after processing
    let sql_uri = format!(
        "sql:SELECT * FROM test_on_consume?db_url={}&delay=100&initialDelay=50&onConsume=INSERT INTO processed_rows (id, name) VALUES (:#id, :#name)",
        conn_str
    );
    let consumer_route = RouteBuilder::from(sql_uri.as_str())
        .to("mock:consumed")
        .route_id("sql-on-consume-test")
        .build()
        .unwrap();

    ctx.add_route_definition(consumer_route).unwrap();
    ctx.start().await.unwrap();

    // Wait for consumer to poll and process
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    ctx.stop().await.unwrap();

    // Check for errors
    if let Some(error_ep) = mock.get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    // Verify onConsume query was executed
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM processed_rows")
        .fetch_one(&pool)
        .await
        .expect("Failed to count processed_rows");

    // Should have at least 1 row in processed_rows (from onConsume)
    assert!(count >= 1, "onConsume should have inserted into processed_rows");
}

// ===========================================================================
// Consumer empty result set tests
// ===========================================================================

/// Test 10: Consumer empty result - Verify no exchange sent when query returns empty and route_empty_result_set=false
#[tokio::test]
#[ignore = "Requires Docker"]
async fn test_consumer_empty_result() {
    let container = setup_postgres_container().await;
    let conn_str = get_connection_string(&container).await;
    let pool = create_pool(&conn_str).await;

    // Setup empty table
    setup_test_table(&pool, "test_empty").await;
    // Don't insert any data - table is empty

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(SqlComponent::new());
    ctx.register_component(mock.clone());
    ctx.set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

    // Consumer with routeEmptyResultSet=false (default)
    let sql_uri = format!(
        "sql:SELECT * FROM test_empty?db_url={}&delay=100&initialDelay=50&routeEmptyResultSet=false",
        conn_str
    );
    let consumer_route = RouteBuilder::from(sql_uri.as_str())
        .to("mock:consumed")
        .route_id("sql-empty-test")
        .build()
        .unwrap();

    ctx.add_route_definition(consumer_route).unwrap();
    ctx.start().await.unwrap();

    // Wait for multiple poll cycles
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    ctx.stop().await.unwrap();

    // Check for errors
    if let Some(error_ep) = mock.get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint = mock.get_endpoint("consumed").unwrap();
    let exchanges = endpoint.get_received_exchanges().await;

    // With routeEmptyResultSet=false, no exchanges should be sent for empty result
    assert!(
        exchanges.is_empty(),
        "Consumer should not send exchanges for empty result set when routeEmptyResultSet=false"
    );
}

/// Test 10b: Consumer with routeEmptyResultSet=true should send empty result
#[tokio::test]
#[ignore = "Requires Docker"]
async fn test_consumer_empty_result_routed() {
    let container = setup_postgres_container().await;
    let conn_str = get_connection_string(&container).await;
    let pool = create_pool(&conn_str).await;

    // Setup empty table
    setup_test_table(&pool, "test_empty_routed").await;
    // Don't insert any data - table is empty

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(SqlComponent::new());
    ctx.register_component(mock.clone());
    ctx.set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"));

    // Consumer with routeEmptyResultSet=true and useIterator=false (batch mode)
    // In batch mode, an empty result set will still send an exchange with an empty array
    let sql_uri = format!(
        "sql:SELECT * FROM test_empty_routed?db_url={}&delay=100&initialDelay=50&routeEmptyResultSet=true&useIterator=false",
        conn_str
    );
    let consumer_route = RouteBuilder::from(sql_uri.as_str())
        .to("mock:consumed")
        .route_id("sql-empty-routed-test")
        .build()
        .unwrap();

    ctx.add_route_definition(consumer_route).unwrap();
    ctx.start().await.unwrap();

    // Wait for multiple poll cycles
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    ctx.stop().await.unwrap();

    // Check for errors
    if let Some(error_ep) = mock.get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint = mock.get_endpoint("consumed").unwrap();
    let exchanges = endpoint.get_received_exchanges().await;

    // With routeEmptyResultSet=true, exchanges should be sent even for empty result
    assert!(
        !exchanges.is_empty(),
        "Consumer should send exchanges for empty result set when routeEmptyResultSet=true"
    );
}
