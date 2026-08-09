# SurrealDB

The SurrealDB component connects routes to a SurrealDB multi-model database. One scheme covers twelve operations: document CRUD, graph edges, vector search, function calls, and live-query change data capture. Both directions share the same `surrealdb:` scheme. The Consumer is push-based and limited to `live`; the Producer handles the other eleven operations.

A producer route writes a user and selects the table:

```rust,ignore
use camel_builder::RouteBuilder;

let create = RouteBuilder::from("timer:tick?period=2000&repeatCount=1")
    .set_body(serde_json::json!({"name": "Alice", "age": 30}))
    .to("surrealdb:create?datasource=demo&table=users")
    .build()?;

let list = RouteBuilder::from("timer:tick?period=2000&repeatCount=1&delay=500")
    .to("surrealdb:select?datasource=demo&table=users")
    .build()?;
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: create-user
    from: "timer:tick?period=2000&repeatCount=1"
    steps:
      - set_body: '{"name":"Alice","age":30}'
      - to: "surrealdb:create?datasource=demo&table=users"

  - id: list-users
    from: "timer:tick?period=2000&repeatCount=1&delay=500"
    steps:
      - to: "surrealdb:select?datasource=demo&table=users"
```

</details>

A live-query route turns table changes into a stream of Exchanges:

```rust,ignore
let live = RouteBuilder::from("surrealdb:live?datasource=demo&table=events")
    .to("log:info?showBody=true")
    .build()?;
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: events-cdc
    from: "surrealdb:live?datasource=demo&table=events"
    steps:
      - to: "log:info?showBody=true"
```

</details>

## URI

```
surrealdb:<operation>?datasource=<name>[&table=<t>][&id=<r>][&edge=<e>][&from=<r>][&to=<r>][&function=<f>][&top_k=<n>][&metric=<m>][&vector_field=<f>][&query=<surrealql>]
```

The path segment is the operation. The component rejects the URI if `datasource` is missing, the operation is unknown, or `output=stream` is set. Streaming output is not supported. All results materialize as `Body::Json(Vec<Value>)`.

| Operation | Direction | Required URI params | Description |
| --- | --- | --- | --- |
| `query` | producer / polling | — | Run raw SurrealQL from URI, header, or body |
| `select` | producer / polling | `table` | Select all rows or one row by `id` |
| `create` | producer | `table` | Insert a new record from a JSON body |
| `update` | producer | `table`, `id` | Merge JSON body fields into an existing record |
| `upsert` | producer | `table`, `id` | Replace record content (`id` present or new) |
| `delete` | producer | `table`, `id` | Delete a record by id |
| `patch` | producer | `table`, `id` | Apply RFC 6902 JSON Patch operations |
| `relate` | producer | `table`, `edge`, `from`, `to` | Create a graph edge between two records |
| `vector` | producer | `table` | Store a vector field on a record |
| `search` | producer | `table`, `top_k` | KNN vector similarity search |
| `run` | producer | `function` | Run a SurrealDB function (`fn::`, `math::`, `string::`) |
| `live` | consumer | `table` | Subscribe to live-query notifications (push model) |

## Common parameters

| Parameter | Required | Default | Description |
| --- | --- | --- | --- |
| `datasource` | yes | — | Named datasource from `Camel.toml` |
| `table` | per operation | — | Target table for the operation |
| `id` | per operation | — | Record id in `table:key` form |
| `from` | `relate` | — | Source RecordId, full `table:key` form |
| `to` | `relate` | — | Target RecordId, full `table:key` form |
| `edge` | `relate` | — | Edge table name (the relationship) |
| `top_k` | `search` | — | Number of nearest neighbors to return (must be `> 0`) |
| `metric` | no | `cosine` | `cosine`, `euclidean`, or `manhattan` (case-insensitive) |
| `vector_field` | no | `embedding` | Field that holds the vector |
| `function` | `run` | — | Function name, validated as ASCII identifier with `::` |
| `query` | no | — | Inline SurrealQL for the `query` operation |

Retry policy parameters (`retryEnabled`, `retryMaxAttempts`, `retryInitialDelayMs`, `retryMultiplier`, `retryMaxDelayMs`, `retryJitter`) match `camel-sql` by name and default. They configure pool-establishment retry today. ADR-0013 owns the retry semantics; producer operations do not retry to avoid duplicating non-idempotent writes.

## Producer

