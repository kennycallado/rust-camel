# Routing Slip

The Routing Slip is a Message Router from Hohpe and Woolf. It reads a header that holds a comma-separated list of endpoints. It routes the exchange through each one in sequence.

```rust,ignore
{{#include ../../../examples/routing-slip/src/main.rs:routing-slip-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: routing-slip-demo
  from: timer:tick?period=1000&repeatCount=10
  steps:
    - set_header:
        key: slip
        value: "log:step-a?showBody=true,log:step-b?showBody=true"
    - routing_slip:
        simple: "${header.slip}"
```

The Rust example uses a `process` closure to alternate the `slip` header between two endpoint lists. YAML `set_header` sets one fixed value, so the YAML form routes every exchange through `log:step-a` then `log:step-b`.

</details>

The route sets a `slip` header that holds one of two endpoint lists, then calls `.routing_slip(...)`. The closure takes a `&Exchange` and returns an `Option<String>`. It reads the `slip` header and returns the string. The processor splits the string on the `uri_delimiter` (default `,`), resolves each URI, and calls each endpoint in turn. The exchange that one endpoint returns feeds into the next. If the closure returns `None`, the exchange passes through unchanged.

Each step sees the mutations the previous step made. A `log` endpoint prints the body. An endpoint can append a header or transform the body. Any of these changes what the next endpoint receives. In the example the body carries the identifier `message #N`, and that identifier propagates from the first step to the last.

The slip is computed once. The closure runs on the initial exchange, and the resulting list drives the whole sequence. Each endpoint mutates the exchange, but the list of endpoints itself is fixed. To change the path between exchanges, vary the header value upstream of the slip, as the example does with its counter.

The Routing Slip is sequential. The [Recipient List](recipient-list.md) also evaluates its expression once. It sends a copy to every endpoint and aggregates the results, instead of threading one exchange through a chain. Use the Routing Slip when the exchange must visit a sequence of endpoints in order, each one building on the last. Use the Recipient List for one-shot fan-out to an independent set. The [Dynamic Router](dynamic-router.md) re-evaluates its destination after every hop, so it suits a path that each endpoint steers.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the routing slip compiles into a `RoutingSlipService` that runs as a `Service<Exchange>` in the Tower middleware pipeline. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/routing-slip`](https://github.com/kennycallado/rust-camel/tree/main/examples/routing-slip).
