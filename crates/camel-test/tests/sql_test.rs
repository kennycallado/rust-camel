//! Integration tests for SQL component.
//!
//! Uses testcontainers to spin up PostgreSQL instances for testing.
//!
//! **Requires Docker to be running.** Tests will fail if Docker is unavailable.
//!
//! **Requires `integration-tests` feature to compile and run.**

#![cfg(feature = "integration-tests")]

mod support;

use std::io::Write;

use camel_api::CamelError;
use camel_api::Value;
use camel_api::body::Body;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::test_support::tls;
use camel_component_mock::MockComponent;
use camel_component_sql::SqlComponent;
use camel_test::CamelTestContext;
use sqlx::AnyPool;
use support::wait::wait_until;
use testcontainers::ContainerAsync;
use testcontainers::core::WaitFor;
use testcontainers::runners::AsyncRunner;
use testcontainers::{CopyTargetOptions, GenericImage, ImageExt};
use testcontainers_modules::postgres::Postgres;

fn install_sqlx_drivers() {
    sqlx::any::install_default_drivers();
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

async fn create_pool(conn_str: &str) -> AnyPool {
    install_sqlx_drivers();
    sqlx::any::AnyPoolOptions::new()
        .max_connections(5)
        .connect(conn_str)
        .await
        .expect("Failed to connect to database")
}

async fn wait_for_mock_exchanges(mock: &MockComponent, endpoint_name: &str, min_count: usize) {
    let endpoint = mock
        .get_endpoint(endpoint_name)
        .unwrap_or_else(|| panic!("missing mock endpoint: {endpoint_name}"));

    wait_until(
        "sql mock exchange arrival",
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(100),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(endpoint.get_received_exchanges().await.len() >= min_count) }
        },
    )
    .await
    .unwrap();
}

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

#[tokio::test(flavor = "multi_thread")]
async fn producer_select() {
    let container = setup_postgres_container().await;
    let conn_str = get_connection_string(&container).await;
    let pool = create_pool(&conn_str).await;

    setup_test_table(&pool, "test_select").await;
    sqlx::query("INSERT INTO test_select (name, value) VALUES ('Alice', 100), ('Bob', 200)")
        .execute(&pool)
        .await
        .expect("Failed to insert test data");

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .to(format!(
            "sql:SELECT * FROM test_select ORDER BY id?db_url={}&sslMode=disable",
            conn_str
        ))
        .to("mock:result")
        .route_id("sql-select-test")
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

    let exchanges = endpoint.get_received_exchanges().await;
    let body = &exchanges[0].input.body;

    let json = match body {
        Body::Json(v) => v,
        _ => panic!("Body should be JSON"),
    };
    let arr = json.as_array().expect("Body should be a JSON array");
    assert_eq!(arr.len(), 2, "Should have 2 rows");

    let first = &arr[0];
    assert_eq!(first.get("name").and_then(|v| v.as_str()), Some("Alice"));
    assert_eq!(first.get("value").and_then(|v| v.as_i64()), Some(100));

    let row_count = exchanges[0]
        .input
        .header("CamelSql.RowCount")
        .and_then(|v| v.as_u64());
    assert_eq!(row_count, Some(2));
}

#[tokio::test(flavor = "multi_thread")]
async fn producer_insert() {
    let container = setup_postgres_container().await;
    let conn_str = get_connection_string(&container).await;
    let pool = create_pool(&conn_str).await;

    setup_test_table(&pool, "test_insert").await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("name", Value::String("Charlie".into()))
        .set_header("value", Value::Number(300.into()))
        .to(format!(
            "sql:INSERT INTO test_insert (name, value) VALUES (:#name, :#value)?db_url={}&sslMode=disable",
            conn_str
        ))
        .to("mock:result")
        .route_id("sql-insert-test")
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

    let exchanges = endpoint.get_received_exchanges().await;

    let update_count = exchanges[0]
        .input
        .header("CamelSql.UpdateCount")
        .and_then(|v| v.as_u64());
    assert_eq!(update_count, Some(1), "Should have inserted 1 row");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM test_insert")
        .fetch_one(&pool)
        .await
        .expect("Failed to count rows");
    assert_eq!(count, 1, "Table should have 1 row");
}

