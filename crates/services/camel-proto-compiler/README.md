# camel-proto-compiler

> Runtime `.proto` file compilation for rust-camel

## Overview

`camel-proto-compiler` compiles `.proto` files at runtime and returns a `prost_reflect::DescriptorPool` for dynamic protobuf and gRPC use cases.

## Features

- Compile `.proto` files to `DescriptorPool` at runtime
- Vendored `protoc` (zero-install) with `PROTOC` env var fallback
- Thread-safe cache keyed by `(proto path, SHA-256 content hash, ordered include-path hash)`. The include-path hash uses canonical paths when available and supplied paths otherwise.
- FIFO cache eviction at the configurable `max_entries` ceiling (default 1000)
- Returns `prost-reflect` `DescriptorPool` directly (no round-trip)

## Known limitation

`compile_proto` writes descriptor sets to `std::env::temp_dir()` with names from a process-local counter. Calls in one process use distinct names. Concurrent processes that share a temporary directory can select the same name and return the wrong descriptor pool or a decode error. Track the fix in `rc-gr8k`.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-proto-compiler = "*"
```

## Usage

```rust
use camel_proto_compiler::{compile_proto, ProtoCache};

let pool = compile_proto("path/to/service.proto", &[])?;

let cache = ProtoCache::new();
let pool = cache.get_or_compile("path/to/service.proto", &[])?;
```

## License

Apache-2.0
