# Exchange and Message

An Exchange is the unit of work that flows through a pipeline. It carries an input Message, an optional output Message, properties, an error state, and an exchange pattern. Each processor reads the Exchange, mutates it, and returns it. The next processor picks up the modified Exchange.

```rust,ignore
{{#include ../../../examples/exchange-uow/src/main.rs:exchange-lifecycle}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: "on-complete"
    from: "direct:on-complete"
    steps:
      # The body-formatting Rust closure maps to a registered bean in YAML.
      - bean:
          name: "format-completed-body"
          method: "process"
      - to: "log:uow-complete?showBody=true&showCorrelationId=true"

  - id: "on-failure"
    from: "direct:on-failure"
    steps:
      - bean:
          name: "format-failed-body"
          method: "process"
      - to: "log:uow-failed?showBody=true&showCorrelationId=true"

  - id: "uow-success"
    from: "timer:uow-success?delay=0&period=1200&repeatCount=4"
    on_complete: "direct:on-complete"
    steps:
      - bean:
          name: "set-order-body"
          method: "process"
      - to: "log:uow-main-success?showBody=true"

  - id: "uow-failure"
    from: "timer:uow-failure?delay=0&period=1800&repeatCount=2"
    on_failure: "direct:on-failure"
    steps:
      # The Rust closure sets the body then calls set_error. A YAML route
      # needs a bean or function step to produce the failure.
      - bean:
          name: "set-body-and-fail"
          method: "process"
```

</details>

The include shows two routes. The `uow-success` route sets the body to `"order-123"`, then forwards the Exchange to a log sink. The `uow-failure` route sets the body, then calls `exchange.set_error(...)` to mark the Exchange as failed. Both routes attach a `UnitOfWorkConfig` that fires a hook route when the Exchange exits the pipeline.

## Exchange fields

The `Exchange` struct lives in `crates/camel-api/src/exchange.rs`. Its fields:

| Field | Type | Purpose |
|---|---|---|
| `input` | `Message` | Incoming message. Always present. |
| `output` | `Option<Message>` | Response message. Set for `InOut` exchanges. |
| `properties` | `HashMap<String, Value>` | Exchange-scoped key-value map. Cross-step scratch space. |
| `extensions` | `HashMap<String, Arc<dyn Any + Send + Sync>>` | Non-serializable values like channel senders. |
| `error` | `Option<CamelError>` | Failure state. Controls pipeline resolution. |
| `pattern` | `ExchangePattern` | `InOnly` (fire-and-forget) or `InOut` (request-reply). |
| `correlation_id` | `String` | UUID v4 for tracing across steps. |
| `otel_context` | `opentelemetry::Context` | Active span for distributed tracing. |

The `pattern` field defaults to `InOnly`. Call `Exchange::new_in_out(...)` to build an `InOut` exchange. The Consumer checks the pattern to decide whether to wait for a reply.

## Message: body and headers

A Message holds the payload plus its metadata. Two fields:

- `body: Body` — the payload.
- `headers: HashMap<String, Value>` — metadata about the body. Keys are strings. Values are `serde_json::Value`.

Components construct the initial input Message when a Consumer fires. Processors read and write it through `exchange.input` and `exchange.output`.

### Body variants

The body is a typed enum, not raw bytes. Each variant tags the payload with its format:

| Variant | Holds |
|---|---|
| `Empty` | No content. Default. |
| `Bytes(Bytes)` | Raw bytes. |
| `Text(String)` | UTF-8 string. |
| `Json(serde_json::Value)` | Parsed JSON. |
| `Xml(String)` | XML string. |
| `Stream(StreamBody)` | Lazy byte stream. Single-consumption. |

The `Body` enum is `#[non_exhaustive]` (ADR-0049). New variants may appear in a future release without a breaking-change semver bump.

Every variant can materialize to bytes. Call `body.materialize()` to collect the full payload as `Bytes`, or `body.into_bytes(limit)` to cap the read. Producers use this to serialize the body before I/O.

### Typed body access

Processors read the body in two ways. For a quick string peek, call `body.as_text()`, which returns `Option<&str>`. For a typed conversion, call `exchange.body_as::<T>()`. This method uses the `FromBody` trait.

`FromBody` has built-in implementations for `String`, `Vec<u8>`, `bytes::Bytes`, and `serde_json::Value`. Each implementation converts from compatible body variants and rejects the rest with `CamelError::TypeConversionFailed`. For custom serde types, the `impl_from_body_via_serde!` macro generates an implementation from any type that implements `DeserializeOwned`.

The example shows both access styles. The `on-complete` hook reads the body with `as_text()`. It then rewrites the body with a direct field assignment: `exchange.input.body = Body::Text(...)`.

## Headers versus properties

Headers and properties are both string-keyed maps of `Value`. They differ in scope.

Headers live on a Message. They describe that specific message: content type, encoding, custom fields set by a component. Read a header with `exchange.input.header("content-type")`. Set one with `exchange.input.set_header("source", "timer")`. Headers belong to the message they were set on. When the pipeline swaps input for output, headers do not carry over.

Properties live on the Exchange itself. They survive body and message changes. A processor sets a property with `exchange.set_property("attempt", 1)`. The next step reads it with `exchange.property("attempt")`. Use properties for cross-step state that is not part of the payload.

The error machinery uses properties. When `set_error()` fires, it auto-populates three property keys: `CamelExceptionMessage`, `CamelExceptionKind`, and `CamelExceptionCaught`. All languages read error context through these keys. Call `handle_error()` to set `CamelExceptionHandled` to `true` and clear the error.

## Exchange through the pipeline

The Runtime wraps each Exchange in a `UnitOfWorkConfig` layer and passes it to the Route pipeline. Each processor receives the Exchange, transforms it, and returns it. The next processor picks up the result. This chain continues until the last step completes.

When a processor fails, it sets the error state with `exchange.set_error(...)`. The pipeline executor detects the error and resolves the outcome. The `UnitOfWorkConfig` fires its `on_failure` hook instead of `on_complete`.

The pipeline resolves the Exchange to a `PipelineOutcome`: `Completed`, `Stopped`, or `Failed` (ADR-0024). See [routes and pipelines](routes-pipelines.md) for how the executor produces each outcome and how structural EIPs interact with it.

**Reference**: [API contracts](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-api/CONTEXT.md) · [Runtime](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-core/CONTEXT.md)
