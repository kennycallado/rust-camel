# camel-component-surrealdb

SurrealDB Component for document, graph, vector, function, and live-query
operations. `README.md` owns user-facing configuration and examples. This file
owns crate-local architecture and ADR links.

## Language

**SurrealQL**:
SurrealDB query language for data, graph, vector, and schema operations.
_Avoid_: SQL (unless referring to a specific SQL-like syntax property)

**RecordId**:
Typed SurrealDB key in `table:key` form. Keys can be numeric, UUID, or string
values.
_Avoid_: row ID, primary key

**SurrealDB operation**:
The path segment in a `surrealdb:<operation>?...` Endpoint URI. The 12 values
are `query`, `select`, `create`, `update`, `upsert`, `delete`, `patch`,
`relate`, `vector`, `search`, `run`, and `live`.
_Avoid_: command, action

**SurrealDB datasource**:
A named `Camel.toml` datasource. `db_url` selects the server. The `extra` map
contains `namespace`, `database`, `username`, and `password`.
_Avoid_: connection string, database config

## Log-level policy

Per ADR-0012, this component does not emit `error!` in production code.
Recoverable and handler-owned conditions use `warn!`:

- The live Consumer warns on downstream rejection, live-query errors, task
  shutdown failures, and shutdown timeout.
- The Producer warns when untrusted Exchange data supplies raw SurrealQL and
  when no output `RecordId` can be derived.
- The PollingConsumer warns on query failure or timeout.
- Bundle registration and datasource health checks warn when setup cannot
  complete.

Errors that terminate an operation return `CamelError`, `SurrealDbError`, or a
failed Consumer task. Route error handling or supervision owns the final signal.
Any future `error!` site requires an ADR-0012 annotation and the corresponding
metric or health signal.

## Contract surface

### Accepted

- The 12 operations listed under `SurrealDB operation`.
- Input headers `CamelSurrealDbQuery`, `CamelSurrealDbParams`, and
  `CamelSurrealDbVector`.
- Output headers `CamelSurrealDbRecordId`, `CamelSurrealDbAction`, and
  `CamelSurrealDbTable`.
- Datasource fields `db_url`, `namespace`, `database`, `username`, and
  `password`.

### Rejected

- `output=stream`. Results materialize as `Body::Json`.
- Credentials in an Endpoint URI. Credentials belong in the named datasource.
- A `live` operation over HTTP. Live queries require `ws` or `wss`.

## Security hardening

### Identifier validation

`query::validate_identifier` accepts ASCII letters, digits, and `_`. The first
character must be a letter or `_`. Interpolated table, edge, and vector-field
names pass this check before query construction.

`query::validate_record_key` accepts the key forms required by SurrealDB. It
rejects whitespace, quotes, semicolons, and backslashes. Dynamic bodies,
vectors, and query parameters use SDK bindings instead of string
interpolation. Standard CRUD operations use the SurrealDB fluent API.

### Credential redaction

`redact_db_url` removes user information before a URL enters a log or error
message. `SurrealDbEndpointConfig` does not store `db_url`. The redaction path
has a regression test for embedded credentials.

### Raw-query trust boundary

ADR-0032 classifies Exchange headers and bodies as untrusted. The `query`
operation resolves SurrealQL from `CamelSurrealDbQuery` or the body only when
the operator sets `allow_dynamic_query=true` in the Endpoint URI. The default
is `false`: header and body query text is ignored, and the config query is
used exclusively. This mirrors camel-sql's `allow_dynamic_query` gate.

## Dependency boundary

The crate imports `surrealdb` directly in `query.rs`, `pool_factory.rs`,
`producer.rs`, `consumer.rs`, and `error.rs`. No project-owned adapter trait
wraps the SDK.

This is the stable database-driver class described in ADR-0020, not the
fast-changing provider class that required `LlmProvider`. Reassess this choice
if upstream API churn starts to spread beyond SurrealDB-specific modules. At
that point, introduce a `SurrealExecutor` port and confine SDK types to an
adapter plus `pool_factory.rs`.

## Retry

`SurrealDbPoolFactory` uses `retry_async` with
`NetworkRetryPolicy::default()` for transport establishment. It retries only
connection-class failures. Authentication and namespace/database selection do
not retry.

Producer operations do not retry. A non-idempotent write can reach the server
before the client observes a transport failure, so automatic replay can create
duplicates. `SurrealDbEndpointConfig.retry` preserves the public contract for a
future read-side retry classifier. ADR-0013 owns the retry semantics.
