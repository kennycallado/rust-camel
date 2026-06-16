# camel-component-surrealdb

SurrealDB multi-model database component for [rust-camel](https://crates.io/crates/camel-api).

Provides **document**, **graph**, **vector**, and **live query** operations through the [`surrealdb`](https://crates.io/crates/surrealdb) crate (v3.x). Integrates with the rust-camel datasource domain via `PoolFactory`.

## Overview

SurrealDB is a multi-model database that supports document, graph (RELATE), and vector similarity search in a single engine. This component exposes those capabilities through 9 operations, each triggered by a URI path segment:

| Operation | Kind | Description |
|-----------|------|-------------|
| `query` | Producer / Polling | Run raw SurrealQL from body, header, or URI |
| `select` | Producer / Polling | Select all records or a single record by ID |
| `create` | Producer | Create a record from JSON body |
| `update` | Producer | Merge-update a record by ID |
| `upsert` | Producer | Create-or-replace a record by ID (full content) |
| `delete` | Producer | Delete a record by ID |
| `patch` | Producer | Apply RFC 6902 JSON Patch operations (add/remove/replace/change) |
| `relate` | Producer | Create a graph edge (RELATE) between two records |
| `vector` | Producer | Store/embed a vector field on a record |
| `search` | Producer | KNN vector similarity search |
| `run` | Producer | Execute a SurrealDB function (`fn::`, `math::`, `string::`, etc.) |
| `live` | Consumer | Push-based LIVE SELECT (change data capture) |

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-component-surrealdb = "*"
```

For CLI usage, the `surrealdb` feature is enabled by default in `camel-cli`.

## URI Format

```
surrealdb:{operation}?datasource={name}&table={table}&...
```

All endpoints require a named `datasource` (defined in `Camel.toml`). Additional parameters depend on the operation.

## Datasource Configuration

Define datasources in `Camel.toml`:

```toml
[default.datasources.mydb]
db_url = "ws://localhost:8000"
provider = "surrealdb"

[default.datasources.mydb.extra]
namespace = "test"
database = "test"
username = "root"
password = "root"
```

The `extra` map requires four fields: `namespace`, `database`, `username`, `password`.

**Protocols:** `ws://`, `wss://`, `http://`, `https://`. The `live` operation requires WebSocket (`ws`/`wss`) — HTTP-based live queries are rejected at endpoint creation.

## URI Options

### Common Options

| Option | Default | Description |
|--------|---------|-------------|
| `datasource` | _(required)_ | Named datasource from `Camel.toml` |
| `table` | - | Target table name (required for most operations) |
| `retryEnabled` | `true` | Enable retry with capped exponential backoff (see [Retry](#retry) below). Currently consumed at pool-create; producer-level retry is reserved for future use. |
| `retryMaxAttempts` | `10` | Max retry attempts (`0` = unlimited) |
| `retryInitialDelayMs` | `100` | Initial backoff delay in ms |
| `retryMultiplier` | `2.0` | Exponential backoff multiplier |
| `retryMaxDelayMs` | `30000` | Cap on per-attempt delay |
| `retryJitter` | `0.2` | Jitter factor in `[0.0, 1.0]` |

<a name="retry"></a>

#### Retry

The `retry*` URI params configure a [`NetworkRetryPolicy`](https://docs.rs/camel-component-api)
that is stored on the endpoint config as a public contract (parameter names
mirror `camel-sql` exactly).

**Current consumption:**

- **Datasource pool creation** (`SurrealDbPoolFactory::create`) retries the
  `connect()` transport-establishment call using `NetworkRetryPolicy::default()`.
  Only `Connection`-class errors are retried. Post-connect steps (`signin`,
  `use_ns`, `use_db`) are not retried — auth failures are permanent.

**Not yet consumed:**

- **Producer operations** are not currently wrapped in per-operation retry.
  Today's producer paths map SDK errors to non-retryable `Query` variants
  (see ADR-0013 — resending a non-idempotent write that may have reached the
  server would risk duplicates). The `retry` field on the endpoint config is
  the public contract for when a future classifier refinement distinguishes
  transport-drop from query-rejected on read operations, at which point
  producer-side retry can be wired without breaking the URI contract.

### Operation-Specific Options

#### SELECT

| Option | Default | Description |
|--------|---------|-------------|
| `table` | _(required)_ | Table to select from |
| `id` | - | Record ID. If omitted, selects all records in the table |

#### CREATE

| Option | Default | Description |
|--------|---------|-------------|
| `table` | _(required)_ | Table to insert into |

Body: JSON object with record fields.

#### UPDATE

| Option | Default | Description |
|--------|---------|-------------|
| `table` | _(required)_ | Table name |
| `id` | _(required)_ | Record ID to update |

Body: JSON object with fields to merge.

#### UPSERT

| Option | Default | Description |
|--------|---------|-------------|
| `table` | _(required)_ | Table name |
| `id` | _(required)_ | Record ID to create or replace |

Body: JSON object with full record content. Unlike `update` (which merges), `upsert` replaces the entire record content.

#### PATCH

| Option | Default | Description |
|--------|---------|-------------|
| `table` | _(required)_ | Table name |
| `id` | _(required)_ | Record ID to patch |

Body: JSON array of RFC 6902 patch operations:

```json
[
  {"op": "replace", "path": "/age", "value": 30},
  {"op": "add", "path": "/email", "value": "user@example.com"},
  {"op": "remove", "path": "/temp"},
  {"op": "change", "path": "/bio", "value": "diff string"}
]
```

Supported operations: `add`, `remove`, `replace`, `change`.

#### DELETE

| Option | Default | Description |
|--------|---------|-------------|
| `table` | _(required)_ | Table name |
| `id` | _(required)_ | Record ID to delete |

#### RELATE

| Option | Default | Description |
|--------|---------|-------------|
| `table` | _(required)_ | Source table (informational; the actual source/target tables come from `from`/`to` RecordIds) |
| `edge` | _(required)_ | Edge table name (the relationship) |
| `from` | _(required)_ | Full source RecordId in `table:key` form (e.g. `user:1`) |
| `to` | _(required)_ | Full target RecordId in `table:key` form (e.g. `topic:42`) |

Both `from` and `to` must be **full RecordIds** (`table:key`), not bare keys.
Bare keys are rejected at endpoint creation with a validation error — this
prevents silently coercing `from=1` into `{config.table}:1` and pointing the
edge at the wrong record.

Body: JSON object with edge properties.

#### VECTOR

| Option | Default | Description |
|--------|---------|-------------|
| `table` | _(required)_ | Table name |
| `vector_field` | `embedding` | Field name for the vector |

Body: JSON object containing the vector field as an array of `f32`.

#### SEARCH

| Option | Default | Description |
|--------|---------|-------------|
| `table` | _(required)_ | Table to search |
| `top_k` | _(required)_ | Number of nearest neighbors to return |
| `metric` | `cosine` | Distance metric: `cosine`, `euclidean`, `manhattan` |
| `vector_field` | `embedding` | Field containing the vectors |

Body: JSON array of `f32` (the query vector).

#### RUN

| Option | Default | Description |
|--------|---------|-------------|
| `function` | _(required)_ | Function name (e.g. `fn::add`, `math::sum`, `string::len`) |

Body: JSON array of positional arguments passed to the function. Each element becomes `$arg0`, `$arg1`, etc.

The function name is validated as a SurrealDB function identifier (ASCII alphanumeric, underscore, and `::` only — no injection).

#### LIVE

| Option | Default | Description |
|--------|---------|-------------|
| `table` | _(required)_ | Table to watch for changes |

Requires WebSocket protocol. Each CREATE/UPDATE/DELETE on the table pushes an Exchange through the route.

## Exchange Headers

### Input Headers

| Header | Operations | Description |
|--------|------------|-------------|
| `CamelSurrealDbQuery` | `query` | Override SurrealQL from header (priority: header > body > URI `query`) |
| `CamelSurrealDbParams` | `query` | JSON object map of `$param` → value bindings |
| `CamelSurrealDbVector` | `search` | Query vector as JSON array (alternative to body) |

### Output Headers

| Header | Operations | Description |
|--------|------------|-------------|
| `CamelSurrealDbRecordId` | all | RecordId (`table:key`) of the affected record, when one can be determined. For `update`/`upsert`/`delete`/`patch` it is constructed from URI `table`+`id` config; for `create`/`vector`/`relate` (and any write op whose URI config omits `id`) it is extracted from the response `body.id`. For `relate`, the value is the EDGE record's id (not the source node). Skipped (logged at `debug!`) when no id can be determined. |
| `CamelSurrealDbAction` | `live` | Action type: `CREATE`, `UPDATE`, `DELETE` |
| `CamelSurrealDbTable` | `live` | Table that triggered the notification |

## Result Body Shape

All multi-row operations (`query`, `select` without `id`, `search`) return
results as a materialized JSON array via `Body::Json(Vec<Value>)`. There is no
streaming output mode — SurrealDB's Rust SDK does not expose a row-at-a-time
stream for these operations, so all results are collected before the producer
returns.

## Usage

### CRUD — Create + Select

```rust
use camel_builder::RouteBuilder;
use camel_component_surrealdb::SurrealDbComponent;
use camel_core::context::CamelContext;

let mut ctx = CamelContext::builder().build().await?;
ctx.register_component(SurrealDbComponent::with_catalog(catalog));

// Create a user from JSON body
let route = RouteBuilder::from("direct:create-user")
    .set_body(json!({"name": "Alice", "age": 30}))
    .to("surrealdb:create?datasource=mydb&table=users")
    .build()?;

// Select all users
let route2 = RouteBuilder::from("direct:list-users")
    .to("surrealdb:select?datasource=mydb&table=users")
    .build()?;

// Select one by ID
let route3 = RouteBuilder::from("direct:get-user")
    .to("surrealdb:select?datasource=mydb&table=users&id=123")
    .build()?;
```

### Graph Edge (RELATE)

```rust
// Create a "knows" edge from user:1 to topic:42.
// `from` and `to` MUST be full RecordIds in `table:key` form.
let route = RouteBuilder::from("direct:relate")
    .set_body(json!({"since": "2024-01-15"}))
    .to("surrealdb:relate?datasource=mydb&table=user&edge=knows&from=user:1&to=topic:42")
    .build()?;
```

### Vector Similarity Search

```rust
// Store a vector
let store = RouteBuilder::from("direct:store-embedding")
    .set_body(json!({"text": "hello world", "embedding": [0.1, 0.2, 0.3]}))
    .to("surrealdb:vector?datasource=mydb&table=docs&vector_field=embedding")
    .build()?;

// Search top 5 nearest by cosine
let search = RouteBuilder::from("direct:search")
    .set_body(json!([0.15, 0.25, 0.35]))
    .to("surrealdb:search?datasource=mydb&table=docs&top_k=5&metric=cosine")
    .build()?;
```

### Raw SurrealQL Query

```rust
// From URI
let route = RouteBuilder::from("direct:count")
    .to("surrealdb:query?datasource=mydb&query=SELECT%20count()%20FROM%20users")
    .build()?;

// From body (dynamic)
let route2 = RouteBuilder::from("direct:dynamic")
    .set_body("SELECT * FROM type::table($tbl) WHERE age > $min")
    .set_header("CamelSurrealDbParams", json!({"tbl": "users", "min": 18}))
    .to("surrealdb:query?datasource=mydb")
    .build()?;
```

### Live Consumer (Change Data Capture)

```rust
// Push-based: each CREATE/UPDATE/DELETE on "events" triggers the route
let route = RouteBuilder::from("surrealdb:live?datasource=mydb&table=events")
    .to("log:info")
    .build()?;
```

Each notification Exchange has:
- **Body:** the full record as JSON
- **`CamelSurrealDbAction`:** `CREATE`, `UPDATE`, or `DELETE`
- **`CamelSurrealDbTable`:** the table name

### Polling Consumer (pollEnrich)

The `select` and `query` operations support `PollingConsumer` for `pollEnrich` integration:

```rust
let route = RouteBuilder::from("direct:enrich")
    .poll_enrich("surrealdb:select?datasource=mydb&table=config&id=app:settings", 5000)
    .to("log:info")
    .build()?;
```

## Health Check

The `SurrealDbPoolFactory` implements `check()` by running `INFO FOR DB` against the datasource. Health status is reported through the standard `HealthCheckRegistry`.

## Security

- Identifier validation: `table`, `edge`, and `vector_field` parameters are validated as ASCII identifiers (no injection or Unicode tricks)
- Record IDs (`id`, `from`, `to`) allow a broader charset (digits, hyphens, colons for UUIDs and composite keys) but are still validated against injection patterns
- RELATE uses validated direct interpolation (SurrealDB v3 does not support bind variables in the RELATE position)
- Credentials are stored in `Camel.toml` extra map, never in route URIs

## Documentation

- [SurrealDB Documentation](https://surrealdb.com/docs)
- [SurrealQL Reference](https://surrealdb.com/docs/surrealql)
- [surrealdb Rust SDK](https://docs.rs/surrealdb)

## License

Licensed under either of Apache License, Version 2.0 or MIT License at your option.

## Contributing

Contributions are welcome. Please see the [rust-camel contributing guidelines](../../../README.md#contributing).
