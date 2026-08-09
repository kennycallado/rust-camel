# File

The file component is a directory poller for source routes and a disk writer for sink routes. The same crate covers both directions. The Consumer watches a directory for new or changed files. The Producer writes the Exchange body to disk under a chosen file name.

The file-pipeline example shows both directions with an upper-case transform and a dead-letter channel:

```rust,ignore
{{#include ../../../examples/file-pipeline/src/main.rs:file-pipeline-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: file-pipeline-demo
    from: "file:/tmp/rust-camel-pipeline/input?delete=true&initialDelay=0&delay=500&readTimeout=5000"
    error_handler:
      dead_letter_channel: "log:dead-letter?showBody=true&showHeaders=true&showCorrelationId=true"
    steps:
      - bean:
          name: uppercase-transform
          method: apply
      - to: "file:/tmp/rust-camel-pipeline/output?fileExist=Override&writeTimeout=5000"
      - to: "log:pipeline?showHeaders=true&showBody=true&showCorrelationId=true"
```

The `.process()` closure has no YAML step. Register the upper-case transform as a bean and call it with a `bean:` step. The Rust example writes to temp directories. Substitute real paths in the `from` and `to` URIs.

</details>

## Source

`file:{input}?delete=true&initialDelay=0&delay=500&readTimeout=5000` polls the input directory every `delay` milliseconds after an `initialDelay` wait. The Consumer submits one Exchange per detected file. The file body becomes the Exchange body. `readTimeout=5000` bounds how long a poll waits for new content before it yields an empty result.

`delete=true` removes each file after a successful read. The deletion happens after the Route processes the Exchange. A route failure leaves the file in place, so the next poll retries it. Omit `delete=true` to keep every file after consumption. This fits audit pipelines and idempotent replay.

The Consumer is event-driven. It polls the directory on a schedule and pushes each file as an Exchange. It does not implement the on-demand `PollingConsumer` SPI. The consumer task starts with the Route and runs until the Route stops.

## Sink

`file:{output}?fileExist=Override&writeTimeout=5000` writes the Exchange body to the output directory. The `fileExist` parameter controls what happens when the target file already exists:

| Value | Behavior |
| --- | --- |
| `Override` | Replace the target file. Default |
| `Append` | Add the body to the end of the target file |
| `Fail` | Reject the write if the target exists |
| `Ignore` | Skip the write if the target exists |
| `TryRename` | Write through a temp file, then rename. Requires `tempPrefix` |

The `Override` and `TryRename` strategies route through one private atomic-write helper. The helper writes a temp file, then renames it over the target. The `Fail` strategy uses the OS-level atomic `create_new(true)` directly. The `TryRename` strategy requires an explicit `tempPrefix`. The Producer rejects path-traversal segments in `fileName` before any file operation. It also rejects a cross-filesystem rename (EXDEV) rather than falling back to a non-atomic copy.

`fileName` resolves from the `CamelFileName` Exchange header. A route can set this header in a `process` step to control the output name. The Producer creates missing parent directories on the path. Validation runs at Endpoint creation and at the Producer boundary. It fails closed (ADR-0033).

Set `durable=true` for crash safety. The Producer then fsyncs the temp file, performs the rename, then fsyncs the parent directory, in that order. This is crash-safe but slower. Leave `durable=false` for speed when crash safety is not required.

## Pipeline shape

The example chains the source to a `process` step, then a sink, then log, then a dead-letter error handler. The `from:` Endpoint and the `to:` Endpoint share one CamelContext and resolve at Route start.

Use the file component when a directory on disk is the source or the sink. Use a different source when the data lives on a remote system that exposes a pull interface, such as SFTP. The file component polls a local path only. Use a different sink when the destination must be transactional, replicated, or ordered. File writes succeed per file with no cross-file consistency.

The atomic-write contract and the accepted `fileExist` values live in the [camel-file CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-file/CONTEXT.md). The example source is at [`examples/file-pipeline`](https://github.com/kennycallado/rust-camel/tree/main/examples/file-pipeline).