#[tokio::test(flavor = "multi_thread")]
async fn producer_update() {
    let container = setup_postgres_container().await;
    let conn_str = get_connection_string(&container).await;
    let pool = create_pool(&conn_str).await;

    setup_test_table(&pool, "test_update").await;
    sqlx::query("INSERT INTO test_update (name, value) VALUES ('Dave', 400)")
        .execute(&pool)
        .await
        .expect("Failed to insert test data");

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("new_value", Value::Number(999.into()))
        .set_header("target_name", Value::String("Dave".into()))
        .to(format!(
            "sql:UPDATE test_update SET value = :#new_value WHERE name = :#target_name?db_url={}&sslMode=disable",
            conn_str
        ))
        .to("mock:result")
        .route_id("sql-update-test")
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

    let exchanges = endpoint.get_received_exchanges().await;

    let update_count = exchanges[0]
        .input
        .header("CamelSql.UpdateCount")
        .and_then(|v| v.as_u64());
    assert_eq!(update_count, Some(1), "Should have updated 1 row");

    let value: i64 = sqlx::query_scalar("SELECT value FROM test_update WHERE name = 'Dave'")
        .fetch_one(&pool)
        .await
        .expect("Failed to get value");
    assert_eq!(value, 999, "Value should be updated to 999");
}

#[tokio::test(flavor = "multi_thread")]
async fn producer_delete() {
    let container = setup_postgres_container().await;
    let conn_str = get_connection_string(&container).await;
    let pool = create_pool(&conn_str).await;

    setup_test_table(&pool, "test_delete").await;
    sqlx::query("INSERT INTO test_delete (name, value) VALUES ('ToDelete', 500), ('ToKeep', 600)")
        .execute(&pool)
        .await
        .expect("Failed to insert test data");

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("target_name", Value::String("ToDelete".into()))
        .to(format!(
            "sql:DELETE FROM test_delete WHERE name = :#target_name?db_url={}&sslMode=disable",
            conn_str
        ))
        .to("mock:result")
        .route_id("sql-delete-test")
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

    let exchanges = endpoint.get_received_exchanges().await;

    let update_count = exchanges[0]
        .input
        .header("CamelSql.UpdateCount")
        .and_then(|v| v.as_u64());
    assert_eq!(update_count, Some(1), "Should have deleted 1 row");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM test_delete")
        .fetch_one(&pool)
        .await
        .expect("Failed to count rows");
    assert_eq!(count, 1, "Table should have 1 row remaining");
}

#[tokio::test(flavor = "multi_thread")]
async fn producer_select_one() {
    let container = setup_postgres_container().await;
    let conn_str = get_connection_string(&container).await;
    let pool = create_pool(&conn_str).await;

    setup_test_table(&pool, "test_select_one").await;
    sqlx::query("INSERT INTO test_select_one (name, value) VALUES ('SingleRow', 700)")
        .execute(&pool)
        .await
        .expect("Failed to insert test data");

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .to(format!(
            "sql:SELECT * FROM test_select_one WHERE name = 'SingleRow'?db_url={}&sslMode=disable&outputType=SelectOne",
            conn_str
        ))
        .to("mock:result")
        .route_id("sql-select-one-test")
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

    let exchanges = endpoint.get_received_exchanges().await;
    let body = &exchanges[0].input.body;

    let json = match body {
        Body::Json(v) => v,
        _ => panic!("Body should be JSON"),
    };
    assert!(json.is_object(), "Body should be a JSON object, not array");
    assert_eq!(json.get("name").and_then(|v| v.as_str()), Some("SingleRow"));
    assert_eq!(json.get("value").and_then(|v| v.as_i64()), Some(700));
}

