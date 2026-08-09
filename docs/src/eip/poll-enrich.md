# Poll Enrich

The Poll Enrich is a Content Enricher variant from Hohpe and Woolf. It polls a passive resource and feeds the result into the exchange.

```rust,ignore
{{#include ../../../examples/file-pollenrich/src/main.rs:poll-enrich-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: pollenrich-demo
  from: timer:tick?period=1000&repeatCount=3
  steps:
    - poll_enrich:
        uri: "file:/tmp/rust-camel-pollenrich?fileName=config.json&noop=true"
        timeout: 5000
    - stream_cache: true
    - to: log:enriched?showBody=true&showHeaders=true&showCorrelationId=true
```

</details>

The `.poll_enrich(uri, timeout)` step reads from a polling consumer. It waits up to `timeout` milliseconds for a message. A file or database is a passive resource. It stores data but does not push it. Poll Enrich pulls that data on demand. The example reads a config file each time the timer fires.

The `EnrichmentStrategy` trait decides how the polled exchange combines with the original. The default `UseEnrichedBody` strategy replaces the original body with the polled body. It keeps the original headers and properties. It discards the headers from the polled exchange. Write a custom strategy when you need to merge both bodies or preserve the polled headers.

When the poll returns no message within the timeout, the step calls `on_no_poll`. The default behavior passes the original exchange through unchanged. `ThrowOnNoPoll` wraps a base strategy and errors the exchange instead. Use it when missing data is a failure, not a normal condition.

Choose Poll Enrich when the source is a passive consumer. Choose the [Content Enricher](content-enricher.md) when the source is an active endpoint that receives a request and returns a response. Poll Enrich reads. Content Enricher calls.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the step compiles into a `Service<Exchange>` in the Tower pipeline. The strategy contract and the `PollEnrichService` implementation are documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

**Reference**: [Processor crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md) · [Example source](https://github.com/kennycallado/rust-camel/tree/main/examples/file-pollenrich)
