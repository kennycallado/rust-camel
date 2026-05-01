# camel-proto-compiler

> Runtime `.proto` file compilation for rust-camel

## Overview

`camel-proto-compiler` compiles `.proto` files at runtime and returns a `prost_reflect::DescriptorPool` for dynamic protobuf and gRPC use cases.

## Features

- Compile `.proto` files to `DescriptorPool` at runtime
- Vendored `protoc` (zero-install) with `PROTOC` env var fallback
- Thread-safe cache keyed by `(path, SHA-256 content hash)`
- Returns `prost-reflect` `DescriptorPool` directly (no round-trip)

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
