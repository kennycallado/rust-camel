# camel-opensearch

Producer-only OpenSearch component for document, index, bulk, and search
operations over `opensearch:` and `opensearchs:` endpoints.

## Language

**OpenSearchProducer**:
Outbound `Producer` implemented as a Tower `Service<Exchange>`. It owns
backpressure, retry, and lazy access to a shared OpenSearch client.
_Avoid_: OpenSearch client, consumer, connector

**OpenSearchOperation**:
Operation selected from the `CamelOpenSearch.Operation` header or endpoint
configuration. The enum recognizes 11 operation names. `MULTISEARCH` is reserved
but not implemented.
_Avoid_: action, command, query type

**OpenSearchEndpointConfig**:
Per-endpoint settings parsed from the URI and merged with `OpenSearchConfig`
defaults. It includes connection settings, target index, operation, timeout,
pagination, bulk limits, and network retry policy.
_Avoid_: global config, client config

**OpenSearchBundle**:
`ComponentBundle` for the `opensearch` configuration key. It registers both the
`opensearch` and `opensearchs` component schemes.
_Avoid_: component, registry, plugin

## `#[non_exhaustive]` posture

ADR-0049 does not place `camel-opensearch` in its mandatory contract-crate set.
The crate uses its Rule 3 framework case by case.

| Public type | Posture | Rationale |
|---|---|---|
| `OpenSearchOperation` | Exhaustive enum | `UNKNOWN(String)` is the forward-compatibility catch-all for programmatic values. URI parsing rejects unknown operation names. |
| `OpenSearchConfig` | Exhaustive struct | Deserialized global configuration. Consumers do not need a forward-compatible struct-literal contract. |
| `OpenSearchEndpointConfig` | `#[non_exhaustive]` struct | Endpoint configuration can add fields without breaking external construction patterns. |

## Architecture

- This component is producer-only. `OpenSearchEndpoint::create_consumer()`
  returns `EndpointCreationFailed`.
- `OpenSearchProducer` creates the `OpenSearch` client on the first `call()`.
  `Arc<Mutex<Option<OpenSearch>>>` serializes initialization, then each call
  clones and reuses the client. The mutex guard is dropped before any network
  await.
- A semaphore caps in-flight calls at 128. `poll_ready()` reserves a permit and
  maps a closed semaphore to `CamelError::ConsumerStopping` as required by
  ADR-0019 and ADR-0024.
- `retry_async` uses `NetworkRetryPolicy`. Only network failures, timeouts, and
  HTTP 5xx responses are transient. Input errors and HTTP 4xx responses fail
  without retry.
- All 11 recognized operation paths avoid string-built queries. Ten operations
  use typed `opensearch` crate request builders and structured JSON or NDJSON
  bodies. `MULTISEARCH` returns a permanent not-implemented error before it
  builds a request. No operation interpolates exchange data into query text.

## Trust boundary

ADR-0032 treats operator configuration as trusted and Exchange headers and
bodies as untrusted.

- `index_name` comes from operator endpoint configuration. URI parsing still
  validates its length, character set, null bytes, and traversal segments and
  fails closed.
- `CamelOpenSearch.Id` is an untrusted Exchange header. Current GET, DELETE,
  UPDATE, EXISTS, and explicit-ID INDEX paths pass it to typed `_doc/{id}`
  request builders without component validation. Validation remains pending in
  audit finding I1, tracked by `rc-25j3`. This is a known gap, not an accepted
  contract.
- Search and document bodies remain untrusted data, but serde supplies
  structured JSON values to typed request builders. The route grants the
  OpenSearch capability. The component does not interpret arbitrary query DSL
  by string concatenation.

## Related decisions

- ADR-0012 defines the log-policy classification below.
- ADR-0019 and ADR-0024 define producer readiness and shutdown signaling.
- ADR-0032 defines the Exchange-data trust boundary.
- ADR-0049 supplies the case-by-case API framework but does not bind this crate.
- ADR-0051 governs credential redaction. OpenSearch config `Debug`
  implementations redact passwords.
- ADR-0007 consumer shutdown is not applicable because this component has no
  consumer.

## Log-level policy

Per ADR-0012, this component's `error!` sites are categorized as:

- **system-broken** (`OpenSearchProducer::build_client`): endpoint initialization
  failures from URL parsing or transport construction prevent the producer from
  functioning. The static method has no runtime handle for an inline health-pin
  call. It keeps `error!` with `// log-policy: system-broken`. The log is the
  operator signal.

Reviewer: `r_glm` verifies this classification against source during review.