#[tokio::test(flavor = "multi_thread")]
async fn producer_batch() {
    let container = setup_postgres_container().await;
    let conn_str = get_connection_string(&container).await;
    let pool = create_pool(&conn_str).await;

    setup_test_table(&pool, "test_batch").await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let batch_body = serde_json::json!([["Alice", 100], ["Bob", 200], ["Charlie", 300]]);

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(Value::from(batch_body))
        .to(format!(
            "sql:INSERT INTO test_batch (name, value) VALUES (#, #)?db_url={}&sslMode=disable&batch=true",
            conn_str
        ))
        .to("mock:result")
        .route_id("sql-batch-test")
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

    let exchanges = endpoint.get_received_exchanges().await;

    let update_count = exchanges[0]
        .input
        .header("CamelSql.UpdateCount")
        .and_then(|v| v.as_u64());
    assert_eq!(update_count, Some(3), "Should have inserted 3 rows");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM test_batch")
        .fetch_one(&pool)
        .await
        .expect("Failed to count rows");
    assert_eq!(count, 3, "Table should have 3 rows");
}

#[tokio::test(flavor = "multi_thread")]
async fn producer_noop() {
    let container = setup_postgres_container().await;
    let conn_str = get_connection_string(&container).await;
    let pool = create_pool(&conn_str).await;

    setup_test_table(&pool, "test_noop").await;
    sqlx::query("INSERT INTO test_noop (name, value) VALUES ('NoOpTest', 800)")
        .execute(&pool)
        .await
        .expect("Failed to insert test data");

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(Body::Text("OriginalBody".into()))
        .to(format!(
            "sql:UPDATE test_noop SET value = 999 WHERE name = 'NoOpTest'?db_url={}&sslMode=disable&noop=true",
            conn_str
        ))
        .to("mock:result")
        .route_id("sql-noop-test")
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

    let exchanges = endpoint.get_received_exchanges().await;

    let body = exchanges[0].input.body.as_text();
    assert_eq!(
        body,
        Some("OriginalBody"),
        "Body should be preserved in noop mode"
    );

    let update_count = exchanges[0]
        .input
        .header("CamelSql.UpdateCount")
        .and_then(|v| v.as_u64());
    assert_eq!(update_count, Some(1), "Update count should still be set");

    let value: i64 = sqlx::query_scalar("SELECT value FROM test_noop WHERE name = 'NoOpTest'")
        .fetch_one(&pool)
        .await
        .expect("Failed to get value");
    assert_eq!(value, 999, "Value should be updated even in noop mode");
}

