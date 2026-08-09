# Streaming Splitter

The Streaming Splitter is a Message Routing pattern from Hohpe and Woolf. It splits a streaming body into individual exchanges one fragment at a time, without first buffering the full body in memory.

```rust,ignore
{{#include ../../../examples/streaming-split/src/main.rs:streaming-split-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: streaming-split-demo
  from: file:data.ndjson
  steps:
    - split:
        streaming: true
        stream:
          format: ndjson
        aggregation: collect_all
        steps:
          - to: log:fragment?showBody=true&showCorrelationId=true
    - to: log:aggregated?showBody=true&showCorrelationId=true
```

</details>

The included example builds a `Body::Stream` that holds three NDJSON chunks. A `StreamingSplitterService` reads the stream through a `StreamSplitCodec`, which resolves the format from the content type. For `application/x-ndjson`, the codec parses each line into a separate fragment exchange. The sub-pipeline logs each fragment. When the split scope closes, the aggregation strategy combines the fragment outputs into the result body.

The streaming variant is the memory-efficient alternative to the [Splitter](splitter.md). The Splitter materializes every fragment before it processes the first one. The Streaming Splitter pulls one fragment from the source, runs the sub-pipeline, then pulls the next. A multi-gigabyte NDJSON file or a long-running log stream fits in constant memory. The codec reads bytes lazily, so the source produces data only as fast as the sub-pipeline accepts it.

Backpressure flows through the segment boundary. When the sub-pipeline pauses, the segment stops pulling from the stream, and the source stops producing. A `Stopped` outcome drops the underlying stream and returns the fragment exchange to the outer pipeline. The outer pipeline sees the same outcome shape it would from the eager Splitter.

Per [ADR-0025](../adr/0025-outcome-aware-structural-eips.md), streaming split is an outcome-aware structural EIP. Stop and Failed outcomes flow through the `CompiledStep::Segment` boundary with the fragment exchange intact. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/streaming-split`](https://github.com/kennycallado/rust-camel/tree/main/examples/streaming-split).
