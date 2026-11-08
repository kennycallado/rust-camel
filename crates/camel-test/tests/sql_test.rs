//! Integration tests for SQL component.
//!
//! Uses testcontainers to spin up PostgreSQL instances for testing.
//!
//! **Requires Docker to be running.** Tests will fail if Docker is unavailable.
//!
//! **Requires `integration-tests` feature to compile and run.**

#![cfg(feature = "integration-tests")]

mod support;
use support::install_crypto_provider;
use support::postgres::{shared_postgres, shared_postgres_17, shared_postgres_tls};

use camel_api::CamelError;
use camel_api::Value;
use camel_api::body::Body;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::test_support::timeout_wait;
use camel_component_mock::MockComponent;
use camel_component_sql::SqlComponent;
use camel_test::CamelTestContext;
use sqlx::AnyPool;
use sqlx::Row;
use support::wait::wait_until;

fn install_sqlx_drivers() {
    sqlx::any::install_default_drivers();
}

async fn create_pool(conn_str: &str) -> AnyPool {
    install_sqlx_drivers();
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        sqlx::any::AnyPoolOptions::new()
            .max_connections(5)
            .connect(conn_str),
    )
    .await
    .expect("sql pool connect timed out after 30s")
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
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP TABLE IF EXISTS {}",
        table_name
    )))
    .execute(pool)
    .await
    .ok();

    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE TABLE {} (id SERIAL PRIMARY KEY, name VARCHAR(255), value INTEGER)",
        table_name
    )))
    .execute(pool)
    .await
    .expect("Failed to create table");
}

#[tokio::test(flavor = "multi_thread")]
async fn producer_select() {
    install_crypto_provider();
    let conn_str = shared_postgres().await.to_string();
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
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
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
    install_crypto_provider();
    let conn_str = shared_postgres().await.to_string();
    let pool = create_pool(&conn_str).await;

    setup_test_table(&pool, "test_insert").await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;
    h.ctx()
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
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
    install_crypto_provider();
    let conn_str = shared_postgres().await.to_string();
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
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
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
    install_crypto_provider();
    let conn_str = shared_postgres().await.to_string();
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
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
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
    install_crypto_provider();
    let conn_str = shared_postgres().await.to_string();
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
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
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
    install_crypto_provider();
    let conn_str = shared_postgres().await.to_string();
    let pool = create_pool(&conn_str).await;

    setup_test_table(&pool, "test_batch").await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;
    h.ctx()
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
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

// ── rc-fef4z: batch binds must never reinterpret numeric bits ───────────

async fn setup_table(pool: &AnyPool, ddl: &str) {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP TABLE IF EXISTS {}",
        extract_table_name(ddl)
    )))
    .execute(pool)
    .await
    .ok();

    sqlx::query(sqlx::AssertSqlSafe(ddl.to_string()))
        .execute(pool)
        .await
        .expect("Failed to create batch-kinds table");
}

fn extract_table_name(ddl: &str) -> String {
    ddl.split_whitespace()
        .nth(2)
        .expect("DDL must name a table")
        .to_string()
}

/// Adds one batch=true insert route for `endpoint` with `body` to the
/// harness. The caller starts the harness and waits for mock results.
async fn run_batch_route(
    h: &CamelTestContext,
    timer_name: &str,
    period_ms: u64,
    body: serde_json::Value,
    endpoint: &str,
) {
    let route = RouteBuilder::from(&format!(
        "timer:{timer_name}?period={period_ms}&repeatCount=1"
    ))
    .set_body(Value::from(body))
    .to(endpoint.to_string())
    .to("mock:result")
    .route_id(format!("sql-batch-kinds-{timer_name}"))
    .build()
    .unwrap();

    h.add_route(route).await.unwrap();
}

/// Reads `c1`/`c2` ordered by id, decoding as f64.
async fn read_f64_cols(pool: &AnyPool, table: &str, c1: &str, c2: &str) -> Vec<(f64, f64)> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {c1}, {c2} FROM {table} ORDER BY id"
    )))
    .fetch_all(pool)
    .await
    .expect("Failed to read stored rows");

    rows.into_iter()
        .map(|row| {
            let a: f64 = row.try_get(0).expect("first column should decode as f64");
            // REAL columns decode as f32 through the Any driver's strict
            // type compatibility; widen for comparison.
            let b: f32 = row.try_get(1).expect("second column should decode as f32");
            (a, f64::from(b))
        })
        .collect()
}

