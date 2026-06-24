# camel-processor

> Message processors for rust-camel (EIP implementations)

## Overview

`camel-processor` provides implementations of Enterprise Integration Patterns (EIP) as processors that transform, filter, and route messages in rust-camel pipelines. These are the building blocks you use when defining routes.

## Features

- **Filter**: Conditional message routing based on predicates
- **Choice**: Content-based routing with `when`/`otherwise` branches
- **Log**: Logging processor for debugging and monitoring
- **SetHeader / DynamicSetHeader**: Set message headers
- **SetBody / MapBody**: Transform message bodies
- **Splitter**: Split messages into multiple fragments
- **Aggregator**: Aggregate multiple messages into one
- **Multicast**: Send to multiple destinations in parallel
- **RecipientList**: Dynamically resolve endpoints from expression at runtime
- **Dynamic Router**: Route to endpoints determined at runtime by a closure; includes same-destination loop detection
- **WireTap**: Fire-and-forget message copying
- **Circuit Breaker**: Resilience pattern implementation
- **Error Handler**: Centralized error handling
- **Stop**: Stop processing immediately
- **Script**: Execute mutating expressions via `ScriptMutator`; changes to headers, properties, and body propagate back with atomic rollback on error
- **Delayer**: Delay message processing with fixed or dynamic (header-based) duration
- **DoTryService** — `doTry / doCatch / doFinally` EIP with catch-by-variant, catch-by-predicate, onWhen filter, ADR-0019 dispositions (Handled / Propagate), and Camel-parity finally semantics.
- **Stream Handling**: Processors that consume streams replace the body with a JSON placeholder `{"placeholder": true}`
- **Security Policy Layer**: Tower middleware that enforces authorization before forwarding to the inner service
- **Marshal / Unmarshal**: Serialize/deserialize message bodies using pluggable data formats (JSON, XML, ZIP)
- **Streaming Splitter**: Split lazy streams (ZIP entries, NDJSON, text lines, binary chunks) with one-entry lookahead and aggregation. Supports `streaming: true` in YAML DSL with auto format detection from `Content-Type`

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-processor = "*"
```

## Usage

### Using Processors Directly

```rust
use camel_processor::{
    FilterService, LogProcessor, LogLevel, SetHeader, SetBody,
    MapBody, SplitterService, MulticastService, DelayerService
};
use camel_api::{Exchange, Message, Body, Value, BoxProcessor, DelayConfig};

// Filter exchanges
let filter = FilterService::new(
    |ex: &Exchange| ex.input.body.as_text().map(|t| t.len() > 5).unwrap_or(false),
    my_processor
);

// Log messages
let logger = LogProcessor::new(LogLevel::Info, "Processing message".to_string());

// Set a header
let set_header = SetHeader::new(identity, "source", Value::String("api".into()));

// Map body
let upper = MapBody::new(identity, |body: Body| {
    body.as_text().map(|t| Body::Text(t.to_uppercase())).unwrap_or(body)
});

// Delay with fixed duration
let delayer = DelayerService::new(DelayConfig::new(500));

// Delay with dynamic header
let dynamic_delayer = DelayerService::new(
    DelayConfig::new(1000).with_dynamic_header("CamelDelayMs")
);
```

### With RouteBuilder (Recommended)

```rust
use camel_builder::RouteBuilder;

let route = RouteBuilder::from("timer:tick")
    .set_header("processed", Value::Bool(true))
    .filter(|ex| ex.input.body.as_text().is_some())
        .log("Processing text message", LogLevel::Info)
        .map_body(|b| Body::Text(b.as_text().unwrap_or("").to_uppercase()))
    .end_filter()
    .to("mock:result")
    .build()
    .unwrap();
