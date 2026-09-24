# Design: protofix

## Context

Resolution today (`crates/services/camel-proto-compiler/src/compiler.rs:25-27`):

```rust
let protoc = std::env::var_os("PROTOC")
    .map(PathBuf::from)
    .unwrap_or(protoc_bin_vendored::protoc_bin_path()?);
```

`Option::unwrap_or` evaluates its argument before choosing, so
`protoc_bin_path()?` always runs. Inside `protoc-bin-vendored` 3.x:

- The facade `protoc_bin_vendored::protoc_bin_path()` returns
  `Result<PathBuf, Error>`. `Err` happens only for an unsupported
  OS/arch (`ArchCrate::detect`).
- On a supported platform it delegates to a platform crate, whose
  `protoc_bin_path()` builds `<CARGO_MANIFEST_DIR of the platform
  crate>/bin/protoc` and asserts the path exists. `CARGO_MANIFEST_DIR`
  is baked by `env!` at vendored compile time. When the cargo registry
  layout is absent (deployed prebuilt binary), the assert fires:
  `internal: protoc not found`. This is a panic, not an `Err`, so the
  caller's `?` cannot propagate it. The process dies at route load.

The existing `ProtoCompileError::VendoredProtoc` variant maps only the
facade `Err` (unsupported platform). It can never fire for the missing
binary case. `camel-dataformat-protobuf::ProtobufDataFormat::new`
maps any `ProtoCompileError` into `CamelError::TypeConversionFailed`,
so a typed error already reaches route load as a normal failure.

Constraints:

- Workspace edition is 2024: `std::env::set_var` / `remove_var` are
  `unsafe` (in-test precedent: `camel-bench` `src/lib.rs`).
- Tests that invoke the vendored lookup are serialized by the existing
  `PROTOC_COMPILE_LOCK` (rc-alwn) because vendored extraction races
  under parallel test runs. New tests that read or mutate `PROTOC`
  must take the same lock and restore the variable afterwards. One
  existing test, `test_concurrent_compiles_do_not_clobber`, currently
  calls `compile_proto` without the lock; its four spawned threads run
  concurrently regardless of an outer guard, so taking the lock in the
  outer test body preserves its concurrency claim while closing the
  race against the new `PROTOC`-mutating tests. This change adds the
  guard there.
- No workspace profile sets `panic = "abort"`; unwinding is available
  everywhere the workspace builds.
- Zone lease: `crates/services/camel-proto-compiler`,
  `docs/src/data-formats/protobuf.md`, `data-formats` spec. No edits in
  `camel-dataformat-protobuf` or any other crate.
- ADR-0049 (`#[non_exhaustive]` policy) targets contract-crate enums.
  `ProtoCompileError` lives in a service crate and is not in the
  policy's application set, so no attribute change accompanies the new
  variant.

## Decisions

### D1 — Lazy resolution via explicit `match`

Replace the eager `unwrap_or` with an explicit `match` (not
`unwrap_or_else`): the two arms read as distinct resolution strategies,
and the environment arm returns before any vendored symbol is touched.
`PROTOC` set to any value wins verbatim and is never re-resolved: a
`PROTOC` value that is broken at execution time (not executable, not
found) surfaces the ordinary execution error (`Io` from spawn, or
`ProtocFailed` with the child's stderr). The vendored fallback is not
attempted. Empty string is honored as set, for the same reason: an
explicit override is explicit.

### D2 — `catch_unwind` containment, not an existence probe

The vendored panic is contained with
`std::panic::catch_unwind(AssertUnwindSafe(vendored_lookup))`.

Rejected alternative: probe the vendored path for existence before
calling `protoc_bin_path()`. The path is built from the platform
crate's private `cargo_manifest_dir()` (an `env!` baked at vendored
compile time). There is no public accessor. A probe would have to
reconstruct the vendored crate's internal layout from outside, which
couples this crate to vendored internals that may change between
versions. `catch_unwind` is robust against any panic the vendored
facade or platform crates raise, present or future.

`AssertUnwindSafe` is sound here: the wrapped closure returns an owned
`Result<PathBuf, _>` and shares no mutable state with the caller.

