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

The included example builds a `Body::Stream` that holds three NDJSON chunks. The route calls `.stream_split(expression)` on the `RouteBuilder`, where the expression is a `StreamingSplitExpression`. The compiler turns that expression into a `StreamingSplitSegment`. The expression reads the stream through a `StreamSplitCodec`, which resolves the format from the content type. For `application/x-ndjson`, the codec parses each line into a separate fragment exchange. The sub-pipeline logs each fragment. When the split scope closes, the aggregation strategy combines the fragment outputs into the result body.

Each fragment carries `CamelSplitIndex` always, and `CamelSplitComplete` as a boolean on every fragment (`true` only on the last one). `CamelSplitSize` is unknown until end-of-stream, so it is present only on the last fragment. A `Body::Stream` original body in the parent exchange is sanitized to `Body::Empty` before aggregation. Because the last fragment is only known once the source signals its end (one-entry lookahead), its completion stamps are applied at end-of-stream; the source is never polled past that point.

Archive formats are not handled by the incremental codec. The `zip`, `tar`, and `tar.gz` format names route to materialized archive splitters: the [ZIP Splitter](zip-splitter.md) and the [TAR Splitter](tar-splitter.md) each parse their own archive structure, with no generic archive-splitting abstraction between them.

The streaming variant is the memory-efficient alternative to the [Splitter](splitter.md). The Splitter materializes every fragment before it processes the first one. The Streaming Splitter pulls one fragment from the source, runs the sub-pipeline, then pulls the next. A multi-gigabyte NDJSON file or a long-running log stream fits in constant memory. The codec reads bytes lazily, so the source produces data only as fast as the sub-pipeline accepts it.

Backpressure flows through the segment boundary. When the sub-pipeline pauses, the segment stops pulling from the stream, and the source stops producing. A `Stopped` outcome drops the underlying stream and returns the fragment exchange to the outer pipeline. The outer pipeline sees the same outcome shape it would from the eager Splitter.

Per [ADR-0025](../adr/0025-outcome-aware-structural-eips.md), streaming split is an outcome-aware structural EIP. Stop and Failed outcomes flow through the `CompiledStep::Segment` boundary with the fragment exchange intact. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/streaming-split`](https://github.com/kennycallado/rust-camel/tree/main/examples/streaming-split).