```

## Available Processors

| Processor | Purpose |
|-----------|---------|
| `FilterService` | Route based on conditions |
| `ChoiceService` | Content-based routing (when/otherwise) |
| `LogProcessor` | Log exchange information |
| `SetHeader` | Set static header values |
| `DynamicSetHeader` | Set headers from expressions |
| `SetBody` | Replace message body |
| `MapBody` | Transform message body |
| `SplitterService` | Split messages |
| `StreamingSplitterService` | Split lazy streams with lookahead (ZIP, future CSV/Tar) |
| `AggregatorService` | Combine messages |
| `MulticastService` | Parallel routing |
| `WireTapService` | Side-channel routing |
| `CircuitBreakerLayer` | Fault tolerance (circuit-breaker gate) |
| `ErrorHandlerLayer` | Error handling (Tower layer; prefer `RouteChannelService` + `DefaultRouteErrorHandler` for new code per ADR-0019) |
| `CompiledStep::Stop` | Stop processing (control flow via PipelineOutcome::Stopped) |
| `ScriptMutator` | Execute mutating scripts that modify Exchange headers, properties, or body |
| `MarshalService` | Marshal body using a DataFormat (e.g., Json → Text) |
| `UnmarshalService` | Unmarshal body using a DataFormat (e.g., Text → Json) |
| `DelayerService` | Delay message processing by a fixed or dynamic duration |
| `RecipientListService` | Dynamic recipient list — resolve endpoints from expression at runtime |
| `DynamicRouterService` | Dynamic router — resolve destination at runtime via closure; detects same-destination loops |
| `SecurityPolicyLayer` | Tower Layer that wraps a `SecurityPolicy` for authorization checks |
| `SecurityPolicyService` | Tower Service produced by `SecurityPolicyLayer`; evaluates the policy per exchange |

## Route Error Handler

The `RouteErrorHandler` trait owns ALL error handling logic — DLC, retry, and onException policies.
It is called from `RouteChannelService` (boundary errors) and `run_steps` (step errors).

### `RouteErrorHandler` trait (since 0.16.0)

```rust
#[async_trait]
pub trait RouteErrorHandler: Send + Sync {
    /// Match a policy for the given error. Called once before retry.
    fn match_policy(&self, err: &CamelError) -> Option<PolicyId>;

    /// Phase 1: Retry the failed step.
    async fn retry_step(&self, policy: Option<PolicyId>,
        step: &mut BoxProcessor, original: Exchange, error: CamelError) -> RetryOutcome;

    /// Phase 2: Determine step disposition after retry exhaustion.
    async fn handle_step(&self, policy: Option<PolicyId>,
        exchange: Exchange, error: CamelError) -> Result<StepDisposition, CamelError>;

    /// Handle boundary (infrastructure) errors.
    async fn handle_boundary(&self, kind: BoundaryKind,
        exchange: Exchange, error: CamelError) -> Result<Exchange, CamelError>;
}
```

### `DefaultRouteErrorHandler`

The production implementation of `RouteErrorHandler`. Encapsulates retry/onException/DLC logic.

```rust
let handler = DefaultRouteErrorHandler::new(
    Some(dlc_producer),                        // Dead Letter Channel processor
    vec![(exception_policy, Some(handler))],   // (ExceptionPolicy, optional handler processor)
);
```

### `CircuitBreakerGate`

A reusable circuit-breaker gate with explicit `before_call()` / `after_result()` API. Not a Tower
Layer — it is designed to be composed inside `RouteChannelService`.

```rust
let gate = CircuitBreakerGate::new(CircuitBreakerConfig::new()
    .failure_threshold(5)
    .open_duration(Duration::from_secs(30))
    .fallback(fallback_processor));

// Check before calling the pipeline:
match gate.before_call() {
    CircuitBreakerDecision::Allow => { /* proceed */ }
    CircuitBreakerDecision::Fallback(p) => { /* use fallback */ }
    CircuitBreakerDecision::Reject(e) => { /* return error */ }
}

// Report result after the call:
gate.after_result(&result);
```

### `CircuitBreakerDecision`

| Variant | Description |
|---------|-------------|
| `Allow` | Circuit is closed or half-open — proceed with pipeline |
| `Fallback(BoxProcessor)` | Circuit is open with a fallback processor configured |
| `Reject(CamelError)` | Circuit is open with no fallback — reject the call |

> **Deprecated:** `ErrorHandlerLayer` and `ErrorHandlerService` are deprecated since 0.16.0.
> Use `RouteChannelService` (from `camel-core`) + `DefaultRouteErrorHandler` (from `camel-processor`)
> instead. See the `RouteChannelService` documentation in `camel-core` for the new approach.

## Security Policy Layer

`SecurityPolicyLayer` is a Tower `Layer` that intercepts every `Exchange` and evaluates a `SecurityPolicy` before forwarding to the inner service. It is the standard way to add authorization to any Camel pipeline.

### How it works

The layer produces a `SecurityPolicyService` that calls `policy.evaluate(&mut exchange)` for each exchange and branches on the `AuthorizationDecision`:

| Decision | Behavior |
|----------|----------|
| `Granted { principal }` | Stores principal properties on the exchange (subject, issuer, audience, scopes, roles, claims) and forwards to the inner service |
| `Denied { reason, required, actual }` | Returns `CamelError::Unauthorized` with a message containing reason, required, and actual roles/scopes |
| `Err(e)` | Propagates the error (e.g., `CamelError::Unauthenticated`) |

### Usage

```rust
use std::sync::Arc;
use camel_processor::SecurityPolicyLayer;
use camel_api::security_policy::SecurityPolicy;