Panic payload is extracted by downcasting to `String` then `&str`
(vendored `assert!` formats a `String` payload); unknown payloads map
to a fixed string. The default panic hook still prints one panic
message before the unwind is caught. That noise is accepted: the
alternative (hook swap) is global-state surgery out of proportion for a
fallback path, and the typed error that follows carries the guidance.

Residual risk: a `panic = "abort"` build would make the containment
moot. No workspace profile sets it, and the release profile keeps
unwind. The design accepts this boundary.

### D3 — One typed variant for every "no usable protoc": `ProtocUnavailable`

```rust
#[error("protoc unavailable: {detail}. Set PROTOC to a protoc binary, or use a build where the vendored protoc is present")]
ProtocUnavailable { detail: String },
```

Every vendored lookup failure produces this variant. `detail` carries
the panic message for the missing-binary case and the vendored facade
error text for the unsupported-platform case. From the operator's view
both mean the same thing: no usable protoc behind the fallback, and the
remedy is the same: set `PROTOC`. The display string names that remedy
because this error reaches route load on deployments where the
environment is the operator's only lever.

The existing `VendoredProtoc` variant is removed. It could only be
produced by the facade `Err` (unsupported OS/arch), which now feeds
`ProtocUnavailable`; on supported platforms it was unproducible. A grep
at proposal time found no constructor, `match`, or `matches!` arm for
it outside its own definition, so removal breaks no in-workspace code.
One variant per reachable failure keeps the enum honest.

### D4 — Injectable resolution seam

```rust
pub(crate) fn resolve_protoc_with(
    vendored: impl FnOnce() -> Result<PathBuf, ProtoCompileError>,
) -> Result<PathBuf, ProtoCompileError>
```

The function reads `PROTOC`; if set it returns the path without calling
`vendored`. Otherwise it maps the vendored outcome in a single pass —
the closure's `Err` is the final error, passed through verbatim; the
seam never wraps an `Err` in another variant (that would duplicate the
remedy text in the diagnostic). The seam itself creates
`ProtocUnavailable` in exactly one case: the caught panic. The default
closure performs the D3 mapping (vendored facade `Err` becomes
`ProtocUnavailable` with the vendored error text), so injected test
closures observe pure pass-through of whatever variant they return:
`Ok(Ok(path))` yields the path, `Ok(Err(e))` yields `e` unchanged,
`Err(panic_payload)` yields `ProtocUnavailable { detail }`.

The default `resolve_protoc()` supplies the real closure:
`protoc_bin_vendored::protoc_bin_path()` whose `Err` becomes
`ProtocUnavailable { detail }` inside the closure. The closure returns
`ProtoCompileError` (not the vendored `Error`) because vendored `Error`
has private fields and no public constructor, so tests cannot fabricate
one; the seam's contract is an already-typed outcome.

Testability is two-layer by design, because `compile_proto` takes no
injection parameter (public signature unchanged):

- Seam-level unit tests prove the short-circuit and the containment:
  with `PROTOC` set, an injected resolver that panics or records
  invocation is never called; with `PROTOC` unset, an injected resolver
  that panics with the vendored message yields
  `Err(ProtocUnavailable)` while the test completes.
- One `compile_proto`-level test proves the override end to end: a fake
  protoc script records its invocation in a marker file and serves a
  prebuilt descriptor fixture to `--descriptor_set_out`; the test
  asserts the marker exists and the returned pool decodes with the
  expected message.

`compile_proto` switches to `resolve_protoc()`. Signature and behavior
for the success path are unchanged.

### D5 — Docs truth

`docs/src/data-formats/protobuf.md`: the intro sentence keeps "no
compile-time code generation" only where true, and a new
"Protoc resolution" section states the order: `PROTOC` environment
override, vendored fallback, typed load failure with remedy. The crate
README ("Known limitation") and the crate-level doc comment in
`lib.rs` (which already mentions `protoc_bin_path()`) state the same
order. One truth, three surfaces, no divergence.

### D6 — No changes outside the zone

`camel-dataformat-protobuf` and the other dependents already consume
`compile_proto` as `Result` and map failures into `CamelError`. The
panic-to-error conversion inside `camel-proto-compiler` is sufficient
for the failure to surface as a route/document load error everywhere.
gRPC and DSL callers get the same containment for free.
