# ZIP Splitter

The ZIP Splitter is a specialized Splitter from Hohpe and Woolf. It decomposes a multi-entry ZIP archive into one exchange per entry. Each entry flows through the route as its own message.

```rust,ignore
{{#include ../../../examples/zip-splitter/src/main.rs:zip-splitter-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: zip-marshal-route
  from: timer:zip-marshal?period=3000&repeatCount=2
  error_handler: {}
  steps:
    - set_body: "payload for ZIP compression"
    - log: "Route 3: Original body"
    - marshal: zip
    - log: "Route 3: After marshal to ZIP"
    - unmarshal: zip
    - log: "Route 3: After unmarshal from ZIP (round-trip!)"
    - to: log:zip-result?showBody=true
```

</details>

The included route runs a timer that fires twice. The `.marshal("zip")` step compresses the body into a single-entry archive. The `.unmarshal("zip")` step decompresses it back. The route logs the byte sizes at each step. A reader can watch the body shrink during compression and grow during decompression.

The actual split happens through the `zip_splitter` expression and the `StreamingSplitterService`. The expression reads the ZIP central directory and emits one exchange per entry. Each exchange carries the entry body plus headers: `CAMEL_ZIP_ENTRY_NAME`, `CAMEL_ZIP_ENTRY_PATH`, `CAMEL_ZIP_ENTRY_SIZE`, and `CAMEL_ZIP_ENTRY_INDEX`. The `ZipSplitConfig` struct caps the entry count, the per-entry size, and the total decompressed size.

The ZIP Splitter is format-specific. It knows how to parse the ZIP archive structure. The [Streaming Splitter](streaming-splitter.md) is the generic alternative. It splits any byte stream through a codec that the content type selects. Use the ZIP Splitter when the input is a ZIP archive and you need per-entry headers. Use the Streaming Splitter for NDJSON, log lines, or raw byte chunks.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the splitter compiles into a `Service<Exchange>` step in the Tower middleware pipeline. The per-entry sub-pipeline compiles into child steps on the same route channel. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/zip-splitter`](https://github.com/kennycallado/rust-camel/tree/main/examples/zip-splitter).
