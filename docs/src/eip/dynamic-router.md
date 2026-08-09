# Dynamic Router

The Dynamic Router is a Message Router from Hohpe and Woolf. It computes the destination endpoint at runtime from exchange data, instead of selecting from a fixed set of branches.

```rust,ignore
{{#include ../../../examples/dynamic-router/src/main.rs:dynamic-router-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: dynamic-router-demo
  from: timer:tick?period=1000&repeatCount=10
  steps:
    - set_header:
        key: destination
        value: a
    - dynamic_router:
        simple: "log:routed-${header.destination}?showBody=true&showHeaders=true"
```

The Rust example rotates the `destination` header through `a`, `b`, `c`. YAML `set_header` sets one fixed value, so the YAML form routes every exchange to `log:routed-a`. The Simple expression `log:routed-${header.destination}` mirrors the Rust closure.

</details>

The `.dynamic_router(Arc::new(|exchange| ...))` step takes a closure of type `Fn(&Exchange) -> Option<String>`. The router calls the closure and forwards the exchange to the endpoint it returns. Then it calls the closure again on the result. The loop ends when the closure returns `None`. Each hop receives the exchange as the previous endpoint left it. That endpoint can mutate a header or the body. The next closure call then reads the changed value and either returns a new destination or `None` to stop.

The example reads the `destination` header that an upstream `process` step sets. It returns the matching `log:routed-{dest}` endpoint. A single `Some` value may also carry several endpoints separated by the `uri_delimiter` (default `,`). The router visits all of them within one iteration before it calls the closure again.

Two safeguards stop a runaway loop. The closure must not return the same endpoint on consecutive iterations. A hop that leaves the routing data unchanged trips this guard. The router then raises an error instead of spinning. An iteration cap (`max_iterations`, default 1000) and an optional timeout bound the loop. To end routing after a single hop, have that hop clear the value the closure reads, or return `None`.

The Dynamic Router differs from the [Recipient List](recipient-list.md). The Dynamic Router re-evaluates its expression after every hop, so each endpoint can steer the exchange to the next. The Recipient List evaluates its expression once and sends the exchange to every endpoint on that list. Use the Dynamic Router when each hop can change where the exchange goes next. Use the Recipient List for one-shot fan-out to a known set.

The closure runs synchronously on the route channel. Any I/O the closure performs blocks the next step until the closure returns. If you need async work, resolve the destination in an upstream `process` step and store it in a header. Let the dynamic router read that header. This keeps the router a small, fast step.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the dynamic router compiles into a `DynamicRouterService` that runs as a `Service<Exchange>` in the Tower middleware pipeline. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/dynamic-router`](https://github.com/kennycallado/rust-camel/tree/main/examples/dynamic-router).