/// Reads `c1`/`c2` ordered by id, decoding as i64.
async fn read_i64_cols(pool: &AnyPool, table: &str, c1: &str, c2: &str) -> Vec<(i64, i64)> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {c1}, {c2} FROM {table} ORDER BY id"
    )))
    .fetch_all(pool)
    .await
    .expect("Failed to read stored rows");

    rows.into_iter()
        .map(|row| {
            let a: i64 = row.try_get(0).expect("first column should decode as i64");
            let b: i64 = row.try_get(1).expect("second column should decode as i64");
            (a, b)
        })
        .collect()
}

/// Reads `c1`/`c2` ordered by id as text (for NUMERIC columns).
async fn read_text_cols(pool: &AnyPool, table: &str, c1: &str, c2: &str) -> Vec<(String, String)> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {c1}::text, {c2}::text FROM {table} ORDER BY id"
    )))
    .fetch_all(pool)
    .await
    .expect("Failed to read stored rows");

    rows.into_iter()
        .map(|row| {
            let a: String = row.try_get(0).expect("first column should decode as text");
            let b: String = row.try_get(1).expect("second column should decode as text");
            (a, b)
        })
        .collect()
}

/// Regression for bd rc-fef4z, reporter's exact scenario: both reported
/// bodies through the SAME endpoint (same pool, same statement cache) in
/// sequence. Every parameter position carries one wire encoding, so the
/// stored values are exact — never reinterpreted bits, never an error.
#[tokio::test(flavor = "multi_thread")]
async fn batch_mixed_int_float_kinds_never_corrupts() {
    install_crypto_provider();
    let conn_str = shared_postgres_17().await.to_string();
    let pool = create_pool(&conn_str).await;

    setup_table(
        &pool,
        "CREATE TABLE test_batch_mixed (id SERIAL PRIMARY KEY, f DOUBLE PRECISION, i INTEGER)",
    )
    .await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;
    h.ctx()
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let endpoint = format!(
        "sql:INSERT INTO test_batch_mixed (f, i) VALUES (#, #)?db_url={}&sslMode=disable&batch=true",
        conn_str
    );

    // Both reported bodies in one message: position f mixes a float item
    // with integer items, and the reverse profile — the reporter's exact
    // values. Fired twice so the second message reuses the warmed statement
    // cache of the same endpoint.
    let combined_body = serde_json::json!([[10.5, 1], [20, 2], [3, 3], [5, 4], [6.5, 5]]);
    let route = RouteBuilder::from("timer:kinds?period=50&repeatCount=2")
        .set_body(Value::from(combined_body))
        .to(endpoint)
        .to("mock:result")
        .route_id("sql-batch-kinds")
        .build()
        .unwrap();
    h.add_route(route).await.unwrap();
    h.start().await;

    wait_for_mock_exchanges(h.mock(), "result", 2).await;
    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    // Reporter's exact sequence, twice: f = 10.5, 1e-322, 1.5e-323, 5,
    // 4.619e18 on the unfixed code; exact values after the fix.
    let rows = sqlx::query(sqlx::AssertSqlSafe(
        "SELECT f, i FROM test_batch_mixed ORDER BY id",
    ))
    .fetch_all(&pool)
    .await
    .expect("Failed to read stored rows");

    let f_vals: Vec<f64> = rows
        .iter()
        .map(|row| row.try_get(0).expect("f column should decode as f64"))
        .collect();
    let i_vals: Vec<i64> = rows
        .iter()
        .map(|row| row.try_get(1).expect("i column should decode as i64"))
        .collect();
    assert_eq!(
        f_vals,
        vec![10.5, 20.0, 3.0, 5.0, 6.5, 10.5, 20.0, 3.0, 5.0, 6.5],
        "f column corrupted (numeric bits reinterpreted?)"
    );
    assert_eq!(
        i_vals,
        vec![1, 2, 3, 4, 5, 1, 2, 3, 4, 5],
        "i column corrupted"
    );
}

