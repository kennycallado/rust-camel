# camel-component-sql

> SQL component for rust-camel integration framework

## Overview

The SQL component provides comprehensive database integration for rust-camel, supporting both producer (query execution) and consumer (polling) patterns. It enables seamless interaction with SQL databases using parameterized queries, batch operations, and streaming results.

## Features

- **Producer Mode**: Execute SQL queries (SELECT, INSERT, UPDATE, DELETE)
- **Consumer Mode**: Poll database tables for new rows
- **Parameter Binding**: Named (`:#name`), positional (`#`), IN clause (`:#in:ids`), and expressions (`:#${body.field}`)
- **Batch Operations**: Execute multiple inserts/updates in a single transaction
- **Streaming Results**: Stream large result sets as NDJSON without loading all rows into memory
- **Post-Processing Hooks**: `onConsume`, `onConsumeFailed`, `onConsumeBatchComplete` for consumer workflows
- **Connection Pooling**: Configurable pool with min/max connections, idle timeout, and max lifetime
- **Async Native**: Built on `tokio` and `sqlx`

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-component-sql = "*"
```

## URI Format

```
sql:<query>?db_url=<database-url>&options
sql:file:<path-to-sql-file>?db_url=<database-url>&options
```

- `<query>`: SQL query with optional parameter placeholders
- `<path-to-sql-file>`: Path to a file containing the SQL query
- `<database-url>`: Database connection URL (required)

## URI Options

### Connection Options

| Option | Default | Description |
|--------|---------|-------------|
| `db_url` | (required) | Database connection URL |
| `maxConnections` | `5` | Maximum connections in pool |
| `minConnections` | `1` | Minimum connections in pool |
| `idleTimeoutSecs` | `300` | Idle connection timeout in seconds |
| `maxLifetimeSecs` | `1800` | Maximum connection lifetime in seconds |

### Query Options

| Option | Default | Description |
|--------|---------|-------------|
| `outputType` | `SelectList` | Output format: `SelectList`, `SelectOne`, `StreamList` |
| `placeholder` | `#` | Character used for parameter placeholders |
| `separator` | `;` | Statement separator for multi-statement queries |
| `noop` | `false` | If true, preserve original body after DML operations |

### Consumer Options

| Option | Default | Description |
|--------|---------|-------------|
| `delay` | `500` | Polling delay in milliseconds |
| `initialDelay` | `1000` | Initial delay before first poll (ms) |
| `maxMessagesPerPoll` | - | Maximum rows to process per poll |
| `onConsume` | - | SQL to execute after successful row processing |
| `onConsumeFailed` | - | SQL to execute after failed row processing |
| `onConsumeBatchComplete` | - | SQL to execute after batch completes |
| `routeEmptyResultSet` | `false` | Process empty result sets |
| `useIterator` | `true` | Process rows individually (true) or as batch (false) |
| `expectedUpdateCount` | - | Expected rows affected (error if mismatch) |
| `breakBatchOnConsumeFail` | `false` | Stop batch processing on failure |

### Producer Options

| Option | Default | Description |
|--------|---------|-------------|
| `batch` | `false` | Enable batch mode (body must be array of arrays) |
| `useMessageBodyForSql` | `false` | Use message body as SQL query |

## Parameter Binding

The SQL component supports multiple parameter placeholder styles:

### Positional Parameters (`#`)

```sql
INSERT INTO users (name, age) VALUES (#, #)
```

Body must be a JSON array: `["Alice", 30]`

### Named Parameters (`:#name`)

```sql
SELECT * FROM users WHERE id = :#id AND status = :#status
```

Values resolved from body (if JSON object), headers, or properties.

### IN Clause (`:#in:name`)

```sql
SELECT * FROM users WHERE id IN (:#in:ids)
```

Value must be a JSON array: `[1, 2, 3]` → expands to `IN ($1, $2, $3)`

### Expression Parameters (`:#${expr}`)

```sql
SELECT * FROM users WHERE id = :#${body.user_id} AND name = :#${header.userName}
```

Supported expressions: `body.field`, `header.name`, `property.key`

## Usage

### SELECT Query

```rust
use camel_builder::RouteBuilder;
use camel_component_sql::SqlComponent;

let mut ctx = CamelContext::new();
ctx.register_component(SqlComponent::new());

let route = RouteBuilder::from("direct:get-users")
    .to("sql:SELECT * FROM users?db_url=postgres://localhost/mydb")
    .build()?;
ctx.add_route_definition(route)?;
```

### Parameterized Query

```rust
// Named parameters from body
let route = RouteBuilder::from("direct:get-user")
    .set_body(Body::Json(serde_json::json!({"id": 42})))
    .to("sql:SELECT * FROM users WHERE id = :#id?db_url=postgres://localhost/mydb")
    .build()?;

// Positional parameters from array body
let route = RouteBuilder::from("direct:insert")
    .set_body(Body::Json(serde_json::json!(["Alice", 30])))
    .to("sql:INSERT INTO users (name, age) VALUES (#, #)?db_url=postgres://localhost/mydb")
    .build()?;
```

### Streaming Large Results

