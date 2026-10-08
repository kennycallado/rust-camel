# Proposal: protox-compiler

## Why

The protobuf data format and the gRPC component (`protoFile=`) compile
`.proto` files at runtime through `camel-proto-compiler`. That crate
starts a `protoc` subprocess. The `protoc-bin-vendored` crate returns a
path that Cargo baked in at build time (`CARGO_MANIFEST_DIR`). In a
released artifact that path points to the CI runner registry and does not
exist on the user machine.

Bd `rc-15md7` records the measured result: the published Docker images
0.51 to 0.56 cannot start any `protobuf:` route or any `grpc://...?protoFile=`
route. The `PROTOC` workaround also fails on the default `FROM scratch`
image, because the compile path writes a descriptor to a temporary file and
the image has no `/tmp`. The docs say the vendored `protoc` is "embedded in
standard builds". That statement is false.

Owner premise: the project must need nothing external at runtime, on any
platform or architecture. Evidence and measurements are in
`.opencode/fleet/inbox/reference/protox-e_opus-report.md`.

## What Changes

- `camel-proto-compiler` compiles `.proto` source in-process with `protox`
  0.9.1 (pure Rust). `compile_proto` and `ProtoCache::get_or_compile` keep
  their signatures.
- The descriptor pool is built by decoding `encode_file_descriptor_set()`,
  not with `Compiler::descriptor_pool()` (wrong `packed` flag).
- Safety guards: a bracket nesting-depth limit of 64 (`{ < [`) on every
  source file the compiler reads (top-level and imports) and on option text
  inside descriptor sets, and `catch_unwind` around the whole protox
  compile and pool decode.
- Editions are unsupported: sources (`edition = "2023"`) and descriptor sets
  with `syntax = "editions"` fail with a typed error that tells the user to
  rewrite the schema as `proto3` or `proto2`. A precompiled set cannot work
  around this, because prost-reflect 0.16 rejects `syntax = "editions"`.
- Every place that accepts a `.proto` path also accepts a precompiled
  descriptor set (`.binpb`, `.pb`, `.desc`, `.protoset`), chosen by file
  extension. It also gives self-contained schemas (imports inside the set)
  and a way around any future protox parser defect.
- The runtime `protoc` path is removed: `PROTOC` env, vendored resolver,
  panic-hook swap, temporary descriptor file.
- BREAKING: `ProtoCompileError` loses `ProtocUnavailable` and `ProtocFailed`,
  gains `Compile { path, detail }`, and becomes `#[non_exhaustive]`.
- Docs, crate README, and the `data-formats` spec stop claiming that
  `protoc` is needed or embedded. ADR-0084 records the decision. The
  `default-deptree.txt` golden fixture is regenerated (new protox crates).

Excluded: the sealed-artifact path (`camel compile` extracts `protoFile`
assets to `std::env::temp_dir()` at boot; one follow-up bd). The `build.rs` usages in `camel-component-grpc`, `camel-validator`,
`camel-xslt`, `camel-jms`, `camel-cxf` and `camel-bench` (build time only; one
follow-up bd). `ProtoCache` invalidation for changed imports (`rc-me2ii`).
`camel-bridge` native downloads (`rc-grvqh`).

## Acceptance criteria

- `compile_proto` succeeds on the repo fixtures and on a kitchen-sink
  fixture with `PROTOC` unset, `PATH` empty, and `TMPDIR` pointing to a
  missing directory (the `camel run`, Rust API and library paths; sealed
  artifacts are out of scope, see above).
- No `Command`, `tempfile`, `temp_dir`, `PROTOC`, or `protoc-bin-vendored`
  remains in the `camel-proto-compiler` runtime code or runtime dependencies.
- Wire bytes for proto3 `[packed=false]` and proto2 `[packed=true]` match
  protoc. Nesting depth 100 and 10 000 (braces and angle brackets, in sources, imports and descriptor sets) return a typed error without panic or abort.
- A precompiled descriptor set loads through `compile_proto` and `ProtoCache`.
- Existing tests of `camel-proto-compiler`, `camel-dataformat-protobuf`,
  `camel-component-grpc`, `camel-dsl` (protobuf feature) and `camel-cli`
  compile pass. AGENTS.md quality gates pass for the touched crates.

## Risk budget

Accepted: new error texts (protox wording, same line and column as protoc);
editions are unsupported until prost-reflect supports them; about 10 new MIT/Apache crates; +0.8 MB
binary. Out of bounds: any runtime executable lookup, any temporary file,
any wire difference from protoc on the measured fixtures, any panic or
abort from malformed `.proto` input.

Affected crates: `camel-proto-compiler` (main), `camel-dataformat-protobuf`
and `camel-component-grpc` (docs only), `camel-cli` (deptree fixture).
Bd: `rc-15md7`.
