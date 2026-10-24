# camel-proto-compiler

> Runtime `.proto` file compilation for rust-camel

## Overview

`camel-proto-compiler` compiles `.proto` files in-process with `protox`, a pure Rust parser. It returns a `prost_reflect::DescriptorPool` for dynamic protobuf and gRPC use cases. A path that accepts a `.proto` file also accepts a precompiled descriptor set (`.binpb`, `.pb`, `.desc`, `.protoset`).

## Features

- Compile `.proto` files to `DescriptorPool` at runtime with no external tool
- Accept a precompiled `FileDescriptorSet` in place of a source file
- Typed `ProtoCompileError` (`ProtoNotFound`, `Io`, `Compile`, `DescriptorDecode`), marked `#[non_exhaustive]`
- Thread-safe, memory-only cache. For `.proto` sources the key covers the whole compile input: the proto path, the ordered include-path hash, and a content fingerprint over the root plus its transitive imports as resolved (canonical import name, order-stable). Missing imports fail the compile and cache nothing. For precompiled descriptor sets the key is the path and content hash; includes are ignored. Hits re-read the import closure; only the compile is skipped. Eviction is FIFO (`with_max_entries`, default 1000).
- Returns `prost-reflect` `DescriptorPool` directly (no round-trip)

## Source preflight

Source compilation preloads the reachable import closure before it builds any pool. It reads each file through the same guarded include chain (user includes, then the parent directory, then embedded well-known types), parses it with `File::from_source`, and stores it in an immutable snapshot. Missing imports are recorded, not errors.

The compiler then validates the snapshot: no import cycles, the chain and work bounds, and the source lifetime bound. Only after validation does it construct the `protox::Compiler`. Pool building reads the snapshot alone. A source file is never reopened, so replacing or removing a path after preflight cannot change the returned pool.

## Known limitations

- Protobuf editions are unsupported, in sources and in descriptor sets. Rewrite the schema as `proto3` or `proto2`.
- Bracket nesting deeper than 64 is rejected.
- Source imports are bounded to 256 distinct include-resolved files and 64 MiB cumulative. Embedded well-known types count toward neither bound.
- Every dependency graph is bounded to an import chain of 256 files and a public-import traversal work `W = N + sum E(d)` of 100 000. A source graph is also bounded to a lifetime `(N + 1) * W` of 100 000. These are conservative bounds on traversal calls, not exact operation counts.

## Migration

`PROTOC` is ignored. The `ProtocUnavailable` and `ProtocFailed` variants are gone. Match the new `Compile { path, detail }` variant for compile failures.

These schema limits are part of the breaking change: nesting depth 64, source import graph 256 files and 64 MiB, 16 MiB per input, and a 256-file import chain with 100 000 public-import traversal work on every descriptor graph. The hardening commit marks them with `!`.

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
