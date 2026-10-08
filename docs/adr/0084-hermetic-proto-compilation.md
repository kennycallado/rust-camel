# ADR-0084: Hermetic proto compilation (in-process protox, no runtime protoc)

- Status: Accepted (decided 2026-10-06; bd rc-15md7)
- Source: bd rc-15md7; openspec change protox-compiler
- Amends: ADR-0075 intent (self-contained artifact)

## Context

`camel-proto-compiler` compiled `.proto` files through a `protoc`
subprocess. The `protoc-bin-vendored` crate embeds a `protoc` path that
Cargo bakes in at build time under `CARGO_MANIFEST_DIR`. In a released
artifact that path points at the CI runner and does not exist on the user
machine.

Bd `rc-15md7` records the measured result. Published Docker images 0.51 to
0.56 cannot start a `protobuf:` route. They also cannot start a `grpc://`
route with `protoFile=`. The `PROTOC` workaround fails on the default
`FROM scratch` image. That image has no `/tmp`, and the compile path wrote
a descriptor to a temporary file. The docs claimed the vendored `protoc`
is "embedded in standard builds". That statement is false.

## Decision

Compile `.proto` source in-process with `protox` 0.9.1. The workspace
pins the exact version `=0.9.1`. The compiler builds the pool by decoding
`encode_file_descriptor_set()`. It never calls `Compiler::descriptor_pool()`,
which reports a wrong `packed` flag.

Two guards protect the process. A bracket-nesting scan rejects depth above
64 in every source file and in descriptor-set option text. `catch_unwind`
converts any compiler panic into a typed error. No global panic hook is
installed.

Every path that accepts a `.proto` also accepts a precompiled descriptor
set. The extension chooses the mode: `.binpb`, `.pb`, `.desc`, or
`.protoset`. Includes are ignored for a descriptor set. `PROTOC` is
ignored. `ProtoCompileError` loses `ProtocUnavailable` and `ProtocFailed`,
gains `Compile { path, detail }`, and becomes `#[non_exhaustive]`. This is
a breaking change.

Owner premise: no built-in runtime feature needs an executable outside the
`camel` binary. Build scripts need only the Rust toolchain. `exec`,
`containers`, and the Java bridges are explicit opt-in exceptions.

## Rejected options

- **B, embed and extract `protoc`:** needs a writable executable
  directory, fails on `FROM scratch`, adds 9 to 14 MB per target, covers
  8 targets only.
- **C, docs only:** keeps the defect.
- **A plus a `PROTOC` override:** `PROTOC` is a generic variable that
  hides a difference between development and production.

## Consequences

Protobuf editions are unsupported, in sources and in descriptor sets,
until prost-reflect supports them. Bd `rc-sed5l` tracks the upstream work.
Error texts follow protox.

The build-time `build.rs` migration is bd `rc-x2rlm`. Bd `rc-joask` is
resolved by mission 350: sealed artifacts resolve `protoFile` in memory
through the embedded-source registry. One gap stays open: `rc-me2ii`
(cache invalidation on an imported file change).
`camel-bridge` downloads (bd `rc-grvqh`) need their own decision.

## References

- bd rc-15md7; openspec change `protox-compiler`
- ADR-0075 (self-contained executable artifact format)
