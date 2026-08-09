# Content Enricher

The Content Enricher is a Message Translator from Hohpe and Woolf. It calls a producer endpoint and feeds the response into the exchange.

```rust,ignore
{{#include ../../../examples/content-enricher/src/main.rs:content-enricher-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: content-enricher-demo
  from: timer:tick?period=1000&repeatCount=3
  steps:
    - enrich: "direct:enrich-data"
    - to: log:enriched?showBody=true&showCorrelationId=true

- id: enrichment-source
  from: direct:enrich-data
  steps:
    - set_body:
        value: "enriched-value"
```

</details>

The `.enrich(uri)` step sends the exchange to a producer endpoint and waits for a response. The example routes the call to `direct:enrich-data`. A second route consumes from that endpoint and supplies the enrichment payload. The two-route pattern keeps the enrichment source out of the main route. Multiple consumers can share the same source.

The `EnrichmentStrategy` trait decides how the response combines with the original exchange. The default `UseEnrichedBody` strategy replaces the original body with the response body. It keeps the original headers and properties. It discards the response headers. Write a custom strategy when you need to merge both bodies or preserve the response headers.

The difference from [Poll Enrich](poll-enrich.md) is the data source. Content Enricher calls an active endpoint that receives a request and returns a response. Poll Enrich reads from a polling consumer that holds passive data. Use Content Enricher to pull data from a service. Use Poll Enrich to read from a file or database.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the step compiles into a `Service<Exchange>` in the Tower pipeline. The `EnrichService` calls the producer inside the step and merges the response before the step returns. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

**Reference**: [Processor crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md) · [Example source](https://github.com/kennycallado/rust-camel/tree/main/examples/content-enricher)
