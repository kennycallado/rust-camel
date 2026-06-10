# camel-component-file

> File component for rust-camel

## Overview

The File component provides file system integration for rust-camel. It can read files from directories (consumer) and write files to directories (producer), supporting various file handling strategies.

## Features

- **Consumer**: Poll directories for new files
- **Producer**: Write files with various strategies, streaming directly to disk (zero-copy)
- **File filtering**: Include/exclude patterns (regex)
- **Idempotent consumer**: With `noop=true`, files already seen in the current run are skipped automatically (in-memory deduplication)
- **Recursive directory scanning**
- **Atomic writes**: Override strategy writes to a temp file then renames (panic-safe cleanup)
- **Streaming**: Zero-copy reads (consumer) and writes (producer) via `Body::into_async_read()`
- **Health Check**: Async directory metadata probe for consumers
- **Security**: Path traversal protection
- **Done file support**: Wait for marker files before processing (`doneFileName`)
- **Depth control**: `maxDepth`, `minDepth` for recursive traversal
- **Glob filtering**: `antInclude`/`antExclude` with glob patterns
- **Extension filtering**: `includeExt`/`excludeExt` with compound extension support
- **Sorting & limiting**: `sortBy` (name/length/modified), `shuffle`, `maxMessagesPerPoll`
- **TryRename**: Atomic write-to-temp-then-rename producer strategy

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-component-file = "*"
```

## URI Format

```
file:directoryPath[?options]
```

## Consumer Options

| Option | Default | Description |
|--------|---------|-------------|
| `delay` | `500` | Poll interval in milliseconds |
| `initialDelay` | `1000` | Initial delay before first poll |
| `noop` | `false` | Don't delete or move files; enables idempotent consumer (seen files are skipped) |
| `delete` | `false` | Delete files after processing |
| `move` | `.camel` | Directory to move processed files to |
| `include` | - | Regex pattern for files to include |
| `exclude` | - | Regex pattern for files to exclude |
| `recursive` | `false` | Scan subdirectories |
| `fileName` | - | Only process files matching this name |
| `readTimeout` | `30000` | Timeout for reading files (ms) |
| `doneFileName` | - | Done file name pattern (static or dynamic with `${file:name}` / `${file:name.noext}`). Consumer only processes files whose done marker exists |
| `maxDepth` | `MAX` | Maximum directory traversal depth |
| `minDepth` | `0` | Minimum depth to start accepting files |
| `maxMessagesPerPoll` | `0` | Max files per poll (0 = unlimited). Combined with `eagerMaxMessagesPerPoll` |
| `eagerMaxMessagesPerPoll` | `true` | If true, stop walk at limit. If false, collect all then truncate after sort |
| `antInclude` | - | Comma-separated glob patterns to include (e.g., `*.txt,*.csv`) |
| `antExclude` | - | Comma-separated glob patterns to exclude |
| `includeExt` | - | Comma-separated extensions to include (e.g., `txt,csv`). Supports compound extensions (`tar.gz`) |
| `excludeExt` | - | Comma-separated extensions to exclude |
| `shuffle` | `false` | Randomize candidate order (deterministic seed for reproducibility) |
| `sortBy` | - | Sort specification: `file:name`, `file:length`, `file:modified`. Prefix with `reverse:` and/or `ignoreCase:`. Multi-group with `;` |

## Producer Options

| Option | Default | Description |
|--------|---------|-------------|
| `fileName` | - | Output file name (or use CamelFileName header) |
| `fileExist` | `Override` | Strategy: Override, Append, Fail, Ignore, TryRename. TryRename requires `tempPrefix` |
| `tempPrefix` | - | Temp file prefix for atomic writes |
| `autoCreate` | `true` | Create directories automatically |
| `writeTimeout` | `30000` | Timeout for writing files (ms) |

## Usage

### Consumer: Read Files

```rust
use camel_builder::RouteBuilder;
use camel_component_file::FileComponent;

let route = RouteBuilder::from("file:/data/input?noop=true")
    .log("Processing file", camel_processor::LogLevel::Info)
    .to("mock:processed")
    .build()?;
```

### Consumer: Delete After Processing

```rust
let route = RouteBuilder::from("file:/data/inbox?delete=true")
    .process(|ex| async move {
        // Process file content
        Ok(ex)
    })
    .build()?;
```

### Consumer: Move After Processing

```rust
let route = RouteBuilder::from("file:/data/inbox?move=.done")
    .to("direct:process")
    .build()?;
```

### Consumer: Filter by Pattern

```rust
// Only process .csv files
let route = RouteBuilder::from("file:/data/input?include=.*\\.csv")
    .to("mock:csv-files")
    .build()?;
```

### Producer: Write Files

```rust
let route = RouteBuilder::from("timer:tick?period=1000")
    .set_body("Generated content")
    .set_header("CamelFileName", Value::String("output.txt".into()))
    .to("file:/data/output")
    .build()?;
```

### Producer: Append Mode

```rust
let route = RouteBuilder::from("direct:logs")
    .set_header("CamelFileName", Value::String("app.log".into()))
    .to("file:/var/log?fileExist=Append")
    .build()?;