#[tokio::test(flavor = "multi_thread")]
async fn consumer_polling() {
    let container = setup_postgres_container().await;
    let conn_str = get_connection_string(&container).await;
    let pool = create_pool(&conn_str).await;

    setup_test_table(&pool, "test_consumer").await;
    sqlx::query("INSERT INTO test_consumer (name, value) VALUES ('ConsumerRow', 900)")
        .execute(&pool)
        .await
        .expect("Failed to insert test data");

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let sql_uri = format!(
        "sql:SELECT * FROM test_consumer WHERE name = 'ConsumerRow'?db_url={}&sslMode=disable&delay=100&initialDelay=50",
        conn_str
    );
    let consumer_route = RouteBuilder::from(sql_uri.as_str())
        .to("mock:consumed")
        .route_id("sql-consumer-test")
        .build()
        .unwrap();

    h.add_route(consumer_route).await.unwrap();
    h.start().await;

    wait_for_mock_exchanges(h.mock(), "consumed", 1).await;
    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint = h.mock().get_endpoint("consumed").unwrap();
    let exchanges = endpoint.get_received_exchanges().await;

    assert!(
        !exchanges.is_empty(),
        "Consumer should have received at least one exchange"
    );

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

#[tokio::test(flavor = "multi_thread")]
async fn consumer_on_consume() {
    let container = setup_postgres_container().await;
    let conn_str = get_connection_string(&container).await;
    let pool = create_pool(&conn_str).await;

    setup_test_table(&pool, "test_on_consume").await;
    sqlx::query("CREATE TABLE IF NOT EXISTS processed_rows (id INTEGER, name VARCHAR(255))")
        .execute(&pool)
        .await
        .expect("Failed to create processed_rows table");

    sqlx::query("INSERT INTO test_on_consume (id, name, value) VALUES (1, 'ToProcess', 1000)")
        .execute(&pool)
        .await
        .expect("Failed to insert test data");

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let sql_uri = format!(
        "sql:SELECT * FROM test_on_consume?db_url={}&sslMode=disable&delay=100&initialDelay=50&onConsume=INSERT INTO processed_rows (id, name) VALUES (:#id, :#name)",
        conn_str
    );
    let consumer_route = RouteBuilder::from(sql_uri.as_str())
        .to("mock:consumed")
        .route_id("sql-on-consume-test")
        .build()
        .unwrap();

    h.add_route(consumer_route).await.unwrap();
    h.start().await;

    wait_for_mock_exchanges(h.mock(), "consumed", 1).await;
    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM processed_rows")
        .fetch_one(&pool)
        .await
        .expect("Failed to count processed_rows");

    assert!(
        count >= 1,
        "onConsume should have inserted into processed_rows"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn consumer_empty_result() {
    let container = setup_postgres_container().await;
    let conn_str = get_connection_string(&container).await;
    let pool = create_pool(&conn_str).await;

    setup_test_table(&pool, "test_empty").await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let sql_uri = format!(
        "sql:SELECT * FROM test_empty?db_url={}&sslMode=disable&delay=100&initialDelay=50&routeEmptyResultSet=false",
        conn_str
    );
    let consumer_route = RouteBuilder::from(sql_uri.as_str())
        .to("mock:consumed")
        .route_id("sql-empty-test")
        .build()
        .unwrap();

    h.add_route(consumer_route).await.unwrap();
    h.start().await;

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint = h.mock().get_endpoint("consumed").unwrap();
    let exchanges = endpoint.get_received_exchanges().await;

    assert!(
        exchanges.is_empty(),
        "Consumer should not send exchanges for empty result set when routeEmptyResultSet=false"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn consumer_on_consume_failed() {
    let container = setup_postgres_container().await;
    let conn_str = get_connection_string(&container).await;
    let pool = create_pool(&conn_str).await;

    setup_test_table(&pool, "test_on_consume_failed_src").await;
    sqlx::query(
        "INSERT INTO test_on_consume_failed_src (id, name, value) VALUES (1, 'WillFail', 42)",
    )
    .execute(&pool)
    .await
    .expect("Failed to insert test data");

    sqlx::query("CREATE TABLE IF NOT EXISTS failed_rows (id INTEGER, name VARCHAR(255))")
        .execute(&pool)
        .await
        .expect("Failed to create failed_rows table");

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;

    let sql_uri = format!(
        "sql:SELECT * FROM test_on_consume_failed_src?db_url={}&sslMode=disable&delay=100&initialDelay=50\
         &onConsumeFailed=INSERT INTO failed_rows (id, name) VALUES (:#id, :#name)",
        conn_str
    );

    let consumer_route = RouteBuilder::from(sql_uri.as_str())
        .process(|_ex| async move {
            Err::<_, CamelError>(CamelError::ProcessorError(
                "simulated downstream failure".into(),
            ))
        })
        .route_id("sql-on-consume-failed-test")
        .build()
        .unwrap();

    h.add_route(consumer_route).await.unwrap();
    h.start().await;

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    h.stop().await;

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM failed_rows")
        .fetch_one(&pool)
        .await
        .expect("Failed to count failed_rows");

    assert!(
        count >= 1,
        "onConsumeFailed should have inserted at least one row into failed_rows"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn consumer_empty_result_routed() {
    let container = setup_postgres_container().await;
    let conn_str = get_connection_string(&container).await;
    let pool = create_pool(&conn_str).await;

    setup_test_table(&pool, "test_empty_routed").await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let sql_uri = format!(
        "sql:SELECT * FROM test_empty_routed?db_url={}&sslMode=disable&delay=100&initialDelay=50&routeEmptyResultSet=true&useIterator=false",
        conn_str
    );
    let consumer_route = RouteBuilder::from(sql_uri.as_str())
        .to("mock:consumed")
        .route_id("sql-empty-routed-test")
        .build()
        .unwrap();

    h.add_route(consumer_route).await.unwrap();
    h.start().await;

    wait_for_mock_exchanges(h.mock(), "consumed", 1).await;
    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint = h.mock().get_endpoint("consumed").unwrap();
    let exchanges = endpoint.get_received_exchanges().await;

    assert!(
        !exchanges.is_empty(),
        "Consumer should send exchanges for empty result set when routeEmptyResultSet=true"
    );
}

/// Spins up a PostgreSQL container with TLS enabled using generated self-signed
/// certs. Returns the container and the SSL connection string.
async fn setup_postgres_tls_container() -> (ContainerAsync<GenericImage>, String) {
    install_sqlx_drivers();

    // Generate self-signed CA + server cert (SANs: localhost, 127.0.0.1, ::1)
    let (_ca_pem, cert_pem, key_pem) = tls::gen_server_cert();

    // Write cert/key to temp files for with_copy_to
    let cert_file = tempfile::NamedTempFile::new().unwrap();
    let key_file = tempfile::NamedTempFile::new().unwrap();
    cert_file.as_file().write_all(cert_pem.as_bytes()).unwrap();
    key_file.as_file().write_all(key_pem.as_bytes()).unwrap();
    // Persist so the files outlive the TempDir and are available for container copy
    let cert_path = cert_file.into_temp_path();
    let key_path = key_file.into_temp_path();

    // Write entrypoint wrapper that initializes DB, sets up SSL certs, then starts postgres
    let entrypoint_script = r#"#!/bin/sh
set -e
if [ ! -f $PGDATA/PG_VERSION ]; then
    gosu postgres initdb -D $PGDATA
    # Start temporarily to set password (ssl=off, default host rules work for localhost)
    gosu postgres pg_ctl -D $PGDATA start -w -o "-c ssl=off"
    gosu postgres psql -c "ALTER USER postgres PASSWORD 'postgres';"
    gosu postgres pg_ctl -D $PGDATA stop -w
    # Convert default 'host' rules to 'hostssl' to enforce SSL-only at server level
    sed -i 's/^host /hostssl /' $PGDATA/pg_hba.conf
    # Allow SSL connections from Docker bridge network
    echo "hostssl all all 0.0.0.0/0 md5" >> $PGDATA/pg_hba.conf
    echo "hostssl all all ::0/0 md5" >> $PGDATA/pg_hba.conf
fi
cp /etc/postgresql/tls/server.pem $PGDATA/server.pem
cp /etc/postgresql/tls/server-key.pem $PGDATA/server-key.pem
chown postgres:postgres $PGDATA/server.pem $PGDATA/server-key.pem
chmod 0600 $PGDATA/server-key.pem
# Start SSL postgres in background
gosu postgres postgres \
    -c ssl=on \
    -c ssl_cert_file=$PGDATA/server.pem \
    -c ssl_key_file=$PGDATA/server-key.pem &
PG_PID=$!
# Wait until actually accepting connections, then emit unique marker
until pg_isready -h 127.0.0.1 -p 5432 -U postgres 2>/dev/null; do sleep 0.2; done
echo "SSL_POSTGRES_READY" >&2
wait $PG_PID
"#;
    let entrypoint_file = tempfile::NamedTempFile::new().unwrap();
    entrypoint_file
        .as_file()
        .write_all(entrypoint_script.as_bytes())
        .unwrap();
    let entrypoint_path = entrypoint_file.into_temp_path();

    // Build TLS-enabled postgres container.
    // Custom entrypoint initializes the DB, copies certs into PGDATA with correct
    // ownership/perms, then starts postgres with SSL enabled.
    // NOTE: GenericImage methods (with_wait_for, with_entrypoint) must come before ImageExt methods.
    let image = GenericImage::new("postgres", "16-alpine")
        .with_wait_for(WaitFor::message_on_stderr("SSL_POSTGRES_READY"))
        .with_entrypoint("/usr/local/bin/ssl-entrypoint.sh")
        .with_copy_to(
            CopyTargetOptions::new("/etc/postgresql/tls/server.pem"),
            cert_path.to_path_buf(),
        )
        .with_copy_to(
            CopyTargetOptions::new("/etc/postgresql/tls/server-key.pem"),
            key_path.to_path_buf(),
        )
        .with_copy_to(
            CopyTargetOptions::new("/usr/local/bin/ssl-entrypoint.sh").with_mode(0o755),
            entrypoint_path.to_path_buf(),
        );

    let container = image.start().await.unwrap();

    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let conn_str = format!(
        "postgres://postgres:postgres@127.0.0.1:{}/postgres?sslmode=require",
        port
    );
    eprintln!("PostgreSQL TLS connection: {}", conn_str);

    (container, conn_str)
}

#[tokio::test(flavor = "multi_thread")]
async fn sql_ssl_connection_roundtrip() {
    let (_container, conn_str) = setup_postgres_tls_container().await;

    // Verify SSL handshake via direct PgPool (bypasses `any` driver)
    let pg_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&conn_str)
        .await
        .expect("Failed to connect with sslmode=require");

    // Confirm TLS is active
    sqlx::query("CREATE EXTENSION IF NOT EXISTS sslinfo")
        .execute(&pg_pool)
        .await
        .expect("Failed to create sslinfo extension");

    let ssl_row: (bool,) = sqlx::query_as("SELECT ssl_is_used()")
        .fetch_one(&pg_pool)
        .await
        .expect("Failed to query ssl_is_used");
    assert!(ssl_row.0, "SSL must be active on the connection");

    // Create test table and insert data
    sqlx::query("CREATE TABLE ssl_test (id SERIAL PRIMARY KEY, name VARCHAR(255), value INTEGER)")
        .execute(&pg_pool)
        .await
        .expect("Failed to create ssl_test table");

    sqlx::query("INSERT INTO ssl_test (name, value) VALUES ('TLSRow', 443)")
        .execute(&pg_pool)
        .await
        .expect("Failed to insert TLS test data");

    pg_pool.close().await;

    // Now verify through the SQL component.
    // sslmode=require in the connection string forces the client to negotiate TLS;
    // combined with hostssl pg_hba rules, this ensures the connection is encrypted.
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .to(format!(
            "sql:SELECT * FROM ssl_test?db_url={}&sslMode=require",
            conn_str
        ))
        .to("mock:result")
        .route_id("sql-ssl-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    wait_for_mock_exchanges(h.mock(), "result", 1).await;
    h.stop().await;

    // Check no errors occurred
    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("SSL route had errors: {:?}", errors[0].error);
        }
    }

    let endpoint = h.mock().get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    let body = &exchanges[0].input.body;
    let json = match body {
        Body::Json(v) => v,
        _ => panic!("Body should be JSON"),
    };
    let arr = json.as_array().expect("Body should be a JSON array");
    assert_eq!(arr.len(), 1, "Should have 1 row");
    assert_eq!(arr[0].get("name").and_then(|v| v.as_str()), Some("TLSRow"));
    assert_eq!(arr[0].get("value").and_then(|v| v.as_i64()), Some(443));
}