```rust
// Stream results as NDJSON (memory efficient for large datasets)
let route = RouteBuilder::from("direct:export")
    .to("sql:SELECT * FROM large_table?db_url=postgres://localhost/mydb&outputType=StreamList")
    .to("file:/tmp/export.ndjson")
    .build()?;
```

### Batch Insert

```rust
// Batch insert with transaction
let route = RouteBuilder::from("direct:batch-insert")
    .set_body(Body::Json(serde_json::json!([
        ["Alice", 30],
        ["Bob", 25],
        ["Charlie", 35]
    ])))
    .to("sql:INSERT INTO users (name, age) VALUES (#, #)?db_url=postgres://localhost/mydb&batch=true")
    .build()?;
```

### Dynamic Query from Body

```rust
// Use message body as SQL
let route = RouteBuilder::from("direct:dynamic")
    .set_body(Body::Text("SELECT COUNT(*) FROM users".to_string()))
    .to("sql:placeholder?db_url=postgres://localhost/mydb&useMessageBodyForSql=true")
    .build()?;
```

### Polling Consumer

```rust
// Poll for pending orders and mark as processed
let route = RouteBuilder::from(
    "sql:SELECT * FROM orders WHERE status = 'pending'?\
     db_url=postgres://localhost/mydb\
     &delay=5000\
     &onConsume=UPDATE orders SET status = 'processed' WHERE id = :#id\
     &onConsumeFailed=UPDATE orders SET status = 'failed' WHERE id = :#id"
)
    .process(|ex| async move {
        // Process each order
        Ok(ex)
    })
    .build()?;
```

### Load Query from File

```rust
// Load SQL from external file
let route = RouteBuilder::from("direct:query")
    .to("sql:file:/etc/queries/get-users.sql?db_url=postgres://localhost/mydb")
    .build()?;
```

## Exchange Headers

### Input Headers

| Header | Description |
|--------|-------------|
| `CamelSql.Query` | Override the SQL query from URI |
| `CamelSql.Parameters` | Override parameters as JSON array |

### Output Headers

| Header | Direction | Description |
|--------|-----------|-------------|
| `CamelSql.RowCount` | out | Number of rows returned by SELECT |
| `CamelSql.UpdateCount` | out | Number of rows affected by INSERT/UPDATE/DELETE |

### Consumer Row Headers

When `useIterator=true`, each row's columns are also set as headers:

| Header Pattern | Description |
|----------------|-------------|
| `CamelSql.<column>` | Column value from current row (e.g., `CamelSql.id`, `CamelSql.name`) |

## Output Types

| Type | Description | Body Format |
|------|-------------|-------------|
| `SelectList` | All rows as array | `[{...}, {...}]` |
| `SelectOne` | First row only | `{...}` or empty |
| `StreamList` | Stream rows on demand | NDJSON stream (`{...}\n{...}\n`) |

## Example: Order Processing Pipeline

```rust
use camel_builder::RouteBuilder;
use camel_component_sql::SqlComponent;
use camel_core::CamelContext;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut ctx = CamelContext::new();
    ctx.register_component(SqlComponent::new());

    // Producer: Insert new orders
    let producer = RouteBuilder::from("direct:create-order")
        .process(|ex| async move {
            // Transform input to order record
            Ok(ex)
        })
        .to("sql:INSERT INTO orders (customer_id, total) VALUES (#, #)?db_url=postgres://localhost/mydb&expectedUpdateCount=1")
        .build()?;

    // Consumer: Process pending orders
    let consumer = RouteBuilder::from(
        "sql:SELECT * FROM orders WHERE status = 'pending' ORDER BY created_at?\
         db_url=postgres://localhost/mydb\
         &delay=2000\
         &maxMessagesPerPoll=10\
         &onConsume=UPDATE orders SET status = 'completed', processed_at = NOW() WHERE id = :#id"
    )
        .process(|ex| async move {
            let order_id = ex.input.header("CamelSql.id").and_then(|v| v.as_i64());
            println!("Processing order: {:?}", order_id);
            Ok(ex)
        })
        .build()?;

    ctx.add_route_definition(producer)?;
    ctx.add_route_definition(consumer)?;
    ctx.start().await?;

    Ok(())
}
```

## Global Configuration

Configure default connection pool settings in `Camel.toml` that apply to all SQL endpoints:

```toml
[default.components.sql]
max_connections = 5          # Maximum pool connections (default: 5)
min_connections = 1          # Minimum pool connections (default: 1)
idle_timeout_secs = 300      # Idle connection timeout (default: 300)
max_lifetime_secs = 1800     # Max connection lifetime (default: 1800)
```

URI parameters always override global defaults:

```rust
// Uses global pool settings
.to("sql:SELECT * FROM users?db_url=postgres://localhost/mydb")

// Overrides maxConnections from global config
.to("sql:SELECT * FROM users?db_url=postgres://localhost/mydb&maxConnections=10")
```

### Profile-Specific Configuration

```toml
[default.components.sql]
max_connections = 5
min_connections = 1

[production.components.sql]
max_connections = 20
min_connections = 5
idle_timeout_secs = 600
```

## Documentation

- [API Documentation](https://docs.rs/camel-component-sql)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
