# Proposal: protofix

## Why

A protobuf data-format route (`marshal/unmarshal: "protobuf:..."`) kills
the whole process at route load on any environment where the vendored
protoc binary is absent (found during a demo on camel 0.53.0; bd
rc-5bz6r). Two defects stack:

1. `crates/services/camel-proto-compiler/src/compiler.rs` resolves protoc
   with `std::env::var_os("PROTOC").map(PathBuf::from)
   .unwrap_or(protoc_bin_vendored::protoc_bin_path()?)`.
   `Option::unwrap_or` evaluates its argument eagerly, so the vendored
   lookup runs even when `PROTOC` is set. The environment override never
   short-circuits anything.
2. `protoc_bin_vendored` platform crates do not return `Err` when their
   baked-in binary path is missing. They panic through an `assert!`
   ("internal: protoc not found", protoc-bin-vendored-linux-x86_64-3.2.0
   src/lib.rs:20). The path is baked at vendored-crate compile time via
   `env!("CARGO_MANIFEST_DIR")`, so any binary shipped outside its build
   host (for example a prebuilt CLI on a machine without the cargo
   registry layout) dies at route load. The `?` operator never
   propagates because no `Result` is produced.

Net effect: the documented `PROTOC` escape hatch is dead code, and the
failure mode is a process abort instead of a route load error.

The user docs make this worse by hiding the dependency:
`docs/src/data-formats/protobuf.md` states "The format requires no
compile-time code generation" and never mentions that a protoc binary
must exist at runtime.

## What Changes

- Make protoc resolution lazy: an explicit `match` on
  `var_os("PROTOC")` returns the environment path before any vendored
  code runs. An explicit override is honored verbatim: a `PROTOC` value
  that fails at execution time surfaces the execution error (`Io` or
  `ProtocFailed`) and never falls back to the vendored binary.
- Contain the vendored panic: wrap the vendored lookup in
  `catch_unwind` (justification in design.md D2). Every vendored lookup
  failure, panic or `Err`, surfaces as one typed variant, new
  `ProtocUnavailable`, whose message names the `PROTOC` remedy. Route
  and document load fail with a normal error. The process stays alive.
  The now-unreachable `VendoredProtoc` variant is removed (it mapped
  only the vendored facade `Err`, which now feeds `ProtocUnavailable`;
  no in-workspace code constructs or matches it).
- Extract the resolution into an injectable seam
  (`resolve_protoc_with`) so tests simulate a missing vendored binary
  without deleting the real one.
- Tests for the three behavior cases: `PROTOC` set wins and the vendored
  lookup never runs (fake protoc script writes a marker file and serves
  a valid descriptor); `PROTOC` unset plus vendored absent returns a
  typed `ProtocUnavailable` error with the process alive; `PROTOC` unset
  plus vendored present keeps today's behavior (regression).
- Truth in docs: `docs/src/data-formats/protobuf.md` gains the
  resolution order (`PROTOC` override, vendored fallback, typed failure)
  and drops the implication that no toolchain is needed. The crate
  README and crate-level doc comment state the same order.
- Spec delta: a `data-formats` requirement for protobuf runtime protoc
  resolution and failure containment.

## Impact

- Affected crates: `crates/services/camel-proto-compiler` (code, tests,
  README, doc comments). No public signature changes; one public enum
  variant is added (`ProtocUnavailable`) and one unproducible variant is
  removed (`VendoredProtoc`) — all matches are in-workspace and none
  reference `VendoredProtoc` (verified by grep at proposal time).
- Docs: `docs/src/data-formats/protobuf.md`.
- Specs: `openspec/specs/data-formats` delta in this change dir.
- Dependents (`camel-dsl`, `camel-dataformat-protobuf`,
  `camel-component-grpc`, `camel-cli`) consume `compile_proto` as a
  `Result` today. `camel-dataformat-protobuf::ProtobufDataFormat::new`
  already maps compile errors into `CamelError`, so the typed failure
  surfaces as a route load error without changes outside the zone.
- bd: rc-5bz6r.