The Producer executes the eleven non-`live` operations. The body shape the Producer requires depends on the operation. `create`, `update`, `upsert`, `patch`, `relate`, `vector`, and `run` require a `Body::Json` body. `query`, `select`, `search`, and `delete` accept input through URI parameters or headers, so an empty body is valid.

`select` and `query` also expose a `PollingConsumer`. They integrate with the [Poll Enrich](../eip/poll-enrich.md) EIP for on-demand reads. Other operations do not poll.

Result bodies land as `Body::Json`. The Producer sets `CamelSurrealDbRecordId` on writes when the id can be derived from the URI or the response. The id is the edge record for `relate`, not the source node.

The `query` operation reads SurrealQL from the URI, then the `CamelSurrealDbQuery` header, then the Exchange body. The `CamelSurrealDbParams` header binds `$name` placeholders. ADR-0032 classifies Exchange data as untrusted. The component has no `allow_dynamic_query` switch. The route is responsible for sanitizing SurrealQL from external sources before it reaches the Producer.

## Consumer

`surrealdb:live?datasource=<name>&table=<t>` subscribes to a SurrealDB live query. SurrealDB pushes one notification per `CREATE`, `UPDATE`, or `DELETE` on the table. The Consumer submits one Exchange per notification. The body holds the affected record. The Consumer sets `CamelSurrealDbAction` to the action type and `CamelSurrealDbTable` to the table name.

Live queries require a WebSocket transport (`ws://` or `wss://`). The component rejects `http://` and `https://` datasources for the `live` operation at endpoint creation.

The Consumer is event-driven. It does not implement `PollingConsumer`. The `live` operation exposes no `receive()` entry point.

## Datasource

The `datasource` URI parameter names a `Camel.toml` entry. Credentials belong in the datasource, not the URI. The component redacts user information from `db_url` before any URL enters a log line or error message.

```toml
[default.datasources.demo]
db_url = "ws://localhost:8000"
provider = "surrealdb"

[default.datasources.demo.extra]
namespace = "test"
database = "test"
username = "root"
password = "root"
```

The `extra` map carries `namespace`, `database`, `username`, and `password`. The transport schemes are `ws://`, `wss://`, `http://`, and `https://`. The `live` operation requires `ws` or `wss`. See the [Database](database.md#datasource-configuration) page for the shared datasource contract.

## Headers

| Header | Direction | Operations | Description |
| --- | --- | --- | --- |
| `CamelSurrealDbQuery` | input | `query` | SurrealQL text (priority: header > body > URI) |
| `CamelSurrealDbParams` | input | `query` | JSON object map of `$name` → value bindings |
| `CamelSurrealDbVector` | input | `search` | Query vector as JSON array of `f32` (alternative to body) |
| `CamelSurrealDbRecordId` | output | all writes | Resolved `table:key` id when one can be determined |
| `CamelSurrealDbAction` | output | `live` | `CREATE`, `UPDATE`, or `DELETE` |
| `CamelSurrealDbTable` | output | `live` | Table that triggered the notification |

## Security

Identifier validation runs on `table`, `edge`, and `vector_field`. The validator accepts ASCII letters, digits, and `_`. The first character must be a letter or `_`. Whitespace, quotes, semicolons, and backslashes are rejected. Record keys (`id`, `from`, `to`) go through SDK binding and accept the broader charset SurrealDB requires for numeric, UUID, and string keys.

`from` and `to` for `relate` must be full RecordIds in `table:key` form. Bare keys are rejected at endpoint creation. This prevents the edge from silently targeting the wrong record when a route author writes `from=1` instead of `from=user:1`.

The raw-query trust boundary is the documented hardening gap for this component. The `query` operation accepts SurrealQL from the body and the `CamelSurrealDbQuery` header. The route must filter these sources before the Producer. The component exposes no default-deny switch equivalent to `camel-sql`'s `allowDynamicQuery=false`.

The component follows the [ADR-0012](../adr/0012-log-level-convention-handler-contract-boundaries.md) log-level convention. It never emits `error!` in production code. Recoverable and handler-owned conditions use `warn!`. Errors that terminate an operation return `CamelError` or `SurrealDbError`. The route error handler or supervision owns the final signal. ADR-0020 classifies the SurrealDB SDK as a stable database-driver class. The crate does not wrap the SDK in a project-owned adapter trait.

**Reference**: [SurrealDB crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-surrealdb/CONTEXT.md). Example source: [`examples/surrealdb-example`](https://github.com/kennycallado/rust-camel/tree/main/examples/surrealdb-example).
