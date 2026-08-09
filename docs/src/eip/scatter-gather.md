# Scatter-Gather

The Scatter-Gather is a Message Router from Hohpe and Woolf. It broadcasts a copy of the exchange to a fixed list of endpoints in parallel and gathers the responses into one aggregate exchange.

```yaml
- id: scatter-gather-demo
  from: timer:tick?period=1000&repeatCount=3
  steps:
    - scatter_gather:
        endpoints:
          - direct:pricing
          - direct:inventory
          - direct:reviews
        aggregation: collect_all
    - to: log:aggregated?showBody=true
```

The `scatter_gather` step sends the same exchange to every endpoint listed under `endpoints`. All dispatches run in parallel. The `aggregation` field picks the strategy that merges the responses. The default is `last_wins`, which keeps the body of the last branch to complete. Set `aggregation: collect_all` to assemble every branch body into a JSON array. The step after `scatter_gather` receives the merged result.

Scatter-Gather is DSL sugar over [Multicast](multicast.md). The YAML parser lowers the step to a multicast block with `parallel: true` and the chosen aggregation. No new processor or Rust builder method exists for it. Rust code calls `.multicast()` directly. Use Scatter-Gather when the broadcast-and-collect shape is the point. Use a raw `multicast` block when you need the extra knobs it exposes: `parallel_limit`, `stop_on_exception`, or `timeout_ms`.

The endpoints are fixed in the route definition. A route that must compute destinations at runtime uses a [Recipient List](recipient-list.md). The gather is also stateless. No correlation key or completion condition exists. The step collects all parallel responses in one pass and moves on. Stateful accumulation across many exchanges is the [Aggregator](aggregator.md) EIP.

Per [ADR-0025](../adr/0025-outcome-aware-structural-eips.md), the lowered multicast compiles into a `MulticastSegment` that operates on `PipelineOutcome`. Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the segment runs as a `Service<Exchange>` step in the Tower pipeline. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).