/// Homogeneous batches (every item binds the same JSON number kind per
/// position) keep inserting exactly, across the described numeric column
/// types (SMALLINT/INTEGER/REAL/DOUBLE PRECISION).
#[tokio::test(flavor = "multi_thread")]
async fn batch_homogeneous_kinds_unchanged() {
    install_crypto_provider();
    let conn_str = shared_postgres_17().await.to_string();
    let pool = create_pool(&conn_str).await;

    setup_table(
        &pool,
        "CREATE TABLE test_batch_homo_float (id SERIAL PRIMARY KEY, f DOUBLE PRECISION, r REAL)",
    )
    .await;
    setup_table(
        &pool,
        "CREATE TABLE test_batch_homo_int (id SERIAL PRIMARY KEY, s SMALLINT, i INTEGER)",
    )
    .await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;
    h.ctx()
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let float_endpoint = format!(
        "sql:INSERT INTO test_batch_homo_float (f, r) VALUES (#, #)?db_url={}&sslMode=disable&batch=true",
        conn_str
    );
    let int_endpoint = format!(
        "sql:INSERT INTO test_batch_homo_int (s, i) VALUES (#, #)?db_url={}&sslMode=disable&batch=true",
        conn_str
    );

    run_batch_route(
        &h,
        "homo-float",
        50,
        serde_json::json!([[1.5, 10.5], [2.5, 20.5]]),
        &float_endpoint,
    )
    .await;
    run_batch_route(
        &h,
        "homo-int",
        200,
        serde_json::json!([[1, 100], [2, 200], [3, 30000]]),
        &int_endpoint,
    )
    .await;
    h.start().await;

    wait_for_mock_exchanges(h.mock(), "result", 2).await;
    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let float_rows = read_f64_cols(&pool, "test_batch_homo_float", "f", "r").await;
    assert_eq!(
        float_rows,
        vec![(1.5, 10.5), (2.5, 20.5)],
        "all-float batch wrong (REAL/DOUBLE wire widths?)"
    );
    let int_rows = read_i64_cols(&pool, "test_batch_homo_int", "s", "i").await;
    assert_eq!(
        int_rows,
        vec![(1, 100), (2, 200), (3, 30000)],
        "all-int batch wrong (SMALLINT/INTEGER wire widths?)"
    );
}

/// NUMERIC columns have no `Any` parameter-type mapping, so batches fall
/// back to the scan + uncached statements. Two sequential messages through
/// the same pool exercise both the probe-created and the probe-hit
/// incompatible cache entries; values stay exact.
#[tokio::test(flavor = "multi_thread")]
async fn batch_numeric_column_fallback_works_twice() {
    install_crypto_provider();
    let conn_str = shared_postgres_17().await.to_string();
    let pool = create_pool(&conn_str).await;

    setup_table(
        &pool,
        "CREATE TABLE test_batch_numeric (id SERIAL PRIMARY KEY, n NUMERIC, m NUMERIC)",
    )
    .await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;
    h.ctx()
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let endpoint = format!(
        "sql:INSERT INTO test_batch_numeric (n, m) VALUES (#, #)?db_url={}&sslMode=disable&batch=true",
        conn_str
    );
    let route = RouteBuilder::from("timer:num-kinds?period=100&repeatCount=2")
        .set_body(Value::from(serde_json::json!([[20, 1], [6.5, 2]])))
        .to(endpoint)
        .to("mock:result")
        .route_id("sql-batch-numeric-kinds")
        .build()
        .unwrap();
    h.add_route(route).await.unwrap();
    h.start().await;

    wait_for_mock_exchanges(h.mock(), "result", 2).await;
    h.stop().await;

    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }

    let rows = read_text_cols(&pool, "test_batch_numeric", "n", "m").await;
    assert_eq!(
        rows,
        vec![
            ("20".to_string(), "1".to_string()),
            ("6.5".to_string(), "2".to_string()),
            ("20".to_string(), "1".to_string()),
            ("6.5".to_string(), "2".to_string()),
        ],
        "NUMERIC batch corrupted or reordered"
    );
}

