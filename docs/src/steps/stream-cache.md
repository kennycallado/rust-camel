# Stream Cache

The Stream Cache step converts a `Body::Stream` into `Body::Bytes` up to a
threshold. A stream drains on the first read. Steps that must read the body
more than once cannot do so against a stream. Stream Cache reads the stream
once, stores the bytes, and replaces the body. Every later step sees the same
bytes.

```rust,ignore
RouteBuilder::from("timer:tick?period=1000")
    .route_id("stream-cache-demo")
    .stream_cache_default()
    .to("log:cached?showBody=true")
    .build()?;
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: stream-cache-demo
  from: timer:tick?period=1000
  steps:
    - stream_cache: true
    - to: log:cached?showBody=true
```

</details>

The `.stream_cache(threshold)` call sets the maximum byte count the step stores
in memory. Bodies smaller than the threshold become `Body::Bytes`. Bodies larger
than the threshold stay as `Body::Stream`. The default threshold is 128 KB
(`DEFAULT_STREAM_CACHE_THRESHOLD`). Call `.stream_cache_default()` to use this
default.

In YAML, `stream_cache: true` enables caching with the default threshold. The
form `stream_cache: { threshold: N }` sets a custom threshold in bytes.

Stream Cache is not an EIP. It is a pipeline utility that makes stream bodies
safe for multi-read steps. Place a Stream Cache step before a
[Splitter](../eip/splitter.md) that reads the body line by line. Place it before
a [Content Enricher](../eip/content-enricher.md) that sends the original body
and then inspects the response.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the
step compiles into a `Service<Exchange>` in the Tower pipeline. The stream body
type is documented in [camel-api/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-api/CONTEXT.md).
