# Processor

EIP (Enterprise Integration Pattern) processors implemented as Tower middleware services. Each
processor compiles from a DSL Step and is composed into the route pipeline.

## Language

**StreamSplitCodec**:
Trait in `stream_codec.rs` that splits a byte stream into fragment Exchanges. Takes a `StreamSplitInput` (parent Exchange + byte stream + metadata) and produces a `Stream<Item = Result<Exchange>>`. Built-in implementations:
- `NdjsonCodec` — splits on newline-delimited JSON boundaries, parses each line as JSON into the fragment body.
- `LinesCodec` — splits on newline boundaries, each line becomes the fragment body as a UTF-8 string.
- `ChunksCodec` — splits into fixed-size byte chunks (requires `chunk_size` in config).
- `Auto` — resolves format from `StreamMetadata.content_type` (`application/x-ndjson` → Ndjson, `text/plain` → Lines, `application/octet-stream` → Chunks).

**CamelStreamOrigin**:
Exchange property (`"CamelStreamOrigin"`) set on each fragment to trace it back to the parent stream. Value is a UUID generated once per `StreamSplitInput`.

**CamelStreamSourceContentType**:
Exchange property (`"CamelStreamSourceContentType"`) set on each fragment carrying the original stream's content type.

**CamelStreamOffset**:
Exchange property (`"CamelStreamOffset"`) set on each fragment — monotonically increasing zero-based index of the fragment within the stream.

**CamelStreamBatchSize**:
Exchange property (`"CamelStreamBatchSize"`) set on each fragment — total number of fragments expected in this batch (not set for streaming splits where count is unknown).

**StreamSplitInput**:
Groups the parent Exchange, the byte stream (`Pin<Box<dyn Stream<Item = Result<Bytes, CamelError>>>>`), and `StreamMetadata` into one argument for `StreamSplitCodec::split`.

**Header exclusion**:
`fragment_stream_exchange` strips `Content-Length` and `Content-Type` from fragment headers — these belong to the parent stream body, not individual fragments. See `stream_codec.rs:70-74`.

**fragment_stream_exchange**:
Wraps `fragment_exchange` (camel-api) with stream-specific header exclusion. Used by all three built-in codecs to create each fragment Exchange from the parent Exchange and a parsed body.

YAML example (streaming NDJSON split):

```yaml
- split:
    streaming: true
    stream:
      format: ndjson
    steps:
      - to: "log:fragment"
```

## ADR-0012 log-policy sites

All 5 sites in this crate are category **(a) handler-owned** — EIP processor failures that occur
INSIDE the pipeline where the route ErrorHandler owns ERROR responsibility. These have been
downgraded from `error!` to `warn!`.

| File | Line | Category | Annotation |
|------|------|----------|------------|
| `aggregator.rs` | 119 | (a) handler-owned | `// log-policy: handler-owned` — force_complete_all failed |
| `aggregator.rs` | 463 | (a) handler-owned | `// log-policy: handler-owned` — timeout task failed |
| `log.rs` | 64 | (a) handler-owned | `// log-policy: handler-owned` — LogProcessor::call default Error level |
| `log.rs` | 113 | (a) handler-owned | `// log-policy: handler-owned` — DynamicLog::call default Error level |
| `wire_tap.rs` | 87 | (a) handler-owned | `// log-policy: handler-owned` — WireTap processing error |

## Metrics

Metrics instrumentation is not yet wired for most processors. See TODO(PROC-004) in
`camel-processor/src/log.rs` for the broader instrumentation gap.
