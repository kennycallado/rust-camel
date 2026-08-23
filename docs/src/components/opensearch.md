# OpenSearch

The OpenSearch component runs document, index, and search operations against an OpenSearch cluster. The `OpenSearchProducer` is a Tower `Service<Exchange>` that owns a shared client and a 128-permit semaphore. The `MULTISEARCH` operation is reserved but not yet implemented. The component is producer-only. The Endpoint rejects `create_consumer()` with `EndpointCreationFailed`.

## Index a document

The Producer sends the Exchange body as the document. The optional `CamelOpenSearch.Id` header sets the document ID. Without it, OpenSearch assigns a server-generated ID.

```rust,ignore
use camel_api::Exchange;
use camel_builder::RouteBuilder;

let route = RouteBuilder::from("direct:index-user")
    .set_header("CamelOpenSearch.Id", serde_json::json!("user-42"))
    .set_body(serde_json::json!({
        "name": "Alice",
        "age": 30
    }))
    .to("opensearch://localhost:9200/users?operation=INDEX")
    .build()?;
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: index-user
    from: "direct:index-user"
    steps:
      - set_header:
          CamelOpenSearch.Id: "user-42"
      - set_body:
          name: "Alice"
          age: 30
      - to: "opensearch://localhost:9200/users?operation=INDEX"
```

</details>

## Search documents

The Producer sends the body as the search query. The `size` and `from` URI params fill in pagination when the body does not set them.

```rust,ignore
use camel_builder::RouteBuilder;

let route = RouteBuilder::from("direct:search-users")
    .set_body(serde_json::json!({
        "query": {
            "match": { "name": "Alice" }
        }
    }))
    .to("opensearch://localhost:9200/users?operation=SEARCH&size=25&from=0")
    .build()?;
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: search-users
    from: "direct:search-users"
    steps:
      - set_body:
          query:
            match:
              name: "Alice"
      - to: "opensearch://localhost:9200/users?operation=SEARCH&size=25&from=0"
```

</details>

## Bulk operations

The body is a JSON array of action-and-document pairs. The Producer serializes each element to a line and rejects the call when the total payload exceeds `max_bulk_bytes`.

```rust,ignore
use camel_builder::RouteBuilder;

let route = RouteBuilder::from("direct:bulk")
    .set_body(serde_json::json!([
        { "index": { "_id": "1" } },
        { "name": "Alice", "age": 30 },
        { "index": { "_id": "2" } },
        { "name": "Bob", "age": 25 }
    ]))
    .to("opensearch://localhost:9200/users?operation=BULK&max_bulk_bytes=1048576")
    .build()?;
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: bulk
    from: "direct:bulk"
    steps:
      - set_body:
          - index:
              _id: "1"
          - name: "Alice"
            age: 30
          - index:
              _id: "2"
          - name: "Bob"
            age: 25
      - to: "opensearch://localhost:9200/users?operation=BULK&max_bulk_bytes=1048576"
```

</details>

## URI

```text
opensearch://<host>:<port>/<index>?operation=<op>[&option=value...]
opensearchs://<host>:<port>/<index>?operation=<op>[&option=value...]  (TLS)
```

| Parameter | Required | Default | Description |
| --- | --- | --- | --- |
| `host` | no | `localhost` | OpenSearch hostname. Falls back to the global config |
| `port` | no | `9200` | OpenSearch port. Falls back to the global config |
| `indexName` | yes | — | Target index name. Overrides the index from the URI path. Lowercase letters, digits, hyphens, underscores. Max 255 bytes |
| `operation` | no | `SEARCH` | One of the operations in the table below |
| `username` | no | — | Basic auth username. Falls back to the global config |
| `password` | no | — | Basic auth password. Falls back to the global config |
| `timeout_ms` | no | `30000` | Per-request timeout in milliseconds |
| `size` | no | — | Search result page size. Filled into the body when absent |
| `from` | no | — | Search result offset. Filled into the body when absent |
| `max_bulk_bytes` | no | — | Maximum serialized bulk payload size. `BULK` rejects larger payloads |
| `retryEnabled` | no | `true` | Enable retry on transient failures |
| `retryMaxAttempts` | no | `10` | Maximum retry attempts |
| `retryInitialDelayMs` | no | `100` | Initial backoff delay |
| `retryMultiplier` | no | `2.0` | Backoff multiplier |
| `retryMaxDelayMs` | no | `30000` | Maximum backoff delay |
| `retryJitter` | no | `0.2` | Jitter factor between 0.0 and 1.0 |

