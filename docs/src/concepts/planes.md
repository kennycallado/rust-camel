# Data plane vs control plane

rust-camel splits its runtime into two planes (ADR-0001). The data plane processes every Exchange through a Route pipeline. The control plane manages the lifecycle of Components, Endpoints, Consumers, and Routes. The split keeps the hot path fast and the cold path safe.

```rust,ignore
{{#include ../../../examples/hello-world/src/main.rs:first-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: "hello-world"
    from: "timer:tick?period=1000&repeatCount=5"
    steps:
      - set_header:
          key: "source"
          value: "timer"
      - to: "log:info?showHeaders=true&showCorrelationId=true"
```

</details>

The example shows both planes at work. The `RouteBuilder::from(...)` chain builds a Tower service pipeline. That pipeline is the data plane. The `ctx.add_route_definition(...)` and `ctx.start()` calls route through the control plane. Each plane has its own contracts, its own performance budget, and its own trait hierarchy.

## Data plane: the hot path

The data plane processes every Exchange. Each step in the pipeline is a Tower `Service<Exchange>`. A `Filter` wraps a `BoxProcessor`. A `Choice` routes to one of several `BoxProcessor` arms. A `WireTap` forks to a secondary `BoxProcessor`. EIP composition maps cleanly to Tower's `Service` plus `Layer` pair (ADR-0001).

This is the hot path. Every microsecond matters. Tower's `poll_ready` and `call` protocol gives backpressure from the first step back to the Consumer. Services are cheap to clone and compose. The data plane must stay free of locks, allocations, and blocking operations. A step that blocks starves every Exchange behind it.

## Control plane: the cold path

The control plane manages lifecycle. Components, Endpoints, and Consumers use their own trait hierarchy with `start`, `stop`, `suspend`, `resume`, and health operations. These operations do not fit Tower's request/response model (ADR-0001).

Lifecycle commands flow through the RuntimeBus. The bus records intent, projects route status, and starts or stops the Consumer. The control plane uses synchronous-projection CQRS with optimistic versioning and optional journal persistence (ADR-0002). Safety wins over speed here. The control plane can afford heavier abstractions because it runs on the cold path.

## Why the separation exists

The split serves two goals.

**Performance.** The data plane must not block on control-plane locks. Every Exchange pays for the hot path. The control plane can take its time. It can persist commands, project state, and run health checks without charging that cost to throughput.

**Safety.** The data plane cannot mutate route state. It cannot start or stop a Consumer. It cannot change a Component registration. This isolation prevents runtime data from corrupting configuration. camel-core enforces the boundary at the module level. Each bounded context is a vertical slice with its own ports and adapters (ADR-0045). The data plane is not CQRS. The control plane is.

## Trust boundary

Exchange data is untrusted (ADR-0032). Operator configuration is trusted. Headers, body, properties, and correlation keys inside an Exchange are adversary-controlled. This is the trust boundary.

No untrusted exchange datum may drive a control-plane action. It may not drive an unbounded numeric or resource decision. It may not reach an executable or interpretable sink. Every such crossing requires validation, bounding, or a capability check.

This rule prevents injection attacks. A hostile header cannot start or stop a Route. A crafted body cannot inflate a throttle limit or a loop cap. The pre-1.0 audit found eight violations of this principle. Each one became a Batch 1 security fix.

## Cancellation

The control plane can stop a Route while Exchanges are in flight. A per-start `tokio::task_local!` cancel token carries the cancellation signal into the pipeline (ADR-0043). The step loop checks the token between steps, before each call. If the token is set, the pipeline returns `Failed(ConsumerStopping)`.

Graceful stop drains in-flight Exchanges to completion first. The cancel check is a backstop. It fires only after the drain timeout expires, so stragglers exit at the next step boundary instead of hanging. The token is task-local, not compiled into the pipeline. A compiled-in token would survive a restart as a cancelled child and fail every new Exchange. The task-local resets on each start from a fresh child token.

**Reference**: [camel-core crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-core/CONTEXT.md)