// Suppose you have a SecurityPolicy implementation:
let policy: Arc<dyn SecurityPolicy> = Arc::new(MyPolicy);

// Wrap it as a Tower layer:
let layer = SecurityPolicyLayer::new(policy);

// Compose in a Tower pipeline:
let service = layer.layer(my_inner_service);
```

When composed with other processors, the security check runs before the inner service sees the exchange:

```rust
use tower::ServiceBuilder;
use camel_processor::{SecurityPolicyLayer, LogProcessor};

let service = ServiceBuilder::new()
    .layer(SecurityPolicyLayer::new(my_policy))
    .service(LogProcessor::new(LogLevel::Info, "Authorized request".into()));
```

Exchanges that are denied or encounter an evaluation error never reach the inner service.

## Data Formats

The `DataFormat` trait defines serialization/deserialization for message bodies:

| Format | Marshal (structured → wire) | Unmarshal (wire → structured) |
|--------|---------------------------|------------------------------|
| `json` | `Body::Json` → `Body::Text` | `Body::Text`/`Body::Bytes` → `Body::Json` |
| `xml`  | `Body::Json` → `Body::Text` | `Body::Text`/`Body::Bytes`/`Body::Xml` → `Body::Json` |
| `zip`  | Any non-empty `Body` → `Body::Bytes` (single-entry ZIP) | `Body::Bytes`/`Body::Text` → `Body::Bytes` (decompressed) |

### Usage with RouteBuilder

```rust
use camel_builder::RouteBuilder;

let route = RouteBuilder::from("direct:in")
    .unmarshal("json")
    .marshal("xml")
    .to("mock:out")
    .build()
    .unwrap();
```

### YAML DSL

```yaml
steps:
  - unmarshal: json
  - marshal: xml
```

## ZIP Splitter

The `ZipSplitter` produces one exchange per ZIP entry using a lazy stream. It inherits the parent exchange context (headers, properties, OTel) and enforces safety limits (max entries, max decompressed size, path traversal prevention).

### DSL Usage

```yaml
steps:
  - split:
      streaming: true
      stream:
        format: zip
      steps:
        - to: "file:output"
```

Accepts `Body::Bytes`, `Body::Text`, or `Body::Stream` — streams are materialized with a size limit before extraction. Stale `Content-Length`/`Content-Type` headers are stripped from child entries.

### Programmatic API

```rust
use camel_processor::{StreamingSplitterService, ZipSplitConfig, zip_splitter};
use camel_api::{AggregationStrategy, Body, Exchange, Message};

let config = ZipSplitConfig {
    max_entries: 100,
    max_total_decompressed_size: 50 * 1024 * 1024, // 50 MB
    ..Default::default()
};
let expression = zip_splitter(config);
let sub_pipeline = my_processor;
let mut svc = StreamingSplitterService::new(
    expression,
    sub_pipeline,
    AggregationStrategy::LastWins,
    true, // stop_on_exception
);
```

### Entry Headers

Each ZIP entry exchange includes these headers:

| Header | Type | Description |
|--------|------|-------------|
| `CamelZipEntryName` | String | Entry filename |
| `CamelZipEntryPath` | String | Full path within archive |
| `CamelZipEntryIndex` | u64 | Zero-based index |
| `CamelZipEntrySize` | u64 | Uncompressed size |
| `CamelZipEntryCompressedSize` | u64 | Compressed size |
| `CamelZipEntryCrc32` | u32 | CRC-32 checksum |
| `CamelZipEntryIsDirectory` | bool | True for directory entries |
| `CamelZipEntryCompression` | String | Compression method (e.g. "Deflated") |

## Documentation

- [API Documentation](https://docs.rs/camel-processor)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
