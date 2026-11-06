# Custom middleware

A middleware layer wraps a compiled unit in a Tower `Layer`. rust-camel exposes three granularities: one producer, one route section, and the whole route. Each hook wraps the next unit at route compile time, so the layer sees a `BoxProcessor` and returns one.

## Wrap a producer

```rust,ignore
{{#include ../../../examples/middleware-layer/src/main.rs:layered-to}}
```

`to_layered` takes a URI and a layer. The runtime resolves the producer for that URI exactly like `to`, then wraps the resolved producer. The layer sees one send, not the rest of the pipeline. Here `TimeoutLayer` bounds each send to 250 ms.

## Wrap a route section

```rust,ignore
{{#include ../../../examples/middleware-layer/src/main.rs:section}}
```

`layered` takes a layer and a closure. The closure receives a `SectionBuilder`, which offers the step methods but no route metadata, `.parameters()`, `.route_layer()`, or `.build()`. Its steps become one segment wrapped by the layer. Nested `to_layered` and `layered` calls work inside the section.

## Wrap the whole route

```rust,ignore
{{#include ../../../examples/middleware-layer/src/main.rs:route-layer}}
```

`route_layer` is route metadata, so its position in the chain does not matter. Repeated calls nest: the last-added layer is the outermost. The layer wraps the entire compiled route, and the route error handler retries that whole unit.

The example implements `ProcessorLayer` by hand because `tower::limit::RateLimit` is not `Clone` and cannot use the blanket impl. `Buffer` sits outside `RateLimit`: its handle is cloneable and every clone shares the one worker that owns the rate state. `CamelErrorAdapter` maps the buffer's `BoxError` back to `CamelError`.

## How layers compile

A producer layer wraps the producer resolved for its URI. A section compiles in segment mode, so a `Stop` inside the section propagates as `Stopped` instead of being flattened into a successful send. Whole-route layers nest around the full step list, first-added innermost and last-added outermost.

## Error mapping

`From<BoxError> for CamelError` gives every wrapped service one error type. A native `CamelError` passes through unchanged. A Tower timeout becomes a timeout-classified `CamelError`, and any other error becomes a generic middleware `CamelError`. The original error stays reachable through `source()` and `downcast_ref` on the error chain.

**Reference**: [Middleware API](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-api/src/layer.rs) · [Example source](https://github.com/kennycallado/rust-camel/tree/main/examples/middleware-layer)
