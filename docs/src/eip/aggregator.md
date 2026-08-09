# Aggregator

The Aggregator is a Message Routing pattern from Hohpe and Woolf. It collects related exchanges into a bucket and emits one combined exchange when a completion condition holds.

```rust,ignore
{{#include ../../../examples/aggregator/src/main.rs:aggregator-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: aggregator-demo
  from: timer:orders?period=200&repeatCount=12
  error_handler:
    retry:
      max_attempts: 1
  steps:
    - aggregate:
        header: "orderId"
        completion_size: 3
        max_buckets: 100
        bucket_ttl_ms: 60000
    - to: log:batch?showBody=true&showCorrelationId=true
```

</details>

Each incoming exchange carries a correlation key in a header. The `correlate_by("orderId")` call names that header. Exchanges that share the key land in the same bucket. An `AggregationFn` folds each new exchange into the bucket seed to build the emitted batch body. In the included route, the `process` step rotates the `orderId` header through `"A"`, `"B"`, and `"C"`, so three buckets fill in parallel.

A bucket completes when it reaches its size limit or when its inactivity timeout fires. `complete_when_size(3)` flushes the bucket after three exchanges arrive for that key. `complete_when_timeout(Duration)` flushes it after a period with no new exchange. The `bucket_ttl` setting caps how long an incomplete bucket can live before the background sweep evicts it. The config validator rejects any setup with no memory bound, so set `max_buckets`, a timeout, or `bucket_ttl`.

Exchanges that arrive before a bucket completes still pass through the pipeline. They carry the `CamelAggregatorPending` property and an empty body. The `process` step after the aggregator checks that property and skips them. Only completed batches carry the `CamelAggregatedKey` and `CamelAggregatedSize` properties.

Use the Aggregator when many correlated messages arrive over time and you want one combined output. Multicast does the opposite job. It sends one message to many endpoints at once. The Aggregator collects. Multicast fans out.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the aggregator compiles into a `Service<Exchange>` step in the Tower middleware pipeline. The processor contract and the divergences from Apache Camel are documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/aggregator`](https://github.com/kennycallado/rust-camel/tree/main/examples/aggregator).
