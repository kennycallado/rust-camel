# Circuit breaker

The Circuit Breaker is a System Management pattern from Hohpe and Woolf. It trips after repeated failures against a downstream service, then short-circuits further calls for a cool-down so the dependency can recover.

```rust,ignore
{{#include ../../../examples/circuit-breaker/src/main.rs:circuit-breaker-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: circuit-breaker-demo
  from: timer:cb-test?period=1000&repeatCount=15
  circuit_breaker:
    failure_threshold: 3
    open_duration_ms: 3000
  error_handler:
    dead_letter_channel: log:cb-fallback?showBody=true&showHeaders=true&showCorrelationId=true
  steps:
    - to: direct:failing-service
    - to: log:cb-success?showBody=true&showCorrelationId=true
```

</details>

The breaker cycles through three states. In **Closed**, traffic flows. Each failed call increments a consecutive-failure counter. A successful call resets that counter to zero. When the counter reaches `failure_threshold`, the breaker trips into **Open**. In Open, the breaker rejects every call with `CamelError::CircuitOpen`. The route never touches the downstream service. The breaker holds Open for `open_duration_ms`, then enters **HalfOpen**.

HalfOpen admits a single probe call. The breaker rejects concurrent callers in that window so the probe runs in isolation. A probe that succeeds closes the breaker and resets the counter. A probe that fails reopens the breaker for another full cool-down. This single-probe design stops a backlog of traffic from stampeding the service. The dependency gets one probe, not a flood, at the first sign of recovery.

Use the circuit breaker to protect a downstream service. Do not use it to repair a transient blip. Retry sends more traffic at a failing call. The circuit breaker sends none. Pair them when a flaky dependency needs a retry for transient errors but a hard stop during a sustained outage. Per [ADR-0019](../adr/0019-error-disposition-pipeline-recovery.md), a route with an `error_handler` compiles the breaker into a gate on `RouteChannelService` rather than a pipeline step. Boundary rejections flow through `RouteErrorHandler::handle_boundary`. The route's `error_handler` can then route `CircuitOpen` to a dead-letter sink, as the example does with `log:cb-fallback`.

The example source is at [`examples/circuit-breaker`](https://github.com/kennycallado/rust-camel/tree/main/examples/circuit-breaker).

## Half-open fallback asymmetry

During the probe-in-flight window, concurrent callers behave differently by route shape. A route with an `error_handler` compiles the breaker into a `CircuitBreakerGate`. The gate serves the fallback to concurrent callers while the probe runs. A route without an `error_handler` compiles the breaker into a Tower `CircuitBreakerService`. That service rejects concurrent callers with `CircuitOpen` while the probe runs, even when a fallback is configured.

Both behaviors are sound. The gate keeps a single probe in flight and serves stale fallback data. The service keeps a single probe in flight and rejects. The asymmetry is intentional. See [Route structure](../yaml-dsl/route-structure.md) for the YAML form.

## Business errors are breaker-neutral

A business error counts as a success for both breaker implementations, in
every state. It never advances the failure counter in Closed. A HalfOpen
probe that ends in a business error succeeds and closes the breaker.
Only technical failures open the breaker or fail the probe.
[ADR-0097](../adr/0097-business-error-envelope.md) records this as owner
decision Q4. Both breakers follow it: the `CircuitBreakerGate` compiled
from a route with an `error_handler`, and the Tower
`CircuitBreakerService` compiled from a route without one.

This rule interacts with the `http:` component. The HTTP producer raises
`CamelError::HttpOperationFailed` when the response status falls outside
the `okStatusCodeRange` parameter. The default range is `200-299`, so a
`4xx` response is a technical failure and counts toward opening the
breaker. When `4xx` responses are expected outcomes of the business call,
widen the range with `okStatusCodeRange=200-499`. Then only `5xx`
responses raise `HttpOperationFailed`, and `4xx` responses count as
successes for the breaker. The widened range also accepts `3xx`
responses, so a redirect that the producer does not follow counts as a
success too.