## Operations

The Producer selects the operation from the `CamelOpenSearch.Operation` header. The header overrides the URI `operation` param. Invalid header values fall back to the URI value. URI parsing rejects unknown operation names at config time.

| Operation | Body | Required headers | Description |
| --- | --- | --- | --- |
| `INDEX` | document | optional `CamelOpenSearch.Id` | Index a document. Creates or replaces |
| `GET` | — | `CamelOpenSearch.Id` | Retrieve a document by ID |
| `DELETE` | — | `CamelOpenSearch.Id` | Delete a document by ID |
| `UPDATE` | partial doc | `CamelOpenSearch.Id` | Apply a partial update |
| `EXISTS` | — | `CamelOpenSearch.Id` | Check whether a document exists |
| `SEARCH` | query | — | Run a search query |
| `BULK` | action+doc array | — | Run multiple actions in one request |
| `MULTIGET` | id list | — | Retrieve multiple documents |
| `DELETE_INDEX` | — | — | Delete the entire index |
| `PING` | — | — | Probe the cluster |
| `MULTISEARCH` | — | — | Reserved. Returns a permanent not-implemented error |

## Headers

| Header | Direction | Description |
| --- | --- | --- |
| `CamelOpenSearch.Id` | input | Document ID for `INDEX`, `GET`, `DELETE`, `UPDATE`, `EXISTS` |
| `CamelOpenSearch.Operation` | input | Operation name. Overrides the URI `operation` param |

The Producer reads the index name from operator endpoint configuration. URI parsing validates the index name against OpenSearch naming rules and fails closed on null bytes, traversal segments, and invalid characters. The `CamelOpenSearch.Id` header is untrusted Exchange data. Current typed request builders pass it to `_doc/{id}` paths without component-side validation. This gap is tracked as audit finding I1.

## TLS

Use the `opensearchs` scheme to enable HTTPS. The base URL builder selects `https://` from the scheme. Basic auth credentials apply to both schemes.

```yaml
routes:
  - id: secure-search
    from: "direct:search-secure"
    steps:
      - to: "opensearchs://opensearch.example.com:9200/users?operation=SEARCH&username=admin&password=secret"
```

## Global configuration

Set defaults in `Camel.toml` under the `opensearch` key. URI params always override global values. Unset URI fields fall back to the global config after URI parsing.

```toml
[default.components.opensearch]
host = "opensearch.internal"
port = 9200
username = "app-user"
password = "${OPENSEARCH_PASSWORD}"
default_operation = "SEARCH"
index_name = "events"
timeout_ms = 30000
```

The `OpenSearchConfig::Debug` implementation redacts the password to `<redacted>`. The same redaction applies to `OpenSearchEndpointConfig::Debug`.

## Concurrency and retry

The Producer holds a `Semaphore` that caps in-flight calls at 128. `poll_ready()` returns ready unconditionally. The `call` future waits on the semaphore. A closed semaphore maps to `CamelError::ConsumerStopping` (ADR-0019, ADR-0024). The shared client initializes lazily on the first call. `Arc<Mutex<Option<OpenSearch>>>` serializes initialization, then each call clones and reuses the client.

Retry uses `NetworkRetryPolicy`. The retry loop classifies failures as transient or permanent:

- **Transient**: HTTP 5xx, network failures, timeouts. Retried up to `max_attempts`
- **Permanent**: HTTP 4xx, missing headers, parse failures. Surfaced immediately

All operations except `MULTISEARCH` and `UNKNOWN` use typed request builders from the `opensearch` crate. The component never interpolates Exchange data into query text. Bodies become structured JSON values that serde supplies to the request builder.

## Error handling

Operation failures log at `warn!` with the index name and error. Client initialization failures log at `error!` because the producer cannot function without a client. ADR-0012 classifies initialization failures as system-broken. The Producer surfaces transient retry exhaustion and permanent failures as `CamelError::ProcessorError`. The route `ErrorHandler` owns the operational signal.

**Reference**: [camel-opensearch CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-opensearch/CONTEXT.md). Example source: [`examples/opensearch-example`](https://github.com/kennycallado/rust-camel/tree/main/examples/opensearch-example).
