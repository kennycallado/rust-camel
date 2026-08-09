# Throttler

The Throttler is a System Management pattern from Hohpe and Woolf. It caps how many exchanges a route processes in a fixed time window. Downstream services never receive traffic faster than they can handle.

```rust,ignore
{{#include ../../../examples/throttler/src/main.rs:throttler-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: throttler-demo
  from: timer:tick?period=100&repeatCount=20
  steps:
    - throttle:
        max_requests: 2
        period_secs: 1
        strategy: delay
        steps:
          - to: log:throttled?showBody=true
```

</details>

The `.throttle(2, Duration::from_secs(1))` call sets the rate limit. At most two exchanges pass per second. The included route fires a timer 20 times at 100-millisecond intervals, which yields ten exchanges per second. The throttler holds the excess in an internal queue and releases those exchanges as the one-second window refills. Each released exchange flows into the `.to("log:throttled?showBody=true")` step inside the throttle scope. The `.end_throttle()` call closes the scope.

The default Delay strategy queues excess exchanges instead of dropping them. The throttler never discards an exchange on its own. This protects a downstream service from bursts without losing messages. A route that must reject instead of queue composes a filter on a backpressure signal.

Use the Throttler when a downstream service or external API imposes a rate limit. Database writers, third-party HTTP endpoints, and metered SaaS APIs reject or fail when traffic exceeds their quota. The Throttler smooths the source rate to fit that contract.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the throttler compiles into a `Service<Exchange>` step in the Tower middleware pipeline. The rate-limit window lives inside the service. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/throttler`](https://github.com/kennycallado/rust-camel/tree/main/examples/throttler).
