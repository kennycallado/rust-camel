# Delayer

The Delayer is a System Management pattern from Hohpe and Woolf. It holds an exchange in the pipeline for a fixed or dynamic duration. The exchange then moves to the next step.

```rust,ignore
{{#include ../../../examples/delayer/src/main.rs:delayer-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: delayer-fixed
  from: timer:tick?period=2000&repeatCount=5
  steps:
    - delay:
        delay_ms: 500
    - to: log:delayed?showBody=true
```

</details>

The included route fires a timer every two seconds. The `.delay(Duration::from_millis(500))` step suspends the exchange for a fixed half-second. The `log:delayed` step receives the exchange only after the timer elapses. In the YAML DSL the same fixed pause is the `delay_ms` field. Use a fixed delay to space messages apart by a known interval. Common cases are rate-limited APIs and scheduled batches.

For a per-message pause, set the `dynamic_header` option to a header name. Write the delay in milliseconds to that header on each exchange. The service reads the header value and clamps it to `max_delay_ms` (default 3,600,000 ms, one hour). A missing or non-numeric header falls back to `delay_ms`. The builder equivalent is `delay_with_header`. The example registers it on a second route that sets `CamelDelayMs` to 1000. A dynamic delay suits routes where the producer supplies a retry-after or backoff value.

The Delayer differs from the Throttler. The Delayer pauses every exchange by the configured amount. The Throttler paces the rate of accepted exchanges over a window. A route that needs both can place a Delayer after a Throttler.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the delayer compiles into a `Service<Exchange>` step. Its `call` method awaits a `tokio::time::sleep` and returns the exchange when the timer elapses. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/delayer`](https://github.com/kennycallado/rust-camel/tree/main/examples/delayer).