/// Values that cannot be bound safely (integer beyond 2^53 at a float
/// position, non-numeric JSON at a numeric position, REAL overflow) fail
/// with a typed error and write no row.
#[tokio::test(flavor = "multi_thread")]
async fn batch_bad_values_fail_without_rows() {
    install_crypto_provider();
    let conn_str = shared_postgres_17().await.to_string();
    let pool = create_pool(&conn_str).await;

    setup_table(
        &pool,
        "CREATE TABLE test_batch_guard (id SERIAL PRIMARY KEY, f DOUBLE PRECISION, r REAL)",
    )
    .await;

    let cases = [
        (serde_json::json!([[9007199254740993i64, 1]]), "2^53"),
        (
            serde_json::json!([["12345678", 1]]),
            "string value at a numeric parameter",
        ),
        (
            serde_json::json!([[1.0, 1e40]]),
            "overflows the described REAL",
        ),
    ];

    for (idx, (body, expected_fragment)) in cases.iter().enumerate() {
        let h = CamelTestContext::builder()
            .with_timer()
            .with_mock()
            .with_component(SqlComponent::new())
            .build()
            .await;
        h.ctx()
            .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
            .await
            .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
            .await;

        let endpoint = format!(
            "sql:INSERT INTO test_batch_guard (f, r) VALUES (#, #)?db_url={}&sslMode=disable&batch=true",
            conn_str
        );
        let route = RouteBuilder::from(&format!("timer:guard-{idx}?period=50&repeatCount=1"))
            .set_body(Value::from(body.clone()))
            .to(endpoint)
            .to("mock:result")
            .route_id(format!("sql-batch-guard-{idx}"))
            .build()
            .unwrap();
        h.add_route(route).await.unwrap();
        h.start().await;

        let error_ep = h.mock().get_endpoint("error").expect("mock error endpoint");
        wait_until(
            "batch guard typed error arrival",
            std::time::Duration::from_secs(5),
            std::time::Duration::from_millis(100),
            || {
                let error_ep = error_ep.clone();
                async move { Ok(!error_ep.get_received_exchanges().await.is_empty()) }
            },
        )
        .await
        .unwrap();
        h.stop().await;

        let errors = error_ep.get_received_exchanges().await;
        let text = format!("{:?}", errors[0].error);
        assert!(
            text.contains(expected_fragment),
            "case {idx}: expected typed error mentioning {expected_fragment:?}, got {text}"
        );

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM test_batch_guard")
            .fetch_one(&pool)
            .await
            .expect("Failed to count rows");
        assert_eq!(count, 0, "case {idx}: failed batch must write no rows");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn producer_noop() {
    install_crypto_provider();
    let conn_str = shared_postgres().await.to_string();
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
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
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
    install_crypto_provider();
    let conn_str = shared_postgres().await.to_string();
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
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
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
    install_crypto_provider();
    let conn_str = shared_postgres().await.to_string();
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
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
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
    install_crypto_provider();
    let conn_str = shared_postgres().await.to_string();
    let pool = create_pool(&conn_str).await;

    setup_test_table(&pool, "test_empty").await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;
    h.ctx()
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
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
    install_crypto_provider();
    let conn_str = shared_postgres().await.to_string();
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
    install_crypto_provider();
    let conn_str = shared_postgres().await.to_string();
    let pool = create_pool(&conn_str).await;

    setup_test_table(&pool, "test_empty_routed").await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;
    h.ctx()
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
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

/// A NUMERIC column is outside sqlx's `Any` type set: the route must fail with a
/// typed error naming the column, the driver-reported type, and the CAST
/// workaround — never a null-substituted body (bd rc-s7lb0).
#[tokio::test(flavor = "multi_thread")]
async fn producer_numeric_column_fails_loudly() {
    install_crypto_provider();
    let conn_str = shared_postgres().await.to_string();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;
    h.ctx()
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .to(format!(
            "sql:SELECT 1.5::numeric AS price?db_url={}&sslMode=disable",
            conn_str
        ))
        .to("mock:result")
        .route_id("sql-numeric-type-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    wait_for_mock_exchanges(h.mock(), "error", 1).await;
    h.stop().await;

    let result_ep = h.mock().get_endpoint("result").unwrap();
    assert!(
        result_ep.get_received_exchanges().await.is_empty(),
        "undecodable column must never produce a body"
    );

    let error_ep = h.mock().get_endpoint("error").unwrap();
    let errors = error_ep.get_received_exchanges().await;
    assert!(!errors.is_empty(), "NUMERIC column must fail the route");
    let err_msg = errors[0]
        .error
        .as_ref()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(err_msg.contains("price"), "column named: {err_msg}");
    assert!(err_msg.contains("Numeric"), "driver type named: {err_msg}");
    assert!(
        err_msg.contains("CAST(price AS"),
        "workaround shown: {err_msg}"
    );
}

/// A TIMESTAMPTZ column is outside sqlx's `Any` type set: typed error with the
/// column, the driver type, and the CAST AS text workaround (bd rc-s7lb0).
#[tokio::test(flavor = "multi_thread")]
async fn producer_timestamptz_column_fails_loudly() {
    install_crypto_provider();
    let conn_str = shared_postgres().await.to_string();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;
    h.ctx()
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .to(format!(
            "sql:SELECT now()::timestamptz AS created_at?db_url={}&sslMode=disable",
            conn_str
        ))
        .to("mock:result")
        .route_id("sql-timestamptz-type-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    wait_for_mock_exchanges(h.mock(), "error", 1).await;
    h.stop().await;

    let result_ep = h.mock().get_endpoint("result").unwrap();
    assert!(
        result_ep.get_received_exchanges().await.is_empty(),
        "undecodable column must never produce a body"
    );

    let error_ep = h.mock().get_endpoint("error").unwrap();
    let errors = error_ep.get_received_exchanges().await;
    assert!(!errors.is_empty(), "TIMESTAMPTZ column must fail the route");
    let err_msg = errors[0]
        .error
        .as_ref()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(err_msg.contains("created_at"), "column named: {err_msg}");
    assert!(
        err_msg.contains("Timestamptz"),
        "driver type named: {err_msg}"
    );
    assert!(
        err_msg.contains("CAST(created_at AS text)"),
        "text workaround shown: {err_msg}"
    );
}

/// A non-UTF-8 BYTEA value is a real server-sent value: it must fail with a
/// typed error. On the unfixed code path it silently became JSON null —
/// data loss disguised as a value (bd rc-s7lb0).
#[tokio::test(flavor = "multi_thread")]
async fn producer_non_utf8_bytea_fails_loudly() {
    install_crypto_provider();
    let conn_str = shared_postgres().await.to_string();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;
    h.ctx()
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .to(format!(
            "sql:SELECT decode('00ff00', 'hex') AS payload?db_url={}&sslMode=disable",
            conn_str
        ))
        .to("mock:result")
        .route_id("sql-bytea-type-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    // Either the typed error dead-letters (fixed behavior) or a body arrives
    // (unfixed behavior: {"payload": null}). Wait for whichever lands first.
    let error_ep = h.mock().get_endpoint("error").unwrap();
    let result_ep = h.mock().get_endpoint("result").unwrap();
    wait_until(
        "sql bytea outcome (error dead-letter or body)",
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(100),
        || {
            let error_ep = error_ep.clone();
            let result_ep = result_ep.clone();
            async move {
                Ok(error_ep.get_received_exchanges().await.len() >= 1
                    || result_ep.get_received_exchanges().await.len() >= 1)
            }
        },
    )
    .await
    .unwrap();
    h.stop().await;

    let results = result_ep.get_received_exchanges().await;
    assert!(
        results.is_empty(),
        "non-UTF-8 BYTEA must fail loudly, not substitute null; got body: {:?}",
        results.first().map(|ex| &ex.input.body)
    );

    let errors = error_ep.get_received_exchanges().await;
    assert!(!errors.is_empty(), "non-UTF-8 BYTEA must fail the route");
    let err_msg = errors[0]
        .error
        .as_ref()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(err_msg.contains("payload"), "column named: {err_msg}");
    assert!(err_msg.contains("BLOB"), "driver type named: {err_msg}");
    assert!(
        err_msg.contains("CAST(payload AS text)"),
        "workaround shown: {err_msg}"
    );
}

/// A plain SQL NULL column stays JSON null — the typed undecodable error must
/// never fire on it (bd rc-s7lb0).
#[tokio::test(flavor = "multi_thread")]
async fn producer_sql_null_stays_null() {
    install_crypto_provider();
    let conn_str = shared_postgres().await.to_string();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;
    h.ctx()
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .to(format!(
            "sql:SELECT 7::bigint AS ok_col, NULL::text AS maybe_null?db_url={}&sslMode=disable",
            conn_str
        ))
        .to("mock:result")
        .route_id("sql-null-test")
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
    let body = match &exchanges[0].input.body {
        Body::Json(v) => v,
        other => panic!("Body should be JSON, got: {other:?}"),
    };
    let row = &body.as_array().expect("SelectList body should be an array")[0];
    assert_eq!(row.get("ok_col").and_then(|v| v.as_i64()), Some(7));
    assert!(
        row.get("maybe_null").map(|v| v.is_null()).unwrap_or(false),
        "SQL NULL must stay null: {row}"
    );
}

/// The polling consumer surfaces one clear typed error per undecodable poll
/// (via bridgeErrorHandler) — never a null-substituted row (bd rc-s7lb0).
#[tokio::test(flavor = "multi_thread")]
async fn consumer_undecodable_column_fails_loudly() {
    install_crypto_provider();
    let conn_str = shared_postgres().await.to_string();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(SqlComponent::new())
        .build()
        .await;
    h.ctx()
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let sql_uri = format!(
        "sql:SELECT 1.5::numeric AS price?db_url={}&sslMode=disable&delay=200&initialDelay=50&bridgeErrorHandler=true",
        conn_str
    );
    let consumer_route = RouteBuilder::from(sql_uri.as_str())
        .to("mock:consumed")
        .route_id("sql-consumer-type-test")
        .build()
        .unwrap();

    h.add_route(consumer_route).await.unwrap();
    h.start().await;

    // The bridged poll error lands on the route downstream (mock:error via the
    // dead-letter channel, or mock:consumed as a pre-errored exchange, depending
    // on where the route delivers it). Wait for either to receive one.
    let error_ep = h.mock().get_endpoint("error").unwrap();
    let consumed_ep = h.mock().get_endpoint("consumed").unwrap();
    wait_until(
        "sql consumer undecodable outcome",
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(100),
        || {
            let error_ep = error_ep.clone();
            let consumed_ep = consumed_ep.clone();
            async move {
                Ok(error_ep.get_received_exchanges().await.len() >= 1
                    || consumed_ep.get_received_exchanges().await.len() >= 1)
            }
        },
    )
    .await
    .unwrap();
    h.stop().await;

    let mut errored: Vec<String> = Vec::new();
    for exchange in error_ep.get_received_exchanges().await {
        match exchange.error {
            Some(e) => errored.push(e.to_string()),
            None => panic!("clean body on error endpoint: {:?}", exchange.input.body),
        }
    }
    for exchange in consumed_ep.get_received_exchanges().await {
        match exchange.error {
            Some(e) => errored.push(e.to_string()),
            None => panic!(
                "null-substituted row reached the route: {:?}",
                exchange.input.body
            ),
        }
    }
    assert!(
        !errored.is_empty(),
        "undecodable poll must surface an error"
    );
    for msg in &errored {
        assert!(msg.contains("price"), "column named: {msg}");
        assert!(msg.contains("Numeric"), "driver type named: {msg}");
        assert!(msg.contains("CAST(price AS"), "workaround shown: {msg}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn sql_ssl_connection_roundtrip() {
    install_crypto_provider();
    let conn_str = shared_postgres_tls().await.to_string();

    // Verify SSL handshake via direct PgPool (bypasses `any` driver)
    let pg_pool = timeout_wait(
        "PgPool TLS connect",
        std::time::Duration::from_secs(60),
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&conn_str),
    )
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
        .lock() // allow-test-wait: harness ctx mutex test-owned and uncontended in-test
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
