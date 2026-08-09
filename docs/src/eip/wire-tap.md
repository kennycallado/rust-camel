# Wire Tap

The Wire Tap is a Message Router from Hohpe and Woolf. It sends a copy of the exchange to a tap endpoint for inspection or monitoring while the original exchange continues down the route unchanged.

```rust,ignore
{{#include ../../../examples/wiretap/src/main.rs:wire-tap-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: wiretap-demo
  from: timer:tick?period=1000&repeatCount=5
  steps:
    - wire_tap: log:monitor?showBody=true&showCorrelationId=true
    - to: log:main?showBody=true&showCorrelationId=true
```

</details>

The included route fires a timer and calls `.wire_tap("log:monitor?...")`. The tap clones the exchange and dispatches the clone to `log:monitor`, which logs the body and correlation id. The main pipeline then runs the original exchange through `.to("log:main?...")`, which logs the same body. The tap runs as fire-and-forget. An error on the tap endpoint does not stop the main flow, and the main flow does not wait for the tap to finish.

The Wire Tap differs from [Multicast](multicast.md) in scope. A wire tap is one side channel. It does not return a value into the main pipeline. The exchange that continues down the route is the same exchange the tap saw before the clone. Multicast is the main flow: it fans out to every branch and aggregates the results back into the pipeline. Use a wire tap to observe an exchange without changing it. Put a consumer on the main pipeline when it must affect the result.

The clone duplicates the body. A wire tap on an exchange with a multi-megabyte body copies that body for the tap endpoint, which costs memory and CPU. For large bodies, pair the route with a [Claim Check](claim-check.md) step that stashes the body and passes a reference id. The tap then reads the reference id without copying the body.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the wire tap compiles into a `Service<Exchange>` step in the Tower middleware pipeline. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/wiretap`](https://github.com/kennycallado/rust-camel/tree/main/examples/wiretap).