```

### Producer: Atomic Writes (TryRename)

```rust
// Atomic write: temp file then rename. No partial files visible to consumers.
let route = RouteBuilder::from("direct:data")
    .set_header("CamelFileName", Value::String("data.json".into()))
    .to("file:/data/output?fileExist=TryRename&tempPrefix=.tmp")
    .build()?;
```

### Consumer: Done File Marker

```rust
// Only process files when a .done marker exists alongside them
let route = RouteBuilder::from("file:/data/input?doneFileName=${file:name}.done&delete=true")
    .to("direct:process")
    .build()?;
```

### Consumer: Sort by File Attribute

```rust
// Process oldest files first
let route = RouteBuilder::from("file:/data/input?sortBy=file:modified&delete=true")
    .to("direct:process")
    .build()?;

// Process largest files first, limited to 10 per poll
let route = RouteBuilder::from("file:/data/input?sortBy=reverse:file:length&maxMessagesPerPoll=10")
    .to("direct:process")
    .build()?;
```

### Consumer: Extension & Glob Filtering

```rust
// Only process CSV and TXT files
let route = RouteBuilder::from("file:/data/input?includeExt=csv,txt&recursive=true")
    .to("direct:process")
    .build()?;

// Glob pattern filtering with depth limit
let route = RouteBuilder::from("file:/data/input?antInclude=report-*.txt&recursive=true&maxDepth=3")
    .to("direct:process")
    .build()?;
```

## Exchange Headers

### Consumer Headers

| Header | Description |
|--------|-------------|
| `CamelFileName` | Relative file path |
| `CamelFileNameOnly` | File name only |
| `CamelFileAbsolutePath` | Absolute file path |
| `CamelFileLength` | File size in bytes |
| `CamelFileLastModified` | Last modified timestamp |
| `CamelFilePath` | Starting directory path |
| `CamelFileParent` | Parent directory of the file |
| `CamelFileCanonicalPath` | Canonical (resolved) absolute path |
| `CamelFileRelativePath` | Relative path from starting directory |

### Producer Headers

| Header | Description |
|--------|-------------|
| `CamelFileName` | Output file name (required if no fileName option) |
| `CamelFileNameProduced` | Absolute path of written file |

## Example: File Pipeline

```rust
use camel_builder::RouteBuilder;
use camel_component_file::FileComponent;
use camel_core::CamelContext;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut ctx = CamelContext::new();
    ctx.register_component("file", Box::new(FileComponent::new()));

    // Process files from input, write to output
    let route = RouteBuilder::from("file:/data/input?move=.processed&delay=1000")
        .process(|ex| async move {
            // Transform content
            let body = ex.input.body.as_text().unwrap_or("").to_uppercase();
            let mut ex = ex;
            ex.input.body = Body::Text(body);
            Ok(ex)
        })
        .set_header_fn("CamelFileName", |ex| {
            let original = ex.input.header("CamelFileNameOnly")
                .and_then(|v| v.as_str())
                .unwrap_or("output");
            Value::String(format!("processed_{}", original))
        })
        .to("file:/data/output")
        .build()?;

    ctx.add_route(route).await?;
    ctx.start().await?;

    tokio::signal::ctrl_c().await?;
    ctx.stop().await?;

    Ok(())
}
```

## Streaming & Memory Management

The File component streams data directly between the body and disk using `tokio::io::copy` — no intermediate buffers. Both consumer (reading) and producer (writing) operate without materializing the full payload in RAM, making it suitable for arbitrarily large files.

## Global Configuration

Configure default file polling and timeout settings in `Camel.toml` that apply to all File endpoints:

```toml
[default.components.file]
delay_ms = 500               # Poll interval (default: 500)
initial_delay_ms = 1000      # Initial delay before first poll (default: 1000)
read_timeout_ms = 30000      # Read timeout (default: 30000)
write_timeout_ms = 30000     # Write timeout (default: 30000)
```

URI parameters always override global defaults:

```rust
// Uses global delay (500ms)
.from("file:/data/input?noop=true")

// Overrides delay from global config
.from("file:/data/input?delay=2000&noop=true")
```

### Profile-Specific Configuration

```toml
[default.components.file]
delay_ms = 500

[production.components.file]
delay_ms = 1000         # Less frequent polling in production
read_timeout_ms = 60000 # Longer timeout for large files
```

### Known Limitation

Due to how the `#[derive(UriConfig)]` macro bakes defaults into the generated code, the global configuration uses **duration comparison**: if a URI-specified duration equals the macro's default, it will be overridden by the global config value. Explicitly setting a URI parameter to its default value (e.g., `?delay=500`) is indistinguishable from "not set" and will use the global config.

## Security

The File component includes protection against path traversal attacks. Attempts to write files outside the configured directory (e.g., using `../` in the filename) will be rejected with an error.

## Health Check

The `file` component registers an async health check via `AsyncHealthCheck`.

- **Probe**: Directory metadata check via `tokio::fs::metadata()` (consumers only; producers always report Healthy)
- **Healthy**: Target directory exists and is accessible
- **Degraded**: Directory missing, inaccessible, or probe times out

Health checks are exposed via the health server:

```toml
[observability.health]
enabled = true
port = 8080
```

## Documentation

- [API Documentation](https://docs.rs/camel-component-file)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
