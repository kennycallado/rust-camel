# Multicast

The Multicast is a Message Router from Hohpe and Woolf. It sends a copy of the exchange to every endpoint in a fixed list and merges the responses back into one exchange.

```rust,ignore
{{#include ../../../examples/multicast/src/main.rs:multicast-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: multicast-demo
  from: timer:tick?period=2000&repeatCount=3
  error_handler:
    retry:
      max_attempts: 1
  steps:
    - set_body:
        value: hello from multicast
    - multicast:
        parallel: true
        aggregation: collect_all
        steps:
          - to: log:channel-a?showBody=true&showCorrelationId=true
          - to: log:channel-b?showBody=true&showCorrelationId=true
          - to: log:channel-c?showBody=true&showCorrelationId=true
    - to: log:summary?showBody=true&showCorrelationId=true
```

</details>

The example sets a text body and a `broadcast-id` header, then opens a multicast block over three `log` endpoints. The `parallel: true` flag dispatches all three branches at once instead of one after another. Each branch receives its own clone of the exchange, so a branch that mutates the body or headers does not affect its siblings. The clone also carries a `CamelMulticastIndex` property and a `CamelMulticastComplete` flag. A branch can read these to learn its position in the fan-out. The `collect_all` strategy gathers each branch response body into a JSON array, and the step that follows the block, `log:summary`, receives that array as its body.

The aggregation strategy decides what the post-multicast exchange carries. `LastWins` (the default) keeps the body of the last branch to complete and discards the rest. `CollectAll` assembles every branch body into a JSON array. `Original` passes the input exchange through unchanged and drops all branch output. A `Custom` variant takes a function of type `MulticastAggregationFn` for merge logic the built-ins do not cover. Three other knobs live on the block. `parallel_limit` caps how many branches run at once when `parallel` is on. `stop_on_exception` fails the whole block on the first branch error. `timeout` bounds how long the block waits for slow branches.

Pick Multicast when the destinations are fixed in the route and you want every one to run. Pick the [Recipient List](recipient-list.md) when a header or expression must compute the destinations at runtime. [Scatter-Gather](scatter-gather.md) is YAML sugar over Multicast with parallel dispatch and `collect_all`. Use it when the broadcast-and-collect shape is the whole point. A [Wire Tap](wire-tap.md) sends one copy to a side channel and never merges a result back.

Per [ADR-0025](../adr/0025-outcome-aware-structural-eips.md), multicast is an outcome-aware structural EIP. A branch that returns `PipelineOutcome::Stopped` propagates that stop out of the block. Aggregation does not run on stopped output. The route error handler sees any partial outcome through the same boundary that step errors use. Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the block compiles into a `Service<Exchange>` step in the Tower pipeline, with each branch a child step on the route channel. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/multicast`](https://github.com/kennycallado/rust-camel/tree/main/examples/multicast).
