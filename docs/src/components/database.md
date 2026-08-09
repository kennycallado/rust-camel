# Database

The database components connect routes to SQL and SurrealDB. Two crates cover the two database families. The SQL component uses `sqlx` for PostgreSQL, MySQL, and SQLite. The SurrealDB component uses the SurrealDB SDK for document, graph, vector, and live-query operations.

## SQL

The SQL component executes queries against relational databases through `sqlx`. It covers both directions. A Consumer polls query results. A Producer executes statements.

### URI

```
sql:<query>[?outputType=<mode>&dataSource=<name>&allowDynamicQuery=<bool>&bridgeErrorHandler=<bool>]
```

| Parameter | Required | Default | Description |
| --- | --- | --- | --- |
| `query` | yes | — | SQL query or statement. Can also come from a file |
| `outputType` | no | `SelectList` | `SelectList` (JSON array) or `StreamList` (NDJSON stream) |
| `dataSource` | no | `default` | Named datasource from `Camel.toml` |
| `allowDynamicQuery` | no | `false` | Allow query from Exchange header or body (ADR-0032) |
| `bridgeErrorHandler` | no | `false` | Route poll errors through the error handler |

### Consumer

`sql:SELECT * FROM users?outputType=SelectList` polls the query result. The Consumer submits one Exchange per poll cycle. The body carries the result set as `Body::Json`.

`outputType=StreamList` exposes rows as a lazy NDJSON stream. Combined with the streaming split EIP, this gives at-least-once row processing. The Consumer fetches rows from the database as the stream drains. It does not commit the batch until the whole split pipeline completes. If the pipeline crashes mid-stream, the batch re-delivers on the next poll. When a streaming split sub-pipeline uses `sql:` as a producer, set the pool `max_connections` to at least 2 to avoid a deadlock.

### Producer

`sql:UPDATE users SET name = :#name WHERE id = :#id` executes a statement with named parameters. The Producer supports positional `#` and named `:#<token>` placeholders. Named parameters resolve from the Exchange body, headers, and properties through `ExchangeLookupPath`.

The `allowDynamicQuery` parameter defaults to `false`. In this mode, the Producer ignores the `CamelSql.Query` header and the Exchange body. It uses only the query from its Endpoint configuration. This default enforces the exchange-data trust boundary (ADR-0032). Set `allowDynamicQuery=true` to let the route supply the query. The opt-in makes the route responsible for validating the source.

### Placeholder syntax

The SQL placeholder parser accepts these forms:

- **Positional `#`**. Body must be a JSON array. Bindings come from index.
- **Named `:#<token>`**. Resolves from the body JSON tree, headers, or properties.
- **IN-clause `:#in:<token>`**. Value must resolve to a JSON array.
- **Expression `:#${<expr>}`**. Escape hatch that uses `ExchangeLookupPath`.
- **PostgreSQL `::cast`**. `:#id::text` resolves the placeholder and leaves `::text` as the SQL cast.

The full contract surface with accepted, rejected, and forbidden patterns lives in the SQL crate CONTEXT.

## SurrealDB

The SurrealDB component connects routes to SurrealDB for document, graph, vector, function, and live-query operations. It uses the SurrealDB SDK directly.

### URI

```
surrealdb:<operation>?[dataSource=<name>&...]
```

The operation is the path segment in the URI. The 12 supported operations are:

| Operation | Description |
| --- | --- |
| `query` | Execute raw SurrealQL |
| `select` | Select records by ID or table |
| `create` | Create a new record |
| `update` | Update an existing record |
| `upsert` | Insert or update a record |
| `delete` | Delete a record |
| `patch` | Apply a JSON Patch to a record |
| `relate` | Create a graph edge between records |
| `vector` | Vector search operation |
| `search` | Full-text search |
| `run` | Run a SurrealDB function |
| `live` | Subscribe to live query notifications |

### Consumer

`surrealdb:live?dataSource=my-db` subscribes to live query notifications. The Consumer submits one Exchange per live event. Live queries require a WebSocket connection (`ws` or `wss`). HTTP connections are rejected for live operations.

### Producer

`surrealdb:query?dataSource=my-db` executes SurrealQL. The query comes from the `CamelSurrealDbQuery` header, the Exchange body, or the Endpoint configuration. Results materialize as `Body::Json`. The `output=stream` option is rejected.

### Security

Credentials belong in the named datasource, not in the Endpoint URI. The `redact_db_url` function removes user information before a URL enters a log or error message. Identifier validation rejects whitespace, quotes, semicolons, and backslashes in interpolated table, edge, and vector-field names.

The raw-query trust boundary is an open hardening gap. The `query` operation accepts SurrealQL from the `CamelSurrealDbQuery` header or the body before it falls back to Endpoint configuration. Route authors must filter these sources before the Producer. The SurrealDB crate CONTEXT documents the gap.

## Datasource configuration

Both components use named datasources from `Camel.toml`:

```toml
[components.datasources.my-db]
db_url = "postgres://user:pass@localhost/mydb"
```

The SQL component also supports inline connection parameters in the URI. The SurrealDB component requires credentials in the named datasource.

**Reference**: [SQL crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-sql/CONTEXT.md), [SurrealDB crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-surrealdb/CONTEXT.md).
