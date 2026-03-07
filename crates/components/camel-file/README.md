# camel-component-file

> File component for rust-camel

## Overview

The File component provides file system integration for rust-camel. It can read files from directories (consumer) and write files to directories (producer), supporting various file handling strategies.

## Features

- **Consumer**: Poll directories for new files
- **Producer**: Write files with various strategies
- **File filtering**: Include/exclude patterns (regex)
- **Post-processing**: Delete, move, or no-op after processing
- **Recursive directory scanning**
- **Atomic writes**: Temp file prefix support
- **Streaming**: Zero-copy file reading via ReaderStream (lazy evaluation)
- **Security**: Path traversal protection

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-component-file = "0.2"
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
| `noop` | `false` | Don't delete or move files after processing |
| `delete` | `false` | Delete files after processing |
| `move` | `.camel` | Directory to move processed files to |
| `include` | - | Regex pattern for files to include |
| `exclude` | - | Regex pattern for files to exclude |
| `recursive` | `false` | Scan subdirectories |
| `fileName` | - | Only process files matching this name |
| `readTimeout` | `30000` | Timeout for reading files (ms) |

## Producer Options

| Option | Default | Description |
|--------|---------|-------------|
| `fileName` | - | Output file name (or use CamelFileName header) |
| `fileExist` | `Override` | Strategy: Override, Append, Fail |
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

### Producer: Atomic Writes

```rust
let route = RouteBuilder::from("direct:data")
    .set_header("CamelFileName", Value::String("data.json".into()))
    .to("file:/data/output?tempPrefix=.tmp")
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

The File component uses lazy evaluation—files are not loaded into memory until the body is accessed. A default 10MB materialization limit prevents out-of-memory conditions when processing large files. This design allows handling files of any size (tested with 150MB+). Users can explicitly materialize with custom limits if needed.

## Security

The File component includes protection against path traversal attacks. Attempts to write files outside the configured directory (e.g., using `../` in the filename) will be rejected with an error.

- **Memory limits**: Default 10MB limit prevents unbounded memory consumption

## Documentation

- [API Documentation](https://docs.rs/camel-component-file)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
