# Load Balancer

The Load Balancer is a Message Router from Hohpe and Woolf. For each exchange it picks one endpoint from a fixed list and sends the exchange there.

```rust,ignore
{{#include ../../../examples/load-balancer/src/main.rs:load-balancer-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: load-balancer-demo
  from: timer:tick?period=1000&repeatCount=10
  steps:
    - set_body:
        value: hello from load balancer
    - load_balance:
        strategy: round_robin
        steps:
          - to: log:server-a?showBody=true
          - to: log:server-b?showBody=true
          - to: log:server-c?showBody=true
```

</details>

The example sets a text body, then opens a load-balance block over three `log` endpoints with the default `round_robin` strategy. For each exchange, the strategy picks one endpoint and dispatches the exchange there. The other two endpoints see nothing. With ten timer ticks over three endpoints, round-robin hands ticks out in order. The first goes to `log:server-a`, the second to `log:server-b`, the third to `log:server-c`. The fourth cycles back to `server-a`.

The strategy decides which endpoint a given exchange hits. `RoundRobin` (the default) cycles the endpoints in order with a shared counter. `Random` picks an index at random on each call. `Weighted` takes a list of `(name, weight)` pairs and draws an endpoint in proportion to its weight. A heavier endpoint gets more traffic. `Failover` tries the endpoints in order. On error it moves to the next, and keeps going until one succeeds or the list is exhausted. Pick round-robin for uniform endpoints of equal capacity. Pick weighted when endpoints differ in throughput. Pick failover when one primary endpoint should serve and the rest stand by as backups.

The Load Balancer sends each exchange to one endpoint. [Multicast](multicast.md) sends a copy to all of them. A [Content-Based Router](content-based-router.md) picks a branch by predicate. The Load Balancer picks by strategy, not by message content. The selection is stateless across exchanges except for the round-robin counter. Under `Random` or `Weighted`, two consecutive exchanges may land on the same endpoint.

Per [ADR-0025](../adr/0025-outcome-aware-structural-eips.md), the load balancer is an outcome-aware structural EIP. A selected branch that returns `PipelineOutcome::Stopped` propagates that stop out of the block. Under `Failover`, a branch that returns `Failed` moves the dispatch to the next endpoint instead of propagating the failure. Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the block compiles into a `Service<Exchange>` step in the Tower pipeline, with each `to` arm a child step on the route channel. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/load-balancer`](https://github.com/kennycallado/rust-camel/tree/main/examples/load-balancer).
